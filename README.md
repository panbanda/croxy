<div align="center">

# Croxy

[![CI](https://github.com/panbanda/croxy/actions/workflows/ci.yml/badge.svg)](https://github.com/panbanda/croxy/actions/workflows/ci.yml)
[![Release](https://img.shields.io/github/v/release/panbanda/croxy)](https://github.com/panbanda/croxy/releases)
[![License: MIT](https://img.shields.io/badge/License-MIT-yellow.svg)](https://opensource.org/licenses/MIT)

**Take control of your AI traffic.**

A lightweight local proxy that sits between your AI tools and the Anthropic API.
See every request in real time. Route models to any provider. One line to set up,
zero code changes.

![Croxy demo](docs/croxy-optimized.gif)

</div>

---

## Install

```bash
brew install panbanda/croxy/croxy
```

Pre-built binaries are also available on the [releases page](https://github.com/panbanda/croxy/releases).

## Quick Start

```sh
croxy init                          # create ~/.config/croxy/config.toml
croxy start                         # start in background
eval "$(croxy shellenv)"            # point AI tools at croxy
```

Add to your shell profile for automatic setup:

```sh
eval "$(croxy shellenv)"
```

That's it. Claude Code, Cursor, and any Anthropic-compatible client will now route through croxy automatically.

## What You Get

- **Live dashboard** -- requests per minute, token throughput, response time percentiles (p50/p95/p99), per-model breakdowns, status code distribution, and error tracking, all updating in real time
- **Model routing** -- regex-based rules send requests to different providers (Anthropic, Ollama, vllm-mlx, anything Anthropic-compatible) based on model name
- **Zero integration** -- one `eval` in your shell profile, no SDK changes, no per-project config
- **Foreground or background** -- run with a TUI dashboard, detach to background, or reattach to a running instance

## Configuration

`croxy init` creates a starter config at `~/.config/croxy/config.toml` with Anthropic and Ollama pre-configured. Edit it to add providers and routing rules.

See the [configuration guide](docs/configuration.md) for the full reference, including provider setup for Ollama, vllm-mlx, and mixed-provider routing.

## CLI

```
croxy                  Run in foreground with TUI dashboard
croxy start            Start in background
croxy stop             Stop background instance
croxy init             Create default config file
croxy shellenv         Print ANTHROPIC_BASE_URL export if running
croxy config get|set   Read or modify config values
```

## License

MIT - see [LICENSE](LICENSE) for details.
