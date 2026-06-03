use by_tui::{
    ansi_code, ansi_rule, ansi_style, char_index_at_width, cursor_to, display_width,
    format_analytics_display, format_answer, format_conversation_history,
    format_conversation_message, format_goal_status, format_iteration_exhausted,
    format_iteration_header, format_mulog_event, format_number, format_observation,
    format_status_summary, format_thought, format_todo_list, format_todo_progress,
    format_tool_calls, format_tool_results, format_trace, format_usage_summary, format_usage_table,
    format_welcome_banner, render_markdown_lines, set_scroll_region, strip_ansi, truncate_chars,
    word_wrap, AnalyticsDisplay, ConversationMessage, CostActual, CostAnalysis, CostThroughput,
    MulogEvent, PromptQualityDimensions, PromptQualityScore, StatusSummary, TodoItem, ToolArgument,
    ToolCall, ToolResult, TraceEntry, UsageCall, UsageInputTokenBreakdown, UsageSummary,
    UsageTokenGroup, UsageTokenPart, WasteDetection, WastePattern, WelcomeBanner, ANSI_BOLD,
    ANSI_GREEN,
};

#[test]
fn display_width_matches_clojure_tui_width_rules() {
    assert_eq!(display_width("abc"), 3);
    assert_eq!(display_width("한글"), 4);
    assert_eq!(display_width("⚠\u{fe0f}"), 2);
    assert_eq!(display_width("a\u{200d}b"), 2);
    assert_eq!(display_width("\u{1b}[31mred\u{1b}[0m"), 3);
    assert_eq!(char_index_at_width("ab한", 2), 2);
}

#[test]
fn word_wrap_and_string_helpers_match_tui_format_contracts() {
    assert_eq!(
        word_wrap("alpha beta gamma", 10),
        vec!["alpha beta".to_string(), "gamma".to_string()]
    );
    assert_eq!(truncate_chars("abcdef", 4), "abc…");
    assert_eq!(format_number(Some(-1234567)), "-1,234,567");
    assert_eq!(format_number(None), "0");
    assert_eq!(strip_ansi("\u{1b}[1;32mok\u{1b}[0m"), "ok");
}

#[test]
fn ansi_helpers_project_escape_sequences_without_terminal_io() {
    assert_eq!(ansi_code("bold"), Some(ANSI_BOLD));
    assert_eq!(ansi_code(":green"), Some(ANSI_GREEN));
    assert_eq!(
        ansi_style("ok", &[ANSI_BOLD, ANSI_GREEN], true),
        "\u{1b}[1m\u{1b}[32mok\u{1b}[0m"
    );
    assert_eq!(ansi_style("ok", &[ANSI_BOLD], false), "ok");
    assert_eq!(
        ansi_rule(Some("Ready"), Some(16), false),
        "──── Ready ─────"
    );
    assert_eq!(ansi_rule(None, Some(4), false), "----");
    assert_eq!(cursor_to(2, 5), "\u{1b}[2;5H");
    assert_eq!(set_scroll_region(3, 20), "\u{1b}[3;20r");
}

