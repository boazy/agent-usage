# Rust Terminal-Styling Crates for agent-usage (research as of 2026-09-05)

**Use case:** CLI app printing styled ANSI to stdout, then exiting; possible later ratatui adoption.
**Key question:** which styling API survives a ratatui migration instead of becoming a dead end?

## TL;DR / Shortlist

1. **anstyle + anstream — the recommended pick.** anstyle types are *the* interop language of the Rust CLI ecosystem, and **ratatui-core ships first-class, bidirectional `From`/`TryFrom` conversions between `anstyle::Style` and `ratatui::style::Style`** (behind an optional `anstyle` feature, `ratatui-core/src/style/anstyle.rs`). Zero-dep styling + automatic stream adaptation. Migration later is literally `Style::from(anstyle_style)`.
2. **owo-colors — strong runner-up.** Actively maintained (4.4.0, Aug 2026), ergonomic, and the official `anstyle-owo-colors` adapter (updated Mar 2026) bridges it to anstyle types; anstream's own docs list owo-colors as a recommended pairing for runtime styling.
3. **crossterm's style module — safe but heavyweight.** Its color/attribute model maps nearly 1:1 to ratatui (ratatui's default backend *is* crossterm 0.29), but pulling all of crossterm just for println styling is a lot of dependency.

**Avoid for this use case:** `colored` and `console` (string-wrapper APIs, no reusable `Style` value to transfer), `yansi` and `nu-ansi-term` (no conversion path to ratatui; one stale, one niche). Newcomers (`farben`, `terminal_style`, `opaline`) are too young/0.x to bet on.

---

## Comparison table

