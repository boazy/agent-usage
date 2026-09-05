# Agent Usage

A Rust dashboard for usage limits and balances across coding-agent accounts. It
imports stored credentials from Codex and Oh My Pi (OMP), groups duplicate
accounts, and queries each supported billing provider concurrently.

The default command prints an ordinary, vertically unbounded report. `--tui`
opens the interactive dashboard. Both views use the same account panes, built-in
themes, and four-level `█▓▒░` gauges.

## Build and run

```sh
cargo build --release
./target/release/agent-usage
./target/release/agent-usage --report --width 140
./target/release/agent-usage --tui
./target/release/agent-usage --json
./target/release/agent-usage --config ./config.toml --theme solarized-dark
```

Reports and the dashboard fit as many columns of at least 44 characters as the
width allows. Panes go into the shortest available column, so shorter boxes do
not leave gaps beneath taller neighbors. Each pane has a one-cell inset around
its content; wrapping and scrolling include that padding. Narrow terminals use
one column.
Piped reports default to 100 columns, contain no cursor-drawing commands, and
have no terminal-height limit. `--no-progress` suppresses progress messages; the
report gauges remain visible.

`--json` emits normalized account metadata and usage, not raw provider responses
or credentials. Timestamps are Unix epoch milliseconds. Source warnings are
written to stderr.

## Normalized usage model

Every provider returns `usage.plan` and `usage.limits`. The `limits` object contains:

| Field | Meaning |
| --- | --- |
| `allowances` | Named usage allowances with an optional window, reset timestamp, and typed credits |
| `balances` | Named credit balances with optional expiry timestamps |
| `banked_resets` | Saved resets with an optional count and expiry; a count-only aggregate stays one entry |
| `global_reset_at` | Optional subscription-wide reset timestamp |

Each allowance separates its `title` from its `window`. Its `credits.count`
records either allocated and consumed amounts together (`full`), only the
allocation (`allocated`), only remaining or consumed amounts, or `unknown`.
Amounts retain integer or decimal representation. `credits.unit` distinguishes
currencies, generic credits, percentages, and unknown units.

For example, a USD allowance with 25.5 consumed out of 100 has this credits object:

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

Percentages are derived only when allocation and consumption are both known. A
remaining-only amount never implies an allocation of 100. Unknown counts differ
from zero; overdrawn balances retain negative remaining amounts. Banked resets
share one heading and occupy one line per entry, including entries without a
known expiry. Long reset titles are shortened to preserve the expiry.

This JSON schema replaces the previous `usage.windows`, flat `usage.credits`,
and redundant `used_percent` fields; those aliases are not emitted. Remote reset
and expiry timestamps remain Unix epoch milliseconds.

Internally, `Millis` preserves that Unix-millisecond representation. `chrono`
parses RFC3339 timestamps and calculates calendar-month resets; a monotonic
`Instant` is not used for remote timestamps.

## Providers and credential support

Providers represent separate billing systems. Claude models accessed through
Antigravity remain part of the Antigravity account; they are not merged into a
Claude subscription.

| Billing provider | Supported credential | Reported data |
| --- | --- | --- |
| Codex | OAuth | Usage windows, additional meters, saved reset credits and available credit balances |
| Claude | OAuth | Subscription windows, scoped limits, and available extra-usage spending limits |
| OpenRouter | API key | Key spending, key budget and remaining budget; management keys can also report account-wide credits |
| Antigravity | OAuth with a stored project ID | Shared quota groups and windows; compatible older endpoints can supply model-level quota data |
| Cursor | OMP OAuth session, or stored API-key bearer for legacy usage only | Subscription allowances, on-demand spending, and reset dates; OAuth sessions can also supply membership plan and verified profile name/email |

Codex and ordinary Claude API keys do not expose the subscription-usage
endpoints used here. OpenRouter OAuth credentials and Antigravity API keys are
also unsupported. These credentials are still discovered and displayed as
unavailable. Unknown providers receive their own unavailable account panes.
Missing or unrecognized quota amounts stay unknown; they are never replaced with
fabricated usage.

A failed account does not prevent other accounts from updating. Requests have an
account-wide timeout, response bodies are size-limited, and redirects are
disabled. Concurrency is bounded across overlapping refreshes.

## Credential sources

Without an explicit source list, the dashboard discovers:

