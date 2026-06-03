use by_llm::{
    build_acp_request, build_anthropic_request, build_claude_code_request,
    build_openai_compatible_request, build_provider_request, ChatMessage, ProviderChatConfig,
};
use serde_json::json;

#[test]
fn openai_compatible_request_matches_chat_completions_shape() {
    let request = build_openai_compatible_request(
        &ProviderChatConfig {
            model: "gpt-4o".to_string(),
            temperature: Some(0.2),
            max_tokens: Some(64),
            drop_temperature: false,
        },
        &[
            ChatMessage::system("You are Brainyard."),
            ChatMessage::user("What is 2+2?"),
        ],
    );

    assert_eq!(
        request,
        json!({
            "model": "gpt-4o",
            "messages": [
                {"role": "system", "content": "You are Brainyard."},
                {"role": "user", "content": "What is 2+2?"}
            ],
            "temperature": 0.2,
            "max_tokens": 64
        })
    );
}

#[test]
fn openai_compatible_request_drops_temperature_when_model_rejects_it() {
    let request = build_openai_compatible_request(
        &ProviderChatConfig {
            model: "gpt-5".to_string(),
            temperature: Some(0.0),
            max_tokens: None,
            drop_temperature: true,
        },
        &[ChatMessage::user("hello")],
    );

    assert_eq!(
        request,
        json!({
            "model": "gpt-5",
            "messages": [{"role": "user", "content": "hello"}]
        })
    );
}

#[test]
fn anthropic_request_extracts_system_and_defaults_max_tokens() {
    let request = build_anthropic_request(
        &ProviderChatConfig {
            model: "claude-sonnet-4-5".to_string(),
            temperature: Some(0.0),
            max_tokens: None,
            drop_temperature: false,
        },
        &[
            ChatMessage::system("Stable instructions"),
            ChatMessage::system("Output only text"),
            ChatMessage::user("What is 2+2?"),
        ],
    );

    assert_eq!(
        request,
        json!({
            "model": "claude-sonnet-4-5",
            "system": "Stable instructions\n\nOutput only text",
            "messages": [{"role": "user", "content": "What is 2+2?"}],
            "max_tokens": 4096,
            "temperature": 0.0
        })
    );
}

#[test]
fn claude_code_request_matches_subprocess_shape() {
    let request = build_claude_code_request(
        &ProviderChatConfig {
            model: "claude-sonnet-4-6".to_string(),
            temperature: Some(0.2),
            max_tokens: Some(32),
            drop_temperature: false,
        },
        &[
            ChatMessage::system("Stable instructions"),
            ChatMessage::system("Output only text"),
            ChatMessage::user("What is 2+2?"),
        ],
    );

    assert_eq!(
        request,
        json!({
            "argv": [
                "claude", "-p",
                "--no-session-persistence",
                "--output-format", "json",
                "--tools", "",
                "--setting-sources", "",
                "--strict-mcp-config",
                "--disable-slash-commands",
                "--max-turns", "1",
                "--model", "claude-sonnet-4-6",
                "--max-tokens", "32",
                "--system-prompt", "Stable instructions\n\nOutput only text"
            ],
            "stdin": "What is 2+2?",
            "system_prompt_spooled": false
        })
    );
}

#[test]
fn acp_request_flattens_system_and_conversation_into_prompt_block() {
    let request = build_acp_request(
        &ProviderChatConfig {
            model: "stub-model".to_string(),
            temperature: None,
            max_tokens: None,
            drop_temperature: false,
        },
        &[
            ChatMessage::system("System text"),
            ChatMessage::user("First"),
            ChatMessage::assistant("Second"),
            ChatMessage::user("Third"),
        ],
    );

    assert_eq!(
        request,
        json!({
            "backend": "stub",
            "model": "stub-model",
            "prompt": [{
                "type": "text",
                "text": "System text\n\n[user]: First\n\n[assistant]: Second\n\n[user]: Third"
            }],
            "timeout_ms": 600000
        })
    );
}

#[test]
fn provider_request_dispatches_by_clojure_message_format() {
    let config = ProviderChatConfig {
        model: "model".to_string(),
        temperature: None,
        max_tokens: Some(8),
        drop_temperature: false,
    };
    let messages = [ChatMessage::user("hello")];

    let openai = build_provider_request("openrouter", &config, &messages).unwrap();
    assert_eq!(openai.operation, "chat/completions");
    assert_eq!(openai.request["max_tokens"], 8);

    let anthropic = build_provider_request("anthropic-max", &config, &messages).unwrap();
    assert_eq!(anthropic.operation, "messages");
    assert_eq!(anthropic.request["max_tokens"], 8);

    let claude_code = build_provider_request("claude-code", &config, &messages).unwrap();
    assert_eq!(claude_code.operation, "claude-code/subprocess");
    assert_eq!(claude_code.request["stdin"], "hello");

    let acp = build_provider_request("acp", &config, &messages).unwrap();
    assert_eq!(acp.operation, "acp/session-prompt");
    assert_eq!(acp.request["prompt"][0]["text"], "hello");
}
