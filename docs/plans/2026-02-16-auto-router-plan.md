# Auto-Router Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use panda:executing-plans to implement this plan task-by-task.

**Goal:** Add AI-based auto-routing to croxy using Arch-Router-1.5B, triggered by `model: "auto"`.

**Architecture:** Routes with `description` fields are sent to an Arch-Router endpoint for classification. The router model returns a route name, which maps to a provider. Pattern-based routes continue to work unchanged. Falls through to default on any failure.

**Tech Stack:** Rust, axum, reqwest, serde, ratatui, regex, figment (TOML)

**Risk Assessment:**
- Central files touched: `proxy.rs` (handle_request is the hot path), `router.rs` (resolve is called per-request), `metrics.rs` (RequestRecord used everywhere)
- The `routed: bool` -> `RoutingMethod` enum change propagates to metrics, logging, TUI, and attach -- coordinate carefully
- No new crate dependencies needed

---

### Task 1: Add RoutingMethod enum to metrics

**Files:**
- Modify: `src/metrics.rs:10-23` (RequestRecord struct)
- Modify: `src/metrics.rs:218-232` (sample_record in tests)

**Step 1: Add RoutingMethod enum and update RequestRecord**

In `src/metrics.rs`, add above `RequestRecord`:

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RoutingMethod {
    Pattern,
    Auto,
    Default,
}

impl std::fmt::Display for RoutingMethod {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            RoutingMethod::Pattern => write!(f, "pattern"),
            RoutingMethod::Auto => write!(f, "auto"),
            RoutingMethod::Default => write!(f, "default"),
        }
    }
}
```

In `RequestRecord`, replace `pub routed: bool` with `pub routing_method: RoutingMethod`.

**Step 2: Fix all compilation errors from the rename**

Update every reference to `routed` -> `routing_method` across the codebase:
- `src/metrics.rs:225` test helper: `routing_method: RoutingMethod::Default`
- `src/proxy.rs:329`: `routing_method: RoutingMethod::Default` (will update to real value in Task 5)
- `src/router.rs:12`: `ResolvedRoute.routed` -> `ResolvedRoute.routing_method: RoutingMethod`
- `src/router.rs:49`: default route: `routing_method: RoutingMethod::Default`
- `src/router.rs:85`: matched route: `routing_method: RoutingMethod::Pattern`
- `src/router.rs:96`: fallback: `routing_method: RoutingMethod::Default`
- `src/attach.rs:36`: `routing_method: RoutingMethod::Default` (log entries don't record method yet)
- `src/tui/views/models.rs:35`: `let routed = records.iter().any(|r| r.routed)` -> update to use `routing_method`

**Step 3: Run tests**

Run: `cargo test`
Expected: All existing tests pass with the renamed field.

**Step 4: Commit**

```
feat(metrics): replace routed bool with RoutingMethod enum
```

---

### Task 2: Update metrics logging and log parsing

**Files:**
- Modify: `src/metrics.rs:139-148` (log_record JSON serialization)
- Modify: `src/attach.rs:14-24` (LogEntry struct)
- Modify: `src/attach.rs:26-43` (parse_log_entry)

**Step 1: Update log_record to serialize routing_method**

In `src/metrics.rs` `log_record`, add to the JSON object:

```rust
"routing_method": record.routing_method.to_string(),
```

**Step 2: Update LogEntry and parse_log_entry for backwards compatibility**

In `src/attach.rs`, add to `LogEntry`:

```rust
routing_method: Option<String>,
```

In `parse_log_entry`, map it:

```rust
routing_method: match entry.routing_method.as_deref() {
    Some("pattern") => RoutingMethod::Pattern,
    Some("auto") => RoutingMethod::Auto,
    _ => RoutingMethod::Default,
},
```

**Step 3: Run tests**

Run: `cargo test`
Expected: All tests pass. Old log entries without `routing_method` field parse as `Default`.

**Step 4: Commit**

```
feat(metrics): serialize routing_method in log entries
```

---

### Task 3: Update TUI to show PTN/AUT/DEF labels

**Files:**
- Modify: `src/tui/views/models.rs:35-46` (model table indicator)
- Modify: `src/tui/views/overview.rs:229-277` (live log rows)

**Step 1: Update model table indicator**

In `src/tui/views/models.rs`, replace the routed indicator logic (lines 35-46):

```rust
use crate::metrics::RoutingMethod;

