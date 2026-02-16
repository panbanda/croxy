# Changelog

## [1.1.0](https://github.com/panbanda/croxy/compare/croxy-v1.0.2...croxy-v1.1.0) (2026-02-16)


### Features

* **tui:** enhance dashboard with scrollbars, time ago, and duration colors ([7d520fc](https://github.com/panbanda/croxy/commit/7d520fc6ecfbdeb6ba8bcc686990d8f0ecc7045f))
* **tui:** enhance dashboard with scrollbars, time ago, and duration colors ([43d99f9](https://github.com/panbanda/croxy/commit/43d99f939d659e71f9e61a5da45a165238a3c411))

## [1.0.2](https://github.com/panbanda/croxy/compare/croxy-v1.0.1...croxy-v1.0.2) (2026-02-15)


### Bug Fixes

* align cli description with package metadata ([2f0fcb7](https://github.com/panbanda/croxy/commit/2f0fcb7ef1eae41d6ce65e336c4bdb11eb046e06))

## [1.0.1](https://github.com/panbanda/croxy/compare/croxy-v1.0.0...croxy-v1.0.1) (2026-02-15)


### Bug Fixes

* align cli description with homebrew formula and fix tap push ([60b7140](https://github.com/panbanda/croxy/commit/60b7140b3ae193e1cafc39f0f3b741cce9142736))

## [1.0.0](https://github.com/panbanda/croxy/compare/croxy-v0.2.0...croxy-v1.0.0) (2026-02-15)


### ⚠ BREAKING CHANGES

* Config keys `[backends.X]` and `backend = "X"` are now `[provider.X]` and `provider = "X"`. The metrics JSON log key changes from `"backend"` to `"provider"`. Users must update their config.toml.

### Bug Fixes

* add serde alias for legacy "backend" key in metrics logs ([17b73dd](https://github.com/panbanda/croxy/commit/17b73ddc06c1b5c333b007d399c9ff8f8e210a5e))


### Reverts

* remove backward compat alias for legacy "backend" log key ([e59cae3](https://github.com/panbanda/croxy/commit/e59cae38d88acca3d1ac3a2a1bc78dcba6435ae3))


### Code Refactoring

* rename backend to provider throughout codebase ([3c08adf](https://github.com/panbanda/croxy/commit/3c08adf6306ee4dc771ee6a342dca460d3f50dc4))

## [0.2.0](https://github.com/panbanda/croxy/compare/croxy-v0.1.0...croxy-v0.2.0) (2026-02-15)


### Features

* add model-routing proxy with real-time metrics TUI ([1617cef](https://github.com/panbanda/croxy/commit/1617cefc25a1cad65f9c07cda6cf2653ebd285cb))
* real-time LLM metrics proxy with per-model token and latency insights ([6677c36](https://github.com/panbanda/croxy/commit/6677c3640825100b3d74b7849bf34b3bead558ba))


### Bug Fixes

* address code review feedback from CodeRabbit ([b248cff](https://github.com/panbanda/croxy/commit/b248cffa077bcd85b1cc9aab20e14a7e25f3f295))
* address third-pass review findings and user-reported issues ([71a80f5](https://github.com/panbanda/croxy/commit/71a80f51394554ccc5d3d3cb1298016beaad5a81))
* deadlock risk, idempotent homebrew push, minor hardening ([b2e1dc3](https://github.com/panbanda/croxy/commit/b2e1dc32f9000f39c0321eb40671b127f9925427))
* fix YAML heredoc indentation and remove expect() on header value ([c7c82f1](https://github.com/panbanda/croxy/commit/c7c82f125d59b552f608ed45c4e8914173f70c54))
