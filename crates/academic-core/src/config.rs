use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::str::FromStr;

use chrono_tz::Tz;
use cron::Schedule;
use regex::Regex;
use serde::{Deserialize, Serialize};
use url::Url;

use crate::error::{Error, Result};

/// Top-level configuration loaded from `academic-agent.toml`.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Config {
    pub general: GeneralConfig,
    pub llm: LlmConfig,
    pub discord: DiscordConfig,
    pub hermes: HermesConfig,
    pub cron: CronConfig,
    pub server: ServerConfig,
    pub n8n: N8nConfig,
}

/// Filesystem paths and general behavior settings.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct GeneralConfig {
    pub inbox_dir: PathBuf,
    pub archive_dir: PathBuf,
    pub calendar_dir: PathBuf,
    pub db_path: PathBuf,
    pub timezone: String,
    pub backup_processed: bool,
    pub default_duration_hours: u32,
}

impl Default for GeneralConfig {
    fn default() -> Self {
        Self {
            inbox_dir: PathBuf::from("inbox"),
            archive_dir: PathBuf::from("archive"),
            calendar_dir: PathBuf::from("data/calendar"),
            db_path: PathBuf::from("data/state.redb"),
            timezone: "Asia/Jakarta".to_string(),
            backup_processed: true,
            default_duration_hours: 2,
        }
    }
}

/// Chat Completions compatible LLM inference and fallback settings.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct LlmConfig {
    pub api_base: String,
    pub api_key: String,
    pub model: String,
    pub timeout_secs: u64,
    pub fallback_to_heuristic: bool,
    pub temperature: f32,
}

impl Default for LlmConfig {
    fn default() -> Self {
        Self {
            api_base: "${ACADEMIC_LLM_API_BASE}".to_string(),
            api_key: "${ACADEMIC_LLM_API_KEY}".to_string(),
            model: "gpt-4o-mini".to_string(),
            timeout_secs: 45,
            fallback_to_heuristic: true,
            temperature: 0.1,
        }
    }
}

/// Discord dispatch transport mode.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum DiscordMode {
    #[default]
    Auto,
    Bot,
    Webhook,
}

/// Discord REST API v10, Gateway Bot, and Webhook settings.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct DiscordConfig {
    pub enabled: bool,
    pub mode: DiscordMode,
    pub bot_token: String,
    pub application_id: String,
    pub guild_id: String,
    pub channel_id: String,
    pub webhook_url: String,
    pub api_base: String,
    pub gateway_url: String,
    pub allowed_users: Vec<String>,
    pub timeout_secs: u64,
    pub max_retries: u32,
}

impl Default for DiscordConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            mode: DiscordMode::Auto,
            bot_token: "${DISCORD_BOT_TOKEN}".to_string(),
            application_id: "${DISCORD_CLIENT_ID}".to_string(),
            guild_id: "${DISCORD_GUILD_ID}".to_string(),
            channel_id: "${DISCORD_CHANNEL_ID}".to_string(),
            webhook_url: "${DISCORD_WEBHOOK_URL}".to_string(),
            api_base: "https://discord.com/api/v10".to_string(),
            gateway_url: String::new(),
            allowed_users: Vec::new(),
            timeout_secs: 15,
            max_retries: 3,
        }
    }
}

/// Optional Hermes CLI dispatch bridge settings.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct HermesConfig {
    pub enabled: bool,
    pub binary_path: String,
    pub target: String,
    pub timeout_secs: u64,
    pub fallback_on_discord_error: bool,
}

impl Default for HermesConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            binary_path: "hermes".to_string(),
            target: "${HERMES_TARGET}".to_string(),
            timeout_secs: 15,
            fallback_on_discord_error: false,
        }
    }
}

/// Cron reminder and daily digest schedule settings.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct CronConfig {
    pub enabled: bool,
    pub digest_cron: String,
    pub reminder_hours: Vec<i64>,
    pub alert_overdue: bool,
    pub tick_interval_secs: u64,
}

impl Default for CronConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            digest_cron: "0 7 * * *".to_string(),
            reminder_hours: vec![72, 24, 3],
            alert_overdue: true,
            tick_interval_secs: 60,
        }
    }
}

