# MCP (Model Context Protocol) Integration

## Overview

MCP allows the agent to connect to external tool servers, extending beyond the 7 built-in tools to the entire MCP server ecosystem.

## Configuring MCP Servers

Declare MCP servers in the config file:

```toml
# Stdio transport: launch a local subprocess
[mcp.servers.filesystem]
transport = "stdio"
command = "npx"
args = ["-y", "@modelcontextprotocol/server-filesystem", "/Users/me/project"]

[mcp.servers.github]
transport = "stdio"
command = "npx"
args = ["-y", "@modelcontextprotocol/server-github"]
env = { GITHUB_TOKEN = "ghp_xxx" }

# SSE transport: connect to a remote SSE server
[mcp.servers.database]
transport = "sse"
url = "http://localhost:3001/sse"

# Streamable HTTP transport: HTTP POST communication
[mcp.servers.remote-tools]
transport = "streamable-http"
url = "https://tools.example.com/mcp"
headers = { Authorization = "Bearer xxx" }
```

## Transport Types

| Transport | Description | Use Case |
|-----------|-------------|----------|
| `stdio` | Launch local subprocess, communicate via stdin/stdout | Local MCP servers (npx, uvx) |
| `sse` | GET for SSE event stream, POST for requests | Remote MCP servers |
| `streamable-http` | HTTP POST, supports SSE streaming responses | Remote MCP servers |

## Tool Naming

- MCP tool names are used directly when there's no conflict
- On conflict with built-in or other MCP tools, names are auto-prefixed: `mcp__{server}__{tool}`

## Startup Flow

1. Connect to all configured MCP servers
2. Perform MCP protocol handshake (`initialize`) for each server
3. Discover available tools (`tools/list`)
4. Register tools in the tool registry — the agent uses them like built-in tools
5. Gracefully close all connections on exit
