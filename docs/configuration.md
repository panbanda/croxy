# Configuration

## Provider Guides

Croxy works with any provider that speaks the Anthropic Messages API (`/v1/messages`). Requests from clients like Claude Code are forwarded as-is; croxy handles routing, model rewriting, and auth.

### Anthropic (passthrough)

No special configuration needed. Requests are forwarded directly with the client's API key.

```toml
[provider.anthropic]
url = "https://api.anthropic.com"
```

### Ollama

Ollama natively supports the Anthropic Messages API. Use `strip_auth` to remove the Anthropic API key (Ollama doesn't need it), and `stub_count_tokens` because Ollama doesn't implement the `/v1/messages/count_tokens` endpoint that Claude Code calls for token budgeting.

```toml
[provider.ollama]
url = "http://localhost:11434"
strip_auth = true
api_key = "ollama"
stub_count_tokens = true
```

Route specific models to Ollama with a model rewrite:

```toml
[[routes]]
pattern = "sonnet|haiku"
provider = "ollama"
model = "qwen3-coder:30b"
```

### MLX (vllm-mlx)

[vllm-mlx](https://github.com/vllm-mlx/vllm-mlx) runs models on Apple Silicon via MLX and exposes an Anthropic-compatible `/v1/messages` endpoint, including streaming and tool calling.

Install and start the server:

```sh
pip install vllm-mlx
vllm serve mlx-community/Qwen3-Coder-8B-4bit \
  --enable-auto-tool-choice \
  --tool-call-parser hermes
```

The `--enable-auto-tool-choice` and `--tool-call-parser` flags are required for tool calling (used by Claude Code for agentic tasks). The parser must match your model -- see the [vllm-mlx tool calling docs](https://github.com/vllm-mlx/vllm-mlx/blob/main/docs/guides/tool-calling.md) for supported parsers.

Configure croxy to route to it:

```toml
[provider.mlx]
url = "http://localhost:8000"
strip_auth = true
stub_count_tokens = true

[[routes]]
pattern = "sonnet|haiku"
provider = "mlx"
model = "mlx-community/Qwen3-Coder-8B-4bit"
```

### Mixing Providers

A typical setup routes expensive models to Anthropic and cheaper/faster models to a local provider:

```toml
[provider.anthropic]
url = "https://api.anthropic.com"

[provider.mlx]
url = "http://localhost:8000"
strip_auth = true
stub_count_tokens = true

[[routes]]
pattern = "opus"
provider = "anthropic"

[[routes]]
pattern = "sonnet|haiku"
provider = "mlx"
model = "mlx-community/Qwen3-Coder-8B-4bit"

[default]
provider = "anthropic"
```

## Reference

### Providers

| Field | Description |
|-------|-------------|
| `url` | Provider base URL |
| `strip_auth` | Remove Authorization and x-api-key headers before forwarding |
| `api_key` | Set x-api-key header for this provider |
| `stub_count_tokens` | Return `{"input_tokens": 0}` for `/count_tokens` requests |

### Routes

Routes are matched in order against the `model` field in the JSON request body.

| Field | Description |
|-------|-------------|
| `pattern` | Regex matched against the model name (pattern routing) |
| `name` | Unique name for auto-routing (required when `description` is set) |
| `description` | Natural-language description of what this route handles (enables auto-routing) |
| `provider` | Provider to route to |
| `model` | Rewrite the model name before forwarding |

A route may have `pattern`, `name`+`description`, or both. See [docs/router.md](router.md) for details on auto-routing.

Unmatched requests go to `[default].provider`.

### Auto Router

When enabled, requests with `model: "auto"` are classified against route descriptions using an LLM (e.g. Arch-Router).

| Field | Description | Default |
|-------|-------------|---------|
| `auto_router.enabled` | Enable AI-based auto-routing | `false` |
| `auto_router.url` | Classification endpoint (OpenAI-compatible `/v1/chat/completions`) | |
| `auto_router.model` | Model to use for classification | |
| `auto_router.timeout_ms` | Request timeout in milliseconds | `5000` |

### Retention

| Field | Description | Default |
|-------|-------------|---------|
| `retention.enabled` | Enable automatic eviction of old metrics | `true` |
| `retention.minutes` | How long to keep metrics in memory | `60` |

### Metrics Logging

| Field | Description | Default |
|-------|-------------|---------|
| `logging.metrics.enabled` | Write request metrics to disk | `false` |
| `logging.metrics.path` | Path to the JSONL log file | `~/.config/croxy/logs/metrics.jsonl` |
| `logging.metrics.max_size_mb` | Max size per log file before rotation | `50` |
| `logging.metrics.max_files` | Number of rotated files to keep | `5` |

### Server

| Field | Description | Default |
|-------|-------------|---------|
| `server.host` | Bind address | `127.0.0.1` |
| `server.port` | Bind port | `3100` |
| `server.max_body_size` | Max request body size in bytes | `10485760` (10 MiB) |

### Environment Override

Config values can be overridden with `CROXY_` prefixed environment variables (e.g. `CROXY_SERVER_PORT=8080`).

## Files

All state lives under `~/.config/croxy/`:

| File | Purpose |
|------|---------|
| `config.toml` | Configuration |
| `croxy.pid` | PID of detached process |
| `croxy.log` | stdout/stderr of detached process |
| `logs/metrics.jsonl` | Request metrics (when enabled) |