#[test]
fn high_level_tui_formatters_match_no_color_contracts() {
    assert_eq!(
        format_iteration_header(2, Some(5), false),
        "[+] Iteration 2 / 5"
    );
    assert_eq!(
        format_iteration_exhausted(Some(5), Some(5), false),
        "\nReached iteration limit (5/5). Type /continue [N] to resume with more iterations."
    );

    let todos = vec![
        TodoItem {
            description: "Implement parser".to_string(),
            done: true,
            result: Some("covered by fixtures".to_string()),
            independent: true,
        },
        TodoItem {
            description: "Run live Bedrock smoke".to_string(),
            done: false,
            result: None,
            independent: false,
        },
    ];
    assert_eq!(
        format_todo_progress(&todos, false).as_deref(),
        Some("  TODO [1/2] ✓✗")
    );
    let todo_list = format_todo_list(&todos, false).unwrap();
    assert!(todo_list.contains("  TODO [1/2]"));
    assert!(todo_list.contains("[✓] Implement parser (parallel)"));
    assert!(todo_list.contains("covered by fixtures"));

    assert_eq!(
        format_goal_status(false, Some("needs live validation"), false),
        "  ✗ Goal not yet achieved (needs live validation)"
    );
    assert_eq!(
        format_usage_summary(
            &UsageSummary {
                calls: 3,
                tokens: 12_345,
                cost: 0.25
            },
            false
        ),
        "3 calls │ 12,345 tokens │ $0.2500"
    );

    assert_eq!(format_usage_table(&[], false, false), None);
    let usage_history = vec![
        UsageCall {
            latency_ms: Some(1_000),
            input_tokens: 120,
            output_tokens: 15,
            cache_read_tokens: 20,
            cache_write_tokens: 10,
            total_cost: 0.1,
            turn_id: Some("t1".to_string()),
            iteration: Some("1".to_string()),
            provider: Some(":bedrock".to_string()),
            model: Some("m1".to_string()),
            agent_instance_id: Some(":main".to_string()),
            input_token_breakdown: Some(UsageInputTokenBreakdown {
                system_prompt: UsageTokenGroup {
                    estimated_tokens: 50,
                    parts: vec![UsageTokenPart {
                        name: "policy".to_string(),
                        estimated_tokens: 50,
                    }],
                },
                dspy_signature: UsageTokenGroup {
                    estimated_tokens: 5,
                    parts: vec![],
                },
                user_message: UsageTokenGroup {
                    estimated_tokens: 30,
                    parts: vec![UsageTokenPart {
                        name: "query".to_string(),
                        estimated_tokens: 30,
                    }],
                },
                system_context: UsageTokenGroup {
                    estimated_tokens: 10,
                    parts: vec![],
                },
            }),
        },
        UsageCall {
            latency_ms: Some(2_000),
            input_tokens: 80,
            output_tokens: 20,
            total_cost: 0.2,
            turn_id: Some("t2".to_string()),
            iteration: Some("2".to_string()),
            provider: Some(":openai".to_string()),
            model: Some("m2".to_string()),
            agent_instance_id: Some(":worker".to_string()),
            input_token_breakdown: Some(UsageInputTokenBreakdown {
                system_prompt: UsageTokenGroup {
                    estimated_tokens: 40,
                    parts: vec![
                        UsageTokenPart {
                            name: "policy".to_string(),
                            estimated_tokens: 25,
                        },
                        UsageTokenPart {
                            name: "tools".to_string(),
                            estimated_tokens: 15,
                        },
                    ],
                },
                dspy_signature: UsageTokenGroup {
                    estimated_tokens: 4,
                    parts: vec![],
                },
                user_message: UsageTokenGroup {
                    estimated_tokens: 60,
                    parts: vec![
                        UsageTokenPart {
                            name: "query".to_string(),
                            estimated_tokens: 45,
                        },
                        UsageTokenPart {
                            name: "history".to_string(),
                            estimated_tokens: 15,
                        },
                    ],
                },
                system_context: UsageTokenGroup {
                    estimated_tokens: 8,
                    parts: vec![],
                },
            }),
            ..Default::default()
        },
    ];
    let usage_table = format_usage_table(&usage_history, true, false).unwrap();
    assert!(usage_table.contains("Usage (2 calls, 3.0s total, avg 1.5s/call)"));
    assert!(usage_table.contains("── main · 1 calls · 135 tok · $0.1000 ──"));
    assert!(usage_table.contains("── worker · 1 calls · 100 tok · $0.2000 ──"));
    assert!(usage_table.contains("System"));
    assert!(usage_table.contains("SysCtx"));
    assert!(usage_table.contains("bedrock:m1"));
    assert!(usage_table.contains("Cache hit rate:   50% (1/2 calls)"));
    assert!(usage_table.contains("Avg input tokens: 85  |  Avg output tokens: 17"));
    assert!(usage_table.contains("System Prompt Parts (latest):"));
    assert!(usage_table.contains("policy                      25 tok (62%)"));
    assert!(usage_table.contains("User Message Parts (latest):"));
    assert!(usage_table.contains("history                     15 tok (25%)"));
    assert!(usage_table.contains("Growth: User message 30 → 60 tokens (+100%) over 2 calls"));
}

#[test]
fn conversation_status_markdown_and_answer_helpers_are_pure() {
    let user = ConversationMessage {
        role: "user".to_string(),
        content: "hello".to_string(),
        agent_id: None,
    };
    let assistant = ConversationMessage {
        role: "assistant".to_string(),
        content: "done".to_string(),
        agent_id: Some(":worker/main".to_string()),
    };
    assert_eq!(format_conversation_message(&user, false), "❯ You: hello");
    assert_eq!(
        format_conversation_history(&[user, assistant], Some(1), false).as_deref(),
        Some("Agent (main): done")
    );

    assert_eq!(
        format_status_summary(
            &StatusSummary {
                agent_id: "coact-agent".to_string(),
                status: "running".to_string(),
                iteration: Some(2),
                max_iterations: Some(5),
                todo_progress: Some("1/2".to_string()),
                goal_achieved: Some(false),
            },
            false
        ),
        "Agent Status\n  Agent:      coact-agent\n  Status:     running\n  Iteration:  2 / 5\n  TODO:       1/2\n  Goal:       in progress\n"
    );

    assert_eq!(
        render_markdown_lines("# Title\n- **bold** item\n> `quote`", 40, false),
        vec![
            "Title".to_string(),
            "  • bold item".to_string(),
            "│ quote".to_string()
        ]
    );

    let answer = format_answer("**ok**", 50, false).unwrap();
    assert!(answer.starts_with("\n┌"));
    assert!(answer.contains(" ok "));
    assert!(answer.ends_with('┘'));
}

