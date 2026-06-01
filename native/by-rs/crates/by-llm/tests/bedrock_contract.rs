use aws_sdk_bedrockruntime::{
    operation::converse::ConverseOutput as AwsConverseOperationOutput,
    types::{
        ContentBlock, ConversationRole, ConverseOutput as AwsConverseOutput, Message, StopReason,
        TokenUsage,
    },
};
use by_llm::{
    build_bedrock_request, build_bedrock_request_with_cache_zones, default_bedrock_region,
    project_bedrock_converse_output, reshape_bedrock_response, resolve_bedrock_runtime_options,
    BedrockConfig, BedrockRuntimeInputs, CacheZone, ChatMessage,
};
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

#[test]
fn live_bedrock_output_projection_matches_dry_run_response_shape() {
    let message = Message::builder()
        .role(ConversationRole::Assistant)
        .content(ContentBlock::Text("Hello".to_string()))
        .content(ContentBlock::Text(" world".to_string()))
        .build()
        .unwrap();
    let usage = TokenUsage::builder()
        .input_tokens(7)
        .output_tokens(2)
        .total_tokens(9)
        .cache_read_input_tokens(3)
        .cache_write_input_tokens(4)
        .build()
        .unwrap();
    let output = AwsConverseOperationOutput::builder()
        .output(AwsConverseOutput::Message(message))
        .stop_reason(StopReason::EndTurn)
        .usage(usage)
        .build()
        .unwrap();

    let raw = project_bedrock_converse_output(&output);
    assert_eq!(
        raw,
        json!({
            "output": {
                "message": {
                    "role": "assistant",
                    "content": [
                        {"text": "Hello"},
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
        })
    );

    assert_eq!(
        reshape_bedrock_response(raw),
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

#[test]
fn bedrock_runtime_options_follow_clojure_region_and_profile_precedence() {
    let options = resolve_bedrock_runtime_options(BedrockRuntimeInputs {
        explicit_region: Some("ap-northeast-2".to_string()),
        aws_region: Some("us-west-2".to_string()),
        aws_default_region: Some("eu-west-1".to_string()),
        explicit_profile: Some("explicit".to_string()),
        aws_profile: Some("env".to_string()),
        aws_default_profile: Some("default-env".to_string()),
    });

    assert_eq!(options.region, "ap-northeast-2");
    assert_eq!(options.aws_profile.as_deref(), Some("explicit"));

    let options = resolve_bedrock_runtime_options(BedrockRuntimeInputs {
        explicit_region: None,
        aws_region: Some("us-west-2".to_string()),
        aws_default_region: Some("eu-west-1".to_string()),
        explicit_profile: None,
        aws_profile: Some("env".to_string()),
        aws_default_profile: Some("default-env".to_string()),
    });

    assert_eq!(options.region, "us-west-2");
    assert_eq!(options.aws_profile.as_deref(), Some("env"));

    let options = resolve_bedrock_runtime_options(BedrockRuntimeInputs {
        explicit_region: Some("   ".to_string()),
        aws_region: None,
        aws_default_region: None,
        explicit_profile: Some("".to_string()),
        aws_profile: None,
        aws_default_profile: None,
    });

    assert_eq!(options.region, default_bedrock_region());
    assert_eq!(options.aws_profile, None);
}

#[test]
fn bedrock_request_splits_system_cache_zones_and_keeps_last_user_cache_point() {
    let request = build_bedrock_request_with_cache_zones(
        &BedrockConfig {
            model: "amazon.nova-lite-v1:0".to_string(),
            temperature: Some(0.0),
            max_tokens: None,
            prompt_cache: true,
            drop_temperature: false,
        },
        &[
            ChatMessage::system("Preamble\n\nZone A\n\nMiddle\n\nZone B\n\nTail"),
            ChatMessage::user("What is next?"),
        ],
        &[
            CacheZone {
                key: "a".to_string(),
                text: "Zone A".to_string(),
            },
            CacheZone {
                key: "b".to_string(),
                text: "Zone B".to_string(),
            },
        ],
    );

    assert_eq!(
        request,
        json!({
            "modelId": "amazon.nova-lite-v1:0",
            "messages": [
                {"role": "user", "content": [
                    {"text": "What is next?"},
                    {"cachePoint": {"type": "default"}}
                ]}
            ],
            "inferenceConfig": {"temperature": 0.0},
            "system": [
                {"text": "Preamble"},
                {"text": "Zone A"},
                {"cachePoint": {"type": "default"}},
                {"text": "Middle"},
                {"text": "Zone B"},
                {"cachePoint": {"type": "default"}},
                {"text": "Tail"}
            ]
        })
    );
}

#[test]
fn bedrock_request_caps_system_cache_zones_at_three() {
    let request = build_bedrock_request_with_cache_zones(
        &BedrockConfig {
            model: "amazon.nova-lite-v1:0".to_string(),
            temperature: None,
            max_tokens: None,
            prompt_cache: true,
            drop_temperature: false,
        },
        &[
            ChatMessage::system("Zone 1\n\nZone 2\n\nZone 3\n\nZone 4\n\nTail"),
            ChatMessage::user("Go"),
        ],
        &[
            CacheZone {
                key: "1".to_string(),
                text: "Zone 1".to_string(),
            },
            CacheZone {
                key: "2".to_string(),
                text: "Zone 2".to_string(),
            },
            CacheZone {
                key: "3".to_string(),
                text: "Zone 3".to_string(),
            },
            CacheZone {
                key: "4".to_string(),
                text: "Zone 4".to_string(),
            },
        ],
    );

    assert_eq!(
        request.get("system"),
        Some(&json!([
            {"text": "Zone 1"},
            {"cachePoint": {"type": "default"}},
            {"text": "Zone 2"},
            {"cachePoint": {"type": "default"}},
            {"text": "Zone 3"},
            {"cachePoint": {"type": "default"}},
            {"text": "Zone 4\n\nTail"}
        ]))
    );
}

#[test]
fn bedrock_request_falls_back_to_single_system_cache_point_when_zone_is_missing() {
    let system = "Preamble\n\nZone A";
    let request = build_bedrock_request_with_cache_zones(
        &BedrockConfig {
            model: "amazon.nova-lite-v1:0".to_string(),
            temperature: None,
            max_tokens: None,
            prompt_cache: true,
            drop_temperature: false,
        },
        &[ChatMessage::system(system), ChatMessage::user("Go")],
        &[CacheZone {
            key: "missing".to_string(),
            text: "Not in system".to_string(),
        }],
    );

    assert_eq!(
        request.get("system"),
        Some(&json!([
            {"text": system},
            {"cachePoint": {"type": "default"}}
        ]))
    );
}
