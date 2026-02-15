use std::collections::HashMap;
use std::path::PathBuf;

use serde::Deserialize;

#[derive(Debug, Default, Deserialize)]
pub struct Config {
    #[serde(default)]
    pub server: ServerConfig,
    #[serde(default)]
    pub backends: HashMap<String, BackendConfig>,
    #[serde(default)]
    pub routes: Vec<RouteConfig>,
    #[serde(default)]
    pub default: DefaultRoute,
    #[serde(default)]
    pub logging: LoggingConfig,
}

#[derive(Debug, Default, Deserialize)]
pub struct LoggingConfig {
    #[serde(default)]
    pub metrics: MetricsLogConfig,
}

#[derive(Debug, Deserialize)]
pub struct MetricsLogConfig {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default = "default_metrics_log_path")]
    pub path: String,
    #[serde(default = "default_max_size_mb")]
    pub max_size_mb: u64,
    #[serde(default = "default_max_files")]
    pub max_files: u32,
}

impl Default for MetricsLogConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            path: default_metrics_log_path(),
            max_size_mb: default_max_size_mb(),
            max_files: default_max_files(),
        }
    }
}

fn default_metrics_log_path() -> String {
    dirs::home_dir()
        .map(|h| h.join(".config/croxy/logs/metrics.jsonl"))
        .unwrap_or_else(|| PathBuf::from("/tmp/croxy/logs/metrics.jsonl"))
        .to_string_lossy()
        .to_string()
}

fn default_max_size_mb() -> u64 {
    50
}

fn default_max_files() -> u32 {
    5
}

#[derive(Debug, Deserialize)]
pub struct ServerConfig {
    #[serde(default = "default_host")]
    pub host: String,
    #[serde(default = "default_port")]
    pub port: u16,
    #[serde(default = "default_max_body_size")]
    pub max_body_size: usize,
}

impl Default for ServerConfig {
    fn default() -> Self {
        Self {
            host: default_host(),
            port: default_port(),
            max_body_size: default_max_body_size(),
        }
    }
}

fn default_host() -> String {
    "127.0.0.1".to_string()
}

fn default_port() -> u16 {
    3100
}

fn default_max_body_size() -> usize {
    10 * 1024 * 1024
}

#[derive(Debug, Deserialize)]
pub struct BackendConfig {
    pub url: String,
    #[serde(default)]
    pub strip_auth: bool,
    pub api_key: Option<String>,
    #[serde(default)]
    pub stub_count_tokens: bool,
}

