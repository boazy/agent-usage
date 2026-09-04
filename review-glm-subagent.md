# Code Review: agent-usage CLI
 
> Historical review of the predecessor `codex-usage` implementation; retained package-name references below describe that archived code.

**Scope.** `src/main.rs` (2,021 lines, single file) and `Cargo.toml`, reviewed read-only. The repo contains only `Cargo.toml`, `Cargo.lock`, `.gitignore`, `src/main.rs` (plus an empty `src/bin/`): no README, LICENSE, CI, rustfmt/clippy config, and no `[lints]` section in `Cargo.toml`. Findings cite `src/main.rs:<line>`. Auth material (tokens, JWTs, the built-in OAuth client-id constant at main.rs:319-323) is referenced structurally only — no credential content is quoted anywhere in this report.

---

## 1. Primitive obsession

**[1.1] [high] `AppResult<T> = Result<T, String>` as the global error type — main.rs:16**
17 `.map_err(|e| e.to_string())` sites (248, 258, 259, 362, 407, 408, 480, 509, 535, 592, 598, 695, 696, 700, 701, 731, 808) plus a dozen inline `format!` error constructors (502, 552-556, 562-567, 575-579, 749-753, 765-793, 825-829, 832-838). Every failure becomes an opaque string: callers cannot distinguish "auth file missing" from "network down", `main` can only exit 1 for everything (398-404), and context is flattened at the boundary.
**Recommendation:** a ~40-line hand-rolled enum — `enum AppError { Io(std::io::Error), Json(serde_json::Error), Http { status: u16, body: String }, Config(String), Message(String) }` with `From` impls for `io::Error`/`serde_json::Error`; keep `type AppResult<T> = Result<T, AppError>`. thiserror optional. No hierarchy beyond that — right-sized for a CLI.

**[1.2] [high] Theme colors are `&'static str` in three different encodings — main.rs:27-41, 43-220**
- `bar_warning`/`bar_exhausted`/`bar_unknown` store *color names* ("yellow", "bright_cyan") resolved at runtime by `ansi_color_from_name` (1214-1234), whose `_ => ""` fallback (1232) silently drops color on typos — nothing catches a misspelled name.
- `meter_color`/`window_color`/`reset_color` embed 256-color SGR strings (`"\x1b[38;5;39m"`, lines 49-51 etc.).
- `bar_primary`/`bar_background` embed truecolor SGR strings (`"\x1b[38;2;r;g;bm"`, lines 52-53 etc.).
The type system knows nothing; any of the seven fields can hold garbage that prints verbatim to the terminal.
**Recommendation:** replace with a typed `enum Color { Base(u8), Idx(u8), Rgb(u8,u8,u8) }` (see §3) — one choke point validates and renders; `ansi_color_from_name` and its stringly fallback disappear.

**[1.3] [medium] Presentation strings stored in the data model**
- `BankReset { source: String, expires_in: String }` (905-909) carries pre-formatted text ("expires in …", "expiry not reported") built at 957-975; the fallback even bakes a count into prose, `"Reset credits available: {count}"` (973, asserted at 1726).
- `UsageItem.reset_text: Option<String>` (387) carries "resets in …" built at 1471-1475.
Tests then couple to phrasing (1679-1682, 1727), and the computed durations cannot be reused programmatically.
**Recommendation:** store `Option<u64>` seconds-until-expiry and a plain `Option<u64>` count; format only at the print boundary. `human_duration` (1491-1521) stays the single formatter.

**[1.4] [medium] Timestamps as bare `u64` with a magic s/ms threshold**
`resolve_reset_time` (1479-1484) and `parse_timestamp_to_ms` (1908-1912) independently re-implement "if the value > 1_000_000_000_000 it is already ms, else multiply by 1000". Nothing in the types says which unit a field holds (`UsageWindow.reset_at: Option<u64>` at 331, `ResetCredit.expires_at_ms: Option<u64>` at 902).
**Recommendation:** one `fn epoch_value_to_ms(v: u64) -> u64` with a named, documented cutoff const; ideally wrap in `struct MilliSeconds(u64)`.

