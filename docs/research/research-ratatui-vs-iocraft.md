# ratatui 0.30.x vs iocraft 0.9.x — Head-to-Head for a Rust Usage-Dashboard CLI

Researched 2026-09-05. Sources: crates.io API, docs.rs (0.9.1 / 0.30.2), GitHub (ccbrown/iocraft, ratatui/ratatui) READMEs, CHANGELOGs, source at tags `iocraft-v0.9.1` (main) and `ratatui-v0.30.2`. All numbers fetched live on 2026-09-05 unless marked [INFERENCE].

---

## 1. Built-in styling / color APIs

### ratatui 0.30.2
- **`Color` enum** (`ratatui_core::style::color`, tag `ratatui-v0.30.2`): `Reset`, 16 named ANSI colors (Black…White incl. bright variants), `Indexed(u8)` (256-color), `Rgb(u8,u8,u8)` (truecolor). Also `Color::from_u32(0xRRGGBB)` const helper and serde support.
- **`Style` struct**: builder with `.fg()`, `.bg()`, `.add_modifier()`, `.patch()`; `Styled`/`Stylize` traits give shorthands on strings/spans/widgets (`"hello".red().bold()`). `underline-color` feature flag for colored underlines. Optional `anstyle` and `palette` crate integrations behind features (verified in `ratatui-core/src/style.rs` feature list).
- **`Modifier` bitflags**: BOLD, DIM, ITALIC, UNDERLINED, SLOW_BLINK, RAPID_BLINK, REVERSED, HIDDEN, CROSSED_OUT (stylize.rs docs).
- **Per-widget styling**: every widget takes `Style`/`Styled` (e.g. `Gauge::gauge_style`, `Block::border_style`).
- **256/truecolor**: emitted as SGR 38;5;n / 38;2;r;g;b by the backend (crossterm/termion/termwiz/termina backends).
- **NO_COLOR**: ratatui core has **no** NO_COLOR logic (GitHub code search for `NO_COLOR` in ratatui/ratatui returns 0 hits, checked 2026-09-05). The default crossterm backend inherits crossterm's suppression: `crossterm::style::Colored`'s `ansi_color_disabled()` returns true when `NO_COLOR` is set and non-empty (crossterm `src/style/types/colored.rs`, master) — so colors silently drop, but **the app must handle the "no color → alternative visual encoding" story itself**.

### iocraft 0.9.1
- **`Color` enum** (`src/color.rs`, main): `Reset`, 16 named colors (light/dark pairs), `Rgb { r, g, b }` truecolor, `AnsiValue(u8)` 256-color. iocraft-owned since 0.9.0 (previously a crossterm re-export); `From` conversions to/from crossterm behind the `crossterm` feature.
- **Styling model**: no unified `Style` struct like ratatui. Styling is **per-component props**: `color`, `background_color`, `border_color`, `text_decoration` (UNDERLINE), `weight` (Bold), italic/invert props added in 0.8.4 (CHANGELOG 2026-07-13), `border_style` (single/round/double…), flexbox `LayoutStyle` via taffy.
- **256/truecolor**: `SgrColor` Display emits `38;5;n` for `AnsiValue`, `38;2;r;g;b` for Rgb (color.rs, verified in source).
- **NO_COLOR**: **explicit, first-class support** — `color_output_disabled()` reads `NO_COLOR` (non-empty check, memoized) and suppresses SGR params entirely (color.rs lines 133–162 + unit test `no_color_suppresses_sgr_output`). Matches crossterm semantics, documented in source.

**Verdict**: comparable primitives; ratatui has a richer, composable `Style`+`Modifier`+`Stylize` system; iocraft's NO_COLOR handling is explicit rather than incidental.

---

## 2. One-shot rendering

