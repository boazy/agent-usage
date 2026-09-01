const DEFAULT_BASE_URL: &str = "https://chatgpt.com/backend-api";
const DEFAULT_THEME_NAME: &str = "default";
const USER_AGENT_VALUE: &str = "codex-usage-rs/0.1.0";
const JWT_AUTH_CLAIM: &str = "https://api.openai.com/auth";
const JWT_PROFILE_CLAIM: &str = "https://api.openai.com/profile";
const ANSI_RESET: &str = "\x1b[0m";
const ANSI_BOLD: &str = "\x1b[1m";
const DEFAULT_USAGE_BAR_PROGRESS_CHARS: &str = "█▓▒░ ";