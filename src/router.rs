use std::collections::HashSet;

use regex::Regex;
use tracing::warn;

use crate::config::{AutoRouterConfig, Config};
use crate::metrics::RoutingMethod;

pub struct ResolvedRoute {
    pub provider_name: String,
    pub provider_url: String,
    pub model_rewrite: Option<String>,
    pub strip_auth: bool,
    pub api_key: Option<String>,
    pub stub_count_tokens: bool,
    pub routing_method: RoutingMethod,
}

pub struct RouteCandidate {
    pub name: String,
    pub description: String,
}

struct CompiledRoute {
    pattern: Regex,
    provider_name: String,
    provider_url: String,
    model_rewrite: Option<String>,
    strip_auth: bool,
    api_key: Option<String>,
    stub_count_tokens: bool,
}

struct AutoRouteEntry {
    name: String,
    provider_name: String,
    provider_url: String,
    model_rewrite: Option<String>,
    strip_auth: bool,
    api_key: Option<String>,
    stub_count_tokens: bool,
}

pub struct Router {
    routes: Vec<CompiledRoute>,
    auto_routes: Vec<AutoRouteEntry>,
    auto_candidates: Vec<RouteCandidate>,
    auto_router_config: Option<AutoRouterConfig>,
    default: ResolvedRoute,
}

impl Router {
    pub fn from_config(config: &Config) -> Result<Self, String> {
        let default_provider = config
            .providers
            .get(&config.default.provider)
            .ok_or_else(|| {
                format!(
                    "default provider '{}' not found in providers",
                    config.default.provider
                )
            })?;

        let default = ResolvedRoute {
            provider_name: config.default.provider.clone(),
            provider_url: default_provider.url.clone(),
            model_rewrite: None,
            strip_auth: default_provider.strip_auth,
            api_key: default_provider.api_key.clone(),
            stub_count_tokens: default_provider.stub_count_tokens,
            routing_method: RoutingMethod::Default,
        };

        let mut routes = Vec::new();
        let mut auto_routes = Vec::new();
        let mut auto_candidates = Vec::new();
        let mut seen_names = HashSet::new();

        for route in &config.routes {
            if route.pattern.is_none() && route.description.is_none() {
                return Err(format!(
                    "route for provider '{}' has neither pattern nor description",
                    route.provider
                ));
            }

            if route.description.is_some() && route.name.is_none() {
                return Err(format!(
                    "route for provider '{}' has description but no name",
                    route.provider
                ));
            }

            let provider = config.providers.get(&route.provider).ok_or_else(|| {
                format!("route provider '{}' not found in providers", route.provider)
            })?;

            if let Some(ref pattern_str) = route.pattern {
                let pattern = Regex::new(pattern_str)
                    .map_err(|e| format!("invalid regex '{}': {}", pattern_str, e))?;

                routes.push(CompiledRoute {
                    pattern,
                    provider_name: route.provider.clone(),
                    provider_url: provider.url.clone(),
                    model_rewrite: route.model.clone(),
                    strip_auth: provider.strip_auth,
                    api_key: provider.api_key.clone(),
                    stub_count_tokens: provider.stub_count_tokens,
                });
            }

            if let (Some(name), Some(description)) = (&route.name, &route.description) {
                if !seen_names.insert(name.clone()) {
                    return Err(format!("duplicate route name '{name}'"));
                }

                auto_routes.push(AutoRouteEntry {
                    name: name.clone(),
                    provider_name: route.provider.clone(),
                    provider_url: provider.url.clone(),
                    model_rewrite: route.model.clone(),
                    strip_auth: provider.strip_auth,
                    api_key: provider.api_key.clone(),
                    stub_count_tokens: provider.stub_count_tokens,
                });

                auto_candidates.push(RouteCandidate {
                    name: name.clone(),
                    description: description.clone(),
                });
            }
        }

        let auto_router_config = if config.auto_router.enabled {
            if config.auto_router.url.is_empty() {
                return Err("auto_router.enabled is true but url is empty".to_string());
            }
            if auto_candidates.is_empty() {
                warn!("auto_router is enabled but no routes have descriptions");
            }
            Some(AutoRouterConfig {
                enabled: true,
                url: config.auto_router.url.clone(),
                model: config.auto_router.model.clone(),
                timeout_ms: config.auto_router.timeout_ms,
            })
        } else {
            None
        };

        Ok(Router {
            routes,
            auto_routes,
            auto_candidates,
            auto_router_config,
            default,
        })
    }