### iocraft — `element! { ... }.print()`: VERIFIED, first-class
- README "first program" (fetched 2026-09-05): `element! { View(border_style: …, border_color: Color::Blue) { Text(content: "Hello, world!") } }.print();` — renders the component tree once and exits. This is the crate's advertised happy path.
- docs.rs `ElementExt` (0.9.1): `fn print(&mut self)` — "Renders the element and prints it to stdout"; also `eprint`, `write`, `write_to_fd`, `to_string`.
- Implementation (`src/element.rs`, main): `print()` → `write_to_is_terminal(stdout())` → `write_sized_or_plain`:
  - **TTY**: queries terminal size through `CrosstermBackend::query_size()`, renders `el.render(Some(width))`, writes via `canvas.write_ansi()` — width-constrained, colored, single pass, no terminal clearing (no cursor moves; cursor ends after output).
  - **Non-TTY (pipe/file)**: renders with `max_width: None` and `Canvas::write` → **plain text, no ANSI** (write_impl ansi=false). Piping to `less` gives clean text; but there is no built-in "force color when piped" knob.
  - **No backend compiled** (`crossterm` feature off): `Unsupported` error → falls back to plain unstyled output.
- Caveats: color is always emitted when stdout is a TTY (no color-capability probing — truecolor sent regardless of terminal support, matching crossterm ecosystem norms); NO_COLOR suppresses colors; no clearing/spillover concerns since nothing is diffed.