/// Embedded HTTP API and webhook receiver settings.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct ServerConfig {
    pub bind: String,
    pub api_token: String,
    pub allowed_origins: Vec<String>,
    pub max_upload_bytes: usize,
    pub webdelta_webhook_secret: String,
}

impl Default for ServerConfig {
    fn default() -> Self {
        Self {
            bind: "127.0.0.1:8080".to_string(),
            api_token: "${ACADEMIC_API_TOKEN}".to_string(),
            allowed_origins: vec![
                "http://127.0.0.1:8080".to_string(),
                "http://localhost:8080".to_string(),
            ],
            max_upload_bytes: 16 * 1024 * 1024,
            webdelta_webhook_secret: "${WEBDELTA_WEBHOOK_SECRET}".to_string(),
        }
    }
}

/// Optional outbound n8n webhook bridge settings.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct N8nConfig {
    pub enabled: bool,
    pub webhook_url: String,
    pub secret: String,
    pub timeout_secs: u64,
}

impl Default for N8nConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            webhook_url: "${N8N_WEBHOOK_URL}".to_string(),
            secret: "${N8N_WEBHOOK_SECRET}".to_string(),
            timeout_secs: 10,
        }
    }
}

/// Loads `KEY=VALUE` entries from a `.env` file into the current process environment
/// without overwriting variables that are already set.
pub fn load_dotenv_if_present(path: &Path) -> Result<usize> {
    if !path.exists() {
        return Ok(0);
    }
    let content = std::fs::read_to_string(path)?;
    let mut loaded = 0usize;
    for raw_line in content.lines() {
        let line = raw_line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let line = line.strip_prefix("export ").unwrap_or(line).trim();
        let Some((k, v)) = line.split_once('=') else {
            continue;
        };
        let key = k.trim();
        if key.is_empty() {
            continue;
        }
        let mut val = v.trim();
        if (val.starts_with('"') && val.ends_with('"') && val.len() >= 2)
            || (val.starts_with('\'') && val.ends_with('\'') && val.len() >= 2)
        {
            val = &val[1..val.len() - 1];
        }
        if std::env::var_os(key).is_none() {
            std::env::set_var(key, val);
            loaded += 1;
        }
    }
    Ok(loaded)
}

/// Expands `${VAR_NAME}` placeholders using a caller-supplied lookup function.
/// Legacy environment variable aliases are resolved if the primary name is unset.
pub fn expand_env_vars_with<F>(input: &str, mut lookup: F) -> String
where
    F: FnMut(&str) -> Option<String>,
{
    let re = Regex::new(r"\$\{([A-Za-z0-9_]+)\}").expect("valid env placeholder regex");
    re.replace_all(input, |caps: &regex::Captures<'_>| {
        let name = &caps[1];
        if let Some(val) = lookup(name) {
            return val;
        }
        // Check common legacy aliases without failing if unset
        let alias = match name {
            "ACADEMIC_LLM_API_BASE" => Some("ROUTER_API_BASE"),
            "ACADEMIC_LLM_API_KEY" => Some("ROUTER_API_KEY"),
            _ => None,
        };
        if let Some(alias_key) = alias {
            if let Some(val) = lookup(alias_key) {
                return val;
            }
        }
        String::new()
    })
    .into_owned()
}

/// Normalizes a 5-field standard cron expression (`min hour dom month dow`) into the
/// 6-field schedule format expected by the `cron` crate (`sec min hour dom month dow`).
pub fn normalize_cron_expr(expr: &str) -> Result<String> {
    let parts: Vec<&str> = expr.split_whitespace().collect();
    match parts.len() {
        5 => Ok(format!("0 {}", parts.join(" "))),
        6 | 7 => Ok(parts.join(" ")),
        other => Err(Error::Cron(format!(
            "expected 5, 6, or 7 cron fields in '{expr}', got {other}"
        ))),
    }
}