| Crate | Latest (date) | Truecolor | Auto NO_COLOR / tty detection | → ratatui transferability | Deps (normal) | MSRV | License |
|---|---|---|---|---|---|---|---|
| anstyle | 1.0.14 (2026-03-13) | Yes (`RgbColor`) | Styling only — no detection; pair with anstream | **Native**: `From<anstyle::Style> for ratatui Style` + reverse, colors & effects (ratatui-core, `anstyle` feature) | **0** | 1.66.0 | MIT OR Apache-2.0 |
| anstream | 1.0.0 (2026-02-11) | n/a (stream) | **Yes** — AutoStream auto-detects tty/capabilities and strips/degrades; NO_COLOR respected (via `colorchoice`/`anstyle-query`) | Complementary: adapts the stream, any style crate writes into it | 5 (+2 opt: anstyle-query, anstyle-wincon) | 1.66.0 | MIT OR Apache-2.0 |
| owo-colors | 4.4.0 (2026-08-27) | Yes (`.truecolor()`, `DynColors`) | Opt-in: `if_supports_color()` via `supports-colors` feature (supports-color: tty + NO_COLOR + FORCE_COLOR + TERM=dumb); plain `OwoColorize` emits ANSI unconditionally | Indirect but official: `anstyle-owo-colors` 2.0.5 (`to_owo_style`/`to_owo_colors`) | 0 (+1 opt) | 1.83 | MIT |
| yansi | 1.0.1 (2024-03-13) | Yes (`Color::Rgb`) | Yes with features: `Condition::STDOUT_IS_TTY` (tty), `Condition::NO_COLOR`/env-var conditions (`detect-tty`, `detect-env` features); not automatic by default | None known — no adapter crate; manual mapping (its `Color`/quirks don't map cleanly) | 0 (+1 opt is-terminal) | 1.63 | MIT OR Apache-2.0 |
| colored | 3.1.1 (2026-01-16) | Yes (`.truecolor()`, `CustomColor`) | Yes — `control::SHOULD_COLORIZE` respects NO_COLOR and tty | **Poor**: `Colorize` returns `ColoredString` (a *string wrapper*); no standalone `Style` type to convert | 1 (windows-sys) | 1.80 | MPL-2.0 |
| console | 0.16.4 (2026-07-01) | Yes + `true_colors_enabled()` | **Yes** — `colors_enabled()` auto-detects tty; NO_COLOR/CLICOLOR respected; `set_colors_enabled()` override | **Poor**: `style()` returns `StyledObject` tied to `console::Style`; manual mapping needed | 2 (+2 opt) | 1.71 | MIT |
| crossterm (style) | 0.29.0 (2025-04-05) | Yes (`Color::Rgb`) | **NO_COLOR yes** (since 0.27, incl. `force_color_output` override; 0.29 fixed a bare-`CSI m` bug when NO_COLOR set); **tty: no** — emits ANSI regardless | **Very high**: ratatui 0.30 depends on crossterm 0.29 as default backend; concepts (Color, Attribute/Modifier, ContentStyle→Style) map ~1:1 | 3 (+many opt: events, serde, winapi…) | 1.63.0 (unreleased: 1.85) | MIT |
| nu-ansi-term | 0.50.3 (2025-10-10) | Yes (`Color::Rgb`) | **None** — pure types/formatters, no detection at all | Poor-moderate: `Style`/`Color` are plain data (mappable by hand), but no adapters and a divergent model (`Fixed`, `Infix`/`Prefix`/`Suffix`) | 1 (+1 opt serde) | 1.62.1 | MIT |

### Notable newcomers (all verified on crates.io 2026-09-05)

| Crate | Latest (date) | What it is | Risk assessment |
|---|---|---|---|
| farben | 0.20.0 (2026-06-05; created 2026-03-13) | Markup-macro styling (`cprintln!("[bold red]…[/]")`), optional anstyle interop, MPL-2.0 | Interesting but <6 months old, pre-1.0; no ratatui path beyond anstyle |
| terminal_style | 0.5.0 (2026-03-10; created 2025-07-29) | Minimal ANSI styling lib, MIT | Tiny ecosystem, no detection story, no adapters |
| opaline | 0.4.2 (2026-09-02; created 2026-02-25) | Token-based *theme engine* with adapters for ratatui/owo-colors/crossterm, MSRV 1.85, MIT | Orthogonal: it layers *on top of* a styling crate; watch, don't depend yet |

---

## Per-crate detail & verdicts

### anstyle + anstream — **recommended**
- **anstyle 1.0.14** (2026-03-13): zero dependencies, `const`-friendly style/color/effect types (`AnsiColor`, `Ansi256Color`, `RgbColor`, `Effects`). MSRV 1.66.0, MIT OR Apache-2.0. It defines *types*, not output — no detection, no printing.
- **anstream 1.0.0** (2026-02-11; the crate's 1.0 milestone): wraps stdout/stderr in `AutoStream`, which automatically degrades ANSI per terminal capability and honors NO_COLOR-type conventions (detection delegated to `colorchoice`/`anstyle-query`, both optional). MSRV 1.66.0. Its own docs recommend pairing it with anstyle for public APIs or owo-colors/color-print for runtime/compile-time styling.
- **ratatui transferability — the decisive fact:** ratatui-core (0.1.2, dep of ratatui 0.30.2, June 2026) ships `src/style/anstyle.rs` with `From<anstyle::Style> for ratatui::style::Style` *and* the reverse, `From<anstyle::Color> for Color` (+ `TryFrom` back, panicking only on `Color::Reset`), and `Effects ↔ Modifier` (all five underline styles collapse to `Modifier::UNDERLINED` going to anstyle). Gated behind the optional `anstyle = "^1"` dependency in ratatui-core's Cargo.toml.
- **Verdict:** styling you write as `anstyle::Style` constants converts to ratatui with a one-line `.into()`. Not a dead end — it's the convergence point. anstyle is also what clap uses, making it the de-facto standard for CLI styling types.

### owo-colors — **strong alternative, safe**
- **4.4.0** (2026-08-27, actively maintained; 159.9M total / 33.6M recent downloads). MSRV 1.83, MIT, zero deps (`supports-color` behind the opt-in `supports-colors` feature).
- Truecolor: yes (`.truecolor(r,g,b)`, `DynColors`). Detection: **opt-in, not automatic** — `x.if_supports_color(Stream::Stdout, |t| t.red())` runs the supports-color heuristic (tty, NO_COLOR, FORCE_COLOR, CI, TERM=dumb); plain `.red()` always emits ANSI.
- ratatui transferability: official `anstyle-owo-colors` 2.0.5 (updated 2026-03-13 alongside anstyle 1.0.14) converts `anstyle::Style ⇄ owo_colors::Style`; and anstream's docs explicitly present owo-colors as a supported runtime-styling pairing with `anstream::println!`. So the practical pattern *owo-colors for ergonomics + anstream for stream adaptation* is documented, and anstyle types remain one adapter hop away from ratatui.
- Caveats: MSRV 1.83 is the highest of the mature crates; per-value closure style is noisier than anstyle's data model.

### crossterm style module — **capable, but overkill for plain printing**
- **0.29.0** (2025-04-05). MSRV 1.63.0, MIT. Unreleased changes raise MSRV to 1.85 and drop `IsTty` in favor of `std::io::IsTerminal`.
- Truecolor: yes (`Color::Rgb`). NO_COLOR: respected since 0.27 with `force_color_output` override; 0.29/unreleased fixed color commands emitting a bare reset when NO_COLOR is set. TTY detection: **not automatic** — `Stylize`/`StyledContent` `Display` emits escape codes regardless of stream type (tty check is on you).
- ratatui transferability: highest conceptual overlap — ratatui 0.30's default backend *is* crossterm 0.29 (required dep), and ratatui's `Color`/`Modifier` model descends from it. Hand-mapping `ContentStyle`/`Stylize` usage to `ratatui::style::Style` is mechanical.
- Caveat: ~3 mandatory deps (bitflags, parking_lot, rustix) plus platform plumbing, and you drag an event-loop-capable library into a print-and-exit CLI. Use it only if you already expect heavy TUI overlap.

### yansi — **capable but frozen; conversion-wise a dead end**
- **1.0.1**, released **2024-03-13** — no release in ~2.5 years. 250.3M total downloads (very widely embedded, e.g. via cargo-audit's tree), MSRV 1.63, MIT OR Apache-2.0, zero deps.
- Truecolor: yes. Detection: good design — `Condition::STDOUT_IS_TTY` / `STDERR_IS_TTY`, `Condition::NO_COLOR`/env conditions behind `detect-tty`/`detect-env` features, global `whenever()`, const `Style` builders. But it's *declarative opt-in*, not automatic.
- ratatui transferability: no adapter crate exists; yansi's `Quirk`-based model (bright, wrap, linger, mask) has no direct ratatui equivalent. Migration = manual re-authoring of styles.
- Verdict: fine for a frozen minimal CLI, wrong choice given a probable ratatui future.

### colored — **ergonomic dead end for styles**
- **3.1.1** (2026-01-16), actively maintained, MSRV 1.80, **MPL-2.0** (note: license-different from the MIT/Apache rest), 1 dep (windows-sys).
- Truecolor: yes (`.truecolor()` / `CustomColor`). Detection: automatic — `control::SHOULD_COLORIZE` handles NO_COLOR + tty.
- ratatui transferability: **poor**. The API centers on `Colorize` methods returning `ColoredString` — a string wrapper with embedded style — not a reusable style value. There is a `colored::Style` struct but it's a combinator, and no conversion crates exist. You'd rebuild every style by hand.

### console — **great runtime, poor migration path**
- **0.16.4** (2026-07-01), maintained (console-rs / dtolnay-adjacent), MSRV 1.71, MIT, deps: encode_unicode + windows-sys (+opt libc, unicode-width).
- Truecolor: yes, with explicit `true_colors_enabled()`/`set_true_colors_enabled()`. Detection: best-in-class automatic — tty-based `colors_enabled()` per stream, NO_COLOR/CLICOLOR handling, `set_colors_enabled()` override, `user_attended()`.
- ratatui transferability: **poor**. `style(x).cyan()` returns a `StyledObject` formatter; `console::Style` is not convertible to ratatui types and no adapter exists. Also brings `Term`, `Key`, progress-bar-adjacent machinery you don't need. Choose console for its dialoguer/indicatif family, not for a ratatui-ready style value.

### nu-ansi-term — **no detection, niche**
- **0.50.3** (2025-10-10), maintained by the nushell team, MSRV 1.62.1, MIT, deps: windows-sys (+opt serde). 507.6M total downloads.
- Truecolor: yes (`Color::Rgb`). Detection: **none whatsoever** — pure type/formatting library (fork of ansi_term with gradients, byte strings, `AnsiStrings` diff-optimization).
- ratatui transferability: `Color`/`Style` are plain data so a hand-written `From` is ~30 lines, but nothing official exists and ratatui has no interop with it. Nushell-specific lineage; not the ecosystem convergence point.

### Newcomers — **watch list, not picks**
- **farben 0.20.0** (2026-06-05): markup-macro styling with optional native anstyle support, auto NO_COLOR/FORCE_COLOR, OKLCh color spaces, MPL-2.0, no declared MSRV. Created 2026-03 — too young, pre-1.0.
- **terminal_style 0.5.0** (2026-03-10): minimal truecolor/8-bit styling, MIT. No detection, no adapters, created 2025-07.
- **opaline 0.4.2** (2026-09-02): token-based theme engine with adapters to ratatui/owo-colors/crossterm/anstyle; MSRV 1.85, MIT. It presupposes rather than replaces a styling crate; revisit if themes become a need.

---

## Answer to the key question

**anstyle is not a dead end — it's the endpoint.** Ratatui itself accepts anstyle types first-class: `ratatui::style::Style::from(anstyle::Style)` is maintained inside ratatui-core (feature `anstyle`), covering fg/bg/underline colors and all effects. A CLI built today on `anstyle` for style *data* + `anstream` for automatic stream adaptation/NO_COLOR/tty handling can adopt ratatui by changing print sites only; every `anstyle::Style` constant moves into widgets with `.into()`.

**owo-colors is the closest safe second:** alive and updated (Aug 2026), zero-dep core, and bridged to anstyle by the official `anstyle-owo-colors` adapter plus anstream's documented support. Its ergonomic dead-end risk is the per-value `if_supports_color` closure style, which doesn't map to widgets — keep your styles as anstyle constants and use owo-colors only at the print boundary if you pick it.

**crossterm::style** transfers conceptually (ratatui's backend and type model), but its NO_COLOR support without tty detection, heavy dep tree, and 1.85 MSRV trajectory (unreleased) make it a poor fit for a print-and-exit CLI.

**Dead ends for a ratatui future:** colored and console (string-wrapper output, no style value to convert), yansi and nu-ansi-term (capable types, zero interop, one stale / one niche), and the 2026 newcomers (immature).

## Sources
- crates.io API: owo-colors, yansi, anstyle, anstream, colored, console, crossterm, nu-ansi-term, farben, terminal_style, opaline, anstyle-owo-colors, ratatui, ratatui-core (versions, rust_version, license, dependencies, created/updated dates) — retrieved 2026-09-05.
- docs.rs: yansi 1.0.1 crate docs (conditions, quirks); nu-ansi-term 0.50.3 docs; colored 3.1.1 docs; supports-color 3.0.2 docs; anstream 1.0.0 docs (AutoStream, recommended pairings); console 0.16.4 docs; crossterm 0.29.0 style module docs (`force_color_output`); ratatui-core 0.1.2 `Style`/`Color` docs — retrieved 2026-09-05.
- GitHub `ratatui/ratatui`: `ratatui-core/src/style/anstyle.rs` (full conversion impls) and `ratatui-core/Cargo.toml` (optional `anstyle = "^1"`) — retrieved 2026-09-05.
- GitHub `crossterm-rs/crossterm` `CHANGELOG.md`: NO_COLOR added in 0.27 (with force override); unreleased MSRV 1.85 + `IsTty` removal — retrieved 2026-09-05.
- Web search corroboration (2026-09-05): owo-colors 4.4.0 release/MSRV reporting (crates.io, docs.rs, sunshowers.io rust-cli recommendations); anstyle/anstream ecosystem status; ratatui 0.30 integration coverage (ratatui.rs, reddit r/rust).