**[1.5] [low] CLI fields as strings — main.rs:281, 286**
`auth_file: String` should be `PathBuf` (`expand_path`, 443-450, needs `&str` only for `~/` handling — take `&str` at the boundary, store `PathBuf`). `base_url: String` is re-parsed ad hoc by `normalize_base_url` (452-471) although `url::Url` is already a dependency — parse once and keep `Url`.

**[1.6] [low] Slug→display-name mapping as an inline string match — main.rs:1042-1051**
"spark"→"Spark", "chat"→"Codex", "gpt-reserve"→"Reserve" lives inside `collect_items`. A `const DISPLAY_NAMES: &[(&str, &str)]` table makes the contract explicit and unit-testable in isolation.

**[1.7] [low] Two names for the same error type — main.rs:587**
`build_headers` returns `Result<HeaderMap, String>` instead of `AppResult<HeaderMap>`; identical type, inconsistent spelling.

---

## 2. Duplicated logic / single source of truth

**[2.1] [high] Success-path block duplicated inside `fetch_usage` — main.rs:508-521 vs 534-547**
Both copies do the same 5-step sequence: parse JSON → `parse_usage_payload` → if `reset_credits_available.unwrap_or(0) > 0` fetch banked credits → finish spinner → return. Extract `fn complete_usage(resp, client, base, auth, spinner) -> AppResult<ParsedUsage>`. This also becomes the seam that makes the flow testable once the HTTP client is injectable (§6.3).

**[2.2] [medium] Spinner teardown repeated 4× and missed on error paths**
`pb.finish_and_clear()` at 517-519, 543-545, 559-561, 572-574 — but early `?` returns at 505 (request failure), 509/535 (JSON parse), 533 (retry request) skip it entirely while `enable_steady_tick` (491) keeps the spinner animating.
**Recommendation:** RAII guard (`struct SpinnerGuard(Option<ProgressBar>)` with `Drop` → `finish_and_clear()`); every path then terminates the spinner exactly once.

**[2.3] [medium] Window-pair push duplicated — main.rs:1017-1034 and 1053-1072**
Primary and secondary windows are pushed with identical argument lists, twice (top-level `rate_limit` and per-additional `rate_limit`). Helper `fn push_windows(items: &mut Vec<UsageItem>, meter: &str, rl: &RateLimit, now_ms: u64)` iterating `[&rl.primary_window, &rl.secondary_window].into_iter().flatten()`.

**[2.4] [medium] codex-prefix stripping implemented twice with different semantics**
`additional_limit_slug` strips once via `strip_prefix("codex_")`/`strip_prefix("codex-")` (1389-1397); `slugify` *also* strips via `trim_start_matches("codex-")`/`("codex_")` (1425-1427, repeatedly). The first strip is fully redundant (slugify repeats it), and the two behave differently on inputs like "codex_codex_x". Single source: prefix handling belongs only in `slugify`.

**[2.5] [medium] The label pipeline is spread across three functions**
`additional_limit_slug` (1373-1404) → inline slug→display match (1042-1051) → `normalize_usage_label`/`title_case_slug` fallbacks (1199-1202, 1452-1469). Canonical home: one `fn display_name_for_limit(limit_name: Option<&str>, metered_feature: Option<&str>) -> String` in the formatting module; `collect_items` only calls it.

**[2.6] [low] Time-unit literals repeated — main.rs:1408-1416 vs 1496-1499**
86_400 / 3_600 / 60 appear in both `window_label` and `human_duration`; name them once (`SECS_PER_DAY`, …).

**[2.7] [low] Change-detection field list vs persistence field list drift**
`needs_persisted_refresh` compares five fields including `email` (311-317), but `persist_auth` writes only access/id/refresh/account (708-729) — email alone can never produce a meaningful write. `oauth_client_id` is consistently excluded from both (correct, since persist never stores it). The invariant is only implicit: document the mirror rule, or compare the serialized `tokens` object instead.