/// Parses and validates a cron expression string.
pub fn parse_cron_schedule(expr: &str) -> Result<Schedule> {
    let normalized = normalize_cron_expr(expr)?;
    Schedule::from_str(&normalized)
        .map_err(|e| Error::Cron(format!("invalid cron expression '{expr}': {e}")))
}

fn validate_optional_http_url(label: &str, raw: &str) -> Result<()> {
    let trimmed = raw.trim();
    if trimmed.is_empty() || trimmed.contains("${") {
        return Ok(());
    }
    let parsed = Url::parse(trimmed)
        .map_err(|e| Error::Config(format!("{label} has invalid URL '{trimmed}': {e}")))?;
    if !matches!(parsed.scheme(), "http" | "https") {
        return Err(Error::Config(format!(
            "{label} URL '{trimmed}' must use http or https"
        )));
    }
    Ok(())
}

impl Config {
    /// Loads configuration from disk, reading an adjacent `.env` (or CWD `.env`) first,
    /// expanding `${VAR}` placeholders, and validating all fields.
    pub fn load(path: &Path) -> Result<Self> {
        if let Some(parent) = path.parent() {
            let candidate = if parent.as_os_str().is_empty() {
                Path::new(".env").to_path_buf()
            } else {
                parent.join(".env")
            };
            let _ = load_dotenv_if_present(&candidate);
        }
        let raw = std::fs::read_to_string(path).map_err(|e| {
            Error::Config(format!("failed to read config '{}': {e}", path.display()))
        })?;
        let mut cfg = Self::from_toml_str(&raw)?;
        cfg.resolve_env_from_process();
        cfg.validate()?;
        Ok(cfg)
    }

    /// Parses configuration directly from a TOML string without expanding environment variables.
    pub fn from_toml_str(raw: &str) -> Result<Self> {
        let cfg: Self = toml::from_str(raw)?;
        Ok(cfg)
    }

    /// Expands `${VAR}` placeholders in string settings using a lookup closure.
    pub fn expand_with<F>(&mut self, mut lookup: F)
    where
        F: FnMut(&str) -> Option<String>,
    {
        self.llm.api_base = expand_env_vars_with(&self.llm.api_base, &mut lookup);
        self.llm.api_key = expand_env_vars_with(&self.llm.api_key, &mut lookup);
        self.llm.model = expand_env_vars_with(&self.llm.model, &mut lookup);

        self.discord.bot_token = expand_env_vars_with(&self.discord.bot_token, &mut lookup);
        self.discord.application_id =
            expand_env_vars_with(&self.discord.application_id, &mut lookup);
        self.discord.guild_id = expand_env_vars_with(&self.discord.guild_id, &mut lookup);
        self.discord.channel_id = expand_env_vars_with(&self.discord.channel_id, &mut lookup);
        self.discord.webhook_url = expand_env_vars_with(&self.discord.webhook_url, &mut lookup);
        self.discord.api_base = expand_env_vars_with(&self.discord.api_base, &mut lookup);
        self.discord.gateway_url = expand_env_vars_with(&self.discord.gateway_url, &mut lookup);

        self.hermes.binary_path = expand_env_vars_with(&self.hermes.binary_path, &mut lookup);
        self.hermes.target = expand_env_vars_with(&self.hermes.target, &mut lookup);

        self.server.api_token = expand_env_vars_with(&self.server.api_token, &mut lookup);
        self.server.webdelta_webhook_secret =
            expand_env_vars_with(&self.server.webdelta_webhook_secret, &mut lookup);

        self.n8n.webhook_url = expand_env_vars_with(&self.n8n.webhook_url, &mut lookup);
        self.n8n.secret = expand_env_vars_with(&self.n8n.secret, &mut lookup);
    }

    /// Expands placeholders using `std::env::var`.
    pub fn resolve_env_from_process(&mut self) {
        self.expand_with(|key| std::env::var(key).ok());
        // Apply legacy model fallback if ROUTER_MODEL is set and model is still default
        if self.llm.model == "gpt-4o-mini" {
            if let Ok(m) = std::env::var("ROUTER_MODEL") {
                if !m.trim().is_empty() {
                    self.llm.model = m;
                }
            }
        }
    }

