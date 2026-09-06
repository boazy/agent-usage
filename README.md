# Agent Usage

Agent Usage is a Rust command-line tool and dashboard for tracking usage limits and credit balances across AI coding-agent accounts. The tool imports stored credentials from Codex and Oh My Pi (OMP), deduplicates accounts across sources, and queries supported billing providers concurrently.

Running `agent-usage` without arguments prints a terminal report to standard output. The `--tui` flag opens an interactive full-screen dashboard. Both interfaces use the same account panes, color themes, and four-level `█▓▒░` usage gauges.

## Build and run

```sh
cargo build --release
./target/release/agent-usage
./target/release/agent-usage --report --width 140
./target/release/agent-usage --tui
./target/release/agent-usage --json
./target/release/agent-usage --config ./config.toml --theme solarized-dark
```

Reports and the dashboard fit as many columns of at least 44 characters as the width allows. The layout places each pane into the shortest column to prevent vertical gaps between adjacent boxes. Each pane includes a one-cell inner padding around its content, which applies to line wrapping and scrolling. If the terminal cannot fit multiple 44-character columns, the layout uses a single column.

When stdout is piped to another process or file, reports default to 100 columns, omit cursor-control escapes, and print without height limits. Pass `--no-progress` to suppress progress messages while keeping usage gauges visible in the report.

The `--json` flag emits normalized account metadata and usage without exposing raw credentials or upstream provider payloads. All timestamps use Unix epoch milliseconds. The tool writes source and discovery warnings to stderr.

## Normalized usage model

Every provider returns `usage.plan` and `usage.limits`. The `limits` object contains:

| Field | Meaning |
| --- | --- |
| `allowances` | Named usage allowances with an optional window, reset timestamp, and typed credits |
| `balances` | Named credit balances with optional expiry timestamps |
| `banked_resets` | Saved resets with an optional count and expiry; a count-only aggregate stays one entry |
| `global_reset_at` | Optional subscription-wide reset timestamp |

Each allowance separates its `title` from its `window`. Its `credits.count` object records one of five variants:

- `full`: both allocated and consumed amounts
- `allocated`: allocation only
- `remaining`: remaining amount only
- `consumed`: consumed amount only
- `unknown`: unparsed or unavailable amount

Amounts preserve integer or decimal representation. The `credits.unit` field distinguishes currencies, generic credits, percentages, and unknown units.

For example, a USD allowance with 25.5 consumed out of 100 produces this `credits` object:

```json
{
  "count": {
    "full": {
      "allocated": { "integer": 100 },
      "consumed": { "decimal": 25.5 }
    }
  },
  "unit": { "currency": "usd" }
}
```

The model applies these calculation and display rules:

- Percentages are derived only when allocation and consumption are both known.
- A remaining-only balance never implies an allocation of 100.
- Unknown counts remain distinct from zero.
- Overdrawn balances retain negative remaining amounts.
- Banked resets appear under one heading with one line per entry, even when an expiry timestamp is missing. If a reset title is long, the renderer truncates the title to preserve the expiry label.

The JSON schema emits this structured model directly. It does not output legacy `usage.windows`, flat `usage.credits`, or redundant `used_percent` aliases.

Internally, the `Millis` type stores Unix-millisecond timestamps, while `chrono` handles RFC3339 parsing and calendar-month reset calculations. Remote timestamps do not use monotonic `Instant` values because monotonic clocks cannot represent wall-clock epoch time.

## Providers and credential support

Providers represent separate billing systems. Claude models accessed through Google Antigravity remain part of the Antigravity account; they are not merged into a Claude subscription.

| Billing provider | Supported credential | Reported data |
| --- | --- | --- |
| Codex | OAuth | Usage windows, additional meters, saved reset credits, and available credit balances |
| Claude | OAuth | Subscription windows, scoped limits, and extra-usage spending limits |
| OpenRouter | API key | Key spending, key budget and remaining budget; management keys can also report account-wide credits |
| Antigravity | OAuth with a stored project ID | Shared quota groups and windows; compatible older endpoints can supply model-level quota data |
| Cursor | OMP OAuth session, or stored API-key bearer for legacy usage only | Subscription allowances, on-demand spending, and reset dates; OAuth sessions can also supply membership plan and verified profile name/email |