    pub async fn resolve(
        &self,
        model: &str,
        messages: Option<&Vec<serde_json::Value>>,
        client: &reqwest::Client,
    ) -> ResolvedRoute {
        if model == "auto" {
            if let Some(ref config) = self.auto_router_config
                && let Some(messages) = messages
                && !self.auto_candidates.is_empty()
                && let Some(name) = crate::auto_router::classify(
                    client,
                    config,
                    &self.auto_candidates,
                    messages,
                )
                .await
                && let Some(entry) = self.auto_routes.iter().find(|r| r.name == name)
            {
                return ResolvedRoute {
                    provider_name: entry.provider_name.clone(),
                    provider_url: entry.provider_url.clone(),
                    model_rewrite: entry.model_rewrite.clone(),
                    strip_auth: entry.strip_auth,
                    api_key: entry.api_key.clone(),
                    stub_count_tokens: entry.stub_count_tokens,
                    routing_method: RoutingMethod::Auto,
                };
            }
            return self.make_default();
        }

        self.resolve_pattern(model)
    }

    pub fn resolve_pattern(&self, model: &str) -> ResolvedRoute {
        for route in &self.routes {
            if route.pattern.is_match(model) {
                return ResolvedRoute {
                    provider_name: route.provider_name.clone(),
                    provider_url: route.provider_url.clone(),
                    model_rewrite: route.model_rewrite.clone(),
                    strip_auth: route.strip_auth,
                    api_key: route.api_key.clone(),
                    stub_count_tokens: route.stub_count_tokens,
                    routing_method: RoutingMethod::Pattern,
                };
            }
        }

        self.make_default()
    }

    fn make_default(&self) -> ResolvedRoute {
        ResolvedRoute {
            provider_name: self.default.provider_name.clone(),
            provider_url: self.default.provider_url.clone(),
            model_rewrite: self.default.model_rewrite.clone(),
            strip_auth: self.default.strip_auth,
            api_key: self.default.api_key.clone(),
            stub_count_tokens: self.default.stub_count_tokens,
            routing_method: RoutingMethod::Default,
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
            [provider.anthropic]
            url = "https://api.anthropic.com"
            [provider.ollama]
            url = "http://localhost:11434"
            strip_auth = true
            api_key = "ollama"
            stub_count_tokens = true
            [[routes]]
            pattern = "opus"
            provider = "anthropic"
            [[routes]]
            pattern = "sonnet|haiku"
            provider = "ollama"
            model = "qwen3-coder:30b"
            [default]
            provider = "anthropic"
            "#,
        )
    }

    fn resolve_production(model: &str) -> ResolvedRoute {
        Router::from_config(&production_config())
            .unwrap()
            .resolve_pattern(model)
    }

    #[test]
    fn opus_routes_to_anthropic() {
        let route = resolve_production("claude-opus-4-6");
        assert_eq!(route.provider_url, "https://api.anthropic.com");
        assert_eq!(route.model_rewrite, None);
        assert!(!route.strip_auth);
        assert_eq!(route.api_key, None);
        assert!(!route.stub_count_tokens);
    }

    #[test]
    fn sonnet_routes_to_ollama_with_rewrite() {
        let route = resolve_production("claude-sonnet-4-5-20250929");
        assert_eq!(route.provider_url, "http://localhost:11434");
        assert_eq!(route.model_rewrite.as_deref(), Some("qwen3-coder:30b"));
        assert!(route.strip_auth);
        assert_eq!(route.api_key.as_deref(), Some("ollama"));
        assert!(route.stub_count_tokens);
    }

    #[test]
    fn haiku_routes_to_ollama_with_rewrite() {
        let route = resolve_production("claude-haiku-4-5-20251001");
        assert_eq!(route.provider_url, "http://localhost:11434");
        assert_eq!(route.model_rewrite.as_deref(), Some("qwen3-coder:30b"));
    }

    #[test]
    fn unmatched_model_falls_back_to_default() {
        let route = resolve_production("some-unknown-model");
        assert_eq!(route.provider_url, "https://api.anthropic.com");
        assert_eq!(route.model_rewrite, None);
    }

    #[test]
    fn empty_model_falls_back_to_default() {
        let route = resolve_production("");
        assert_eq!(route.provider_url, "https://api.anthropic.com");
    }

    #[test]
    fn first_matching_route_wins() {
        let cfg = config(
            r#"
            [server]
            [provider.a]
            url = "http://a"
            [provider.b]
            url = "http://b"
            [[routes]]
            pattern = "opus"
            provider = "a"
            [[routes]]
            pattern = "opus"
            provider = "b"
            [default]
            provider = "a"
            "#,
        );
        let router = Router::from_config(&cfg).unwrap();
        let route = router.resolve_pattern("opus");
        assert_eq!(route.provider_url, "http://a");
    }

