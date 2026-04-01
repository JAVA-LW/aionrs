use async_trait::async_trait;
use reqwest::header::{HeaderMap, HeaderValue, AUTHORIZATION, CONTENT_TYPE};
use serde_json::{Value, json};
use tokio::sync::mpsc;

use crate::types::llm::{LlmEvent, LlmRequest};
use crate::types::message::{ContentBlock, Message, Role, StopReason, TokenUsage};
use crate::types::tool::ToolDef;

use super::{LlmProvider, ProviderError};

pub struct OpenAIProvider {
    client: reqwest::Client,
    api_key: String,
    base_url: String,
}

impl OpenAIProvider {
    pub fn new(api_key: &str, base_url: &str) -> Self {
        Self {
            client: reqwest::Client::new(),
            api_key: api_key.to_string(),
            base_url: base_url.to_string(),
        }
    }

    fn build_headers(&self) -> HeaderMap {
        let mut headers = HeaderMap::new();
        let bearer = format!("Bearer {}", self.api_key);
        headers.insert(AUTHORIZATION, HeaderValue::from_str(&bearer).unwrap());
        headers.insert(CONTENT_TYPE, HeaderValue::from_static("application/json"));
        headers
    }

    fn build_messages(messages: &[Message], system: &str) -> Vec<Value> {
        let mut result: Vec<Value> = Vec::new();

        // System message first
        if !system.is_empty() {
            result.push(json!({
                "role": "system",
                "content": system
            }));
        }

        for msg in messages {
            match msg.role {
                Role::User => {
                    // Check if this contains tool results
                    let has_tool_results = msg
                        .content
                        .iter()
                        .any(|b| matches!(b, ContentBlock::ToolResult { .. }));

                    if has_tool_results {
                        // Each tool result becomes a separate "tool" role message
                        for block in &msg.content {
                            if let ContentBlock::ToolResult {
                                tool_use_id,
                                content,
                                ..
                            } = block
                            {
                                result.push(json!({
                                    "role": "tool",
                                    "tool_call_id": tool_use_id,
                                    "content": content
                                }));
                            }
                        }
                    } else {
                        // Plain text user message
                        let text: String = msg
                            .content
                            .iter()
                            .filter_map(|b| {
                                if let ContentBlock::Text { text } = b {
                                    Some(text.as_str())
                                } else {
                                    None
                                }
                            })
                            .collect::<Vec<_>>()
                            .join("\n");
                        result.push(json!({
                            "role": "user",
                            "content": text
                        }));
                    }
                }
                Role::Assistant => {
                    let mut msg_json = json!({ "role": "assistant" });

                    // Extract text content
                    let text: String = msg
                        .content
                        .iter()
                        .filter_map(|b| {
                            if let ContentBlock::Text { text } = b {
                                Some(text.as_str())
                            } else {
                                None
                            }
                        })
                        .collect::<Vec<_>>()
                        .join("");

                    if !text.is_empty() {
                        msg_json["content"] = json!(text);
                    } else {
                        msg_json["content"] = Value::Null;
                    }

                    // Extract tool calls
                    let tool_calls: Vec<Value> = msg
                        .content
                        .iter()
                        .filter_map(|b| {
                            if let ContentBlock::ToolUse { id, name, input } = b {
                                Some(json!({
                                    "id": id,
                                    "type": "function",
                                    "function": {
                                        "name": name,
                                        "arguments": serde_json::to_string(input).unwrap_or_default()
                                    }
                                }))
                            } else {
                                None
                            }
                        })
                        .collect();

                    if !tool_calls.is_empty() {
                        msg_json["tool_calls"] = json!(tool_calls);
                    }

                    result.push(msg_json);
                }
                Role::System => {
                    // Already handled above
                }
                Role::Tool => {
                    // Shouldn't happen in our internal format, but handle gracefully
                    for block in &msg.content {
                        if let ContentBlock::ToolResult {
                            tool_use_id,
                            content,
                            ..
                        } = block
                        {
                            result.push(json!({
                                "role": "tool",
                                "tool_call_id": tool_use_id,
                                "content": content
                            }));
                        }
                    }
                }
            }
        }

        result
    }

    fn build_tools(tools: &[ToolDef]) -> Vec<Value> {
        tools
            .iter()
            .map(|t| {
                json!({
                    "type": "function",
                    "function": {
                        "name": t.name,
                        "description": t.description,
                        "parameters": t.input_schema
                    }
                })
            })
            .collect()
    }

    fn build_request_body(&self, request: &LlmRequest) -> Value {
        let mut body = json!({
            "model": request.model,
            "max_tokens": request.max_tokens,
            "messages": Self::build_messages(&request.messages, &request.system),
            "stream": true,
            "stream_options": { "include_usage": true }
        });

        if !request.tools.is_empty() {
            body["tools"] = json!(Self::build_tools(&request.tools));
        }

        body
    }
}

/// State for accumulating tool call deltas by index
struct ToolCallAccumulator {
    id: String,
    name: String,
    arguments: String,
}

struct StreamState {
    tool_calls: Vec<ToolCallAccumulator>,
    input_tokens: u64,
    output_tokens: u64,
}

impl StreamState {
    fn new() -> Self {
        Self {
            tool_calls: Vec::new(),
            input_tokens: 0,
            output_tokens: 0,
        }
    }