### ratatui 0.30.2 — one-shot is possible but lower-level
- Pattern: `Terminal::with_options(CrosstermBackend::new(stdout()), TerminalOptions { viewport })` + a single `terminal.draw(|f| …)`; `Terminal::draw` autoresizes, diffs buffers, flushes (docs.rs Terminal page; `ratatui-core/src/terminal/init.rs` `with_options`).
- **`Viewport::Inline(height)`** (viewport.rs, 0.30.2): anchors the region at the current cursor row, always spans full terminal width, clamps height to terminal height, shifts up when near bottom. Designed exactly for "UI embedded in a larger CLI flow". `Terminal::insert_before` lets interactive mode print scrollback above the live viewport.
- Alt screen is **opt-in and manual**: `Terminal::new` is fullscreen-flavored but does not enter alternate screen or raw mode; you call crossterm's `EnterAlternateScreen`/`enable_raw_mode` yourself; `Drop` restores the cursor only (BREAKING-CHANGES v0.30.0 note re `show_cursor()` on drop).
- Caveats: you manage raw mode/alt screen yourself; one `draw()` writes the full frame then leaves the cursor below the viewport; a known long-standing issue — **inline viewport flickering under high throughput (issue #584, open since 2023-10-22)** — is irrelevant for single-draw but matters for the interactive mode.
- Width detection: inline viewport derives width from the terminal on `autoresize`; Fixed uses your `Rect`.

**Verdict**: iocraft's one-shot is a single documented method call with sensible TTY/pipe behavior; ratatui's one-shot works (Inline viewport + one draw) but is ~10 lines of setup and you handle crossterm setup/teardown.

---

## 3. Progress bars

### ratatui: built-in widgets
- `ratatui-widgets` 0.3.2 ships **`Gauge`** (percent/ratio, label, `gauge_style`, block, unicode partial blocks) and **`LineGauge`** (single-line with filled/unfilled styles) — `gauge.rs` at tag `ratatui-v0.30.2`, plus `gauge.rs`/`line-gauge.rs` examples and VHS tapes.

### iocraft: no built-in gauge component
- Built-in components (src/components, 0.9.1): `View`, `Text`, `MixedText`, `TextInput`, `ScrollView`, `Button`, `Checkbox`, `Fragment`, `ContextProvider`. **No Gauge/ProgressBar component.**
- The official `examples/progress_bar.rs` hand-rolls one: a `View(width: 60)` border + inner `View(width: Percent(progress), height: 1, background_color: Color::Green)` + percent text, driven by `use_state` + `use_future`. So the flexbox layout makes it easy (~10 lines), but you own it.

### indicatif alongside either?
- indicatif draws via its own crossterm/termion layers to a stream (stderr by default) with its own refresh thread and multi-draw cursor management.
- **iocraft + indicatif**: conflicting. iocraft's render loop owns and row-diffs its canvas region (row-level diff rendering landed in 0.8.3, CHANGELOG 2026-05); indicatif's independent redraws would corrupt/interleave with that region. For a one-shot `print()`, sequential use is fine (print once, then start an indicatif bar), but not concurrent. [INFERENCE from draw paths; no official integration exists]
- **ratatui + indicatif**: same conflict in interactive mode (both reposition the cursor and repaint). The idiomatic path is ratatui's own `Gauge`/`LineGauge` widgets, or `Terminal::insert_before` to push completed lines above an inline viewport.
- **Bottom line for both**: native widget/component is the path; indicatif only works sequentially, never concurrently, with either library.

---

## 4. Maturity deep-dive

| Dimension | ratatui | iocraft |
|---|---|---|
| First release | 2023-02-08 (crates.io) | 2024-09-23 (crates.io) |
| Latest | 0.30.2 (2026-06-19) | 0.9.1 (2026-09-04) |
| Total downloads | 48,654,678 (17.6M last 90d) | 166,641 (51k last 90d) |
| Reverse dependencies | 5,719 | 12 |
| GitHub stars / forks | 22,497 / 765 | 1,531 / 61 |
| Issues open/closed | 139 / 413 (+66 open PRs) | 20 / 60 (+6 open PRs) |
| Contributors (commits) | joshka 435, fdehau 296, orhun 208, kdheepak 142, EdJoPaTo 108, Valentin271 54… (many humans) | ccbrown 193 (~91% excl. bot), everyone else ≤9 |
| Docs | Dedicated site (ratatui.rs, 200 OK), concepts/how-to guides, per-widget docs, BREAKING-CHANGES.md, book-style guides | docs.rs 100% documented ("100% of the crate is documented" — docs.rs 0.9.1 page), README + 16 examples; **no book/site** |
| Ecosystem | Dedicated widgets crate (17M dl), 4 backends (crossterm, termion, termwiz, termina — 0.30.2 added ratatui-termina), 19 widget examples + 4 app examples, tui-prompts/tui-input etc. build on it | 16 examples; no published widget ecosystem (12 reverse deps) |
| Release cadence | Stable minors ~quarterly 2024 (0.27 Jun, 0.28 Aug, 0.29 Oct), then 0.30.0 landed 2025-12-26 after a 13-month alpha cycle; patches 0.30.1/0.30.2 Jun 2026 | 48 versions in 2 years; frequent patch releases; 0.9.0+0.9.1 shipped 2026-09-04 |
| API stability | 0.x semver, majors every ~2–4 months historically; publishes BREAKING-CHANGES.md per version (v0.20→v0.31 sections); 0.30 was a big one: workspace split into ratatui-core/widgets/crossterm/macros, `Backend` gained associated `Error` + `clear_region`, `Marker` non-exhaustive, MSRV 1.86 | 0.x with steady breaking changes: 0.9.0 (2026-09-04) unhooked from crossterm — `Color`, key/mouse event types became iocraft-owned, `write_to_raw_fd`→`write_to_fd`, removed `KeyEventState` re-export; earlier minors regularly renamed props |
| Maintainer responsiveness | Maintainers comment on current issues within ~1 day (e.g. issue #2742 filed 2026-08-30, first maintainer reply same day) | ccbrown responds quickly too (issue #110: filed 2025-07-12, answered same day; #195: answered in 3h) |
| Backlog age | Oldest open issues from 2023–04 (feature requests, e.g. #128 List wrap) — a real but triaged backlog | Oldest open issues from 2024-09 (#13 "more examples", #40 no_std); open bugs #109 (WezTerm rendering inconsistency, open since 2025-07-03), #117 (flickering, open since 2025-08-10) |

### Where iocraft is demonstrably immature TODAY (evidence-backed)
1. **Adoption**: 51k recent downloads and **12 reverse dependencies** vs ratatui's 17.6M/5,719 — the ecosystem has essentially no third-party hardening yet.
2. **Bus factor**: ccbrown wrote ~91% of commits (193 vs next human at 9); ratatui has 6+ active maintainers (MAINTAINERS.md, commit counts above).
3. **No book/guides**: README + doc comments + 16 examples only; ratatui has a whole tutorial site with concepts/how-to sections (ratatui.rs, HTTP 200 on 2026-09-05).
4. **Smaller built-in widget surface**: no Gauge, no Table, no Chart, no Sparkline, no Scrollbar components — tables and progress bars are hand-built from `View`s (official examples/table.rs, examples/progress_bar.rs).
5. **API churn**: breaking changes in 0.9.0 shipped **yesterday** (2026-09-04) — crossterm decoupling changed public types (`Color`, events). 0.7→0.8 also reworked output configuration. Pinning is a moving target.
6. **Unresolved rendering bugs**: #109 WezTerm-specific rendering inconsistency (open 2025-07-03), #117 flickering (open 2025-08-10), #215 OSC 8 hyperlinks stripped in canvas (opened 2026-07-20, 0 comments — though 0.8.5 added OSC 8 for Text/MixedText).
7. **Stale dependency**: taffy pinned at ^0.5.2 while taffy has moved on; upgrade request #119 open since 2025-08-12.
8. **No multi-backend support yet**: `TerminalBackend` trait landed in 0.9.0 to decouple from crossterm, but only the crossterm backend exists today (issue #202 "Terminal backend alternatives" still open; only backend module is `backend/crossterm.rs`).

Counterpoints: iocraft's maintainer is responsive (most issues answered within a day), ships frequently, docs.rs coverage is 100%, CI + Codecov badges active, and the crate is 2 years old with no yanked versions.

---

## Bottom-line recommendation

For a usage-dashboard CLI needing (a) one-shot rich ANSI then exit, (b) interactive TUI, (c) progress bars, (d) tables/boxes/panes, (e) truecolor + NO_COLOR:

**Recommend ratatui 0.30.2 as the primary choice.**

- **(d) is decisive**: you need tables, boxes/panes — ratatui ships `Table`, `Block` (borders/titles), `Gauge`, `LineGauge`, `Tabs`, `Scrollbar`, `Sparkline`, `BarChart`, `List` out of the box; iocraft makes you assemble tables and progress bars from `View`s.
- **(a)**: ratatui's one-shot (`Terminal::with_options` + `Viewport::Inline` + one `draw()`) is more ceremony than iocraft's `element!{...}.print()`, but it's a solved ~10-line pattern, and it degrades the same way (crossterm respects NO_COLOR; pass `Viewport::Fixed` or skip alt-screen for non-TTY).
- **(c)**: `Gauge`/`LineGauge` are built-in; indicatif can't run concurrently with either library, so iocraft gains nothing by allowing indicatif.
- **(e)**: both do truecolor and 256-color; ratatui's NO_COLOR story is indirect (via crossterm's `Colored` suppression), iocraft's is explicit — the one styling category where iocraft is tidier. A one-line `std::env` check before constructing the `Terminal` closes that gap for ratatui.
- **(b)**: ratatui's interactive mode is the industry-standard pattern with 5,719 reverse deps, a maintained book, multi-backend escape hatches (termina backend added in 0.30.2), and a documented breaking-changes trail. iocraft's declarative flexbox model is genuinely pleasant and its one-shot story is better, but you'd be its 13th dependent betting a dashboard on a single-maintainer crate whose public color/event types broke in a release yesterday.

**Pick iocraft instead if** the dashboard leans heavily on flexbox-centric layout, you value the `print()` one-liner, and you accept hand-rolling tables/gauges and pinning patch versions (0.9.x) while the API settles.