**[2.8] [low] Section-header rendering duplicated — main.rs:917-918 vs 1001-1002**
Title + underline pattern twice; a tiny `print_section(theme, title, rule_char)` covers both.

---

## 3. Themes as ANSI vs colors

Current state (evidence in [1.2]): three encodings — 16-color *names* resolved by a string lookup (1214-1234), 256-color strings, truecolor strings — plus a generic `colorize` wrapper (1236-1238) and a bar renderer emitting raw sequences (1166-1197). **No NO_COLOR handling exists anywhere in the file**, and colors are emitted unconditionally to stdout even when piped (917-918, 940-945, 1001-1010, 1117-1125); `output_is_interactive` (911-913) gates only bars/spinner.

**[3.1] [high] Recommendation: typed `Color` enum + a single paint choke point, zero new dependencies**

```rust
enum Color {
    Base(u8),          // 0-15 → SGR 30-37 / 90-97
    Idx(u8),           // xterm 256-palette index → SGR 38;5;N
    Rgb(u8, u8, u8),   // → SGR 38;2;r;g;b
}

fn paint(c: Color, text: &str) -> String; // no-op wrapper when disabled
fn colors_enabled() -> bool;              // stdout tty && NO_COLOR unset (&& CLICOLOR != "0")
```

- `Theme` fields become `Color`; `BUILTIN_THEMES` (43-220) translates mechanically (names → `Base`, `38;5;N` → `Idx`, `38;2;…` → `Rgb`), so current visuals are byte-identical on every terminal.
- `ansi_color_from_name` (1214-1234) and `colorize` (1236-1238) collapse into `paint`; `render_bar_cells` (1166-1197) takes `Color` args and delegates.
- Decide color-on/off once in `run()` (a `OnceLock<bool>` or a threaded flag) — see [6.1].
- Fidelity: keep the existing 256-color choices as `Idx` rather than converting to `Rgb`; converting would change rendered bytes on non-truecolor terminals for zero gain.

**Tradeoffs vs crates:**
- `anstyle` + `anstream`: typed colors plus automatic NO_COLOR/CLICOLOR/piped-stripping. Cost: two more deps and its stream stack. Worth it only if you want `--color=always` to *strip* codes when piping.
- `owo-colors`: light, but the typing benefit ends at the call site — `Theme` would still hold strings, so it does not fix [1.2].
- **Hand-rolled (recommended):** ~40-50 lines total, no deps, full control for a 2k-line CLI. Revisit `anstyle` if piped-stripping semantics become a requirement.

---

## 4. Module split proposal

Boring and proportional: six modules plus `main`. "Moves" cites the current locations.

| New file | Moves (evidence) | ~Lines |
|---|---|---|
| `src/main.rs` | `main` (398-404), `Cli` (272-299), `run` (349-369), `expand_path` (443-450) | 80 |
| `src/theme.rs` | `Theme` (27-41), `BUILTIN_THEMES` (43-220), `config_file_path` (222-224), `available_theme_names`/`theme_by_name`/`resolve_theme*` (226-270), `Color` + `paint` (replaces 1214-1238) | 330 |
| `src/auth.rs` | `AuthRecord` (300-324), `load_auth` (406-441), `persist_auth` (699-797), `TempAuthFile` (582-585 + 633-656), `next_temp_auth_path` (658-670), `create_private_temp_file` (672-692), `sync_directory_for` (694-697), `refresh_access_token` (799-868), `parse_jwt*` (881-897, 1862-1883) | 470 |
| `src/api.rs` | `build_headers` (587-602), `fetch_usage` (473-580), `fetch_reset_credits` (604-632), `normalize_base_url` (452-471), client construction + timeout consts | 260 |
| `src/usage.rs` | `ParsedUsage`/`RateLimit`/`UsageWindow`/`AdditionalRateLimit`/`ResetCredit` (326-347, 371-379, 899-903), `parse_usage_payload` + window/rate-limit/additional parsers (1240-1345), `usage_status` (1347-1371), `parse_timestamp_to_ms`/`parse_rfc3339_to_ms`/`days_from_civil` (1892-1995), `as_*` helpers (1997-2021), the shared `epoch_value_to_ms` | 350 |
| `src/format.rs` | `window_label` (1406-1421), `slugify` (1423-1450), `title_case_slug` (1452-1469), `additional_limit_slug` (1373-1404), `normalize_usage_label` (1199-1202), `fit_width` (1204-1212), `human_duration` (1491-1521), duration consts | 180 |
| `src/render.rs` | `UsageItem`/`UsageStatus`/`BankReset` (381-396, 905-909), `print_usage_report` (915-935), `format_account_plan_line` (937-946), `collect_banked_resets`/`push_banked_reset` (947-989), `print_banked_resets` (991-1011), `collect_items`/`build_usage_item` (1013-1092), `render_usage_item`/`render_usage_bar`/`render_bar_cells` (1094-1197), `resolve_reset_text`/`resolve_reset_time` (1471-1489), `output_is_interactive` (911-913), `now_millis` (1885-1890) | 430 |

