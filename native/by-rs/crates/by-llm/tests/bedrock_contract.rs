use by_llm::{build_bedrock_request, reshape_bedrock_response, BedrockConfig, ChatMessage};
use serde_json::json;

#[test]
fn bedrock_request_uses_converse_shape_and_pulls_out_system_messages() {
    let request = build_bedrock_request(
        &BedrockConfig {
            model: "amazon.nova-lite-v1:0".to_string(),
            temperature: Some(0.0),
            max_tokens: Some(256),
            prompt_cache: false,
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
            "modelId": "amazon.nova-lite-v1:0",
            "messages": [
                {"role": "user", "content": [{"text": "What is 2+2?"}]}
            ],
            "inferenceConfig": {"temperature": 0.0, "maxTokens": 256},
            "system": [{"text": "You are Brainyard."}]
        })
    );
}

#[test]
fn bedrock_request_adds_cache_points_to_system_and_last_user_when_enabled() {
    let request = build_bedrock_request(
        &BedrockConfig {
            model: "amazon.nova-lite-v1:0".to_string(),
            temperature: Some(0.0),
            max_tokens: None,
            prompt_cache: true,
            drop_temperature: false,
        },
        &[
            ChatMessage::system("Stable instructions"),
            ChatMessage::user("First turn"),
            ChatMessage::assistant("Intermediate answer"),
            ChatMessage::user("Second turn"),
        ],
    );

    assert_eq!(
        request,
        json!({
            "modelId": "amazon.nova-lite-v1:0",
            "messages": [
                {"role": "user", "content": [{"text": "First turn"}]},
                {"role": "assistant", "content": [{"text": "Intermediate answer"}]},
                {"role": "user", "content": [{"text": "Second turn"}, {"cachePoint": {"type": "default"}}]}
            ],
            "inferenceConfig": {"temperature": 0.0},
            "system": [{"text": "Stable instructions"}, {"cachePoint": {"type": "default"}}]
        })
    );
}

#[test]
fn bedrock_request_can_drop_temperature_for_models_that_reject_it() {
    let request = build_bedrock_request(
        &BedrockConfig {
            model: "openai.gpt-oss-120b-1:0".to_string(),
            temperature: Some(0.0),
            max_tokens: Some(128),
            prompt_cache: false,
            drop_temperature: true,
        },
        &[ChatMessage::user("hello")],
    );

    assert_eq!(
        request,
        json!({
            "modelId": "openai.gpt-oss-120b-1:0",
            "messages": [
                {"role": "user", "content": [{"text": "hello"}]}
            ],
            "inferenceConfig": {"maxTokens": 128}
        })
    );
}

#[test]
fn bedrock_response_is_projected_to_anthropic_compatible_shape() {
    let response = reshape_bedrock_response(json!({
        "output": {
            "message": {
                "role": "assistant",
                "content": [
                    {"text": "Hello"},
                    {"toolUse": {"name": "ignored-for-now"}},
                    {"text": " world"}
                ]
            }
        },
        "stopReason": "end_turn",
        "usage": {
            "inputTokens": 7,
            "outputTokens": 2,
            "totalTokens": 9,
            "cacheReadInputTokens": 3,
            "cacheWriteInputTokens": 4
        }
    }));

    assert_eq!(
        response,
        json!({
            "content": [
                {"type": "text", "text": "Hello"},
                {"type": "text", "text": " world"}
            ],
            "role": "assistant",
            "stop_reason": "end_turn",
            "usage": {
                "input_tokens": 7,
                "output_tokens": 2,
                "total_tokens": 9,
                "cache_read_input_tokens": 3,
                "cache_creation_input_tokens": 4
            }
        })
    );
}