Codex API keys and standard Claude API keys do not expose subscription usage endpoints. OpenRouter OAuth credentials and Antigravity API keys are also unsupported. Credential discovery still imports these credentials and displays them in unavailable account panes. Unknown providers also receive dedicated unavailable panes. If quota amounts are missing or unrecognized, the tool reports them as unknown and does not estimate usage.

If an account request fails, other accounts continue updating independently. Each request enforces an account timeout, caps response body sizes, and disables HTTP redirects. Concurrency limits apply across initial loading and subsequent refreshes.

## Credential sources

When no explicit source list is configured, Agent Usage automatically discovers credentials from standard locations:

- Codex: `$CODEX_HOME/auth.json`, or `~/.codex/auth.json` if `CODEX_HOME` is unset.
- OMP: the active OMP agent directory's `agent.db`. Legacy `auth.json` is a fallback only when that database is absent. An existing database remains authoritative even if it cannot be read; a stale legacy file is not merged into it.

To determine the active OMP directory, the tool checks `PI_CODING_AGENT_DIR`, `PI_CONFIG_DIR`, active `OMP_PROFILE` or `PI_PROFILE` settings, and standard XDG data directories. Use explicit source paths to monitor other installations or profile directories.

Codex sources evaluate `auth_mode`, OAuth tokens, and `OPENAI_API_KEY` precedence. OMP sources read the SQLite `auth_credentials` table as well as legacy JSON credential maps containing individual credentials or credential arrays. Discovery skips disabled SQLite rows. Discovery reads credential records only, not usage caches, conversation history, or logs.

To monitor multiple Codex installations or OMP stores, define explicit sources in configuration:

```toml
[[sources]]
name = "personal-codex"
kind = "codex"
path = "~/.codex/auth.json"
optional = true

[[sources]]
name = "work-codex"
kind = "codex"
path = "~/.codex-work/auth.json"

[[sources]]
name = "omp"
kind = "omp"
path = "~/.omp/agent/agent.db"
optional = true
```

Source names must be unique. In source configurations, `enabled` defaults to `true` and `optional` defaults to `false`. If an optional source file is missing, discovery ignores it. If any other source fails to load, the tool logs a warning and continues loading the remaining sources. Explicit sources replace automatic discovery. To disable credential discovery completely, set `sources = []`.

### Account identity and privacy

Accounts receive opaque, provider-scoped identifiers that do not depend on display labels. For OAuth credentials, identity incorporates available user and organization or project metadata to keep distinct subscriptions separate. For API keys, accounts are grouped by provider and a cryptographic hash of the complete key, not by the visible key mask. Deduplicated accounts retain every source name that supplied them. When tokens rotate, the tool writes the refreshed grant only to the specific credential record that provided it.

Overview reports and account pickers hide UUIDs, long numeric identifiers, and other high-entropy account IDs. Pickers display a short stable discriminator to distinguish matching accounts without exposing full IDs. The dashboard reveals the full account ID only when a filter or selection isolates a single account. Terminal reports always treat single-account output as an overview and hide raw IDs. Human-readable workspace names remain visible, and `--json` retains full account IDs for automation. Anonymous OAuth accounts display stable numbered labels, while verified account names appear separately whenever they differ from the title.

## Configuration and exclusions

To configure settings, copy [config.example.toml](config.example.toml) to `~/.config/agent-usage/config.toml`, or pass `--config PATH`. On macOS and Linux, `XDG_CONFIG_HOME` overrides the default configuration directory. If a file path specified with `--config` does not exist, the command exits with an error instead of falling back to default discovery.

Configuration precedence follows this order: command-line options override environment variables, and environment variables override file settings. Environment variables use the `AGENT_USAGE_` prefix, such as `AGENT_USAGE_THEME=monokai` and `AGENT_USAGE_CONCURRENCY=3`. Nested configuration keys use a double underscore delimiter.

```toml
[exclude]
sources = ["omp"]
providers = ["antigravity"]
emails = ["personal@example.com"]
account_ids = ["workspace-to-hide"]
api_keys = ["sk-o*1234"]

[[exclude.accounts]]
email = "work@example.com"
provider = "claude"
```

The tool applies exclusions before sending provider requests:

