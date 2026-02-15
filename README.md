<div align="center">

# Croxy

[![CI](https://github.com/panbanda/croxy/actions/workflows/ci.yml/badge.svg)](https://github.com/panbanda/croxy/actions/workflows/ci.yml)
[![Release](https://img.shields.io/github/v/release/panbanda/croxy)](https://github.com/panbanda/croxy/releases)
[![License: MIT](https://img.shields.io/badge/License-MIT-yellow.svg)](https://opensource.org/licenses/MIT)

**See what your AI tools are actually doing.**

Croxy sits between your tools and the Anthropic API, giving you real-time visibility into every request: token usage per request with percentiles, response time percentiles, error breakdowns by status code, and per-provider metrics. Optionally route requests to other providers or local models based on model name.

One line in your shell profile (`eval "$(croxy shellenv)"`) and Claude Code, Cursor, or any Anthropic-compatible client automatically routes through croxy. No SDK changes, no code changes, no per-project config.

</div>

---

## Installation

```bash
cargo install --path .
```

Download pre-built binaries from the [releases page](https://github.com/panbanda/croxy/releases).

## Quick Start

```sh
croxy init                          # create config at ~/.config/croxy/config.toml
# edit ~/.config/croxy/config.toml
croxy start                         # start in background
eval "$(croxy shellenv)"            # set ANTHROPIC_BASE_URL
```

Add to your shell profile for automatic setup:

```sh
eval "$(croxy shellenv)"
```

## Usage

```
croxy [OPTIONS] [COMMAND]

Commands:
  start      Start proxy in background
  stop       Stop a detached instance
  init       Create default config file
  shellenv   Print export ANTHROPIC_BASE_URL=... if running
  config     Read or modify config values

Options:
  -c, --config <FILE>  Config file [default: ~/.config/croxy/config.toml]
  -v, --verbose        Enable debug logging
```

### Foreground

```sh
croxy                               # default config
croxy -c ./config.toml              # custom config
croxy -v                            # debug logging
```

### Detached

```sh
croxy start                         # start in background
croxy stop                          # stop background instance
```

Log output goes to `~/.config/croxy/croxy.log` (truncated on each start). PID is stored in `~/.config/croxy/croxy.pid`.

### Shell Integration

`croxy shellenv` prints `export ANTHROPIC_BASE_URL=http://<host>:<port>` if the proxy is listening, and nothing if it isn't. This makes it safe to put in your `.zshrc`:

```sh
eval "$(croxy shellenv)"
```

If croxy is running, tools like Claude Code will automatically route through it. If it isn't, the variable is simply not set.

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

## Configuration Reference

### Providers

| Field | Description |
|-------|-------------|
| `url` | Provider base URL |
| `strip_auth` | Remove Authorization and x-api-key headers before forwarding |
| `api_key` | Set x-api-key header for this provider |
| `stub_count_tokens` | Return `{"input_tokens": 0}` for `/count_tokens` requests |

### Routes

Routes are matched in order against the `model` field in the JSON request body. Patterns are regular expressions.

| Field | Description |
|-------|-------------|
| `pattern` | Regex matched against the model name |
| `provider` | Provider to route to |
| `model` | Rewrite the model name before forwarding |

Unmatched requests go to `[default].provider`.

### Environment Override

Config values can be overridden with `CROXY_` prefixed environment variables (e.g. `CROXY_SERVER_PORT=8080`).

## Files

All state lives under `~/.config/croxy/`:

| File | Purpose |
|------|---------|
| `config.toml` | Configuration |
| `croxy.pid` | PID of detached process |
| `croxy.log` | stdout/stderr of detached process |

## Contributing

1. Fork the repository
2. Create your feature branch (`git checkout -b feature/amazing-feature`)
3. Commit your changes (`git commit -am 'Add amazing feature'`)
4. Push to the branch (`git push origin feature/amazing-feature`)
5. Create a Pull Request

## License

MIT - see [LICENSE](LICENSE) for details.