    /// Parses the configured IANA timezone string.
    pub fn timezone_tz(&self) -> Result<Tz> {
        self.general.timezone.trim().parse::<Tz>().map_err(|_| {
            Error::Config(format!(
                "invalid IANA timezone '{}'; example: 'Asia/Jakarta' or 'UTC'",
                self.general.timezone
            ))
        })
    }

    /// Validates paths, URLs, timezone, cron schedule, and notifier settings.
    pub fn validate(&self) -> Result<()> {
        let _ = self.timezone_tz()?;
        let _ = parse_cron_schedule(&self.cron.digest_cron)?;
        if self.cron.tick_interval_secs == 0 {
            return Err(Error::Config(
                "cron.tick_interval_secs must be greater than 0".to_string(),
            ));
        }
        for &hours in &self.cron.reminder_hours {
            if hours <= 0 {
                return Err(Error::Config(format!(
                    "cron.reminder_hours must contain positive integers, got {hours}"
                )));
            }
        }
        if self.general.default_duration_hours == 0 {
            return Err(Error::Config(
                "general.default_duration_hours must be greater than 0".to_string(),
            ));
        }
        validate_optional_http_url("llm.api_base", &self.llm.api_base)?;
        validate_optional_http_url("discord.api_base", &self.discord.api_base)?;
        validate_optional_http_url("discord.webhook_url", &self.discord.webhook_url)?;
        if self.n8n.enabled {
            validate_optional_http_url("n8n.webhook_url", &self.n8n.webhook_url)?;
        }
        if self.hermes.enabled && self.hermes.target.trim().is_empty() {
            return Err(Error::Config(
                "hermes.target must not be empty when hermes.enabled is true".to_string(),
            ));
        }
        if self.server.max_upload_bytes == 0 {
            return Err(Error::Config(
                "server.max_upload_bytes must be greater than 0".to_string(),
            ));
        }
        Ok(())
    }

    /// Resolves `general.*` directories relative to `base_dir` if they are relative paths.
    pub fn resolve_paths_relative_to(&mut self, base_dir: &Path) {
        let resolve = |p: &Path| -> PathBuf {
            if p.is_absolute() {
                p.to_path_buf()
            } else {
                base_dir.join(p)
            }
        };
        self.general.inbox_dir = resolve(&self.general.inbox_dir);
        self.general.archive_dir = resolve(&self.general.archive_dir);
        self.general.calendar_dir = resolve(&self.general.calendar_dir);
        self.general.db_path = resolve(&self.general.db_path);
    }

    /// Creates runtime directories (`inbox_dir`, `archive_dir`, `calendar_dir`, db parent).
    pub fn ensure_runtime_dirs(&self) -> Result<()> {
        std::fs::create_dir_all(&self.general.inbox_dir)?;
        std::fs::create_dir_all(&self.general.archive_dir)?;
        std::fs::create_dir_all(&self.general.calendar_dir)?;
        if let Some(parent) = self.general.db_path.parent() {
            if !parent.as_os_str().is_empty() {
                std::fs::create_dir_all(parent)?;
            }
        }
        Ok(())
    }

    /// Returns a starter TOML string suitable for `academic-agent init`.
    pub fn starter_toml() -> String {
        r#"# academic-agent configuration

[general]
inbox_dir = "inbox"
archive_dir = "archive"
calendar_dir = "data/calendar"
db_path = "data/state.redb"
timezone = "Asia/Jakarta"
backup_processed = true
default_duration_hours = 2

[llm]
# Any Chat Completions compatible gateway (local router, Ollama, or hosted endpoint)
api_base = "${ACADEMIC_LLM_API_BASE}"
api_key = "${ACADEMIC_LLM_API_KEY}"
model = "gpt-4o-mini"
timeout_secs = 45
fallback_to_heuristic = true
temperature = 0.1

[discord]
enabled = true
mode = "auto"
bot_token = "${DISCORD_BOT_TOKEN}"
application_id = "${DISCORD_CLIENT_ID}"
guild_id = "${DISCORD_GUILD_ID}"
channel_id = "${DISCORD_CHANNEL_ID}"
webhook_url = "${DISCORD_WEBHOOK_URL}"
timeout_secs = 15
max_retries = 3

[hermes]
enabled = false
binary_path = "hermes"
target = "${HERMES_TARGET}"
timeout_secs = 15
fallback_on_discord_error = false

[cron]
enabled = true
digest_cron = "0 7 * * *"
reminder_hours = [72, 24, 3]
alert_overdue = true
tick_interval_secs = 60

[server]
bind = "127.0.0.1:8080"
api_token = "${ACADEMIC_API_TOKEN}"
allowed_origins = ["http://127.0.0.1:8080", "http://localhost:8080"]
max_upload_bytes = 16777216
webdelta_webhook_secret = "${WEBDELTA_WEBHOOK_SECRET}"

[n8n]
enabled = false
webhook_url = "${N8N_WEBHOOK_URL}"
secret = "${N8N_WEBHOOK_SECRET}"
timeout_secs = 10
"#
        .to_string()
    }
}

