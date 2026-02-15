use regex::Regex;

use crate::config::Config;

pub struct ResolvedRoute {
    pub backend_name: String,
    pub backend_url: String,
    pub model_rewrite: Option<String>,
    pub strip_auth: bool,
    pub api_key: Option<String>,
    pub stub_count_tokens: bool,
    pub routed: bool,
}

struct CompiledRoute {
    pattern: Regex,
    backend_name: String,
    backend_url: String,
    model_rewrite: Option<String>,
    strip_auth: bool,
    api_key: Option<String>,
    stub_count_tokens: bool,
}

pub struct Router {
    routes: Vec<CompiledRoute>,
    default: ResolvedRoute,
}

impl Router {
    pub fn from_config(config: &Config) -> Result<Self, String> {
        let default_backend = config
            .backends
            .get(&config.default.backend)
            .ok_or_else(|| {
                format!(
                    "default backend '{}' not found in backends",
                    config.default.backend
                )
            })?;

        let default = ResolvedRoute {
            backend_name: config.default.backend.clone(),
            backend_url: default_backend.url.clone(),
            model_rewrite: None,
            strip_auth: default_backend.strip_auth,
            api_key: default_backend.api_key.clone(),
            stub_count_tokens: default_backend.stub_count_tokens,
            routed: false,
        };

        let mut routes = Vec::new();
        for route in &config.routes {
            let pattern = Regex::new(&route.pattern)
                .map_err(|e| format!("invalid regex '{}': {}", route.pattern, e))?;

            let backend = config.backends.get(&route.backend).ok_or_else(|| {
                format!("route backend '{}' not found in backends", route.backend)
            })?;

            routes.push(CompiledRoute {
                pattern,
                backend_name: route.backend.clone(),
                backend_url: backend.url.clone(),
                model_rewrite: route.model.clone(),
                strip_auth: backend.strip_auth,
                api_key: backend.api_key.clone(),
                stub_count_tokens: backend.stub_count_tokens,
            });
        }

        Ok(Router { routes, default })
    }

    pub fn resolve(&self, model: &str) -> ResolvedRoute {
        for route in &self.routes {
            if route.pattern.is_match(model) {
                return ResolvedRoute {
                    backend_name: route.backend_name.clone(),
                    backend_url: route.backend_url.clone(),
                    model_rewrite: route.model_rewrite.clone(),
                    strip_auth: route.strip_auth,
                    api_key: route.api_key.clone(),
                    stub_count_tokens: route.stub_count_tokens,
                    routed: true,
                };
            }
        }

        ResolvedRoute {
            backend_name: self.default.backend_name.clone(),
            backend_url: self.default.backend_url.clone(),
            model_rewrite: self.default.model_rewrite.clone(),
            strip_auth: self.default.strip_auth,
            api_key: self.default.api_key.clone(),
            stub_count_tokens: self.default.stub_count_tokens,
            routed: false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use figment::Figment;
    use figment::providers::{Format, Toml};

    fn config(toml: &str) -> Config {
        Figment::new().merge(Toml::string(toml)).extract().unwrap()
    }

    fn production_config() -> Config {
        config(
            r#"
            [server]
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
            pattern = "sonnet|haiku"
            backend = "ollama"
            model = "qwen3-coder:30b"
            [default]
            backend = "anthropic"
            "#,
        )
    }

    fn resolve_production(model: &str) -> ResolvedRoute {
        Router::from_config(&production_config())
            .unwrap()
            .resolve(model)
    }

    #[test]
    fn opus_routes_to_anthropic() {
        let route = resolve_production("claude-opus-4-6");
        assert_eq!(route.backend_url, "https://api.anthropic.com");
        assert_eq!(route.model_rewrite, None);
        assert!(!route.strip_auth);
        assert_eq!(route.api_key, None);
        assert!(!route.stub_count_tokens);
    }

    #[test]
    fn sonnet_routes_to_ollama_with_rewrite() {
        let route = resolve_production("claude-sonnet-4-5-20250929");
        assert_eq!(route.backend_url, "http://localhost:11434");
        assert_eq!(route.model_rewrite.as_deref(), Some("qwen3-coder:30b"));
        assert!(route.strip_auth);
        assert_eq!(route.api_key.as_deref(), Some("ollama"));
        assert!(route.stub_count_tokens);
    }

    #[test]
    fn haiku_routes_to_ollama_with_rewrite() {
        let route = resolve_production("claude-haiku-4-5-20251001");
        assert_eq!(route.backend_url, "http://localhost:11434");
        assert_eq!(route.model_rewrite.as_deref(), Some("qwen3-coder:30b"));
    }

    #[test]
    fn unmatched_model_falls_back_to_default() {
        let route = resolve_production("some-unknown-model");
        assert_eq!(route.backend_url, "https://api.anthropic.com");
        assert_eq!(route.model_rewrite, None);
    }

    #[test]
    fn empty_model_falls_back_to_default() {
        let route = resolve_production("");
        assert_eq!(route.backend_url, "https://api.anthropic.com");
    }

    #[test]
    fn first_matching_route_wins() {
        let cfg = config(
            r#"
            [server]
            [backends.a]
            url = "http://a"
            [backends.b]
            url = "http://b"
            [[routes]]
            pattern = "opus"
            backend = "a"
            [[routes]]
            pattern = "opus"
            backend = "b"
            [default]
            backend = "a"
            "#,
        );
        let router = Router::from_config(&cfg).unwrap();
        let route = router.resolve("opus");
        assert_eq!(route.backend_url, "http://a");
    }

    #[test]
    fn invalid_regex_returns_error() {
        let cfg = config(
            r#"
            [server]
            [backends.a]
            url = "http://a"
            [[routes]]
            pattern = "[invalid"
            backend = "a"
            [default]
            backend = "a"
            "#,
        );
        let err = Router::from_config(&cfg).err().expect("should fail");
        assert!(err.contains("invalid regex"), "got: {err}");
    }

    #[test]
    fn missing_route_backend_returns_error() {
        let cfg = config(
            r#"
            [server]
            [backends.a]
            url = "http://a"
            [[routes]]
            pattern = "test"
            backend = "nonexistent"
            [default]
            backend = "a"
            "#,
        );
        let err = Router::from_config(&cfg).err().expect("should fail");
        assert!(err.contains("not found"), "got: {err}");
    }

    #[test]
    fn resolved_route_includes_backend_name() {
        let route = resolve_production("claude-opus-4-6");
        assert_eq!(route.backend_name, "anthropic");

        let route = resolve_production("claude-sonnet-4-5-20250929");
        assert_eq!(route.backend_name, "ollama");
    }

    #[test]
    fn missing_default_backend_returns_error() {
        let cfg = config(
            r#"
            [server]
            [backends.a]
            url = "http://a"
            [[routes]]
            pattern = "x"
            backend = "a"
            [default]
            backend = "nonexistent"
            "#,
        );
        let err = Router::from_config(&cfg).err().expect("should fail");
        assert!(err.contains("not found"), "got: {err}");
    }
}
