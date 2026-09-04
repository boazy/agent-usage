# Rust TUI Frameworks for codex-usage (2026 survey)

Research date: 2026-09-05. All versions/dates pulled live from the crates.io registry API, docs.rs, and the GitHub API. External research only — no repo reads.

## TL;DR verdict table

| Framework | Last release (crates.io) | Maintenance | One-shot ANSI→stdout | Interactive TUI | Truecolor | NO_COLOR auto | Dep weight | License |
|---|---|---|---|---|---|---|---|---|
| **ratatui** (+crossterm) | 0.30.2 — 2026-06-19 | Very active (repo pushed 2026-09-04; 22.5k★, 48.7M dl) | **Yes** — alt screen is opt-in; one `draw()` on a plain stdout backend | Yes (Fullscreen / Inline / Fixed viewports) | Yes (`Color::Rgb`) | No (app-side) | Light-moderate, no runtime deps | MIT |
| tui-rs (`tui`) | 0.19.0 — 2022-08-14 | **Dead** (repo fdehau/tui-rs archived, last push 2023-08-06) | — | — | — | — | — | MIT |
| cursive | 0.21.1 — 2024-08-03 | Maintenance mode (repo pushed 2026-08-01; 4.8k★) | No (retained-mode event loop) | Yes, rich | Yes via crossterm backend | No | Heavy (multi-backend, libc/ncurses) | MIT |
| tuirealm (tui-realm) | 4.1.0 — 2026-05-02 | Active (repo pushed 2026-07-29; 995★) | No (component/event model on ratatui) | Yes | Inherits ratatui | No | Moderate-heavy (pulls tokio, futures, termion, termwiz) | MIT |
| iced | 0.14.0 — 2025-12-07 | Active (31.4k★) | **N/A — no terminal backend** | N/A | — | — | Very heavy (winit/wgpu) | MIT |
| iocraft | 0.9.1 — 2026-09-04 | Very active (166k dl) | Not first-class [INFERENCE] | Yes (React-like, flexbox via taffy) | Yes (crossterm) | No | Moderate (crossterm, taffy, futures) | MIT OR Apache-2.0 |
| r3bl_tui | 0.7.8 — 2026-01-23 | Active (391k dl) | No | Yes (Elm/React-inspired) | Yes | No | Moderate-heavy | Apache-2.0 |
| ratzilla | 0.3.1 — 2026-06-08 | Active | N/A (WASM/web target) | N/A | — | — | — | MIT |
| turbo-vision | 2.3.0 — 2026-09-04 | Active but tiny (1.2k dl) | No | Yes (Borland-style windows) | Yes | No | Moderate | MIT |

## Per-framework detail

### 1. ratatui + crossterm — the default choice

- **Status:** ratatui 0.30.2 (2026-06-19; 0.30.0 shipped 2025-12-26), MIT, 48.7M total downloads. Repo `ratatui/ratatui` pushed 2026-09-04 (crates.io API, GitHub API). crossterm 0.29.0 (2025-04-05), MIT, 184M downloads.
- **One-shot to stdout without alternate screen: YES, first-class.** The alternate screen + raw mode are opt-in conveniences, not requirements:
  - `ratatui::run()` / `ratatui::init()` → alternate screen + raw mode (docs.rs, *ratatui::init*, 0.30.2).
  - `ratatui::init_with_options()` → **raw mode only, no alternate screen** (docs.rs explicitly documents this; key-differences table in the `init` module).
  - Fully manual path: `Terminal::with_options(CrosstermBackend::new(io::stdout()), TerminalOptions { viewport })` and a single `terminal.draw(...)` — no crossterm screen commands at all; ANSI escapes go straight to stdout, then the process exits. Ideal for the one-shot report mode.
  - Viewports: `Fullscreen`, `Inline(height)` (UI embedded in normal CLI scrollback, with `Terminal::insert_before` for content above), `Fixed(Rect)` (docs.rs `Viewport`, 0.30.2). `Inline` is purpose-built for dashboards that coexist with regular output.