- `sources`: exclude an entire kind (`codex` or `omp`) or a specific source name. Kind exclusions cover custom source names and run before discovery and deduplication.
- `providers`: exclude a billing provider. OMP provider aliases such as `openai-codex`, `anthropic`, and `google-antigravity` are accepted.
- `emails`: exclude an email address across all providers, case-insensitively.
- `accounts`: exclude the combination of an email address and provider.
- `account_ids`: match a stored account/workspace ID or the opaque dashboard ID shown in JSON.
- `api_keys`: match visible portions of the displayed key mask. Patterns must contain exactly one `*`, at most four characters on each side, and four to eight literal characters in total. Literals accept ASCII letters, digits, `-`, or `_`. Examples like `*1234` and `sk-o*1234` are valid; full keys and arbitrary secret substrings are rejected.

Key masks reveal at most the first four and last four characters. Short or malformed keys are masked completely. Exclusion matches found in any duplicate credential apply to the merged account, including email metadata discovered from later sources.

## Interactive controls

| Key | Action |
| --- | --- |
| `/` | Edit a live filter over account label, name, email, or provider |
| Enter | Finish filtering or select a picker item |
| `p` | Search and choose a provider, including All |
| `a` | Search and choose an account from the current provider selection, including All |
| `t` | Search and choose a built-in theme for this session |
| Arrows | Scroll the dashboard or move within a picker |
| Page Up / Page Down | Scroll by a screen or picker page |
| Home / End | Jump to the first or last row or item |
| `r` | Refresh only account panes intersecting the current screen |
| `[` / `]` | Scroll the pending-provider list backward or forward |
| Backspace / Ctrl-U | Delete a character or clear search text in a picker |
| `q`, Escape, Ctrl-C | Quit; `q` enters literal search text in the live filter and pickers |

Pickers filter search results case-insensitively as you type. The account picker matches names, email addresses, and provider names, while the provider and theme pickers match their respective names. Space-separated search terms must all match. Arrow keys and page navigation keys move through filtered results. The provider and account pickers keep the `All` option available even when no accounts match the query; pressing Home followed by Enter selects `All` and clears both the filter query and the previous selection. Pressing Escape or Ctrl-C exits the application rather than closing only the active picker.

An empty live filter restores the full account list. Selecting a provider or account clears the previous live filter text. Resizing the terminal or changing filters causes the dashboard to recompute pane placement and visibility.

Initial usage fetching runs in the background. Each account pane appears as soon as its request finishes, and the layout repacks remaining panes as results arrive. Accounts that have not finished their initial fetch show no placeholder boxes. If an account request fails, an error pane appears for that account and provider, while other accounts from the same provider continue to display their usage.

While requests remain pending, a status box in the bottom-right corner displays the list of active providers. It lists each pending provider once, even when multiple accounts are loading under that provider. A provider leaves the list only when all its accounts finish loading or fail. Filter changes do not interrupt pending background work. The layout reserves space for this status box so it never obscures account data. If the list of pending providers exceeds the available box height, scroll it with `[` and `]`.

Existing usage data remains visible during background refreshes. Automatic refreshes query visible panes at the interval set by `refresh_seconds`. Off-screen panes retain their cached state until scrolled into view and refreshed.

Quitting the dashboard immediately restores the terminal screen and cancels queued requests. Requests already in flight finish within the configured timeout window and write back any rotated tokens before the process exits. Pressing `r` repeatedly does not queue redundant refresh operations. The dashboard also restores the terminal if the application encounters a fatal error or panic.

## Themes and visual hierarchy

All 22 built-in themes use semantic color roles for pane borders, active picker borders, titles, metadata labels and values, allowance windows, and reset times:

- Distinct palette hues differentiate `Sources`, `Plan`, and `Account` values.
- Window labels appear in muted italics inside parentheses.
- Reset labels such as "resets in" are styled separately from remaining durations.
- Bold typography highlights titles and allowance names.
- Underlines identify active picker selections.