#[test]
fn live_free_tui_event_formatters_match_no_color_contracts() {
    let thought = format_thought(
        "Need **plan**\n```clojure\n(+ 1 2)\n```",
        2_000,
        "Thinking",
        80,
        false,
    )
    .unwrap();
    assert!(thought.starts_with("  • Thinking:\n"));
    assert!(thought.contains("    Need plan"));
    assert!(thought.contains("    ┌─\n    │ (+ 1 2)\n    └─"));

    let calls = vec![ToolCall {
        tool_name: "read-file".to_string(),
        tool_args: vec![ToolArgument {
            name: "path".to_string(),
            value: "\"README.md\"".to_string(),
        }],
    }];
    assert_eq!(
        format_tool_calls(&calls, 80, false).as_deref(),
        Some("  → read-file(path=\"README.md\")")
    );

    let results = vec![ToolResult {
        tool_name: "read-file".to_string(),
        tool_result: "content".to_string(),
    }];
    assert_eq!(
        format_tool_results(&results, 300, 80, false).as_deref(),
        Some("  ← read-file: content")
    );

    let observation = format_observation("**ok**", 1_000, 80, false).unwrap();
    assert_eq!(observation, "  • Observation:\n    ┌─\n    │ ok\n    └─");

    let banner = format_welcome_banner(
        &WelcomeBanner {
            agent_id: Some(":coact-agent".to_string()),
            session_id: Some("s1".to_string()),
            lm_provider: Some(":bedrock".to_string()),
            lm_model: Some("amazon.nova-lite-v1:0".to_string()),
        },
        false,
    );
    assert!(
        banner.contains("Brainyard TUI — coact-agent · bedrock/amazon.nova-lite-v1:0 · session s1")
    );
    assert!(banner.contains("Type /help for commands. AI output may be inaccurate."));
}

#[test]
fn analytics_display_matches_no_color_contract() {
    let analytics = AnalyticsDisplay {
        pqs: Some(PromptQualityScore {
            overall_score: 82,
            dimensions: Some(PromptQualityDimensions {
                specificity: 20,
                task_atomicity: 22,
                context_completeness: 17,
                acceptance_criteria: 18,
                clarity: 9,
            }),
            recommendations: vec!["Add explicit live validation gate".to_string()],
        }),
        waste: Some(WasteDetection {
            total_waste_tokens: 1_234,
            total_waste_cost: 0.12,
            waste_percentage: 4.5,
            patterns: vec![WastePattern {
                pattern_id: ":duplicate-context".to_string(),
                severity: ":high".to_string(),
                detail: "Repeated prompt context".to_string(),
            }],
        }),
        cost: Some(CostAnalysis {
            actual: CostActual {
                total_cost: 0.42,
                total_tokens: 12_345,
                call_count: 3,
            },
            savings_potential: 0.05,
            throughput: Some(CostThroughput {
                output_tokens_per_sec: 12.5,
                avg_latency_ms: 800.0,
            }),
        }),
    };

    let rendered = format_analytics_display(&analytics, 80, false).unwrap();
    assert!(rendered.contains("Session Analytics"));
    assert!(rendered.contains("Prompt Quality Score (PQS)"));
    assert!(rendered.contains("Overall: 82/100"));
    assert!(rendered.contains("Specificity:          20/25"));
    assert!(rendered.contains("→ Add explicit live validation gate"));
    assert!(rendered.contains("Waste: 1,234 tokens ($0.1200, 4.5%)"));
    assert!(rendered.contains("[HIGH] duplicate-context: Repeated prompt context"));
    assert!(rendered.contains("Actual cost: $0.4200 (12,345 tokens, 3 calls)"));
    assert!(rendered.contains("Potential savings: $0.0500"));
    assert!(rendered.contains("Throughput: 12.5 tok/s, avg 800ms"));
}

#[test]
fn trace_and_mulog_formatters_match_no_color_contracts() {
    assert_eq!(
        format_trace(
            &TraceEntry {
                agent_id: Some(":main".to_string()),
                depth: 2,
                content: "entered node".to_string(),
            },
            false,
        ),
        "  [trace]     [:main] entered node"
    );

    let chat = MulogEvent {
        event_name: Some(":ai.brainyard.agent/chat-completion".to_string()),
        model: Some("claude-haiku".to_string()),
        total_tokens: Some(1_234),
        cost: Some(0.42),
        ..Default::default()
    };
    assert_eq!(
        format_mulog_event(&chat, false).as_deref(),
        Some("  [mulog] chat model=claude-haiku tokens=1234 cost=$0.4200")
    );

    let rlm = MulogEvent {
        event_name: Some(":ai.brainyard.agent/rlm-iteration".to_string()),
        iteration: Some(3),
        code_block_count: 2,
        terminated: true,
        ..Default::default()
    };
    assert_eq!(
        format_mulog_event(&rlm, false).as_deref(),
        Some("  [mulog] iter=3 blocks=2 FINAL")
    );

    let suppressed = MulogEvent {
        event_name: Some(":ai.brainyard.agent/agent-trace".to_string()),
        ..Default::default()
    };
    assert_eq!(format_mulog_event(&suppressed, false), None);
}
