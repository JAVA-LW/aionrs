use aion_config::auth::AuthConfig;
use aion_config::compat::ProviderCompat;
use aion_config::debug::DebugConfig;
use aion_providers::LlmProvider;
use aion_providers::copilot::CopilotProvider;
use aion_types::llm::{LlmEvent, LlmRequest};
use aion_types::message::{ContentBlock, Message, Role, StopReason};
use serde_json::json;
use wiremock::matchers::{header, method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

fn make_request(model: &str) -> LlmRequest {
    LlmRequest {
        session_id: None,
        model: model.to_string(),
        system: "You are a test assistant.".to_string(),
        messages: vec![Message::new(
            Role::User,
            vec![ContentBlock::Text {
                text: "Hello".to_string(),
            }],
        )],
        tools: vec![],
        max_tokens: 512,
        thinking: None,
        reasoning_effort: None,
    }
}

async fn collect_events(mut rx: tokio::sync::mpsc::Receiver<LlmEvent>) -> Vec<LlmEvent> {
    let mut events = Vec::new();
    while let Some(event) = rx.recv().await {
        events.push(event);
    }
    events
}

fn build_openai_sse_body(data_lines: &[&str]) -> String {
    let mut body = String::new();
    for line in data_lines {
        body.push_str("data: ");
        body.push_str(line);
        body.push_str("\n\n");
    }
    body.push_str("data: [DONE]\n\n");
    body
}

fn build_anthropic_sse_body(text: &str) -> String {
    format!(
        "event: message_start\n\
         data: {{\"type\":\"message_start\",\"message\":{{\"id\":\"msg_test\",\"type\":\"message\",\"role\":\"assistant\",\"content\":[],\"model\":\"claude-3.7-sonnet\",\"stop_reason\":null,\"stop_sequence\":null,\"usage\":{{\"input_tokens\":42,\"output_tokens\":1}}}}}}\n\n\
         event: content_block_start\n\
         data: {{\"type\":\"content_block_start\",\"index\":0,\"content_block\":{{\"type\":\"text\",\"text\":\"\"}}}}\n\n\
         event: content_block_delta\n\
         data: {{\"type\":\"content_block_delta\",\"index\":0,\"delta\":{{\"type\":\"text_delta\",\"text\":\"{text}\"}}}}\n\n\
         event: content_block_stop\n\
         data: {{\"type\":\"content_block_stop\",\"index\":0}}\n\n\
         event: message_delta\n\
         data: {{\"type\":\"message_delta\",\"delta\":{{\"stop_reason\":\"end_turn\",\"stop_sequence\":null}},\"usage\":{{\"output_tokens\":11}}}}\n\n\
         event: message_stop\n\
         data: {{\"type\":\"message_stop\"}}\n\n"
    )
}

fn test_auth(server: &MockServer) -> AuthConfig {
    let mut auth = AuthConfig::for_provider("copilot").unwrap();
    auth.api_base_url = Some(server.uri());
    auth
}

#[tokio::test]
async fn test_copilot_routes_openai_models_to_chat_completions() {
    let server = MockServer::start().await;

    let models_body = json!({
        "data": [{
            "model_picker_enabled": true,
            "id": "gpt-4o",
            "supported_endpoints": ["/chat/completions"]
        }]
    });

    let chunk1 = json!({
        "id": "chatcmpl-001",
        "object": "chat.completion.chunk",
        "choices": [{
            "index": 0,
            "delta": { "role": "assistant", "content": "Hello from Copilot" },
            "finish_reason": null
        }]
    })
    .to_string();
    let chunk2 = json!({
        "id": "chatcmpl-001",
        "object": "chat.completion.chunk",
        "choices": [{
            "index": 0,
            "delta": {},
            "finish_reason": "stop"
        }],
        "usage": {
            "prompt_tokens": 21,
            "completion_tokens": 7
        }
    })
    .to_string();

    Mock::given(method("GET"))
        .and(path("/models"))
        .and(header("authorization", "Bearer test-key"))
        .respond_with(ResponseTemplate::new(200).set_body_json(models_body))
        .mount(&server)
        .await;

    Mock::given(method("POST"))
        .and(path("/chat/completions"))
        .and(header("authorization", "Bearer test-key"))
        .and(header("openai-intent", "conversation-edits"))
        .respond_with(ResponseTemplate::new(200).set_body_raw(
            build_openai_sse_body(&[&chunk1, &chunk2]),
            "text/event-stream",
        ))
        .mount(&server)
        .await;

    let provider = CopilotProvider::new(
        "copilot",
        "test-key",
        &server.uri(),
        ProviderCompat::copilot_defaults(),
        DebugConfig::default(),
        Some(test_auth(&server)),
    );

    let rx = provider.stream(&make_request("gpt-4o")).await.unwrap();
    let events = collect_events(rx).await;

    assert_eq!(events.len(), 2, "expected 2 events, got: {:?}", events);
    match &events[0] {
        LlmEvent::TextDelta(text) => assert_eq!(text, "Hello from Copilot"),
        other => panic!("expected TextDelta, got: {:?}", other),
    }
    match &events[1] {
        LlmEvent::Done { stop_reason, usage } => {
            assert_eq!(*stop_reason, StopReason::EndTurn);
            assert_eq!(usage.input_tokens, 21);
            assert_eq!(usage.output_tokens, 7);
        }
        other => panic!("expected Done, got: {:?}", other),
    }
}

#[tokio::test]
async fn test_copilot_routes_anthropic_models_to_messages_api() {
    let server = MockServer::start().await;

    let models_body = json!({
        "data": [{
            "model_picker_enabled": true,
            "id": "claude-3.7-sonnet",
            "supported_endpoints": ["/v1/messages"]
        }]
    });

    Mock::given(method("GET"))
        .and(path("/models"))
        .and(header("authorization", "Bearer test-key"))
        .respond_with(ResponseTemplate::new(200).set_body_json(models_body))
        .mount(&server)
        .await;

    Mock::given(method("POST"))
        .and(path("/v1/messages"))
        .and(header("authorization", "Bearer test-key"))
        .and(header("x-initiator", "agent"))
        .respond_with(ResponseTemplate::new(200).set_body_raw(
            build_anthropic_sse_body("Hello from Claude via Copilot"),
            "text/event-stream",
        ))
        .mount(&server)
        .await;

    let provider = CopilotProvider::new(
        "copilot",
        "test-key",
        &server.uri(),
        ProviderCompat::copilot_defaults(),
        DebugConfig::default(),
        Some(test_auth(&server)),
    );

    let rx = provider
        .stream(&make_request("claude-3.7-sonnet"))
        .await
        .unwrap();
    let events = collect_events(rx).await;

    assert_eq!(events.len(), 2, "expected 2 events, got: {:?}", events);
    match &events[0] {
        LlmEvent::TextDelta(text) => assert_eq!(text, "Hello from Claude via Copilot"),
        other => panic!("expected TextDelta, got: {:?}", other),
    }
    match &events[1] {
        LlmEvent::Done { stop_reason, usage } => {
            assert_eq!(*stop_reason, StopReason::EndTurn);
            assert_eq!(usage.input_tokens, 42);
            assert_eq!(usage.output_tokens, 11);
        }
        other => panic!("expected Done, got: {:?}", other),
    }
}
