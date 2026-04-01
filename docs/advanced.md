# Advanced Features

## Sub-Agent Spawning

The LLM can use the Spawn tool to create independent sub-agents that run tasks in parallel. Each sub-agent has its own conversation context and full tool set, but shares the parent agent's LLM provider (connection pool reuse).

### Use Cases

- "Search these 3 files simultaneously and summarize each"
- "Run tests and lint in parallel"
- "Search for X in the codebase while reading Y"

### Limits

| Setting | Default | Description |
|---------|---------|-------------|
| Max parallel sub-agents | 5 | Prevents resource exhaustion |
| Sub-agent max turns | 10 | Per sub-agent conversation turn limit |
| Sub-agent max tokens | 4096 | Per sub-agent response token limit |

### Behavior

- Sub-agents auto-approve all tool calls (no confirmation prompts)
- Sub-agents do not save sessions
- Sub-agents run silently (no stdout output)
- All results are merged and returned to the parent agent

---

## Hook System

Event-driven hooks execute shell commands at specific points in the tool lifecycle, enabling auto-formatting, linting, auditing, and more.

### Hook Types

| Type | Trigger | Behavior |
|------|---------|----------|
| `pre_tool_use` | Before tool execution | Non-zero exit blocks the tool |
| `post_tool_use` | After tool execution | Non-blocking; errors are logged |
| `stop` | When agent session ends | Non-blocking |

### Configuration

```toml
# Auto-format Rust files after modification
[[hooks.post_tool_use]]
name = "rustfmt"
tool_match = ["Write", "Edit"]
file_match = ["*.rs"]
command = "rustfmt ${TOOL_INPUT_FILE_PATH}"

# Auto-format TypeScript files after modification
[[hooks.post_tool_use]]
name = "prettier"
tool_match = ["Write", "Edit"]
file_match = ["*.ts", "*.tsx"]
command = "npx prettier --write ${TOOL_INPUT_FILE_PATH}"

# Audit Bash commands
[[hooks.post_tool_use]]
name = "audit-log"
tool_match = ["Bash"]
command = "echo \"$(date): ${TOOL_INPUT_COMMAND}\" >> .aionrs/audit.log"

# Run lint on session end
[[hooks.stop]]
name = "final-lint"
command = "cargo clippy --quiet 2>&1 | tail -5"
```

### Environment Variables

Hook commands can reference these variables via `${VAR}` syntax:

| Variable | Description |
|----------|-------------|
| `TOOL_NAME` | Tool name |
| `TOOL_INPUT` | Full tool input JSON |
| `TOOL_INPUT_FILE_PATH` | File path (if the tool has a file_path parameter) |
| `TOOL_INPUT_COMMAND` | Command (if the tool has a command parameter) |
| `TOOL_INPUT_PATTERN` | Search pattern (if the tool has a pattern parameter) |
| `TOOL_OUTPUT` | Tool output (post_tool_use only) |

### Matching Rules

- `tool_match`: glob patterns matching tool names; empty = match all
- `file_match`: glob patterns matching file paths; empty = match all
- Default timeout: 30 seconds, configurable via `timeout_ms`

---

## Prompt Caching (Anthropic)

Prompt caching stores system prompts and tool definitions on Anthropic's servers, so subsequent requests only process the changed parts.

- **First request**: full input token cost + 25% write premium
- **Subsequent requests**: cached portion costs only 10%
- **Cache TTL**: 5 minutes (auto-renewed on each hit)

### Configuration

```toml
[providers.anthropic]
api_key = "sk-ant-xxx"
prompt_caching = true   # default true (Anthropic only)
```

### Token Stats

With caching enabled, stats show cache data:

```
[turns: 3 | tokens: 100 in (5000 cached) / 200 out | cache: 5000 created, 5000 read]
```

---

## VCR Recording & Replay

Record real API interactions and replay them in tests — no API key or network needed.

### Usage

```bash
# Record mode
VCR_MODE=record VCR_CASSETTE=tests/cassettes/my_test.json \
  aionrs -k sk-ant-xxx "Read Cargo.toml"

# Replay mode (in tests)
VCR_MODE=replay VCR_CASSETTE=tests/cassettes/my_test.json \
  aionrs "Read Cargo.toml"
```

### Features

- Auto-sanitization: sensitive headers (api-key, auth, token) are replaced with `[REDACTED]` during recording
- JSON-formatted cassette files, editable by hand
- Supports recording/replay of SSE streaming responses

---

## CLAUDE.md Auto-Loading

If a `CLAUDE.md` file exists in the current working directory, its contents are automatically injected into the system prompt. Use this for:

- Project-specific coding standards
- Architecture descriptions
- Special working constraints
