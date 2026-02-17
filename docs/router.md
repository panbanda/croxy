# Routing

Croxy supports three routing methods: **pattern**, **auto**, and **default**.

## Pattern Routing

Pattern routing matches the `model` field in the request body against regex patterns defined in `[[routes]]`. Routes are evaluated in order; the first match wins.

```toml
[[routes]]
pattern = "opus"
provider = "anthropic"

[[routes]]
pattern = "sonnet|haiku"
provider = "ollama"
model = "qwen3-coder:30b"
```

When a client sends `model: "claude-sonnet-4-5-20250929"`, the second route matches and the request is forwarded to Ollama with the model rewritten to `qwen3-coder:30b`.

## Auto Routing

Auto routing uses an LLM to classify requests by their conversation content rather than model name. When a client sends `model: "auto"`, croxy sends the conversation to a classification endpoint and routes based on which description best matches.

### Setup

1. Serve a routing model. Croxy is designed for [Arch-Router](https://huggingface.co/katanemo/Arch-Router-1.5B) but works with any model that returns `{"route": "<name>"}` from an OpenAI-compatible chat completions endpoint.

   ```sh
   # Example: serve with mlx_lm
   mlx_lm.server --model mlx-community/Arch-Router-1.5B-4bit --port 8080
   ```

2. Enable auto-routing and point to the endpoint:

   ```toml
   [auto_router]
   enabled = true
   url = "http://localhost:8080/v1/chat/completions"
   model = "mlx-community/Arch-Router-1.5B-4bit"
   timeout_ms = 5000
   ```

3. Add `name` and `description` to routes that should participate in auto-routing:

   ```toml
   [[routes]]
   name = "coding"
   description = "Code generation, debugging, refactoring, and programming tasks"
   provider = "anthropic"
   model = "claude-sonnet-4-5-20250929"

   [[routes]]
   name = "analysis"
   description = "Data analysis, research, and complex reasoning tasks"
   provider = "anthropic"
   model = "claude-opus-4-6"
   ```

### How It Works

When `model: "auto"` is received:

1. Croxy builds a classification prompt from the route descriptions and the conversation history (excluding system messages).
2. The prompt is sent to the `auto_router.url` endpoint.
3. The response is parsed for a route name. Croxy uses layered parsing: full JSON first, then regex extraction as fallback.
4. If the returned name matches a route, that route is used.
5. If classification fails or returns `"other"`, the default provider handles the request.

### Route Descriptions

Write descriptions that are noun-centric and clearly distinguish each route's purpose:

```toml
# Good: specific, distinguishable
name = "coding"
description = "Code generation, debugging, refactoring, and programming tasks"

name = "writing"
description = "Creative writing, copywriting, and content drafting"

# Bad: overlapping, vague
name = "general"
description = "General tasks and questions"
```

### Combining Pattern and Auto Routing

A single route can have both `pattern` and `name`+`description`:

```toml
[[routes]]
name = "coding"
description = "Code generation and programming tasks"
pattern = "sonnet"
provider = "ollama"
model = "qwen3-coder:30b"
```

This route matches `sonnet` requests via pattern and participates in auto-routing classification. The pattern match is checked first for non-`"auto"` models.

## Default Routing

Requests that match no pattern and cannot be auto-routed fall through to `[default].provider`.

## TUI Indicators

The TUI shows routing method in the live log and models table:

| Label | Meaning |
|-------|---------|
| `PTN` | Matched by regex pattern |
| `AUT` | Classified by auto-router |
| `DEF` | Fell through to default provider |