fn redact_field(val: &mut String) {
    let trimmed = val.trim();
    if !trimmed.is_empty() && !(trimmed.starts_with("${") && trimmed.ends_with('}')) {
        *val = "<redacted>".to_string();
    }
}

/// Serializes `Config` to TOML after redacting literal secrets while preserving `${VAR}` placeholders.
pub fn redact_config_toml(config: &Config) -> Result<String> {
    let mut cloned = config.clone();
    redact_field(&mut cloned.llm.api_key);
    redact_field(&mut cloned.discord.bot_token);
    redact_field(&mut cloned.discord.webhook_url);
    redact_field(&mut cloned.server.api_token);
    redact_field(&mut cloned.server.webdelta_webhook_secret);
    redact_field(&mut cloned.n8n.secret);
    Ok(toml::to_string_pretty(&cloned)?)
}

/// Helper for converting an array map into an env lookup closure for unit tests.
pub fn env_map_lookup<'a>(
    vars: &'a BTreeMap<String, String>,
) -> impl FnMut(&str) -> Option<String> + 'a {
    move |key: &str| vars.get(key).cloned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_starter_toml_validates_and_redacts_cleanly() {
        let raw = Config::starter_toml();
        let mut cfg = Config::from_toml_str(&raw).expect("starter TOML parses");
        cfg.validate().expect("starter TOML validates");

        // Redact literal secret while keeping env placeholder
        cfg.discord.bot_token = "secret-bot-token".to_string();
        cfg.server.api_token = "${ACADEMIC_API_TOKEN}".to_string();
        let redacted = redact_config_toml(&cfg).expect("redact succeeds");
        assert!(!redacted.contains("secret-bot-token"));
        assert!(redacted.contains("<redacted>"));
        assert!(redacted.contains("${ACADEMIC_API_TOKEN}"));
    }

    #[test]
    fn test_expand_env_and_validation_guards() {
        let mut map = BTreeMap::new();
        map.insert(
            "ROUTER_API_BASE".to_string(),
            "http://127.0.0.1:20128/v1".to_string(),
        );
        map.insert("DISCORD_CHANNEL_ID".to_string(), "123456".to_string());

        let mut cfg = Config::default();
        cfg.expand_with(env_map_lookup(&map));
        assert_eq!(cfg.llm.api_base, "http://127.0.0.1:20128/v1");
        assert_eq!(cfg.discord.channel_id, "123456");

        cfg.general.timezone = "Mars/Olympus".to_string();
        assert!(cfg.validate().is_err());

        cfg.general.timezone = "Asia/Jakarta".to_string();
        cfg.hermes.enabled = true;
        cfg.hermes.target = "  ".to_string();
        assert!(cfg.validate().is_err());

        cfg.hermes.target = "discord:12345".to_string();
        cfg.validate().expect("valid after fixing target");
    }

    #[test]
    fn test_cron_expr_normalization() {
        assert_eq!(normalize_cron_expr("0 7 * * *").unwrap(), "0 0 7 * * *");
        assert!(parse_cron_schedule("0 7 * * *").is_ok());
        assert!(parse_cron_schedule("invalid cron").is_err());
    }
}