**Visibility and test-coupling implications:**
- The single `mod tests` (1523-1860) imports 17 private items (10 functions, 6 structs, 1 const — 1525-1530). After the split, each test group moves into its owning module as `#[cfg(test)] mod tests` with `use super::*`; child modules see private parent items, so **no visibility loosening is needed**. Don't make anything `pub` for tests' sake.
- Cross-module couplings: render.rs needs `Theme`/`Color` from theme.rs and `ParsedUsage` fields from usage.rs → make struct fields `pub(crate)` (dumb data; keeps them out of any future public API). api.rs and render.rs both consume `AuthRecord` → keep the struct in auth.rs and keep the token fields **non-`pub`**, exposing `pub(crate)` accessors, so "who touches tokens" stays auditable in one file.
- `now_millis` (1885-1890) is used by render code and tests → lives in render.rs as `pub(crate)`.
- Test seams worth taking while splitting: `fetch_usage` builds its client inline (477-480), which is why the refresh/retry flow is untestable. Change its signature to take `client: &Client, base: &str` (constructed in `run`), so api.rs tests can exercise 401→refresh→retry against a local/mock transport. Keep `render_bar_cells` taking `Color` (per §3) so its tests (1535-1570) assert cell counts instead of raw `"\x1b[…m"` substrings — those ANSI assertions are the one place tests are coupled to an encoding that §3 removes.
- `refresh_access_token` could equally live in api.rs (it is an HTTP call); auth.rs is justified because it mutates `AuthRecord`. Pick one, note it, move on.
- Mechanics: one module per commit; the compiler drives the `use` fixes.

---

## 5. Clippy

Current state: no `[lints]` in Cargo.toml, no clippy.toml. Two things surface immediately on a pedantic run: `items_after_test_module` (10 items sit after `mod tests`: parse_jwt_claim, parse_jwt, now_millis, parse_timestamp_to_ms, parse_rfc3339_to_ms, days_from_civil, as_string, as_bool, as_f64, as_u64 — 1862-2021) and style-group nits such as the pointless `items.drain(..)` (930).

Proposed Cargo.toml table:

```toml
[lints.rust]
unsafe_code = "forbid"

[lints.clippy]
all      = { level = "warn", priority = -1 }
pedantic = "warn"

# --- deliberate relaxations for this codebase ---
cast_sign_loss           = "allow" # u64 epoch math: 1167-1169, 1409-1415, 1981-1983
cast_possible_truncation = "allow" # same sites; ranges provably bounded
cast_precision_loss      = "allow" # `as f64` on u64 seconds: 1167, 1409-1415
module_name_repetitions  = "allow" # usage::UsageWindow etc. read fine
missing_errors_doc       = "allow" # binary crate, not a library API
missing_panics_doc       = "allow"
must_use_candidate       = "allow" # churn without value here
wildcard_imports         = "allow" # `use super::*` in per-module tests
similar_names            = "allow" # reset_text/reset_at, bar_primary/bar_background
option_if_let_else       = "allow" # current if-let style is readable
```