// Determine the "best" routing method for this model group
let routing_method = if records.iter().any(|r| r.routing_method == RoutingMethod::Auto) {
    RoutingMethod::Auto
} else if records.iter().any(|r| r.routing_method == RoutingMethod::Pattern) {
    RoutingMethod::Pattern
} else {
    RoutingMethod::Default
};

let (indicator, indicator_style) = match routing_method {
    RoutingMethod::Pattern => ("PTN", Style::default().fg(Color::Cyan)),
    RoutingMethod::Auto => ("AUT", Style::default().fg(Color::Yellow)),
    RoutingMethod::Default => ("DEF", Style::default().fg(Color::DarkGray)),
};
```

Update the Cell:

```rust
Cell::from(indicator).style(indicator_style),
```

Widen the first column from `Constraint::Length(2)` to `Constraint::Length(3)`.

**Step 2: Add Route column to live log**

In `src/tui/views/overview.rs`, add "Route" to the header (line 229-231):

```rust
let header = Row::new(vec![
    "Age", "Model", "Provider", "Route", "Status", "Duration", "In/Out",
])
```

Add a Route cell in each row (after Provider cell):

```rust
use crate::metrics::RoutingMethod;

let (route_label, route_style) = match r.routing_method {
    RoutingMethod::Pattern => ("PTN", Style::default().fg(Color::Cyan)),
    RoutingMethod::Auto => ("AUT", Style::default().fg(Color::Yellow)),
    RoutingMethod::Default => ("DEF", Style::default().fg(Color::DarkGray)),
};
// Add after Provider cell:
Cell::from(route_label).style(route_style),
```

Add column constraint `Constraint::Length(5)` for the Route column.

**Step 3: Run tests and build**

Run: `cargo test && cargo build`
Expected: Compiles and all tests pass.

**Step 4: Commit**

```
feat(tui): display routing method as PTN/AUT/DEF labels
```

---

### Task 4: Add AutoRouterConfig to config

**Files:**
- Modify: `src/config.rs:1-20` (Config struct, imports)
- Modify: `src/config.rs:134-139` (RouteConfig)

**Step 1: Add AutoRouterConfig struct**

```rust
#[derive(Debug, Deserialize)]
pub struct AutoRouterConfig {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default)]
    pub url: String,
    #[serde(default)]
    pub model: String,
    #[serde(default = "default_auto_router_timeout_ms")]
    pub timeout_ms: u64,
}

impl Default for AutoRouterConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            url: String::new(),
            model: String::new(),
            timeout_ms: default_auto_router_timeout_ms(),
        }
    }
}

fn default_auto_router_timeout_ms() -> u64 {
    2000
}
```

Add to `Config`:

```rust
#[serde(default)]
pub auto_router: AutoRouterConfig,
```

**Step 2: Modify RouteConfig to support optional pattern + description**

```rust
#[derive(Debug, Deserialize)]
pub struct RouteConfig {
    pub name: Option<String>,
    pub description: Option<String>,
    pub pattern: Option<String>,
    pub provider: String,
    pub model: Option<String>,
}
```

**Step 3: Add config tests**

```rust
#[test]
fn auto_router_defaults_when_omitted() {
    let cfg: Config = Figment::new().merge(Toml::string("")).extract().unwrap();
    assert!(!cfg.auto_router.enabled);
    assert_eq!(cfg.auto_router.timeout_ms, 2000);
}

#[test]
fn auto_router_config_parses() {
    let cfg: Config = Figment::new()
        .merge(Toml::string(
            r#"
            [auto_router]
            enabled = true
            url = "http://localhost:8080/v1/chat/completions"
            model = "Arch-Router-1.5B"
            timeout_ms = 3000
            "#,
        ))
        .extract()
        .unwrap();
    assert!(cfg.auto_router.enabled);
    assert_eq!(cfg.auto_router.url, "http://localhost:8080/v1/chat/completions");
    assert_eq!(cfg.auto_router.model, "Arch-Router-1.5B");
    assert_eq!(cfg.auto_router.timeout_ms, 3000);
}