- **Widget set:** Table, Block/panes, Layout/Constraint system, Gauge, BarChart, Sparkline, Chart, List, Paragraph, Scrollbar, calendar, canvas, and a large third-party widget ecosystem. Covers every need (boxes/panes, tables, bars) for a usage dashboard.
- **Color model:** 16/256/truecolor (`Color::Rgb`), plus gradients via the `palette` crate. **NO_COLOR is not honored automatically** — neither ratatui nor crossterm consults it; the app must gate styling itself (crossterm only offers `force_color_output`).
- **Dep weight:** 0.30 split into workspace crates (`ratatui-core`, `ratatui-widgets`, `ratatui-crossterm`, plus optional `termina`/`termion`/`termwiz` backends, `ratatui-macros`). No async runtime; crossterm pulls mio/rustix/signal-hook. Reasonable for a 2k-line CLI.
- **License:** MIT (all releases).
- **Fit:** Excellent. One framework, two modes, shared widget code: mode (a) = one `draw()` on a plain stdout backend; mode (b) = `ratatui::init()` event loop. The ratatui book / init docs show exactly this split.

### 2. tui-rs — do not use

- Original `fdehau/tui-rs`: last crate release `tui 0.19.0` on **2022-08-14** (crates.io); repo **archived** (GitHub API; last push 2023-08-06, 10.8k★). The README delegates to ratatui. Superseded; any future bugfixes land only in ratatui.

### 3. cursive — solid but wrong shape for this app

- **Status:** 0.21.1 (2024-08-03), MIT, 1.9M downloads; repo `gyscos/cursive` pushed 2026-08-01 but last release ~2 years old — in maintenance cadence (crates.io, GitHub API).
- **Model:** retained-mode, self-contained event loop (`siv.run()`), high-level views (Dialog, TextView, SelectView, plus ecosystem crates: `cursive_table_view`, `cursive_tree_view`, `cursive-tabs`…). Default backend is **crossterm** (README); ncurses/pancurses/termion/BearLibTerminal backends optional.
- **One-shot mode:** poor fit. No "render once to stdout" API; you'd construct a `Cursive` root, immediately `quit()` — and output still goes through the full-screen backend lifecycle rather than plain stdout. [INFERENCE from documented `siv.run()` model.]
- **Color:** theme-based; truecolor available through the crossterm backend; NO_COLOR not automatic.
- **Dep weight:** heavy for a CLI — `cursive_core` plus backend deps (ncurses, pancurses, termion, libc, crossbeam-channel, …) even if several are optional-by-feature.
- **Fit:** Not recommended here; you'd also lose widget sharing with the one-shot mode since it's a different rendering model entirely.

### 4. tuirealm / tui-realm — ratatui + structure you don't need

- **Status:** `tuirealm` 4.1.0 (2026-05-02), MIT, 229k downloads; `veeso/tui-realm` pushed 2026-07-29 (crates.io, GitHub API).
- **Model:** Elm/React-inspired component framework **on top of ratatui** (Message/Update/view cycle, focus management, prebuilt components: input, radio, select, table…).
- **One-shot mode:** no; it exists to impose an app architecture over the ratatui render loop.
- **Color:** inherits ratatui/crossterm (truecolor yes; NO_COLOR app-side).
- **Dep weight:** 4.x normal deps include **tokio, futures-util, tokio-util, async-trait**, plus termion/termwiz backends — a runtime dependency a ~2k-line sync CLI doesn't want.
- **Fit:** skip for now; if a ratatui app outgrew ad-hoc component management, tuirealm is the established add-on layer (same render stack, easy retrofit later).

### 5. iced — no terminal story

- **Status:** 0.14.0 (2025-12-07), MIT, 31.4k★ — GUI only (winit + wgpu/tiny-skia backends).
- **Terminal support: none.** No official terminal backend; the only related crates are dead experiments (`iced-pancurses`, last updated 2020) or the opposite direction (`iced_term` embeds a terminal emulator *inside* an iced GUI window). Not applicable to either required mode.

### 6. Notable 2025–2026 entrants

