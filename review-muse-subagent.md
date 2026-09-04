# Code review: codex-usage CLI (`src/main.rs`, ~2021 lines)

Independent review. Scope: `src/main.rs`, `Cargo.toml`, `.gitignore` only. No files modified, no binary executed, no network calls. All line references are `src/main.rs:<line>` unless noted. No secrets encountered (only structural references to token fields); none quoted below.

---

## 1. Primitive obsession

### [high] `AppResult<T> = Result<T, String>` erases error context everywhere — src/main.rs:16
Every fallible function returns `Result<_, String>`, built via `.map_err(|e| e.to_string())` at ~20 call sites (e.g. `:248`, `:407`, `:479`, `:700`, `:808`). Consequences: no error kinds, so `run` (`:349`) cannot distinguish "auth file missing" from "network down" from "bad theme name"; no source chains; messages are composed ad hoc at each site (`"usage request failed: {e}"` at `:502`, `"usage endpoint returned HTTP {}..."` at `:575`). `build_headers` (`:587`) even returns the bare `Result<HeaderMap, String>` instead of the alias, showing the alias carries no weight.
**Recommendation (minimal):** keep the CLI's flat `eprintln!("error: {err}")` UX in `main` (`:398`), but introduce a small `enum Error { Auth(String), Network(String), Api { status, body }, Config(String), Io(...) }` with `impl From<std::io::Error>` / `From<serde_json::Error>` / `From<reqwest::Error>` and a single `Display`. That deletes most `map_err` closures without adding ceremony. Do NOT pull in `anyhow`/`thiserror` for a CLI this size — a 30-line enum is enough.

### [high] `Theme` mixes two unrelated color domains as `&'static str` — src/main.rs:27-41
`bar_warning/bar_exhausted/bar_unknown` (`:31-33`) are *indicatif-style color names* (`"yellow"`, `"bright_yellow"`), while `bar_primary/bar_background/meter_color/window_color/reset_color` (`:34-40`) are *raw ANSI escape strings* (`"\x1b[38;2;...m"`, `"\x1b[38;5;...m"`). Nothing in the type prevents swapping them; the compiler sees identical `&'static str`. The cost is real, not theoretical: `render_usage_item` (`:1094`) must translate the name-domain through `ansi_color_from_name` (`:1214`) while the ANSI-domain passes through untouched (`:1104`), and `BUILTIN_THEMES` (`:43-220`, 16 nearly identical 11-line literals) can silently mix the two. A typo like `bar_warning: "\x1b[33m"` would compile and render garbage.
**Recommendation:** split into two newtypes now, even before any color-crate migration: `struct Ansi(&'static str)` for pre-baked escapes and `enum StatusColor { Black, Red, Yellow, ... BrightCyan, ... }` for the status word, with `fn ansi(self) -> &'static str` as the single conversion point (replacing `ansi_color_from_name`). Minimal, zero-dependency, kills the mix-up class.

### [medium] `BankReset.expires_in: String` bakes rendering into the model — src/main.rs:905-909, :947-989
`collect_banked_resets` (`:947`) formats `"expires in {}"` / `"no expiry reported"` / `"expiry not reported"` (`:961-974`) at collection time, and `push_banked_reset` (`:984`) takes `Option<String>` where `None` silently drops the entry (`:985-987`) — a `Stringly` filter masquerading as a parameter. Tests then assert on English prose (`:1679`, `:1727`) instead of on data, so any copy change breaks tests and any new consumer (e.g. `--json`, sorting by expiry) must re-parse English.
**Recommendation:** `struct BankReset { source: String, expires_at_ms: Option<u64> }` and move the `human_duration` call into `print_banked_resets` (`:991`). Same for the count-fallback arm (`:969-977`): represent as a distinct variant or `source` + `None`, not a pre-formatted sentence.

### [medium] `UsageItem` stores display strings, not facts — src/main.rs:381-388, :1078-1092
`meter: String`, `window_label: String`, `reset_text: Option<String>` are all formatted at `build_usage_item` time (`:1085-1091`) via `window_label()` and `resolve_reset_text()`. The renderer (`:1094`) then just concatenates. This makes `collect_items` (`:1013`) untestable except through pixels and freezes decisions (e.g. "resets in …" phrasing) into the data pipeline.
**Recommendation:** store `limit_window_seconds: Option<u64>` and `reset_at_ms: Option<u64>` on `UsageItem`; format in `render_usage_item`. Keeps one formatting choke point per concern (see §2).