#[test]
fn route_with_description_and_pattern() {
    let cfg: Config = Figment::new()
        .merge(Toml::string(
            r#"
            [provider.a]
            url = "http://a"
            [[routes]]
            name = "coding"
            description = "code generation tasks"
            pattern = "opus"
            provider = "a"
            [default]
            provider = "a"
            "#,
        ))
        .extract()
        .unwrap();
    assert_eq!(cfg.routes[0].name.as_deref(), Some("coding"));
    assert_eq!(cfg.routes[0].description.as_deref(), Some("code generation tasks"));
    assert_eq!(cfg.routes[0].pattern.as_deref(), Some("opus"));
}

#[test]
fn route_with_description_only() {
    let cfg: Config = Figment::new()
        .merge(Toml::string(
            r#"
            [provider.a]
            url = "http://a"
            [[routes]]
            name = "coding"
            description = "code generation tasks"
            provider = "a"
            [default]
            provider = "a"
            "#,
        ))
        .extract()
        .unwrap();
    assert!(cfg.routes[0].pattern.is_none());
    assert_eq!(cfg.routes[0].name.as_deref(), Some("coding"));
}
```

**Step 4: Fix existing config tests that use `pattern` as required field**

Existing tests use `pattern = "x"` etc. These still work since `pattern` is now `Option<String>` -- TOML deserialization handles both cases.

**Step 5: Run tests**

Run: `cargo test`

**Step 6: Commit**

```
feat(config): add auto_router config and optional route fields
```

---

### Task 5: Update Router for auto-routing support

**Files:**
- Modify: `src/router.rs` (major refactor)

**Step 1: Add AutoRouteCandidate and update Router struct**

```rust
use crate::config::{AutoRouterConfig, Config};
use crate::metrics::RoutingMethod;

