use by_llm::{
    projected_response_text, reshape_acp_response, reshape_anthropic_response,
    reshape_claude_code_response, reshape_openai_compatible_response, reshape_provider_response,
};
use serde_json::json;

#[test]
fn openai_compatible_response_projects_to_common_text_shape() {
    let projected = reshape_openai_compatible_response(json!({
        "choices": [
            {
                "message": {
                    "role": "assistant",
                    "content": "Hello from OpenAI"
                },
                "finish_reason": "stop"
            }
        ],
        "usage": {
            "prompt_tokens": 11,
            "completion_tokens": 5,
            "total_tokens": 16,
            "prompt_tokens_details": {"cached_tokens": 4}
        }
    }));

    assert_eq!(projected_response_text(&projected), "Hello from OpenAI");
    assert_eq!(
        projected,
        json!({
            "content": [{"type": "text", "text": "Hello from OpenAI"}],
            "role": "assistant",
            "stop_reason": "stop",
            "usage": {
                "input_tokens": 11,
                "output_tokens": 5,
                "total_tokens": 16,
                "cache_read_input_tokens": 4
            }
        })
    );
}

#[test]
fn openai_compatible_response_filters_text_parts() {
    let projected = reshape_openai_compatible_response(json!({
        "choices": [
            {
                "message": {
                    "content": [
                        {"type": "text", "text": "Hello"},
                        {"type": "refusal", "refusal": "ignored"},
                        {"type": "output_text", "text": " world"}
                    ]
                }
            }
        ],
        "usage": {
            "prompt_tokens": 1,
            "completion_tokens": 2
        }
    }));

    assert_eq!(projected_response_text(&projected), "Hello world");
    assert_eq!(projected["role"], "assistant");
    assert_eq!(projected["stop_reason"], serde_json::Value::Null);
    assert!(projected["usage"].get("total_tokens").is_none());
}

#[test]
fn anthropic_response_projects_to_common_text_shape() {
    let projected = reshape_anthropic_response(json!({
        "role": "assistant",
        "content": [
            {"type": "text", "text": "Hello"},
            {"type": "tool_use", "name": "ignored"},
            {"type": "text", "text": " Claude"}
        ],
        "stop_reason": "end_turn",
        "usage": {
            "input_tokens": 13,
            "output_tokens": 6,
            "cache_creation_input_tokens": 2,
            "cache_read_input_tokens": 3
        }
    }));

    assert_eq!(projected_response_text(&projected), "Hello Claude");
    assert_eq!(
        projected,
        json!({
            "content": [
                {"type": "text", "text": "Hello"},
                {"type": "text", "text": " Claude"}
            ],
            "role": "assistant",
            "stop_reason": "end_turn",
            "usage": {
                "input_tokens": 13,
                "output_tokens": 6,
                "cache_creation_input_tokens": 2,
                "cache_read_input_tokens": 3
            }
        })
    );
}

#[test]
fn claude_code_response_projects_result_event_to_common_text_shape() {
    let projected = reshape_claude_code_response(json!([
        {
            "type": "assistant",
            "message": {
                "content": [{"type": "text", "text": "intermediate text"}]
            }
        },
        {
            "type": "result",
            "subtype": "success",
            "result": "Hello from Claude Code",
            "usage": {
                "input_tokens": 17,
                "output_tokens": 4,
                "cache_read_input_tokens": 1,
                "cache_creation_input_tokens": 2
            }
        }
    ]));

    assert_eq!(
        projected_response_text(&projected),
        "Hello from Claude Code"
    );
    assert_eq!(
        projected,
        json!({
            "content": [{"type": "text", "text": "Hello from Claude Code"}],
            "role": "assistant",
            "stop_reason": "end_turn",
            "usage": {
                "input_tokens": 17,
                "output_tokens": 4,
                "cache_read_input_tokens": 1,
                "cache_creation_input_tokens": 2
            }
        })
    );
}

#[test]
fn claude_code_response_prefers_structured_output_tool_input() {
    let projected = reshape_claude_code_response(json!({
        "stdout": "{\"type\":\"assistant\",\"message\":{\"content\":[{\"type\":\"tool_use\",\"name\":\"StructuredOutput\",\"input\":{\"answer\":\"JSON\"}}]}}\n{\"type\":\"result\",\"subtype\":\"success\",\"result\":\"ignored\"}"
    }));

    assert_eq!(projected_response_text(&projected), r#"{"answer":"JSON"}"#);
}

#[test]
fn acp_response_projects_to_common_text_shape() {
    let projected = reshape_acp_response(json!({
        "content": [{"type": "text", "text": "Hello from ACP"}],
        "model": "stub",
        "usage": {
            "input_tokens": 0,
            "output_tokens": 0
        },
        "stop-reason": "end_turn"
    }));

    assert_eq!(projected_response_text(&projected), "Hello from ACP");
    assert_eq!(
        projected,
        json!({
            "content": [{"type": "text", "text": "Hello from ACP"}],
            "role": "assistant",
            "stop_reason": "end_turn",
            "usage": {
                "input_tokens": 0,
                "output_tokens": 0
            }
        })
    );
}

#[test]
fn provider_response_dispatches_by_clojure_message_format() {
    let openai = reshape_provider_response(
        "groq",
        json!({
            "choices": [{"message": {"content": "Groq fixture"}}]
        }),
    )
    .unwrap();
    assert_eq!(projected_response_text(&openai), "Groq fixture");

    let anthropic = reshape_provider_response(
        "anthropic-max",
        json!({
            "content": [{"type": "text", "text": "Max fixture"}]
        }),
    )
    .unwrap();
    assert_eq!(projected_response_text(&anthropic), "Max fixture");

    let claude_code = reshape_provider_response(
        "claude-code",
        json!({"type": "result", "subtype": "success", "result": "Claude fixture"}),
    )
    .unwrap();
    assert_eq!(projected_response_text(&claude_code), "Claude fixture");

    let acp = reshape_provider_response(
        "acp",
        json!({"content": [{"type": "text", "text": "ACP fixture"}]}),
    )
    .unwrap();
    assert_eq!(projected_response_text(&acp), "ACP fixture");
}