### [medium] Timestamps are `Option<u64>` with undocumented ms-vs-s ambiguity — src/main.rs:326-332, :900-903, :1477-1489, :1892-1913
`UsageWindow.reset_at` and `ResetCredit.expires_at_ms` are bare `u64`s; one is documented-ms-by-name, the other is not. The epoch-magnitude heuristic (`> 1_000_000_000_000` ⇒ ms) exists in *two* places (`:1479`, `:1908`) precisely because the type doesn't say what unit it holds. `now_millis` (`:1885`) returns `u64` millis with `unwrap_or(0)` clock-skew collapse (see §6).
**Recommendation:** `struct Millis(u64)` (or `std::time::SystemTime`) for every "absolute time" field, with `fn from_api_value(u64) -> Millis` as the single heuristic home. Keep `reset_after_seconds: Option<u64>` as `Duration`-shaped (`Option<std::time::Duration>` is arguably nicer but `u64` seconds is fine — don't over-abstract durations that are only ever added to `now`).

### [low] `used_percent: Option<f64>` unvalidated at the boundary — src/main.rs:329, :1306, :1115
`parse_usage_window` (`:1305`) accepts any `f64` including `NaN`/negative/huge; clamping happens later in two different ways (`percent_raw.clamp(0.0, 100.0)` at `:1115`; `(percent/100.0).clamp(0.0,1.0)` at `:1356`). Fine at this scale, but the parse fn is the right home: clamp-or-reject once, document that `used_percent` is always finite `0..=100`.
**Recommendation:** normalize in `parse_usage_window` (map non-finite → `None`), keep renderer clamp as defense-in-depth.

### [low] JWT claim-path constants are fine as `&str` — src/main.rs:22-23 — no change
`JWT_AUTH_CLAIM`/`JWT_PROFILE_CLAIM` as `&'static str` is appropriate; a newtype here would be pure ceremony. Noted explicitly so this isn't "fixed" later.

---

## 2. Duplicated logic / single source of truth

### [high] `fetch_usage` success path is copy-pasted for first vs retried request — src/main.rs:508-521 vs :534-547
The "parse payload → maybe fetch reset credits → finish spinner → return" block appears twice, differing only in surrounding context. Any change (e.g. adding a second auxiliary fetch, changing spinner text) must be made in both; they already drift in message wording only by accident of structure.
**Recommendation:** extract `fn finish_success(client, base, auth, spinner, payload) -> AppResult<ParsedUsage>` containing lines `:510-520` verbatim, call from both arms. ~12 lines, zero behavior change.

### [high] Epoch-magnitude heuristic lives in two homes — src/main.rs:1477-1489 vs :1892-1913
`resolve_reset_time` (`:1478-1484`: `if raw_reset_at > 1_000_000_000_000 { ms } else { s*1000 }`) and `parse_timestamp_to_ms` (`:1908-1912`: identical threshold) both guess seconds-vs-milliseconds. Worse, they can *disagree*: `parse_timestamp_to_ms` handles string/RFC-3339 inputs while `resolve_reset_time` only sees the already-parsed `u64`, so an RFC-3339 `reset_at` string parses correctly in one path (reset credits, `:627`) but would be misinterpreted in the other (usage windows, `:1309` via `as_u64` — actually a string `reset_at` in a usage window silently becomes `None` there, a latent inconsistency).
**Recommendation:** canonical home is `parse_timestamp_to_ms`. Make `parse_usage_window` use it for `reset_at` (accept `Value`, not pre-extracted `u64`), and reduce `resolve_reset_time` to pure `Option` plumbing over an already-`Millis` field (see §1 `Millis` proposal). These two findings should be fixed together.

### [medium] Two duration formatters with different dialects — src/main.rs:1406-1421 vs :1491-1521
`window_label` renders `7 days / 5 hours / 30 minutes` (rounded to a single largest unit, long words) while `human_duration` renders `1d 1h 1m` (up to 3 parts, short suffixes). Both hand-roll `86_400`/`3_600`/`60` divisors. A reader (and the test at `:1648-1653`, which asserts a "22 visible cols" geometry against `fit_width("7 days", 11)`) must learn both dialects.
**Recommendation:** keep both dialects (they serve different UI slots: window bucket vs countdown) but extract `const SECS_PER_DAY/HOUR/MINUTE: u64` and document the contract on each fn: "`window_label`: single largest unit, rounded, words" vs "`human_duration`: up to 3 components, short suffixes". Do NOT unify the output — the two slots genuinely want different shapes.

### [medium] "ms-diff → human_duration" computed in two places — src/main.rs:957-964 vs :1471-1475
`collect_banked_resets` (`:959-961`: `expires_ms.saturating_sub(now_ms)/1000` → `human_duration`) and `resolve_reset_text` (`:1473-1474`: identical shape) duplicate the saturating-diff-then-format idiom. After the §1 `BankReset` fix both call sites become `fn reset_in_text(reset_ms: Millis, now_ms: Millis) -> String`; until then, at least route both through one helper.

### [medium] `render_usage_item` duplicates its tail across `Some`/`None` arms — src/main.rs:1113-1149
The `reset_text` append (4 lines) and `status_label` append (4 lines) appear identically in both arms (`:1118-1125` vs `:1135-1142`); only the head (`5.1%` vs `[unknown usage percentage]`) differs. The `None` arm further contains a dead branch: `if use_progress … else …` with byte-identical bodies (`:1143-1147`). The `Some` arm's non-progress path (`:1130`: `"{:<22} {}"`) vs progress path via `render_usage_bar` (`:1128`, which itself formats `"{prefix:<22}{bar} {line}"` at `:1160`) is a third near-duplication of the same geometry.
**Recommendation:** build `line: String` once per arm (only the head differs), then a shared tail helper `append_reset_and_status(&mut line, item, theme)`, then a single `if use_progress` emit. Deletes ~15 lines.

### [medium] `codex_`/`codex-` prefix stripping duplicated — src/main.rs:1389-1398 vs :1423-1430
`additional_limit_slug` strips `codex_`/`codex-` (`:1393-1397`) and then calls `slugify`, which strips them *again* (`:1425-1427`). Harmless today (idempotent) but signals no canonical home. Also `normalize_usage_label` (`:1199-1202`) reimplements "lowercase → slugify → title-case" that call sites could share.
**Recommendation:** `slugify` owns prefix-stripping (keep `:1425-1427`); delete `:1393-1397` and let `additional_limit_slug` call `slugify` directly on the raw source. `normalize_usage_label` stays as the named combo.

### [low] Header rendering pattern duplicated — src/main.rs:915-918 vs :1001-1002
`"Codex plan usage"` + `"================"` and `"Bank resets"` + `"------------"` are the same "title + underline" idiom with hand-counted underline lengths (the second is exactly wrong-proof only by inspection). Minor.
**Recommendation:** `fn print_header(title: &str, theme: &Theme)` where the underline is `"=".repeat(title.chars().count())` (or `"-"` variant param). Trivial; do it while touching render code.

### [low] Auth JWT re-derivation duplicated — src/main.rs:418-431 vs :855-865
`load_auth` derives `oauth_client_id`/`account_id`/`email` from `id_token ?? access_token`; `refresh_access_token` re-derives a subset with slightly different fallback order (id-token-first, then access-token-email-only at `:863`). If claim layout changes, two places must move.
**Recommendation:** `fn derive_identity(access_token: &str, id_token: Option<&str>) -> (Option<String> client_id, Option<String> account_id, Option<String> email)` used by both. Note the current behavioral difference (`refresh_` keeps old `account_id` when re-parse fails via `.or_else(|| auth.account_id.clone())` at `:858-859`, while `load_auth` has no old value) — preserve it at the call site, not inside the helper.

### [low] `colorize` unconditionally appends `ANSI_RESET` — src/main.rs:1236-1238
`format!("{ansi}{text}{ANSI_RESET}")` emits a stray `\x1b[0m` even when `ansi == ""` (the `ansi_color_from_name` fallback at `:1232`, and the `UsageStatus::Ok` `status_color = ""` at `:1096`). Output then contains resets with no corresponding set — harmless on real terminals, noisy in `--no-progress` log capture and in test snapshots.
**Recommendation:** `if ansi.is_empty() { text.to_string() } else { format!(...) }`. One line; also makes the `Ok`-status path allocation-free-ish.

---

## 3. Themes as ANSI vs colors

Current state: `Theme` (`:27-41`) stores 5 raw ANSI escapes + 3 indicatif color-name strings; `BUILTIN_THEMES` (`:43-220`) is ~178 lines of stringly palettes; the only abstraction is `ansi_color_from_name` (`:1214-1234`, a 16-arm hand-rolled SGR table) plus `colorize` (`:1236`) and inline `format!` interpolation (`:1107-1111`). `indicatif` itself only renders the spinner (`:483-495`) — the bar is hand-drawn (`:1166`), so indicatif color names serve no framework need; they exist only to feed the hand-rolled table.

Constraints specific to this CLI: (a) two gamuts are in use — 256-color (`38;5;N`) for meter/window/reset text and truecolor (`38;2;R;G;B`) for the bar pair — so any migration must preserve both or consciously collapse them; (b) output has two modes, interactive-bar (`render_usage_bar`, `:1152`) and log-friendly (`--no-progress` / non-tty, `:482`, `:916`), plus `--json` which bypasses color entirely (`:361`); (c) small binary, `strip = true` release (`Cargo.toml:19-20`), rustls already dominates build time — dependency weight matters more than usual.

Options:

| Approach | Fidelity | NO_COLOR / non-tty | Cost |
|---|---|---|---|
| A. Plain RGB triples + one formatting choke point (no new dep) | Exact: keep today's `38;5;N` values by mapping each used palette index to its RGB once, or store both | Handled in the one choke point (`std::env::var("NO_COLOR")`, `output_is_interactive` already exists at `:911`) | ~40 lines, zero build cost |
| B. `anstyle` (clap already pulls it) | Good for 8/16 colors; weak for 256-palette + truecolor pairs | First-class `anstyle` query support | Near-zero new weight, but awkward API for the bar's interpolated pair |
| C. `owo-colors` | Full truecolor + 256, tiny, no macros needed | Manual `NO_COLOR` check still needed | Small dep, best ergonomics-to-weight for this use |
| D. `nu-ansi-term` / `console` / `colored` | Full | Varies; `console` handles colors-when-piped best | Heavier or more opinionated; unjustified here |

**Recommendation: A now, C only if A proves painful. Concretely:**

1. Change `Theme` to data, not escapes:
```rust
struct Rgb { r: u8, g: u8, b: u8 }
struct Theme {
    name: &'static str,
    status_warning: StatusColor,   // today's bar_warning names, as enum
    status_exhausted: StatusColor,
    status_unknown: StatusColor,
    bar_primary: Rgb, bar_background: Rgb,
    meter: Rgb, window: Rgb, reset: Rgb,
}
```
2. One choke point: `fn paint(rgb: Rgb, text: &str) -> String` that emits `\x1b[38;2;r;g;bm{text}\x1b[0m`, returns `text.to_string()` when `NO_COLOR` is set or output is non-tty. All 5 current ANSI fields and `colorize` route through it; `ANSI_RESET`/`ANSI_BOLD` consts (`:24-25`) retire except inside `paint` (bold should become an explicit param or be dropped — today bold is applied to the meter prefix at `:1108` unconditionally).
3. Converting the existing 256-palette entries: map each used `38;5;N` index to its canonical xterm RGB tuple once (16-entry lookup table in `theme.rs`, tested). This changes exact shade output not at all if the table is correct, and collapses the two gamuts into one truecolor path. Risk: terminals without truecolor support will dither differently than native `38;5;N` — acceptable in 2026, and note it in the PR. If that risk is unacceptable, store `enum Color { Ansi256(u8), Rgb(u8,u8,u8) }` instead — same choke point, ~10 more lines, zero fidelity debate.
4. Do NOT add `owo-colors`/`anstyle`/`nu-ansi-term` in this pass. The CLI's color needs are exactly "paint foreground in a theme color, reset" — one function. A crate earns its place only if themes later need backgrounds, styles, or user-defined CSS-ish theme files. `anstyle`'s presence via clap is not a reason to adopt its API for rendering.

Fidelity note: today's bar pair (`bar_primary` truecolor + `bar_background` muted same-hue) is hand-tuned per theme (e.g. `:52-53`, `:63-64`); keep the pairs as data — do NOT "derive background by darkening primary at runtime" unless a designer signs off, since several backgrounds are hue-shifted, not just darkened.

---

## 4. Module split of `main.rs`

Proportional to a ~2k-line CLI: 6 small modules + a thin `main.rs`. No workspaces, no traits, no plugin architecture.

| File | Moves in (current location) | Public surface (`pub(crate)`) |
|---|---|---|
| `src/main.rs` (~60 lines) | `Cli` (`:278-299`), `main` (`:398`), `run` (`:349-369`) | — (binary root) |
| `src/theme.rs` | `Theme`, `BUILTIN_THEMES` (`:27-220`), `config_file_path`, `available_theme_names`, `theme_by_name`, `resolve_theme_name`, `resolve_theme` (`:222-270`), `ansi_color_from_name` (`:1214`), `colorize`, `ANSI_RESET`, `ANSI_BOLD` (`:24-25`, `:1236`) → later `paint`/`Rgb`/`StatusColor` | `Theme`, `resolve_theme`, `available_theme_names`, `paint`/`colorize` |
| `src/auth.rs` | `AuthRecord` + impl (`:300-324`), `load_auth` (`:406`), `expand_path` (`:443`), persist machinery `TempAuthFile` + impls, `next_temp_auth_path`, `create_private_temp_file`, `sync_directory_for`, `persist_auth` (`:582-797`), `DEFAULT_AUTH_PATH` (`:18`) | `AuthRecord`, `load_auth`, `persist_auth`, `expand_path` |
| `src/api.rs` | `fetch_usage`, `build_headers`, `fetch_reset_credits` (`:473-632`), `refresh_access_token` (`:799-868`), `trim_body` (`:870`), `DEFAULT_BASE_URL`, `USER_AGENT_VALUE` (`:19`,`:21`) | `fetch_usage`, (tests need `trim_body` → `pub(crate)`) |
| `src/usage.rs` | `ParsedUsage`, `UsageWindow`, `RateLimit`, `AdditionalRateLimit`, `ResetCredit`, `BankReset`, `UsageItem`, `UsageStatus` (`:326-396`, `:371-388`, `:899-909`), parse fns (`:1240-1345`), `usage_status`, `additional_limit_slug`, `slugify`, `title_case_slug`, `normalize_usage_label`, `window_label`, `resolve_reset_text/time`, `human_duration`, `collect_*`, `build_usage_item` (`:947-1092`, `:1347-1521`), `as_*` helpers (`:1997-2021`) | Nearly everything `pub(crate)` — this is the most-tested module (see coupling) |
| `src/render.rs` | `print_usage_report`, `format_account_plan_line`, `print_banked_resets`, `render_usage_item`, `render_usage_bar`, `render_bar_cells`, `fit_width`, `output_is_interactive` (`:911-946`, `:991-1238`) | `print_usage_report` |
| `src/time.rs` | `now_millis`, `parse_timestamp_to_ms`, `parse_rfc3339_to_ms`, `days_from_civil` (`:1885-1995`) | `now_millis`, `parse_timestamp_to_ms` |
| `src/jwt.rs` (or fold into `auth.rs`) | `parse_jwt`, `parse_jwt_aud`, `parse_jwt_claim`, `JWT_*_CLAIM` (`:22-23`, `:881-897`, `:1862-1883`) | `parse_jwt_aud`, `parse_jwt_claim` |

Notes:
- **Keep `Cli` in `main.rs`.** It's 22 lines; a `cli.rs` for one struct is ceremony.
- **Fold-or-split judgment:** if 7 modules feels like too many, fold `jwt.rs` into `auth.rs` (sole consumer) and `time.rs` into `usage.rs`. Never fold `render.rs` into `usage.rs` — the model-vs-display separation (§1, §2) is the point of the split.
- **Visibility:** `pub(crate)` on all cross-module items; fields `pub(crate)` within moved structs. No `pub` (this is a binary, not a library — nothing external can import it).
- **Test coupling (hidden):** `mod tests` (`:1523-1860`) does `use super::{...}` (`:1525-1530`) importing `collect_banked_resets`, `collect_items`, `fit_width`, `format_account_plan_line`, `human_duration`, `parse_timestamp_to_ms`, `persist_auth`, `render_bar_cells`, plus model structs and `BUILTIN_THEMES`. After the split those become `use crate::usage::{...}; use crate::render::...; use crate::theme::...; use crate::auth::...; use crate::time::...`. Options: (a) move each test to the module it covers (preferred — unit tests live next to code; the persist test goes to `auth.rs`, bar tests to `render.rs`, slug/duration tests to `usage.rs`/`time.rs`); (b) keep one integration-style `tests/` dir. (a) is boring and standard. Either way the `super::` import list is the checklist that nothing was left private.
- **Order of operations:** split files with zero logic changes first (pure move, `cargo check` green), then do §1/§2 refactors inside the new homes. Do not combine moves with behavior edits in one commit.

---

## 5. Clippy

Current state: **no `[lints]` config at all** (`Cargo.toml:1-20` has no `[lints.clippy]` section; no `#[warn/deny]` attributes in `main.rs`). The code is default-clippy-clean by inspection, but default clippy leaves the interesting lints off.

Proposed `Cargo.toml` addition (concrete table):

```toml
[lints.clippy]
# Correctness-adjacent: keep all of these clean.
uninlined_format_args = "warn"       # `{e}` instead of `{}", e` — code already does this; enforce it
redundant_clone = "warn"             # AuthRecord/UsageWindow derive Clone; catches sloppy clones
len_zero = "warn"
needless_bool_assign = "warn"
# Maintainability: worth the noise at this size.
too_many_lines = "warn"              # flags fetch_usage (~110 lines) and persist_auth (~100) — intended
struct_field_names = "warn"          # UsageWindow.reset_after_seconds/reset_at, ParsedUsage.reset_credits* — intended friction, see below
upper_case_acronyms = "warn"         # Cli.auth_file vs URL/URI casing in normalize_base_url
# Pedantic, enabled selectively (NOT blanket pedantic):
cast_possible_truncation = "warn"    # as_u64's `f as u64` (:2020), `d.as_millis() as u64` (:1888)
cast_precision_loss = "warn"         # `(filled as f64 / 100.0)` (:1167), day/hour float division (:1409-1415)
cast_sign_loss = "warn"              # i64→u64 in parse_rfc3339_to_ms (:1980-1984)
missing_errors_doc = "warn"          # forces docs on new fallible fns once Error enum exists (§1)
missing_panics_doc = "warn"          # documents the .expect in persist_auth (:762), ProgressStyle .unwrap (:487)
```

Deliberately NOT enabled (with reason):
- `pedantic` blanket (`-D clippy::pedantic`): too noisy for a 2k-line CLI. It would flag `must_use_candidate` on ~40 pure fns, `missing_errors_doc`/`missing_panics_doc` before the Error enum exists, `doc_markdown` on every `Codex`/`JWT` mention in comments, and `unsafe_derive_deserialize`-style non-issues. Selective opt-in above captures 90% of the value at 10% of the friction.
- `unwrap_used` / `expect_used` (restriction): the codebase has exactly two (`:487` template, `:762` guard invariant) — both legitimate; a lint would add noise, not safety.
- `indexing_slicing` (restriction): `text.get(0..4)?`/`bytes[4]` slicing in `parse_rfc3339_to_ms` (`:1915-1937`) is bounds-careful via `?`/length guard; pedantic indexing complaints here would obscure real logic.
- `struct_excessive_bools` / `fn_params_excessive_bools`: `usage_status(Option<bool>, Option<bool>)` and `RateLimit{allowed, limit_reached}` would trip these, but the fix is domain modeling (an enum for limit state), not lint suppression — handle via §1-style refactor, not a lint.
- `module_name_repetitions`, `wildcard_imports`: irrelevant until the split; revisit after §4.

Expected friction: low. `cast_*` will require 3-5 `#[allow]` or explicit `u64::try_from` at the float↔int boundaries (`:1167`, `:1409-1415`, `:1888`, `:2020`) — that's the point (the `f as u64` in `as_u64` at `:2020` silently saturates negatives-after-check; the check-then-cast deserves a comment). `too_many_lines` will flag `fetch_usage` and `persist_auth` — resolve by doing the §2 `finish_success` extraction, not by raising the threshold. `struct_field_names` will flag `reset_after_seconds`/`reset_at`/`reset_credits*` — the names are genuinely confusing (see §6 naming); rename rather than allow.

---

## 6. General engineering practices

### [high] No test covers the refresh flow, auth loading, or status thresholds — src/main.rs:1523-1860
16 tests cover: bar geometry (2), account-line redaction (1), slug/display mapping (3), banked resets (4), timestamp parsing (1), human_duration (3), persist_auth round-trip (1), label width (1). Untested: `load_auth` (token fallback chain `:410-431`), `refresh_access_token` (entire `:799-868`, including the keep-old-value fallbacks at `:848-865`), `fetch_usage` 401→refresh→retry state machine (`:527-570`), `usage_status` thresholds (`:1347-1371` — the `frac >= 1.0` + `allowed==Some(true)` interaction is the subtlest logic in the program), `parse_usage_payload`/`parse_rate_limit`/`parse_usage_window` edge cases (all-absent → `None` at `:1289-1295`/`:1311-1317` untested), `normalize_base_url` (`:452-471`), `window_label`/`resolve_reset_time` boundaries, and `needs_persisted_refresh` (notably: it compares `access_token/id_token/refresh_token/account_id/email` at `:311-316` but **omits `oauth_client_id`** — so a rotated client-id alone never persists; likely benign, but untested either way).
**Recommendation:** add focused unit tests for `usage_status` (9-row table: below/at/above 90%, ≥100% × allowed × limit_reached, NaN/None), `normalize_base_url` (chatgpt.com/chat.openai.com/port/trailing-slash/garbage), `resolve_reset_time` (s-vs-ms boundary at exactly `1_000_000_000_000`), and `load_auth` with temp-dir fixtures (mirroring the existing `persist_auth` test pattern at `:1799-1859`). The refresh HTTP flow needs no mock server — extract the "apply refresh response JSON to AuthRecord" half of `refresh_access_token` (`:840-865`) into a pure fn and test that.

### [high] Secrets hygiene is good but relies on discipline, not structure — src/main.rs:587-602, :799-830, :870-879
Verified: tokens go only into `Authorization` headers (`:589-593`) and the refresh form body (`:810-816`); display paths use only `email`/`plan_type` (`:937-946`); `--json` prints the *usage* payload, never auth (`:361-364`); the account-id redaction test (`:1572-1595`) locks this in. Two residual risks: (1) `trim_body` (`:870-879`) echoes up to 1000 chars of *server response bodies* into error strings (`:553-556`, `:562-567`, `:825-829`) — server errors shouldn't contain secrets, but a misbehaving proxy could reflect the `Authorization` header, and the CLI would then print it via `eprintln!` (`:401`); (2) `needs_persisted_refresh` + `persist_auth` rewrite `auth.json` preserving unknown fields (`:700-729`, tested at `:1829-1835`) — correct, but file-permission hardening (`0o600` at `:678`/`:685`, verified at `:1848-1856`) only applies to the temp file; a pre-existing world-readable `auth.json` keeps its mode after atomic rename. **Recommendation:** (1) scrub `trim_body` output for `Bearer`/JWT-looking substrings or cap reflected errors at a smaller limit with a static note — 5 lines; (2) after `fs::rename`, explicitly `set_permissions(0o600)` on the destination too, not just the temp. Both cheap, do with the Error-enum work.

### [medium] Magic numbers/strings without names — src/main.rs passim
`1_000_000_000_000` s/ms threshold (`:1479`, `:1908`); `86_400`/`3_600`/`60` (`:1408-1415`, `:1496-1499`, `:1978`); column widths `12`/`11`/`22` (`:1105-1106`, `:1130`, `:1160`) with the `22` geometry asserted only in a comment (`:1649`); bar width `44` (`:1159`); timeouts `20s` (`:478`, `:806`) vs `10s` (`:609`); temp-file attempts `0..32` (`:737`); body limit `1_000` (`:871`); spinner ticks `["◒","◐","◓","◑"]` + `80ms` (`:488-491`); status threshold `0.9`/`1.0` (`:1361-1368`); OAuth URL + fallback client-id string (`:811`, `:322`); header name `"ChatGPT-Account-Id"` (`:597`); endpoint paths `/wham/usage`, `/wham/rate-limit-reset-credits` (`:475`, `:605`).
**Recommendation:** a `const` block for timeouts, widths, thresholds, and endpoint paths (the endpoint paths and header name belong in `api.rs` after the split). Don't const-ify spinner glyphs or the fallback client id beyond what's there — named consts for values nobody will ever tune is clutter. Priority: widths (layout bugs) and the `0.9` threshold (behavior).

### [medium] `needs_persisted_refresh` omits `oauth_client_id` — src/main.rs:311-317
Five of six `AuthRecord` fields are compared; `oauth_client_id` (`:307`, derived at `:418-421`/`:856`) is not. After a refresh that rotates only the client id, `run` (`:356`) skips `persist_auth`, silently dropping the rotation in memory only. Probably harmless (it's re-derivable from the id token on next launch), but it's either a bug or an undocumented intentional exclusion.
**Recommendation:** include it in the comparison (one line) or comment why excluded. One-line fix; do while adding the refresh tests above.

### [medium] Clock handling: `unwrap_or(0)` collapses clock failure to the epoch — src/main.rs:1885-1890, :658-670
`now_millis` returns `0` when `SystemTime` is before `UNIX_EPOCH`; downstream `saturating_sub` (`:960`, `:1473`) then treats every expiry as far-future. Same pattern in `next_temp_auth_path` (`:660-663`, nanos `unwrap_or(0)` — harmless there since pid+attempt disambiguate). No crash, but "time went backwards → all resets show huge durations" is a confusing failure mode.
**Recommendation:** low priority; if touching `time.rs`, make `now_millis` return `Option<u64>` and have `print_usage_report` fall back to "reset time unknown" instead of epoch math. Don't gold-plate.

### [low] Release profile is minimal — Cargo.toml:19-20
Only `strip = true`. Missing `opt-level`/`lto`/`codegen-units`/`panic` settings are all defensible defaults for a CLI this size (defaults are `opt-level=3`, and LTO buys little on a reqwest-dominated binary), so this is not a defect — but `lto = true` + `codegen-units = 1` would shrink the binary measurably if distribution size matters.
**Recommendation:** leave as-is unless binary size becomes a complaint; if it does, add `lto = true` first (biggest win), nothing else.

### [low] `.gitignore` is `target/` only — .gitignore:1
Fine. No `.env`, credentials, or IDE artifacts are generated by this project; nothing to add. If CI artifacts or `cargo-audit` outputs appear later, revisit.

### [low] Naming nits (fix opportunistically, not as a pass)
- `reset_after_seconds` vs `reset_at` (`:330-331`): one is a duration, one an instant — rename to `reset_in_seconds` / `reset_at_ms`.
- `reset_credits_available: Option<u64>` vs `reset_credits: Option<Vec<ResetCredit>>` (`:376-377`): the count-vs-detail pair reads ambiguously; `reset_credit_count` / `reset_credit_details`.
- `bar_mode: bool` param of `print_banked_resets` (`:991`): actually means "precede with stderr newline when interactive bars were drawn" — rename to `interactive` or `precede_with_blank_stderr`.
- `UsageStatus::Ok` (`:392`): reads as `Result::Ok`; `Active`/`Normal`/`Healthy` is clearer in a status enum. (Careful: touches the `match` at `:1095` and any future `must_use` docs.)
- `push_banked_reset` (`:984`): a 6-line wrapper around `Vec::push` with an `Option` filter — inline it at the two call sites (`:965`, `:971`) once `BankReset` holds data (§1).

---

## Suggested implementation order

1. **Error enum + `trim_body` scrub + `persist_auth` dest permissions** (§1-high, §6-high parts) — unlocks `missing_errors_doc` clippy lint; kills the only secret-reflection risk. Small, safe, no UI change.
2. **`fetch_usage` `finish_success` extraction + `render_usage_item` tail dedup** (§2-high/medium) — pure deletions of duplication, `too_many_lines` goes quiet, sets up the module split.
3. **Unit-test gap fill: `usage_status` table, `normalize_base_url`, `resolve_reset_time` boundary, `load_auth` fixtures, refresh-apply pure fn** (§6-high) — locks behavior *before* the moves below.
4. **`Millis` newtype + unify s/ms heuristic into `parse_timestamp_to_ms`** (§1-medium, §2-high) — the one semantic refactor; protected by the tests from step 3.
5. **Module split, moves-only, `cargo check` green** (§4) — mechanical after steps 1-4; move tests with their modules.
6. **Theme data-ification: `Rgb` + `StatusColor` + `paint` choke point, `NO_COLOR` handling** (§3) — the visible-output change; isolated by now, reviewable as one diff.
7. **`BankReset`/`UsageItem` data-not-text + header helper + `colorize` empty-ansi guard + naming nits** (§1-medium, §2-low, §6-low) — polish pass.
8. **`[lints.clippy]` table + CI-ready `cargo clippy -- -D warnings`** (§5) — last, so the new module layout is what gets linted; fix `cast_*`/threshold findings as they surface.

*Total shape: steps 1-3 are a weekend afternoon with no user-visible change; steps 4-6 are the real work, each independently reviewable; steps 7-8 are hygiene. Do not reorder 3 after 4 — the tests must predate the timestamp refactor.*