    fn get_or_create_tool(&mut self, index: usize) -> &mut ToolCallAccumulator {
        while self.tool_calls.len() <= index {
            self.tool_calls.push(ToolCallAccumulator {
                id: String::new(),
                name: String::new(),
                arguments: String::new(),
            });
        }
        &mut self.tool_calls[index]
    }
}

#[async_trait]
impl LlmProvider for OpenAIProvider {
    async fn stream(
        &self,
        request: &LlmRequest,
    ) -> Result<mpsc::Receiver<LlmEvent>, ProviderError> {
        let url = format!("{}/v1/chat/completions", self.base_url);
        let body = self.build_request_body(request);

        let response = self
            .client
            .post(&url)
            .headers(self.build_headers())
            .json(&body)
            .send()
            .await?;

        let status = response.status();
        if !status.is_success() {
            let body_text = response.text().await.unwrap_or_default();
            if status.as_u16() == 429 {
                return Err(ProviderError::RateLimited {
                    retry_after_ms: 5000,
                });
            }
            return Err(ProviderError::Api {
                status: status.as_u16(),
                message: body_text,
            });
        }

        let (tx, rx) = mpsc::channel(64);

        tokio::spawn(async move {
            if let Err(e) = process_sse_stream(response, &tx).await {
                let _ = tx.send(LlmEvent::Error(e.to_string())).await;
            }
        });

        Ok(rx)
    }
}

async fn process_sse_stream(
    response: reqwest::Response,
    tx: &mpsc::Sender<LlmEvent>,
) -> Result<(), ProviderError> {
    use futures::StreamExt;

    let mut state = StreamState::new();
    let mut buffer = String::new();
    let mut stream = response.bytes_stream();

    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(|e| ProviderError::Connection(e.to_string()))?;
        let text = String::from_utf8_lossy(&chunk);
        buffer.push_str(&text);

        // Process complete lines
        while let Some(line_end) = buffer.find('\n') {
            let line = buffer[..line_end].trim().to_string();
            buffer = buffer[line_end + 1..].to_string();

            if line.is_empty() || line.starts_with(':') {
                continue;
            }

            if let Some(data) = line.strip_prefix("data: ") {
                if data == "[DONE]" {
                    return Ok(());
                }

                let events = parse_sse_chunk(data, &mut state);
                for event in events {
                    if tx.send(event).await.is_err() {
                        return Ok(());
                    }
                }
            }
        }
    }

    Ok(())
}

fn parse_sse_chunk(data: &str, state: &mut StreamState) -> Vec<LlmEvent> {
    let mut events = Vec::new();

    let json: Value = match serde_json::from_str(data) {
        Ok(v) => v,
        Err(_) => return events,
    };

    // Extract usage if present
    if let Some(usage) = json.get("usage") {
        state.input_tokens = usage["prompt_tokens"].as_u64().unwrap_or(state.input_tokens);
        state.output_tokens = usage["completion_tokens"]
            .as_u64()
            .unwrap_or(state.output_tokens);
    }

    let Some(choice) = json["choices"].as_array().and_then(|c| c.first()) else {
        return events;
    };

    let delta = &choice["delta"];

    // Text content
    if let Some(content) = delta["content"].as_str() {
        if !content.is_empty() {
            events.push(LlmEvent::TextDelta(content.to_string()));
        }
    }

    // Tool calls
    if let Some(tool_calls) = delta["tool_calls"].as_array() {
        for tc in tool_calls {
            let index = tc["index"].as_u64().unwrap_or(0) as usize;
            let acc = state.get_or_create_tool(index);

            if let Some(id) = tc["id"].as_str() {
                acc.id = id.to_string();
            }
            if let Some(name) = tc["function"]["name"].as_str() {
                acc.name = name.to_string();
            }
            if let Some(args) = tc["function"]["arguments"].as_str() {
                acc.arguments.push_str(args);
            }
        }
    }

    // Check finish_reason
    if let Some(finish_reason) = choice["finish_reason"].as_str() {
        match finish_reason {
            "tool_calls" => {
                // Emit accumulated tool calls
                for tc in state.tool_calls.drain(..) {
                    let input: Value = serde_json::from_str(&tc.arguments)
                        .unwrap_or(Value::Object(serde_json::Map::new()));
                    events.push(LlmEvent::ToolUse {
                        id: tc.id,
                        name: tc.name,
                        input,
                    });
                }
                events.push(LlmEvent::Done {
                    stop_reason: StopReason::ToolUse,
                    usage: TokenUsage {
                        input_tokens: state.input_tokens,
                        output_tokens: state.output_tokens,
                        cache_creation_tokens: 0,
                        cache_read_tokens: 0,
                    },
                });
            }
            "stop" => {
                events.push(LlmEvent::Done {
                    stop_reason: StopReason::EndTurn,
                    usage: TokenUsage {
                        input_tokens: state.input_tokens,
                        output_tokens: state.output_tokens,
                        cache_creation_tokens: 0,
                        cache_read_tokens: 0,
                    },
                });
            }
            "length" => {
                events.push(LlmEvent::Done {
                    stop_reason: StopReason::MaxTokens,
                    usage: TokenUsage {
                        input_tokens: state.input_tokens,
                        output_tokens: state.output_tokens,
                        cache_creation_tokens: 0,
                        cache_read_tokens: 0,
                    },
                });
            }
            _ => {}
        }
    }

    events
}
