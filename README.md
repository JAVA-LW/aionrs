# aionrs

A Rust-based LLM tool-use agent for the command line. It connects to LLM APIs, autonomously invokes local tools (file I/O, shell, search, etc.), and completes tasks end-to-end.

## Features

- **Multi-provider** — Anthropic, OpenAI (and compatibles like DeepSeek/Ollama), AWS Bedrock, Google Vertex AI
- **7 built-in tools** — Read, Write, Edit, Bash, Grep, Glob, Spawn (sub-agents)
- **MCP client** — Connect to any [Model Context Protocol](https://modelcontextprotocol.io/) server (stdio / SSE / streamable-http)
- **Hook system** — Event-driven automation on tool lifecycle (auto-format, lint, audit)
- **Sub-agent spawning** — Parallel task execution via the Spawn tool
- **Session persistence** — Save and resume conversation history
- **Prompt caching** — Anthropic cache_control for up to 90% cost reduction
- **Profile inheritance** — Named profiles with `extends` for quick provider/model switching
- **OAuth login** — Use Claude.ai subscription directly, no API key needed
- **CLAUDE.md injection** — Auto-load project-specific system prompts

## Quick Start

```bash
# Build from source
cargo build --release

# Generate default config, then add your API key
./target/release/aionrs --init-config
# Edit ~/.config/aionrs/config.toml

# Single-shot mode
aionrs "Read Cargo.toml and explain the dependencies"

# Interactive REPL
aionrs

# Full CLI reference
aionrs --help
```

## Architecture

```
┌──────────────────────────────────────────────────────────────┐
│                      main.rs (CLI / REPL)                    │
├──────────────────────────────────────────────────────────────┤
│  Config          │  Engine (agent loop)  │  Session Manager  │
│  (3-level merge) │  streaming + tools    │  save / resume    │
├──────────────────┼───────────────────────┼───────────────────┤
│  Providers       │  Tool Registry        │  Hook Executor    │
│  ├ Anthropic     │  ├ Built-in (7)       │  ├ pre_tool_use   │
│  ├ OpenAI        │  └ MCP tools (N)      │  ├ post_tool_use  │
│  ├ Bedrock       │                       │  └ stop           │
│  └ Vertex AI     │  MCP Client           │                   │
│                  │  ├ Stdio transport    │  Sub-Agent        │
│                  │  ├ SSE transport      │  Spawner          │
│                  │  └ HTTP transport     │                   │
└──────────────────┴───────────────────────┴───────────────────┘
```

## Documentation

| Document | Description |
|----------|-------------|
| [Getting Started](docs/getting-started.md) | Installation, CLI reference, configuration, usage examples |
| [Built-in Tools](docs/tools.md) | Detailed reference for all 7 tools |
| [MCP Integration](docs/mcp.md) | Model Context Protocol client setup and usage |
| [Providers & Auth](docs/providers.md) | Multi-provider config, profiles, Bedrock, Vertex, OAuth |
| [Advanced Features](docs/advanced.md) | Sub-agents, hooks, prompt caching, VCR, CLAUDE.md |
| [Troubleshooting](docs/troubleshooting.md) | Common errors and solutions |
| [JSON Stream Protocol](docs/json-stream-protocol.md) | Host integration protocol (`--json-stream` mode) |

## Supported Providers

| Provider | Auth | Notes |
|----------|------|-------|
| Anthropic | API Key / OAuth | Prompt caching, streaming, vision |
| OpenAI | API Key | Compatible with DeepSeek, Qwen, Ollama, vLLM |
| AWS Bedrock | SigV4 | Regional endpoints, AWS credential chain |
| Google Vertex AI | GCP OAuth2 / Service Account | Metadata server auto-detection |

## License

MIT