- Codex: `$CODEX_HOME/auth.json`, or `~/.codex/auth.json` when `CODEX_HOME` is
  unset.
- OMP: the active OMP agent directory's `agent.db`. Legacy `auth.json` is a
  fallback only when that database is absent. An existing database remains
  authoritative even if it cannot be read; a stale legacy file is not merged
  into it.

OMP directory selection recognizes `PI_CODING_AGENT_DIR`, `PI_CONFIG_DIR`,
active `OMP_PROFILE`/`PI_PROFILE`, and applicable XDG data directories. Explicit
source paths provide control over other installations and profiles.

Codex sources understand `auth_mode`, OAuth `tokens`, and `OPENAI_API_KEY`
precedence. OMP sources support the SQLite `auth_credentials` table and legacy
JSON maps whose provider values are individual credentials or arrays. Disabled
SQLite credentials are skipped. Discovery reads credential records only, not
usage caches, conversation history, or logs.

Configure additional sources to read multiple Codex installations or OMP stores:

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

Source names must be unique. `enabled` defaults to `true`; `optional` defaults
to `false`. An optional missing source is ignored. Other discovery failures
produce warnings while remaining sources continue. Explicit sources replace
automatic discovery. Set `sources = []` to disable discovery entirely.

Accounts have opaque, provider-scoped IDs independent of their display labels.
OAuth identity includes available user and organization/project information, so
separate subscriptions stay separate. API keys are grouped by provider and a
cryptographic fingerprint of the complete key, never by their displayed mask.
Duplicate accounts retain all included source names. A refresh is saved only to
the credential record that supplied the chosen grant.

Overview reports and account pickers hide UUIDs, long numeric IDs, and similar
opaque account identifiers. A short stable discriminator distinguishes picker
entries without exposing the full ID. The TUI reveals a full account ID only
when an explicit selection or filter leaves one account. A report containing one
account is still an overview. Human-readable workspace names remain visible, and
`--json` retains account IDs for configuration and automation. Anonymous OAuth
accounts use stable numbered labels rather than an opaque ID; verified names
appear separately when they differ from the title.

## Configuration and exclusions

Copy [config.example.toml](config.example.toml) to
`~/.config/agent-usage/config.toml`, or use `--config PATH`. `XDG_CONFIG_HOME`
overrides the configuration directory on macOS and Linux. An explicitly
requested missing config file is an error; it never falls back to automatic
credential discovery.

Explicit CLI values override environment values, which override the config file.
For example, `AGENT_USAGE_THEME=monokai` and `AGENT_USAGE_CONCURRENCY=3` remain
available. Nested environment names use a double underscore.

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

Exclusions are applied before any provider request:

- `sources`: exclude an entire kind (`codex` or `omp`) or one configured source
  name. Kind exclusions cover custom names too, and run before discovery and
  deduplication.
- `providers`: exclude a billing provider. OMP aliases such as `openai-codex`,
  `anthropic`, and `google-antigravity` are accepted.
- `emails`: exclude an email across all providers, case-insensitively.
- `accounts`: exclude the conjunction of an email and provider.
- `account_ids`: match a stored account/workspace ID or the opaque dashboard ID
  shown in JSON.
- `api_keys`: match only visible parts of the displayed key mask. Patterns
  contain exactly one `*`, at most four characters on each side, and four to
  eight literal characters in total. Literals are ASCII letters, digits, `-`, or
  `_`. `*1234` and `sk-o*1234` are valid; complete keys and arbitrary secret
  substrings are rejected.

Key masks reveal at most the first four and last four characters. Short or
malformed keys are completely masked. Exclusion matches found in any included
duplicate credential apply to the merged account, including email metadata
absent from an earlier source.

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
| Home / End | Jump to the first or last row/item |
| `r` | Refresh only account panes intersecting the current screen |
| `[` / `]` | Scroll the pending-provider list backward / forward |
| Backspace / Ctrl-U | Remove a character / clear the search in a picker |
| `q`, Escape, Ctrl-C | Quit; `q` is literal search text in the live filter and all pickers |

Pickers filter as you type, case-insensitively. The account picker matches
names, email addresses, and provider names; the provider and theme pickers match
their names. Space-separated search terms must all match. Arrows and paging move
through the filtered results. Provider and account pickers keep All available
even when nothing matches; Home then Enter selects it and clears the prior
selection and text filter. Escape and Ctrl-C quit rather than dismissing a
picker.