- **iocraft** (`ccbrown/iocraft`) — 0.9.1, updated 2026-09-04, MIT OR Apache-2.0, 166k downloads. Declarative, function-component + hooks API with taffy flexbox layout, built on crossterm (deps: crossterm, taffy, futures, iocraft-macros). Genuinely active and pleasant for the interactive mode; but its model is a continuous render/event loop — a one-shot "print rich report and exit" is not a first-class flow [INFERENCE]. Also adds a JSX-macro + async styling to learn. Worth watching; ratatui still fits the dual-mode requirement better.
- **r3bl_tui** (r3bl-open-core) — 0.7.8 (2026-01-23), Apache-2.0, 391k downloads. Elm/React-style with editor/markdown components; opinionated and heavier; no one-shot mode.
- **ratzilla** (orhun) — 0.3.1 (2026-06-08), MIT. Ratatui compiled to WASM for the *web* — interesting but wrong target for a CLI stdout dashboard.
- **turbo-vision** — 2.3.0 (2026-09-04), MIT, but only ~1.2k downloads; Borland-style windowed TUI; niche.
- **dioxus-tui** — 0.4.3, last release 2024-02-23 — stale.
- Verify-before-trusting note: generic "new framework" search results mention unverifiable names (e.g. "slt" — not found on crates.io); the list above is limited to crates verified in the registry API.

## Shortlist & tradeoffs

1. **ratatui + crossterm (recommend).** Only option that natively serves both modes with shared code: mode (a) via manual `Terminal` on plain stdout (no alt screen, one `draw()`, exit) or `Viewport::Inline`; mode (b) via `ratatui::init()`/`run()` fullscreen loop. Biggest widget ecosystem, healthiest maintenance (0.30.2, June 2026), MIT, no async runtime. Work to accept: NO_COLOR support is a ~5-line app-side check; you write the layout/event glue yourself.
2. **ratatui + tuirealm (conditional).** If the interactive side grows into many focusable components, layer tuirealm on top — same ratatui render stack so the one-shot mode stays untouched. Cost: tokio/futures in the dep tree. Not needed at ~2k lines.
3. **iocraft (runner-up).** Best of the newer entrants for the interactive mode (declarative + flexbox); weaker story for one-shot ANSI-to-stdout and a smaller ecosystem.
4. **Not recommended:** tui-rs (archived), cursive (retained-mode loop conflicts with one-shot mode; heavy deps; release cadence stalled at Aug 2024), iced (no terminal backend), ratzilla (web target), turbo-vision/dioxus-tui (niche/stale).

## Sources (accessed 2026-09-05)

- crates.io registry API: [`ratatui`](https://crates.io/api/v1/crates/ratatui) (0.30.2, 2026-06-19), [`crossterm`](https://crates.io/api/v1/crates/crossterm) (0.29.0, 2025-04-05), [`tui`](https://crates.io/api/v1/crates/tui) (0.19.0, 2022-08-14), [`cursive`](https://crates.io/api/v1/crates/cursive) (0.21.1, 2024-08-03), [`tuirealm`](https://crates.io/api/v1/crates/tuirealm) (4.1.0, 2026-05-02), [`iced`](https://crates.io/api/v1/crates/iced) (0.14.0, 2025-12-07), [`iocraft`](https://crates.io/api/v1/crates/iocraft) (0.9.1, 2026-09-04), [`r3bl_tui`](https://crates.io/api/v1/crates/r3bl_tui) (0.7.8, 2026-01-23), [`ratzilla`](https://crates.io/api/v1/crates/ratzilla) (0.3.1, 2026-06-08), [`turbo-vision`](https://crates.io/api/v1/crates/turbo-vision) (2.3.0, 2026-09-04), [`dioxus-tui`](https://crates.io/api/v1/crates/dioxus-tui) (0.4.3, 2024-02-23) — version/date/license/dependency data.
- docs.rs ratatui 0.30.2: [`ratatui::init` module](https://docs.rs/ratatui/latest/ratatui/init/index.html) (alt-screen vs. raw-mode-only matrix; `Viewport::Inline` usage) and [`Viewport` enum](https://docs.rs/ratatui/latest/ratatui/enum.Viewport.html) (Fullscreen/Inline/Fixed semantics).
- GitHub API: `fdehau/tui-rs` (archived; last push 2023-08-06), `ratatui/ratatui` (pushed 2026-09-04, 22.5k★, MIT), `gyscos/cursive` (pushed 2026-08-01), `veeso/tui-realm` (pushed 2026-07-29), `iced-rs/iced` (31.4k★), `crossterm-rs/crossterm` (pushed 2026-09-01).
- docs.rs [`cursive 0.21.1` README + normalized Cargo.toml](https://docs.rs/crate/cursive/0.21.1/source/Cargo.toml) (crossterm default backend; optional ncurses/pancurses/termion/BLT).
- no-color.org and crossterm `force_color_output` docs — NO_COLOR is an app-side concern; neither ratatui nor crossterm auto-honors it.