Deliberately **kept on** despite pedantic:
- `too_many_lines` — fires on `fetch_usage` (473-580, ~108 lines) and `persist_auth` (699-797, ~99 lines). That pressure is exactly the [2.1]/[2.2] extraction; allow later only if `persist_auth` legitimately stays long.
- `unwrap_used` / `expect_used` — production code has exactly two sites (487 `.unwrap()` on a static template, 763 `.expect(...)`); the test module has 10 expects (1806-1858). Enable both and put `#![allow(clippy::expect_used, clippy::unwrap_used)]` inside the test module so the flood becomes one scoped suppression. (Drop these two if even that feels noisy — they are optional.)
- `unreadable_literal` — not needed; all large literals are already digit-grouped.

Expected friction on first `cargo clippy --all-targets -- -D warnings`: ~25-35 warnings — casts (~8 sites), `option_if_let_else` (~6), doc-lints (auto-allowed), `items_after_test_module` (1, mechanical move), `too_many_lines` (2, fixed by the §2 refactor). Roughly one afternoon including the small fixes.

---

## 6. General engineering practices

**[6.1] [high] Color output is never gated** — ANSI escapes print unconditionally to stdout (917-918, 940-945, 1001-1010, 1117-1125); `codex-usage > out.txt` or any CI run captures escape garbage. Only the bar/spinner is tty-gated (911-913, 482, 916). Fix alongside §3: a `--color auto|always|never` flag, default auto = tty && NO_COLOR unset.

**[6.2] [high] Spinner leaks on early-error paths** — `?` at 505/509/533 exits `fetch_usage` without `finish_and_clear` while the steady tick (491) keeps animating; fix in [2.2].

**[6.3] [high] Test coverage gaps.** The 16 tests (1534-1859) cover bar geometry, labels, banked-reset collection, and one persist round-trip. Untested: (a) the 401/403-refresh-retry decision (527) — extract `fn should_refresh(status, refresh_token: Option<&str>) -> bool` and table-test it; (b) `normalize_base_url` (452-471): host rewrite, port handling, path dropping, invalid-URL fallback; (c) `usage_status` (1347-1371): the 0.9 boundary and the 1.0 × `allowed`/`limit_reached` matrix; (d) the RFC3339 offset path (1951-1967) — only "Z" is tested (1755); (e) `window_label`'s day/hour/minute mapping (only prefix geometry is tested, 1648-1653); (f) `persist_auth` collision-exhaustion (745-759).

**[6.4] [medium] Identical if/else arms — main.rs:1143-1146.** In `render_usage_item`'s `None` branch, both arms print the same `println!("{: <22} {}", prefix, line)` (and `{: <22}` ≡ `{:<22}`). Collapse to one statement, or restore whatever distinction was intended.

**[6.5] [medium] `normalize_base_url` silently swallows bad input — main.rs:470.** An unparseable `--base-url` quietly becomes `DEFAULT_BASE_URL`, so a typo silently queries chatgpt.com. Return an error (or at minimum warn to stderr); non-http schemes are also accepted unchecked.

**[6.6] [medium] Persist failure discards a successful fetch — main.rs:355-360.** `persist_auth(...)?` (357) aborts `run` before `usage_result?` (360): if the token file cannot be rewritten, the user gets an error even though the usage fetch succeeded. Prefer: attempt persist, warn on stderr, continue to rendering.

**[6.7] [medium] RFC3339 parser accepts impossible dates — main.rs:1969-1976.** Day validation is a flat `1..=31` for every month (Feb 30 parses to a wrong epoch) and there is no leap-year rule. Either validate per-month or adopt `time`/`jiff` with parsing-only features (replaces 1915-1995, ~80 lines of subtle hand-rolled code).