An empty live filter restores the unfiltered set. Choosing a provider or account
clears the previous text filter. Resizing and filtering recompute pane placement
and visibility.

Initial usage fetching runs in the background. Each account appears as soon as
its fetch completes, and the boxes repack as details arrive. Accounts that have
not completed their first fetch have no placeholder pane. A failed account gets
an error pane named for its account and provider; other accounts from that
provider keep their results.

While requests are pending, a loading box stays at the bottom right. It lists
each pending provider once, even if several accounts are loading. A provider
leaves the list only when all its pending accounts finish, including failures.
Changing filters does not hide pending work. The layout reserves space for this
box so it does not cover account data. Long lists stay within the viewport; use
`[` and `]` to scroll them.

Existing results remain visible during refresh. Later automatic refreshes use
the visible set at `refresh_seconds` intervals. Off-screen snapshots remain
unchanged until refreshed.

Quitting restores the screen immediately and stops queued fetches. Already
active requests finish within the configured timeout, followed by bounded
credential persistence, so a rotated refresh token is not discarded. Repeated
`r` presses do not accumulate refresh jobs. Terminal cleanup also runs on
returned errors and unwinding panics.

## Themes and visual hierarchy

All 22 built-in themes use semantic color roles for pane borders, active picker
borders, titles, metadata labels and values, allowance windows, and reset times.
`Sources`, `Plan`, and `Account` values have distinct palette hues. Window names
appear in muted chromatic italics inside parentheses; reset labels such as
“resets in” are styled separately from their durations. Bold titles and
allowance names establish hierarchy, while picker selections add an underline.

The role separation takes inspiration from [Yazi's structural, picker, and input
theme
roles](https://github.com/sxyazi/yazi/blob/main/yazi-config/preset/theme-dark.toml).
Colors are explicit RGB palette values in typed `anstyle` styles, not copied
terminal ANSI slots.

Available themes: `default`, `solarized-dark`, `solarized-light`, `monokai`,
`molokai`, `dracula`, `gruvbox-dark`, `gruvbox-light`, `one-dark`, `one-light`,
`nord`, `github-dark`, `github-light`, `nord-dark`, `catppuccin-mocha`,
`tokyo-night`, `everforest`, `kanagawa`, `rose-pine`, `rose-pine-dawn`,
`ayu-dark`, and `catppuccin-latte`.

All themes retain the same primary/muted gauge-color pair at every usage level;
the original 16 themes preserve their existing gauge colors. Color policy is
delegated to `anstream`, including terminal detection and its `NO_COLOR`,
`TERM`, and forced-color handling. Monochrome output still distinguishes gauge
levels by glyph.

## Credential safety and concurrency limits

Credentials are never serialized into report objects and have no debug
formatter. HTTP failures omit response bodies and secret-bearing request
details. Refresh requests use fixed provider endpoints; the Codex base URL
option accepts only official HTTPS ChatGPT origins. Usage operations do not
redeem credits, run model completions, or change subscriptions.

Refreshed credentials are written back to the exact originating JSON entry or
SQLite row. Writes preserve unrelated data, compare against the expected
credential, and use private file permissions. Existing owner-owned readable
files with broader read permissions are accepted with a warning; discovery does
not chmod them. Unsafe symlinks, foreign ownership, hardlinks, and
other-user-writable credential files are rejected.

Current OMP SQLite stores coordinate refreshes through OMP's existing lease
table and use conditional row updates. Older stores use advisory locks and
conditional updates without creating a new lease schema. JSON writes use
cooperative locks, a final content/inode check, a private temporary file, atomic
replacement, and fsync. A separate program that ignores the JSON locks can still
write in the final check-to-replacement interval. Older OMP versions without
shared leases also cannot prevent simultaneous provider-side grant rotation by
another program.

Broker refresh sentinels are never sent to OAuth endpoints. Refresh those grants
through their owning tool. The dashboard does not add a credential broker,
evaluate credential commands, or read secrets from unrelated configuration
files.

## Development and API evidence

```sh
cargo fmt --check
cargo clippy --all-targets -- -D warnings
cargo test
```

Regression tests use generated credential files, temporary SQLite databases, and
localhost HTTP fixtures only. Provider contracts were researched from public
source and official documentation; no live account requests were used for
validation.

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