In `--tui` mode, each theme sets the terminal background color across the entire canvas, including pane whitespace and picker overlays. The role separation takes inspiration from [Yazi's structural, picker, and input theme roles](https://github.com/sxyazi/yazi/blob/main/yazi-config/preset/theme-dark.toml). Colors are defined as explicit RGB values in typed `anstyle` structures rather than copied terminal ANSI slots.

Built-in themes include:

- **Dark palettes**: `default`, `solarized-dark`, `monokai`, `molokai`, `dracula`, `gruvbox-dark`, `one-dark`, `nord`, `nord-dark`, `github-dark`, `catppuccin-mocha`, `tokyo-night`, `everforest`, `kanagawa`, `rose-pine`, `ayu-dark`
- **Light palettes**: `solarized-light`, `gruvbox-light`, `one-light`, `github-light`, `rose-pine-dawn`, `catppuccin-latte`

All themes retain the same primary and muted gauge-color pair at every usage level, and the original 16 themes preserve their existing gauge colors. Color policy is delegated to `anstream`, including terminal detection and its `NO_COLOR`, `TERM`, and forced-color handling. On monochrome terminals, gauges remain legible through glyph density alone across all four levels (`█▓▒░`).

## Credential safety and concurrency limits

Credentials are never serialized into report objects and have no debug formatter. HTTP failures omit response bodies and secret-bearing request details. Token refresh requests target only fixed provider endpoints; any custom Codex base URL must use an official HTTPS ChatGPT origin. Usage operations do not redeem credits, run model completions, or change subscriptions.

Refreshed credentials are written back to the exact originating JSON entry or SQLite row. Writes preserve unrelated data, compare against the expected credential, and use private file permissions. Existing owner-owned readable files with broader read permissions are accepted with a warning; discovery does not chmod them. Unsafe symlinks, foreign ownership, hardlinks, and other-user-writable credential files are rejected.

Current OMP SQLite stores coordinate refreshes through OMP's `auth_credential_refresh_leases` table and use conditional row updates. Older stores use advisory locks and conditional updates without creating a new lease schema. JSON writes use cooperative locks, a final content/inode check, a private temporary file, atomic replacement, and `fsync`. A separate program that ignores the JSON locks can still write in the final check-to-replacement interval. Older OMP versions without shared leases also cannot prevent simultaneous provider-side grant rotation by another program.

Broker refresh sentinels are never sent to OAuth endpoints; refresh those grants through their owning tool. The dashboard does not add a credential broker, evaluate credential commands, or read secrets from unrelated configuration files.

## Development and API evidence

```sh
cargo fmt --check
cargo clippy --all-targets -- -D warnings
cargo test
```

Regression tests use generated credential files, temporary SQLite databases, and localhost HTTP fixtures only. Provider contracts were researched from public source repositories and official documentation; no live account requests were used for validation.

- [OMP credential storage and formats](https://github.com/can1357/oh-my-pi/blob/main/packages/ai/src/auth/sqlite-credential-store.ts)
- [OMP OAuth credential fields](https://github.com/can1357/oh-my-pi/blob/main/packages/ai/src/registry/oauth/types.ts)
- [Codex authentication modes](https://github.com/openai/codex/blob/main/codex-rs/login/src/auth/manager.rs)
- [Codex usage adapter](https://github.com/can1357/oh-my-pi/blob/main/packages/ai/src/usage/openai-codex.ts)
- [Claude usage adapter](https://github.com/can1357/oh-my-pi/blob/main/packages/ai/src/usage/claude.ts)
- [Antigravity usage adapter](https://github.com/can1357/oh-my-pi/blob/main/packages/ai/src/usage/google-antigravity.ts)
- [OpenRouter current-key API](https://openrouter.ai/docs/api/api-reference/api-keys/get-current-api-key)
- [OpenRouter account-credit API](https://openrouter.ai/docs/api/api-reference/credits/get-remaining-credits)
- [Cursor usage and endpoint contracts in OMP](https://github.com/can1357/oh-my-pi/blob/5964a0f7649275bcde818f20073193fd032451f2/packages/ai/src/usage/cursor.ts)
- [Cursor session exchange in OMP](https://github.com/can1357/oh-my-pi/blob/5964a0f7649275bcde818f20073193fd032451f2/packages/ai/src/registry/oauth/cursor.ts)
- [Cursor profile and membership metadata in CodexBar](https://github.com/steipete/CodexBar/blob/cd5f2be234330319fd67b566f3527b98d26b0ab7/Sources/CodexBarCore/Providers/Cursor/CursorStatusProbe.swift)