**[6.8] [medium] Magic numbers.** Bar width 44 (1159); meter/window widths 12/11 and the 22-col pad (1105-1107, 1130, 1144-1146, 1160) — the pad is a no-op since the prefix is already ≥27 visible cols; warning/exhausted thresholds 0.9/1.0 (1361-1368); timeouts 20s/10s (478, 609, 806); 32 temp-file attempts (737); the 1e12 epoch cutoff (1479, 1908). Name the semantic ones (`WARNING_FRACTION`, `BAR_WIDTH`, `USAGE_TIMEOUT`).

**[6.9] [low] `as_u64` routes integers through f64 — main.rs:2015-2021.** Values above 2^53 lose precision. Try `value.as_u64()` first; keep the f64 path only for "123.0"-style strings.

**[6.10] [low] `window_label` rounding quirks — main.rs:1408-1416.** 86_399s renders "24 hours" rather than "1 day"; 59s renders "1 minute". Make bucket-vs-round a deliberate, tested choice.

**[6.11] [low] Hardcoded version in the user agent — main.rs:21.** `USER_AGENT_VALUE` pins the version string; use `concat!("codex-usage-rs/", env!("CARGO_PKG_VERSION"))`.

**[6.12] [low] Stream mixing in `print_banked_resets` — main.rs:996-997.** The blank separator goes to stderr in bar mode while all content goes to stdout — vestigial from an older stderr-drawn bar; unify on stdout.

**[6.13] Secrets hygiene — good, with two nits.** Tokens are never logged; error bodies are length-capped via `trim_body` (870-879); the temp file is created 0600 and removed on drop (650-656, 672-692) with an atomic rename (786) — solid. Keep `AuthRecord` free of `Debug`/`Display` derives (300-308) and add a comment stating that this is deliberate, so a future derive cannot leak tokens. Nit: the built-in OAuth client id lives inside a method (319-323) — it is a public PKCE identifier, not a secret, but move it to a documented `const` beside the other `DEFAULT_*` consts.

**[6.14] [low] Cargo/release/CI.** `[profile.release]` only sets `strip` (Cargo.toml:19-20): add `lto = "thin"` and `codegen-units = 1` for a start-latency CLI. `is_multiple_of` (1878) requires a recent stable Rust (1.87+): pin `rust-version` in Cargo.toml. No README/LICENSE/CI: a minimum CI job is `cargo fmt --check && cargo clippy --all-targets -- -D warnings && cargo test`.

**[6.15] [low] Dependency proportionality (taste).** `config` 0.15 (and its tree) reads exactly one key (246-259); `indicatif` 0.18 is used only for a 12-line spinner (483-495) while bars are hand-rolled anyway. Both defensible; if binary size or build time matters, hand-parse the one TOML key and drop indicatif.

---

## Suggested implementation order

1. **Mechanical, zero-risk (half a day):** move the 10 post-test items (1862-2021) above `mod tests`; collapse the identical if/else (1143-1146); replace `items.drain(..)` with iteration (930); const-ify magic numbers ([6.8]); user-agent version from `CARGO_PKG_VERSION` ([6.11]); add the `[lints]` tables from §5 and fix the first clippy pass.
2. **Small behavior wins:** spinner RAII guard ([2.2]/[6.2]); extract the duplicated success block with an injectable `&Client` ([2.1]); add tests for `should_refresh`, `normalize_base_url`, `usage_status` ([6.3a-c]); make `normalize_base_url` error on bad input ([6.5]); persist-failure warn-and-continue ([6.6]).
3. **Color migration** — the prerequisite for the planned "themes defined via color values": `Color` enum + `paint` choke point ([1.2]/[3.1]); `--color` flag + NO_COLOR/tty gating ([6.1]); re-point bar-cell tests at cell counts.
4. **Data-model cleanups:** `BankReset.expires_in` → `Option<u64>` seconds, count as `u64` ([1.3]); single `epoch_value_to_ms` ([1.4]); move phrasing-coupled test assertions to value assertions.
5. **Module split (§4)** — mechanical and best done last so it carries already-cleaned code; one module per commit; tests move with their functions.
6. **Optional:** RFC3339 via `time`/`jiff` ([6.7]); release profile + CI + `rust-version` ([6.14]); dependency trimming ([6.15]).
