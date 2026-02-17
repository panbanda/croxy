# Auto-Router Design: Preference-Aligned Model Routing

## Overview

Add AI-based request routing to croxy using [Arch-Router-1.5B](https://huggingface.co/katanemo/Arch-Router-1.5B), a compact routing model that classifies user intent against configured route descriptions. This supplements the existing regex-based pattern routing with semantic routing powered by a local (or remote) LLM.

## Decisions

| Decision | Choice | Rationale |
|---|---|---|
| Trigger | Client sends `model: "auto"` | Explicit opt-in, backwards-compatible |
| Input to router model | Full conversation history, excluding system messages | Per Arch-Router docs: routes on conversation history, does not use system prompts |
| Fallback on `"other"` | Fall through to default provider | Proxy should always have a working fallback |
| Fallback on router failure | Fall through to default provider (log warning) | Resilience over correctness for routing |
| Endpoint format | `/v1/chat/completions` (OpenAI-compatible) | Industry standard, portable across mlx_lm/vllm/ollama |
| Prompt format | Arch-Router custom XML prompt (not tool-calling) | Per official docs and examples |
| Response parsing | Try JSON parse, fallback to regex extraction | Handles preamble/quantization artifacts |
| Config: `name` field | Required when `description` is present | Arch-Router returns route by name; bad names degrade quality |
| Config: `pattern` + `description` | Allowed on same route | Avoids duplication; same route works for both matching strategies |
| TUI indicator | `PTN` / `AUT` / `DEF` labels (cyan/yellow/gray) | Unambiguous, monospace-safe, no font concerns |

## Config Schema

```toml
[auto_router]
enabled = true
url = "http://localhost:8080/v1/chat/completions"
model = "Arch-Router-1.5B-4bit"
timeout_ms = 2000

[provider.anthropic]
url = "https://api.anthropic.com"

[provider.ollama]
url = "http://localhost:11434"
strip_auth = true
api_key = "ollama"
stub_count_tokens = true

[[routes]]
name = "complex_reasoning"
description = "complex reasoning, code generation, and difficult analytical tasks"
pattern = "opus"
provider = "anthropic"

[[routes]]
name = "routine_tasks"
description = "simple questions, summaries, and routine conversational tasks"
pattern = "sonnet|haiku"
provider = "ollama"
model = "qwen3-coder:30b"

[default]
provider = "anthropic"
```

### New structs

```rust
// config.rs
pub struct AutoRouterConfig {
    pub enabled: bool,          // default false
    pub url: String,            // required when enabled
    pub model: String,          // required when enabled
    pub timeout_ms: u64,        // default 2000
}

// RouteConfig changes
pub struct RouteConfig {
    pub name: Option<String>,        // required with description
    pub description: Option<String>, // for auto-routing
    pub pattern: Option<String>,     // for regex routing
    pub provider: String,
    pub model: Option<String>,
}
```

### Validation (at Router::from_config)

- `description` without `name`: error
- `auto_router.enabled` with no description routes: warn
- `auto_router.enabled` with invalid/empty `url`: error
- Duplicate route `name` values: error
- Route with neither `pattern` nor `description`: error

## Architecture

### Execution flow

```
handle_request (proxy.rs)
  |
  v
Parse body -> extract model + messages
  |
  v
router.resolve(model, messages, client).await
  |
  v
model == "auto" AND auto_router.enabled AND has description routes?
  |                                    |
  YES                                  NO
  |                                    |
  v                                    v
auto_router::classify()          Try pattern routes in order
  |                                    |
  v                                    v
Some(name) -> lookup route       Match? -> return matched route
None -> fall to default          No match -> return default
  |
  v
Forward request to resolved provider (unchanged pipeline)
```

### New module: auto_router.rs

Responsible for:
1. Constructing the Arch-Router prompt (XML format with routes + conversation)
2. POST to the MLX endpoint via `/v1/chat/completions`
3. Parsing the `{"route": "name"}` response (JSON parse -> regex fallback)
4. Returning `Option<String>` (route name or None)

```rust
pub async fn classify(
    client: &reqwest::Client,
    config: &AutoRouterConfig,
    routes: &[RouteCandidate],     // name + description pairs
    messages: &[serde_json::Value],
) -> Option<String>
```

The prompt follows the documented Arch-Router format:
- System instruction with `<routes>` (JSON array of name/description objects)
- `<conversation>` block (messages array, system messages filtered out)
- Format instruction requesting `{"route": "route_name"}` response

### Router changes (router.rs)

- `resolve()` becomes `async`
- New parameters: `messages` and `client`
- New internal storage: `Vec<AutoRouteCandidate>` built at startup from description routes
- Sentinel check for `"auto"` happens before pattern matching

### Proxy changes (proxy.rs)

- Extract `messages` array from parsed body JSON
- Pass to `router.resolve().await`
- No other changes; downstream pipeline is identical

### Metrics changes

Replace `routed: bool` on `RequestRecord` with:

```rust
pub enum RoutingMethod {
    Pattern,
    Auto,
    Default,
}
```

TUI displays as colored labels:
- `PTN` (cyan) -- pattern match
- `AUT` (yellow) -- auto-routed via Arch-Router
- `DEF` (dark gray) -- default fallback

### Log compatibility (attach.rs)

Parse both old `"routed": true/false` and new `"routing_method": "..."` formats from log files.

## Deliverables

| File | Change |
|---|---|
| `src/config.rs` | Add `AutoRouterConfig`, modify `RouteConfig`, add validation |
| `src/auto_router.rs` | New module: prompt construction, HTTP call, response parsing |
| `src/router.rs` | Async `resolve()`, auto-route candidates, sentinel check |
| `src/proxy.rs` | Extract messages, pass to async resolve |
| `src/metrics.rs` | `RoutingMethod` enum replaces `routed: bool` |
| `src/tui/views/overview.rs` | Display routing method labels |
| `src/tui/views/models.rs` | Display routing method labels |
| `src/attach.rs` | Backwards-compatible log parsing |
| `src/metrics_log.rs` | Serialize new enum |
| `src/lib.rs` | Declare `auto_router` module |
| `docs/router.md` | Full routing documentation |