#[derive(Debug, Deserialize)]
pub struct RouteConfig {
    pub pattern: String,
    pub backend: String,
    pub model: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct DefaultRoute {
    #[serde(default = "default_backend")]
    pub backend: String,
}

impl Default for DefaultRoute {
    fn default() -> Self {
        Self {
            backend: default_backend(),
        }
    }
}

fn default_backend() -> String {
    "anthropic".to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use figment::Figment;
    use figment::providers::{Format, Toml};

    #[test]
    fn deserializes_full_config() {
        let cfg: Config = Figment::new()
            .merge(Toml::string(
                r#"
                [server]
                host = "0.0.0.0"
                port = 8080
                [backends.anthropic]
                url = "https://api.anthropic.com"
                [backends.ollama]
                url = "http://localhost:11434"
                strip_auth = true
                api_key = "ollama"
                stub_count_tokens = true
                [[routes]]
                pattern = "opus"
                backend = "anthropic"
                [[routes]]
                pattern = "sonnet"
                backend = "ollama"
                model = "qwen3:30b"
                [default]
                backend = "anthropic"
                "#,
            ))
            .extract()
            .unwrap();

        assert_eq!(cfg.server.host, "0.0.0.0");
        assert_eq!(cfg.server.port, 8080);
        assert_eq!(cfg.backends.len(), 2);
        assert!(cfg.backends["ollama"].strip_auth);
        assert_eq!(cfg.backends["ollama"].api_key.as_deref(), Some("ollama"));
        assert!(cfg.backends["ollama"].stub_count_tokens);
        assert!(!cfg.backends["anthropic"].strip_auth);
        assert_eq!(cfg.backends["anthropic"].api_key, None);
        assert_eq!(cfg.routes.len(), 2);
        assert_eq!(cfg.routes[1].model.as_deref(), Some("qwen3:30b"));
        assert_eq!(cfg.routes[0].model, None);
        assert_eq!(cfg.default.backend, "anthropic");
    }

    #[test]
    fn uses_default_host_and_port() {
        let cfg: Config = Figment::new()
            .merge(Toml::string(
                r#"
                [server]
                [backends.a]
                url = "http://a"
                [[routes]]
                pattern = "x"
                backend = "a"
                [default]
                backend = "a"
                "#,
            ))
            .extract()
            .unwrap();

        assert_eq!(cfg.server.host, "127.0.0.1");
        assert_eq!(cfg.server.port, 3100);
    }

    #[test]
    fn empty_config_uses_defaults() {
        let cfg: Config = Figment::new().merge(Toml::string("")).extract().unwrap();

        assert_eq!(cfg.server.host, "127.0.0.1");
        assert_eq!(cfg.server.port, 3100);
        assert!(cfg.backends.is_empty());
        assert!(cfg.routes.is_empty());
        assert_eq!(cfg.default.backend, "anthropic");
    }

    #[test]
    fn max_body_size_defaults_to_10mb() {
        let cfg: Config = Figment::new().merge(Toml::string("")).extract().unwrap();

        assert_eq!(cfg.server.max_body_size, 10 * 1024 * 1024);
    }

    #[test]
    fn max_body_size_is_configurable() {
        let cfg: Config = Figment::new()
            .merge(Toml::string(
                r#"
                [server]
                max_body_size = 1048576
                "#,
            ))
            .extract()
            .unwrap();

        assert_eq!(cfg.server.max_body_size, 1_048_576);
    }

    #[test]
    fn config_without_routes_section() {
        let cfg: Config = Figment::new()
            .merge(Toml::string(
                r#"
                [backends.anthropic]
                url = "https://api.anthropic.com"
                [default]
                backend = "anthropic"
                "#,
            ))
            .extract()
            .unwrap();

        assert!(cfg.routes.is_empty());
        assert_eq!(cfg.backends.len(), 1);
    }

    #[test]
    fn boolean_fields_default_to_false() {
        let cfg: Config = Figment::new()
            .merge(Toml::string(
                r#"
                [server]
                [backends.a]
                url = "http://a"
                [[routes]]
                pattern = "x"
                backend = "a"
                [default]
                backend = "a"
                "#,
            ))
            .extract()
            .unwrap();

        assert!(!cfg.backends["a"].strip_auth);
        assert!(!cfg.backends["a"].stub_count_tokens);
        assert_eq!(cfg.backends["a"].api_key, None);
    }

    #[test]
    fn logging_defaults_when_omitted() {
        let cfg: Config = Figment::new().merge(Toml::string("")).extract().unwrap();

        assert!(!cfg.logging.metrics.enabled);
        assert_eq!(cfg.logging.metrics.max_size_mb, 50);
        assert_eq!(cfg.logging.metrics.max_files, 5);
        assert!(cfg.logging.metrics.path.contains("metrics.jsonl"));
    }

    #[test]
    fn logging_metrics_config_parses() {
        let cfg: Config = Figment::new()
            .merge(Toml::string(
                r#"
                [logging.metrics]
                enabled = true
                path = "/tmp/test.jsonl"
                max_size_mb = 100
                max_files = 10
                "#,
            ))
            .extract()
            .unwrap();

        assert!(cfg.logging.metrics.enabled);
        assert_eq!(cfg.logging.metrics.path, "/tmp/test.jsonl");
        assert_eq!(cfg.logging.metrics.max_size_mb, 100);
        assert_eq!(cfg.logging.metrics.max_files, 10);
    }
}