pub struct RouteCandidate {
    pub name: String,
    pub description: String,
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
```

**Step 2: Update from_config with validation**

In `from_config`:
- Build `auto_routes` and `auto_candidates` from routes that have `description`
- Only compile regex for routes that have `pattern`
- Validate: description requires name, no duplicate names, if auto_router enabled check url is non-empty
- Store `auto_router_config` if enabled and there are description routes

**Step 3: Make resolve async**

```rust
pub async fn resolve(
    &self,
    model: &str,
    messages: Option<&Vec<serde_json::Value>>,
    client: &reqwest::Client,
) -> ResolvedRoute
```

Logic:
1. If model == "auto" and auto_router_config is Some and messages is Some:
   - Call `auto_router::classify(client, config, &self.auto_candidates, messages).await`
   - If Some(name), look up in auto_routes by name, return with `routing_method: RoutingMethod::Auto`
   - If None, return default
2. Otherwise, try pattern routes (existing logic) with `routing_method: RoutingMethod::Pattern`
3. Fall through to default with `routing_method: RoutingMethod::Default`

**Step 4: Update tests**

Existing tests call `router.resolve("model")` synchronously. They need to become async or use a sync wrapper. Since pattern-based resolution doesn't actually need async (no HTTP call), add a convenience method:

```rust
pub fn resolve_pattern(&self, model: &str) -> ResolvedRoute
```

This contains the existing sync logic. `resolve()` calls this for non-"auto" models. Tests use `resolve_pattern()` directly, avoiding async in tests that don't test auto-routing.

Add new tests:
- `auto_route_candidates_built_from_description_routes`
- `description_without_name_errors`
- `duplicate_route_names_error`
- `route_without_pattern_or_description_errors`

**Step 5: Run tests**

Run: `cargo test`

**Step 6: Commit**

```
feat(router): add auto-routing support with async resolve
```

---

### Task 6: Create auto_router module

**Files:**
- Create: `src/auto_router.rs`
- Modify: `src/lib.rs` (add module declaration)

**Step 1: Write the module**

```rust
use std::time::Duration;

use regex::Regex;
use serde::Deserialize;
use tracing::{info, warn};

use crate::config::AutoRouterConfig;
use crate::router::RouteCandidate;

const TASK_INSTRUCTION: &str = r#"
You are a helpful assistant designed to find the best suited route.
You are provided with route description within <routes></routes> XML tags:
<routes>

{routes}

</routes>

<conversation>

{conversation}

</conversation>
"#;

const FORMAT_PROMPT: &str = r#"
Your task is to decide which route is best suit with user intent on the conversation in <conversation></conversation> XML tags.  Follow the instruction:
1. If the latest intent from user is irrelevant or user intent is full filled, response with other route {"route": "other"}.
2. You must analyze the route descriptions and find the best match route for user latest intent.
3. You only response the name of the route that best matches the user's request, use the exact name in the <routes></routes>.

Based on your analysis, provide your response in the following JSON formats if you decide to match any route:
{"route": "route_name"}
"#;

#[derive(Deserialize)]
struct ChatResponse {
    choices: Vec<Choice>,
}

#[derive(Deserialize)]
struct Choice {
    message: Message,
}

#[derive(Deserialize)]
struct Message {
    content: Option<String>,
}

fn build_prompt(routes: &[RouteCandidate], messages: &[serde_json::Value]) -> String {
    let route_defs: Vec<serde_json::Value> = routes
        .iter()
        .map(|r| serde_json::json!({"name": &r.name, "description": &r.description}))
        .collect();

    let non_system: Vec<&serde_json::Value> = messages
        .iter()
        .filter(|m| m.get("role").and_then(|r| r.as_str()) != Some("system"))
        .collect();

    let prompt = TASK_INSTRUCTION
        .replace("{routes}", &serde_json::to_string(&route_defs).unwrap_or_default())
        .replace("{conversation}", &serde_json::to_string(&non_system).unwrap_or_default());

    format!("{prompt}{FORMAT_PROMPT}")
}

fn parse_route_name(text: &str, valid_names: &[&str]) -> Option<String> {
    // Try full JSON parse first
    if let Ok(v) = serde_json::from_str::<serde_json::Value>(text.trim()) {
        if let Some(name) = v.get("route").and_then(|r| r.as_str()) {
            if name != "other" && valid_names.contains(&name) {
                return Some(name.to_string());
            }
            return None;
        }
    }

    // Fallback: regex extraction
    let re = Regex::new(r#"\{"route"\s*:\s*"([^"]+)"\}"#).ok()?;
    let captures = re.captures(text)?;
    let name = captures.get(1)?.as_str();
    if name != "other" && valid_names.contains(&name) {
        Some(name.to_string())
    } else {
        None
    }
}

pub async fn classify(
    client: &reqwest::Client,
    config: &AutoRouterConfig,
    routes: &[RouteCandidate],
    messages: &[serde_json::Value],
) -> Option<String> {
    if routes.is_empty() || messages.is_empty() {
        return None;
    }

    let prompt = build_prompt(routes, messages);
    let valid_names: Vec<&str> = routes.iter().map(|r| r.name.as_str()).collect();

    info!(
        route_count = routes.len(),
        model = %config.model,
        "auto-routing request via Arch-Router"
    );

    let body = serde_json::json!({
        "model": &config.model,
        "messages": [{"role": "user", "content": prompt}],
        "max_tokens": 64,
        "temperature": 0.0,
        "response_format": {"type": "json_object"},
    });

    let response = match client
        .post(&config.url)
        .json(&body)
        .timeout(Duration::from_millis(config.timeout_ms))
        .send()
        .await
    {
        Ok(r) => r,
        Err(e) => {
            warn!(error = %e, "auto-router request failed, falling through to default");
            return None;
        }
    };

    let chat: ChatResponse = match response.json().await {
        Ok(c) => c,
        Err(e) => {
            warn!(error = %e, "auto-router response parse failed, falling through to default");
            return None;
        }
    };

    let content = chat.choices.first()?.message.content.as_deref()?;
    let result = parse_route_name(content, &valid_names);

    match &result {
        Some(name) => info!(route = %name, "auto-router selected route"),
        None => warn!(
            response = %content,
            "auto-router returned no match, falling through to default"
        ),
    }

    result
}

#[cfg(test)]
mod tests {
    use super::*;

    fn candidates() -> Vec<RouteCandidate> {
        vec![
            RouteCandidate {
                name: "code_gen".to_string(),
                description: "code generation".to_string(),
            },
            RouteCandidate {
                name: "summarize".to_string(),
                description: "summarization".to_string(),
            },
        ]
    }

    #[test]
    fn parse_clean_json() {
        let names = vec!["code_gen", "summarize"];
        assert_eq!(
            parse_route_name(r#"{"route": "code_gen"}"#, &names),
            Some("code_gen".to_string())
        );
    }

    #[test]
    fn parse_other_returns_none() {
        let names = vec!["code_gen"];
        assert_eq!(parse_route_name(r#"{"route": "other"}"#, &names), None);
    }

    #[test]
    fn parse_unknown_name_returns_none() {
        let names = vec!["code_gen"];
        assert_eq!(parse_route_name(r#"{"route": "unknown"}"#, &names), None);
    }

    #[test]
    fn parse_with_preamble() {
        let names = vec!["code_gen", "summarize"];
        let text = "Based on the analysis, the best route is:\n{\"route\": \"summarize\"}";
        assert_eq!(
            parse_route_name(text, &names),
            Some("summarize".to_string())
        );
    }

    #[test]
    fn parse_garbage_returns_none() {
        let names = vec!["code_gen"];
        assert_eq!(parse_route_name("not json at all", &names), None);
    }

    #[test]
    fn build_prompt_filters_system_messages() {
        let routes = candidates();
        let messages = vec![
            serde_json::json!({"role": "system", "content": "you are helpful"}),
            serde_json::json!({"role": "user", "content": "write code"}),
        ];
        let prompt = build_prompt(&routes, &messages);
        assert!(prompt.contains("write code"));
        assert!(!prompt.contains("you are helpful"));
        assert!(prompt.contains("code_gen"));
        assert!(prompt.contains("summarize"));
    }

    #[test]
    fn build_prompt_includes_all_routes() {
        let routes = candidates();
        let messages = vec![serde_json::json!({"role": "user", "content": "hello"})];
        let prompt = build_prompt(&routes, &messages);
        assert!(prompt.contains("code generation"));
        assert!(prompt.contains("summarization"));
    }
}
```

**Step 2: Add module to lib.rs**

Add `pub mod auto_router;` to `src/lib.rs`.

**Step 3: Run tests**

Run: `cargo test`

**Step 4: Commit**

```
feat: add auto_router module for Arch-Router integration
```

---

### Task 7: Wire up proxy.rs

**Files:**
- Modify: `src/proxy.rs:232-265` (handle_request)

**Step 1: Extract messages and pass to async resolve**

After the body parsing block (line ~263), extract messages:

```rust
let messages = body_json
    .as_ref()
    .and_then(|j| j.get("messages"))
    .and_then(|m| m.as_array())
    .cloned();
```

Change the resolve call:

```rust
let route = state.router.resolve(&model, messages.as_ref(), &state.client).await;
```

**Step 2: Set routing_method on RequestRecord from resolved route**

The `base_record` already gets `routing_method` from `route.routing_method` since Task 1 wired that through.

**Step 3: Run tests and build**

Run: `cargo test && cargo build`

**Step 4: Commit**

```
feat(proxy): pass messages to async router for auto-routing
```

---

### Task 8: Update default config template

**Files:**
- Modify: `src/main.rs:157-192` (cmd_init default config)

**Step 1: Add commented auto_router section to default config**

Add after the `[default]` section:

```toml
# [auto_router]
# enabled = true
# url = "http://localhost:8080/v1/chat/completions"
# model = "Arch-Router-1.5B"
# timeout_ms = 2000
```

**Step 2: Commit**

```
feat: add auto_router section to default config template
```

---

### Task 9: Write docs/router.md

**Files:**
- Create: `docs/router.md`

**Step 1: Write documentation**

Cover:
- Overview of routing modes (pattern, auto, combined)
- Config reference for `[auto_router]` and route fields (`name`, `description`, `pattern`, `provider`, `model`)
- Setting up the MLX endpoint
- Best practices for route descriptions (from Arch-Router docs)
- Fallback behavior
- Example configs: pattern-only, auto-only, hybrid
- TUI routing indicators

**Step 2: Commit**

```
docs: add routing documentation
```

---

### Task 10: Integration verification

**Step 1: Run full test suite**

Run: `cargo test`

**Step 2: Run clippy**

Run: `cargo clippy -- -D warnings`

**Step 3: Build release**

Run: `cargo build --release`

**Step 4: Final commit if any fixes needed**

**Step 5: Create PR**