    #[test]
    fn invalid_regex_returns_error() {
        let cfg = config(
            r#"
            [server]
            [provider.a]
            url = "http://a"
            [[routes]]
            pattern = "[invalid"
            provider = "a"
            [default]
            provider = "a"
            "#,
        );
        let err = Router::from_config(&cfg).err().expect("should fail");
        assert!(err.contains("invalid regex"), "got: {err}");
    }

    #[test]
    fn missing_route_provider_returns_error() {
        let cfg = config(
            r#"
            [server]
            [provider.a]
            url = "http://a"
            [[routes]]
            pattern = "test"
            provider = "nonexistent"
            [default]
            provider = "a"
            "#,
        );
        let err = Router::from_config(&cfg).err().expect("should fail");
        assert!(err.contains("not found"), "got: {err}");
    }

    #[test]
    fn resolved_route_includes_provider_name() {
        let route = resolve_production("claude-opus-4-6");
        assert_eq!(route.provider_name, "anthropic");

        let route = resolve_production("claude-sonnet-4-5-20250929");
        assert_eq!(route.provider_name, "ollama");
    }

    #[test]
    fn missing_default_provider_returns_error() {
        let cfg = config(
            r#"
            [server]
            [provider.a]
            url = "http://a"
            [[routes]]
            pattern = "x"
            provider = "a"
            [default]
            provider = "nonexistent"
            "#,
        );
        let err = Router::from_config(&cfg).err().expect("should fail");
        assert!(err.contains("not found"), "got: {err}");
    }

    #[test]
    fn description_without_name_errors() {
        let cfg = config(
            r#"
            [provider.a]
            url = "http://a"
            [[routes]]
            description = "some task"
            provider = "a"
            [default]
            provider = "a"
            "#,
        );
        let err = Router::from_config(&cfg).err().expect("should fail");
        assert!(err.contains("description but no name"), "got: {err}");
    }

    #[test]
    fn route_without_pattern_or_description_errors() {
        let cfg = config(
            r#"
            [provider.a]
            url = "http://a"
            [[routes]]
            provider = "a"
            [default]
            provider = "a"
            "#,
        );
        let err = Router::from_config(&cfg).err().expect("should fail");
        assert!(
            err.contains("neither pattern nor description"),
            "got: {err}"
        );
    }

    #[test]
    fn duplicate_route_names_error() {
        let cfg = config(
            r#"
            [provider.a]
            url = "http://a"
            [[routes]]
            name = "coding"
            description = "code tasks"
            pattern = "opus"
            provider = "a"
            [[routes]]
            name = "coding"
            description = "other code tasks"
            pattern = "sonnet"
            provider = "a"
            [default]
            provider = "a"
            "#,
        );
        let err = Router::from_config(&cfg).err().expect("should fail");
        assert!(err.contains("duplicate route name"), "got: {err}");
    }

    #[test]
    fn auto_router_enabled_without_url_errors() {
        let cfg = config(
            r#"
            [auto_router]
            enabled = true
            [provider.a]
            url = "http://a"
            [[routes]]
            name = "coding"
            description = "code tasks"
            provider = "a"
            [default]
            provider = "a"
            "#,
        );
        let err = Router::from_config(&cfg).err().expect("should fail");
        assert!(err.contains("url is empty"), "got: {err}");
    }

    #[test]
    fn auto_candidates_built_from_description_routes() {
        let cfg = config(
            r#"
            [provider.a]
            url = "http://a"
            [provider.b]
            url = "http://b"
            [[routes]]
            name = "coding"
            description = "code tasks"
            pattern = "opus"
            provider = "a"
            [[routes]]
            pattern = "sonnet"
            provider = "b"
            [default]
            provider = "a"
            "#,
        );
        let router = Router::from_config(&cfg).unwrap();
        assert_eq!(router.auto_candidates.len(), 1);
        assert_eq!(router.auto_candidates[0].name, "coding");
        assert_eq!(router.auto_routes.len(), 1);
        // Both routes compiled as pattern routes
        assert_eq!(router.routes.len(), 2);
    }

    #[test]
    fn description_only_route_not_in_pattern_routes() {
        let cfg = config(
            r#"
            [provider.a]
            url = "http://a"
            [[routes]]
            name = "coding"
            description = "code tasks"
            provider = "a"
            [default]
            provider = "a"
            "#,
        );
        let router = Router::from_config(&cfg).unwrap();
        assert_eq!(router.routes.len(), 0);
        assert_eq!(router.auto_candidates.len(), 1);
    }
}
