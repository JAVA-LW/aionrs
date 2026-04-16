use aion_protocol::events::{Capabilities, ProtocolEvent};
use aion_types::llm::{
    AccountCreditsInfo, AccountLimitInfo, AccountLimitWindow, AccountLimitsInfo, ProviderModelInfo,
};

fn base_capabilities() -> Capabilities {
    Capabilities {
        tool_approval: true,
        thinking: true,
        effort: false,
        effort_levels: vec![],
        modes: vec!["default".into(), "auto_edit".into(), "yolo".into()],
        current_mode: "default".into(),
        mcp: true,
        current_model: None,
        available_models: vec![],
        account_limits: None,
        context_limit: None,
        compaction: None,
    }
}

#[test]
fn capabilities_serialize_with_all_fields() {
    let caps = Capabilities {
        ..base_capabilities()
    };
    let event = ProtocolEvent::Ready {
        version: "0.2.0".into(),
        session_id: None,
        capabilities: caps,
    };
    let json = serde_json::to_string(&event).unwrap();
    let parsed: serde_json::Value = serde_json::from_str(&json).unwrap();

    assert_eq!(parsed["type"], "ready");
    assert_eq!(parsed["capabilities"]["thinking"], true);
    assert_eq!(parsed["capabilities"]["effort"], false);
    assert!(
        parsed["capabilities"]["effort_levels"]
            .as_array()
            .unwrap()
            .is_empty()
    );
    assert_eq!(parsed["capabilities"]["modes"].as_array().unwrap().len(), 3);
    assert_eq!(parsed["capabilities"]["current_mode"], "default");
}

#[test]
fn config_changed_event_serializes_correctly() {
    let caps = Capabilities {
        thinking: false,
        effort: true,
        effort_levels: vec!["low".into(), "medium".into(), "high".into()],
        mcp: false,
        ..base_capabilities()
    };
    let event = ProtocolEvent::ConfigChanged { capabilities: caps };
    let json = serde_json::to_string(&event).unwrap();
    let parsed: serde_json::Value = serde_json::from_str(&json).unwrap();

    assert_eq!(parsed["type"], "config_changed");
    assert_eq!(parsed["capabilities"]["effort_levels"][1], "medium");
}

#[test]
fn capabilities_with_effort_levels_roundtrip() {
    let caps = Capabilities {
        thinking: false,
        effort: true,
        effort_levels: vec!["low".into(), "medium".into(), "high".into()],
        ..base_capabilities()
    };
    let event = ProtocolEvent::Ready {
        version: "0.2.0".into(),
        session_id: Some("test-session".into()),
        capabilities: caps,
    };
    let json = serde_json::to_string(&event).unwrap();
    let parsed: serde_json::Value = serde_json::from_str(&json).unwrap();

    assert_eq!(parsed["capabilities"]["effort"], true);
    assert_eq!(
        parsed["capabilities"]["effort_levels"]
            .as_array()
            .unwrap()
            .len(),
        3
    );
    assert_eq!(parsed["capabilities"]["effort_levels"][0], "low");
    assert_eq!(parsed["capabilities"]["effort_levels"][2], "high");
    assert_eq!(parsed["session_id"], "test-session");
}

#[test]
fn capabilities_with_provider_metadata_roundtrip() {
    let caps = Capabilities {
        current_model: Some("gpt-5-codex".into()),
        available_models: vec![ProviderModelInfo {
            id: "gpt-5-codex".into(),
            display_name: Some("GPT-5 Codex".into()),
            context_window: Some(272_000),
            effort_levels: vec!["low".into(), "medium".into()],
            default_effort: Some("medium".into()),
        }],
        account_limits: Some(AccountLimitsInfo {
            plan_type: Some("pro".into()),
            limits: vec![AccountLimitInfo {
                limit_id: Some("codex".into()),
                limit_name: None,
                primary: Some(AccountLimitWindow {
                    used_percent: 42.0,
                    window_minutes: Some(5),
                    resets_at: Some(123),
                }),
                secondary: None,
                credits: Some(AccountCreditsInfo {
                    has_credits: true,
                    unlimited: false,
                    balance: Some("9.99".into()),
                }),
            }],
        }),
        context_limit: Some(200_000),
        compaction: Some(aion_protocol::events::CompactionInfo {
            enabled: true,
            context_window: 200_000,
            output_reserve: 20_000,
            autocompact_trigger: 167_000,
            emergency_limit: 197_000,
        }),
        ..base_capabilities()
    };
    let event = ProtocolEvent::ConfigChanged { capabilities: caps };
    let json = serde_json::to_string(&event).unwrap();
    let parsed: serde_json::Value = serde_json::from_str(&json).unwrap();

    assert_eq!(parsed["capabilities"]["current_model"], "gpt-5-codex");
    assert_eq!(
        parsed["capabilities"]["available_models"][0]["context_window"],
        272000
    );
    assert_eq!(parsed["capabilities"]["account_limits"]["plan_type"], "pro");
    assert_eq!(
        parsed["capabilities"]["compaction"]["emergency_limit"],
        197000
    );
}
