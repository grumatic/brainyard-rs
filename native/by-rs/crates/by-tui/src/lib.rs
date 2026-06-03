#![forbid(unsafe_code)]

pub const ANSI_ESC: &str = "\u{001b}[";
pub const ANSI_RESET: &str = "\u{001b}[0m";
pub const ANSI_BOLD: &str = "\u{001b}[1m";
pub const ANSI_DIM: &str = "\u{001b}[2m";
pub const ANSI_ITALIC: &str = "\u{001b}[3m";
pub const ANSI_UNDERLINE: &str = "\u{001b}[4m";
pub const ANSI_REVERSE: &str = "\u{001b}[7m";
pub const ANSI_BLACK: &str = "\u{001b}[30m";
pub const ANSI_RED: &str = "\u{001b}[31m";
pub const ANSI_GREEN: &str = "\u{001b}[32m";
pub const ANSI_YELLOW: &str = "\u{001b}[33m";
pub const ANSI_BLUE: &str = "\u{001b}[34m";
pub const ANSI_MAGENTA: &str = "\u{001b}[35m";
pub const ANSI_CYAN: &str = "\u{001b}[36m";
pub const ANSI_WHITE: &str = "\u{001b}[37m";
pub const ANSI_BRIGHT_BLACK: &str = "\u{001b}[90m";
pub const ANSI_BRIGHT_RED: &str = "\u{001b}[91m";
pub const ANSI_BRIGHT_GREEN: &str = "\u{001b}[92m";
pub const ANSI_BRIGHT_YELLOW: &str = "\u{001b}[93m";
pub const ANSI_BRIGHT_BLUE: &str = "\u{001b}[94m";
pub const ANSI_BRIGHT_MAGENTA: &str = "\u{001b}[95m";
pub const ANSI_BRIGHT_CYAN: &str = "\u{001b}[96m";
pub const ANSI_BRIGHT_WHITE: &str = "\u{001b}[97m";
pub const ANSI_BG_BLACK: &str = "\u{001b}[40m";
pub const ANSI_BG_BRIGHT_BLACK: &str = "\u{001b}[100m";

pub const H_LINE: &str = "─";
pub const V_LINE: &str = "│";
pub const TL_CORNER: &str = "┌";
pub const TR_CORNER: &str = "┐";
pub const BL_CORNER: &str = "└";
pub const BR_CORNER: &str = "┘";
pub const CHECK: &str = "✓";
pub const CROSS_MARK: &str = "✗";
pub const ARROW: &str = "→";
pub const LEFT_ARROW: &str = "←";
pub const BULLET: &str = "•";
pub const ELLIPSIS: &str = "…";

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct StaticFrame {
    pub rows: usize,
    pub cols: usize,
    pub agent: String,
    pub model: String,
    pub status: String,
}

pub fn render_static_frame(frame: &StaticFrame) -> String {
    let rows = frame.rows.max(5);
    let cols = frame.cols.max(20);
    let scroll_rows = rows - 5;

    let mut lines = Vec::with_capacity(rows);
    let scroll_content = [
        "Brainyard by-rs — static TUI preview".to_string(),
        format!("agent {} · model {}", frame.agent, frame.model),
        "ready: Rust compatibility snapshot".to_string(),
    ];
    for line in scroll_content.iter().take(scroll_rows) {
        lines.push(fit(line, cols));
    }

    while lines.len() < scroll_rows {
        lines.push(String::new());
    }

    lines.push(separator(cols));
    lines.push(fit("❯ ", cols));
    lines.push(separator(cols));
    lines.push(fit(" 0:main0*", cols));
    lines.push(fit(
        &format!(
            "{} │ 0 calls │ 0 tokens │ $0.0000",
            status_label(&frame.status)
        ),
        cols,
    ));

    lines.join("\n")
}

pub fn ansi_code(name: &str) -> Option<&'static str> {
    let normalized = name
        .trim()
        .trim_start_matches(':')
        .replace('_', "-")
        .to_ascii_lowercase();
    match normalized.as_str() {
        "reset" => Some(ANSI_RESET),
        "bold" => Some(ANSI_BOLD),
        "dim" => Some(ANSI_DIM),
        "italic" => Some(ANSI_ITALIC),
        "underline" => Some(ANSI_UNDERLINE),
        "reverse" | "reverse-video" => Some(ANSI_REVERSE),
        "black" => Some(ANSI_BLACK),
        "red" => Some(ANSI_RED),
        "green" => Some(ANSI_GREEN),
        "yellow" => Some(ANSI_YELLOW),
        "blue" => Some(ANSI_BLUE),
        "magenta" => Some(ANSI_MAGENTA),
        "cyan" => Some(ANSI_CYAN),
        "white" => Some(ANSI_WHITE),
        "bright-black" => Some(ANSI_BRIGHT_BLACK),
        "bright-red" => Some(ANSI_BRIGHT_RED),
        "bright-green" => Some(ANSI_BRIGHT_GREEN),
        "bright-yellow" => Some(ANSI_BRIGHT_YELLOW),
        "bright-blue" => Some(ANSI_BRIGHT_BLUE),
        "bright-magenta" => Some(ANSI_BRIGHT_MAGENTA),
        "bright-cyan" => Some(ANSI_BRIGHT_CYAN),
        "bright-white" => Some(ANSI_BRIGHT_WHITE),
        "bg-black" => Some(ANSI_BG_BLACK),
        "bg-bright-black" => Some(ANSI_BG_BRIGHT_BLACK),
        _ => None,
    }
}

pub fn ansi_style(text: &str, codes: &[&str], color_enabled: bool) -> String {
    if !color_enabled {
        return text.to_string();
    }
    let mut out = String::new();
    for code in codes {
        out.push_str(code);
    }
    out.push_str(text);
    out.push_str(ANSI_RESET);
    out
}

pub fn ansi_rule(label: Option<&str>, width: Option<usize>, color_enabled: bool) -> String {
    let width = width.unwrap_or(60);
    if let Some(label) = label {
        let label_text = format!(" {label} ");
        let label_len = label_text.chars().count();
        let left_len = 3.max(width.saturating_sub(label_len) / 2);
        let right_len = 3.max(width.saturating_sub(label_len + left_len));
        let left = H_LINE.repeat(left_len);
        let right = H_LINE.repeat(right_len);
        if color_enabled {
            format!(
                "{}{}{}",
                ansi_style(&left, &[ANSI_DIM], true),
                ansi_style(&label_text, &[ANSI_BOLD, ANSI_BRIGHT_WHITE], true),
                ansi_style(&right, &[ANSI_DIM], true)
            )
        } else {
            format!("{left}{label_text}{right}")
        }
    } else if color_enabled {
        ansi_style(&H_LINE.repeat(width), &[ANSI_DIM], true)
    } else {
        "-".repeat(width)
    }
}

pub fn cursor_to(row: usize, col: usize) -> String {
    format!("{ANSI_ESC}{row};{col}H")
}

pub fn set_scroll_region(top: usize, bottom: usize) -> String {
    format!("{ANSI_ESC}{top};{bottom}r")
}

pub fn display_width(text: &str) -> usize {
    let mut i = 0;
    let mut width = 0;
    while i < text.len() {
        if text.as_bytes()[i] == 0x1b {
            i = skip_ansi_seq(text, i);
            continue;
        }
        let ch = text[i..].chars().next().expect("valid char boundary");
        let cp = ch as u32;
        let next = i + ch.len_utf8();
        let char_width = if zero_width_codepoint(cp) {
            0
        } else if wide_codepoint(cp) || emoji_vs16_next(text, next) {
            2
        } else {
            1
        };
        width += char_width;
        i = next;
    }
    width
}

pub fn char_index_at_width(text: &str, limit: usize) -> usize {
    let mut i = 0;
    let mut width = 0;
    while i < text.len() {
        if text.as_bytes()[i] == 0x1b {
            i = skip_ansi_seq(text, i);
            continue;
        }
        let ch = text[i..].chars().next().expect("valid char boundary");
        let cp = ch as u32;
        let next = i + ch.len_utf8();
        let char_width = if zero_width_codepoint(cp) {
            0
        } else if wide_codepoint(cp) || emoji_vs16_next(text, next) {
            2
        } else {
            1
        };
        if char_width > 0 && width + char_width > limit {
            return i;
        }
        width += char_width;
        i = next;
    }
    i
}

pub fn truncate_chars(text: &str, max_len: usize) -> String {
    if text.chars().count() <= max_len {
        return text.to_string();
    }
    if max_len == 0 {
        return String::new();
    }
    let mut out = text
        .chars()
        .take(max_len.saturating_sub(1))
        .collect::<String>();
    out.push_str(ELLIPSIS);
    out
}

pub fn word_wrap(line: &str, max_width: usize) -> Vec<String> {
    if max_width == 0 || display_width(line) <= max_width {
        return vec![line.to_string()];
    }

    let mut remaining = line.to_string();
    let mut result = Vec::new();
    while display_width(&remaining) > max_width {
        let limit_idx = char_index_at_width(&remaining, max_width);
        let mut break_at = last_space_at_or_before(&remaining, limit_idx)
            .filter(|idx| *idx > 0)
            .unwrap_or(limit_idx);
        if break_at == 0 {
            break_at = remaining
                .char_indices()
                .nth(1)
                .map(|(idx, _)| idx)
                .unwrap_or(remaining.len());
        }
        result.push(remaining[..break_at].to_string());
        remaining = remaining[break_at..].trim().to_string();
        if remaining.is_empty() {
            break;
        }
    }
    if !remaining.is_empty() || result.is_empty() {
        result.push(remaining);
    }
    result
}

pub fn format_number(n: Option<i64>) -> String {
    let Some(n) = n else {
        return "0".to_string();
    };
    let negative = n < 0;
    let mut digits = if negative {
        (-(n as i128)).to_string()
    } else {
        (n as i128).to_string()
    };
    let mut grouped = String::new();
    while digits.len() > 3 {
        let tail = digits.split_off(digits.len() - 3);
        grouped.insert_str(0, &format!(",{tail}"));
    }
    grouped.insert_str(0, &digits);
    if negative {
        grouped.insert(0, '-');
    }
    grouped
}

pub fn strip_ansi(text: &str) -> String {
    let mut out = String::new();
    let mut i = 0;
    while i < text.len() {
        if let Some(next) = sgr_sequence_end(text, i) {
            i = next;
            continue;
        }
        let ch = text[i..].chars().next().expect("valid char boundary");
        out.push(ch);
        i += ch.len_utf8();
    }
    out
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TodoItem {
    pub description: String,
    pub done: bool,
    pub result: Option<String>,
    pub independent: bool,
}

#[derive(Clone, Debug, PartialEq)]
pub struct UsageSummary {
    pub calls: i64,
    pub tokens: i64,
    pub cost: f64,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct UsageTokenPart {
    pub name: String,
    pub estimated_tokens: i64,
}

#[derive(Clone, Debug, Eq, PartialEq, Default)]
pub struct UsageTokenGroup {
    pub estimated_tokens: i64,
    pub parts: Vec<UsageTokenPart>,
}

#[derive(Clone, Debug, Eq, PartialEq, Default)]
pub struct UsageInputTokenBreakdown {
    pub system_prompt: UsageTokenGroup,
    pub dspy_signature: UsageTokenGroup,
    pub user_message: UsageTokenGroup,
    pub system_context: UsageTokenGroup,
}

#[derive(Clone, Debug, PartialEq, Default)]
pub struct UsageCall {
    pub latency_ms: Option<i64>,
    pub input_tokens: i64,
    pub output_tokens: i64,
    pub cache_read_tokens: i64,
    pub cache_write_tokens: i64,
    pub total_cost: f64,
    pub turn_id: Option<String>,
    pub iteration: Option<String>,
    pub provider: Option<String>,
    pub model: Option<String>,
    pub agent_instance_id: Option<String>,
    pub input_token_breakdown: Option<UsageInputTokenBreakdown>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ConversationMessage {
    pub role: String,
    pub content: String,
    pub agent_id: Option<String>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct StatusSummary {
    pub agent_id: String,
    pub status: String,
    pub iteration: Option<i64>,
    pub max_iterations: Option<i64>,
    pub todo_progress: Option<String>,
    pub goal_achieved: Option<bool>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ToolArgument {
    pub name: String,
    pub value: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ToolCall {
    pub tool_name: String,
    pub tool_args: Vec<ToolArgument>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ToolResult {
    pub tool_name: String,
    pub tool_result: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct WelcomeBanner {
    pub agent_id: Option<String>,
    pub session_id: Option<String>,
    pub lm_provider: Option<String>,
    pub lm_model: Option<String>,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct AnalyticsDisplay {
    pub pqs: Option<PromptQualityScore>,
    pub waste: Option<WasteDetection>,
    pub cost: Option<CostAnalysis>,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct PromptQualityScore {
    pub overall_score: i64,
    pub dimensions: Option<PromptQualityDimensions>,
    pub recommendations: Vec<String>,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct PromptQualityDimensions {
    pub specificity: i64,
    pub task_atomicity: i64,
    pub context_completeness: i64,
    pub acceptance_criteria: i64,
    pub clarity: i64,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct WasteDetection {
    pub total_waste_tokens: i64,
    pub total_waste_cost: f64,
    pub waste_percentage: f64,
    pub patterns: Vec<WastePattern>,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct WastePattern {
    pub pattern_id: String,
    pub severity: String,
    pub detail: String,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct CostAnalysis {
    pub actual: CostActual,
    pub savings_potential: f64,
    pub throughput: Option<CostThroughput>,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct CostActual {
    pub total_cost: f64,
    pub total_tokens: i64,
    pub call_count: i64,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct CostThroughput {
    pub output_tokens_per_sec: f64,
    pub avg_latency_ms: f64,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct TraceEntry {
    pub agent_id: Option<String>,
    pub depth: usize,
    pub content: String,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct MulogEvent {
    pub event_name: Option<String>,
    pub model: Option<String>,
    pub total_tokens: Option<i64>,
    pub cost: Option<f64>,
    pub duration_ms: Option<i64>,
    pub prompt_tokens: Option<i64>,
    pub completion_tokens: Option<i64>,
    pub input_tokens: Option<i64>,
    pub output_tokens: Option<i64>,
    pub conversation_turns: Option<i64>,
    pub previous_turns_count: Option<i64>,
    pub enable_compaction: Option<bool>,
    pub enable_budget: Option<bool>,
    pub terminated_by: Option<String>,
    pub total_iterations: Option<i64>,
    pub compaction_count: Option<i64>,
    pub iteration: Option<i64>,
    pub code_block_count: usize,
    pub terminated: bool,
    pub stream: bool,
    pub count: Option<i64>,
    pub prompt_len: Option<usize>,
    pub cli_cost: Option<f64>,
    pub num_turns: Option<i64>,
    pub fallback_pairs: Vec<(String, String)>,
}

pub fn render_markdown_lines(text: &str, max_width: usize, color_enabled: bool) -> Vec<String> {
    let max_width = max_width.max(1);
    let mut blocks = Vec::new();
    let mut in_code = false;
    let mut code_lines = Vec::new();

    for line in text.lines() {
        if in_code {
            if line.starts_with("```") {
                blocks.extend(render_code_lines(
                    &code_lines.join("\n"),
                    max_width,
                    color_enabled,
                ));
                code_lines.clear();
                in_code = false;
            } else {
                code_lines.push(line.to_string());
            }
            continue;
        }

        if line.starts_with("```") {
            in_code = true;
        } else if let Some(header) = markdown_header_text(line) {
            let styled = semantic_header(header, color_enabled);
            blocks.extend(word_wrap(&styled, max_width));
        } else if markdown_hr_line(line) {
            blocks.push(semantic_muted(&H_LINE.repeat(max_width), color_enabled));
        } else if let Some(text) = unordered_item_text(line) {
            blocks.extend(render_markdown_list_item(
                text,
                &format!("  {BULLET} "),
                max_width,
                color_enabled,
            ));
        } else if let Some((number, text)) = ordered_item_text(line) {
            blocks.extend(render_markdown_list_item(
                text,
                &format!("  {number}. "),
                max_width,
                color_enabled,
            ));
        } else if let Some(text) = blockquote_text(line) {
            let bar = semantic_style(
                &format!("{V_LINE} "),
                &[ANSI_DIM, ANSI_GREEN],
                color_enabled,
            );
            let bar_w = 2;
            let styled = convert_inline_markdown(text, color_enabled);
            for wrapped in word_wrap(&styled, max_width.saturating_sub(bar_w).max(1)) {
                blocks.push(format!("{bar}{wrapped}"));
            }
        } else if line.trim().is_empty() {
            blocks.push(String::new());
        } else {
            blocks.extend(word_wrap(
                &convert_inline_markdown(line, color_enabled),
                max_width,
            ));
        }
    }

    if in_code {
        blocks.extend(render_code_lines(
            &code_lines.join("\n"),
            max_width,
            color_enabled,
        ));
    }

    blocks
}

pub fn format_iteration_header(
    iteration_count: i64,
    max_iterations: Option<i64>,
    color_enabled: bool,
) -> String {
    let label = if let Some(max_iterations) = max_iterations {
        format!("Iteration {iteration_count} / {max_iterations}")
    } else {
        format!("Iteration {iteration_count}")
    };
    semantic_style(
        &format!("[+] {label}"),
        &[ANSI_BOLD, ANSI_BRIGHT_WHITE],
        color_enabled,
    )
}

pub fn format_iteration_exhausted(
    iteration_count: Option<i64>,
    max_iterations: Option<i64>,
    color_enabled: bool,
) -> String {
    let iteration = iteration_count
        .map(|value| value.to_string())
        .unwrap_or_else(|| "?".to_string());
    let max_iterations = max_iterations
        .map(|value| value.to_string())
        .unwrap_or_else(|| "?".to_string());
    let msg = format!(
        "Reached iteration limit ({iteration}/{max_iterations}). Type /continue [N] to resume with more iterations."
    );
    format!("\n{}", semantic_warning(&msg, color_enabled))
}

pub fn format_thought(
    thought: &str,
    max_len: usize,
    label: &str,
    cols: usize,
    color_enabled: bool,
) -> Option<String> {
    let text = truncate_chars(thought.trim(), max_len);
    if text.trim().is_empty() {
        return None;
    }
    let cols = cols.saturating_sub(1).max(1);
    let indent = "    ";
    let mut sections = Vec::new();
    for segment in split_fenced_segments(&text) {
        if segment.code {
            sections.push(format_code_segment(
                &segment.text,
                indent,
                cols,
                color_enabled,
            ));
        } else {
            let wrap_width = cols.saturating_sub(indent.len()).max(1);
            sections.push(
                render_markdown_lines(&segment.text, wrap_width, color_enabled)
                    .into_iter()
                    .map(|line| format!("{indent}{line}"))
                    .collect::<Vec<_>>()
                    .join("\n"),
            );
        }
    }
    Some(format!(
        "  {}\n{}",
        semantic_muted(&format!("{BULLET} {label}:"), color_enabled),
        sections.join("\n")
    ))
}

pub fn format_tool_calls(
    tool_calls: &[ToolCall],
    cols: usize,
    color_enabled: bool,
) -> Option<String> {
    if tool_calls.is_empty() {
        return None;
    }
    let cols = cols.saturating_sub(1).max(1);
    let prefix = format!("  {} ", semantic_style(ARROW, &[ANSI_CYAN], color_enabled));
    let prefix_w = 4;
    let indent = " ".repeat(prefix_w);
    let wrap_width = cols.saturating_sub(prefix_w).max(1);
    Some(
        tool_calls
            .iter()
            .map(|call| {
                let args = if call.tool_args.is_empty() {
                    "()".to_string()
                } else {
                    let pairs = call
                        .tool_args
                        .iter()
                        .map(|arg| format!("{}={}", arg.name, arg.value))
                        .collect::<Vec<_>>()
                        .join(", ");
                    format!("({pairs})")
                };
                let content = format!("{}{}", tool_name(&call.tool_name, color_enabled), args);
                word_wrap(&content, wrap_width)
                    .into_iter()
                    .enumerate()
                    .map(|(idx, line)| {
                        if idx == 0 {
                            format!("{prefix}{line}")
                        } else {
                            format!("{indent}{line}")
                        }
                    })
                    .collect::<Vec<_>>()
                    .join("\n")
            })
            .collect::<Vec<_>>()
            .join("\n"),
    )
}

pub fn format_tool_results(
    tool_results: &[ToolResult],
    max_len: usize,
    cols: usize,
    color_enabled: bool,
) -> Option<String> {
    if tool_results.is_empty() {
        return None;
    }
    let cols = cols.saturating_sub(1).max(1);
    let prefix = format!(
        "  {} ",
        semantic_style(LEFT_ARROW, &[ANSI_GREEN], color_enabled)
    );
    let prefix_w = 4;
    let indent = " ".repeat(prefix_w);
    let wrap_width = cols.saturating_sub(prefix_w).max(1);
    Some(
        tool_results
            .iter()
            .map(|result| {
                let content = format!(
                    "{}: {}",
                    tool_name(&result.tool_name, color_enabled),
                    semantic_muted(&truncate_chars(&result.tool_result, max_len), color_enabled)
                );
                word_wrap(&content, wrap_width)
                    .into_iter()
                    .enumerate()
                    .map(|(idx, line)| {
                        if idx == 0 {
                            format!("{prefix}{line}")
                        } else {
                            format!("{indent}{line}")
                        }
                    })
                    .collect::<Vec<_>>()
                    .join("\n")
            })
            .collect::<Vec<_>>()
            .join("\n"),
    )
}

pub fn format_observation(
    observation: &str,
    max_len: usize,
    cols: usize,
    color_enabled: bool,
) -> Option<String> {
    let text = truncate_chars(observation.trim(), max_len);
    if text.trim().is_empty() {
        return None;
    }
    let cols = cols.saturating_sub(1).max(1);
    let indent = "    ";
    let max_width = cols.saturating_sub(indent.len() + 2).max(1);
    let body_lines = if text.starts_with("Error:") {
        text.lines()
            .map(|line| semantic_failure(&truncate_chars(line, max_width), color_enabled))
            .collect::<Vec<_>>()
    } else {
        render_markdown_lines(&text, max_width, color_enabled)
    };
    Some(format_named_box(
        "Observation",
        &body_lines,
        indent,
        color_enabled,
    ))
}

pub fn format_todo_progress(items: &[TodoItem], color_enabled: bool) -> Option<String> {
    if items.is_empty() {
        return None;
    }
    let total = items.len();
    let done = items.iter().filter(|item| item.done).count();
    let marks = items
        .iter()
        .map(|item| if item.done { CHECK } else { CROSS_MARK })
        .collect::<String>();
    Some(semantic_muted(
        &format!("  TODO [{done}/{total}] {marks}"),
        color_enabled,
    ))
}

pub fn format_todo_list(items: &[TodoItem], color_enabled: bool) -> Option<String> {
    if items.is_empty() {
        return None;
    }
    let total = items.len();
    let done = items.iter().filter(|item| item.done).count();
    let mut lines = vec![semantic_header(
        &format!("  TODO [{done}/{total}]"),
        color_enabled,
    )];
    for item in items {
        let check = if item.done {
            semantic_success(&format!("[{CHECK}]"), color_enabled)
        } else {
            semantic_muted(&format!("[{CROSS_MARK}]"), color_enabled)
        };
        let desc = if item.done {
            semantic_muted(&item.description, color_enabled)
        } else {
            item.description.clone()
        };
        let independent = if item.independent {
            semantic_muted(" (parallel)", color_enabled)
        } else {
            String::new()
        };
        let mut line = format!("    {check} {desc}{independent}");
        if item.done {
            if let Some(result) = item
                .result
                .as_deref()
                .map(str::trim)
                .filter(|result| !result.is_empty())
            {
                line.push_str("\n      ");
                line.push_str(&semantic_muted(&truncate_chars(result, 200), color_enabled));
            }
        }
        lines.push(line);
    }
    Some(lines.join("\n"))
}

pub fn format_goal_status(
    goal_achieved: bool,
    goal_reasoning: Option<&str>,
    color_enabled: bool,
) -> String {
    let status = if goal_achieved {
        semantic_success(&format!("{CHECK} Goal achieved"), color_enabled)
    } else {
        semantic_failure(
            &format!("{CROSS_MARK} Goal not yet achieved"),
            color_enabled,
        )
    };
    let reason = goal_reasoning
        .map(str::trim)
        .filter(|reason| !reason.is_empty())
        .map(|reason| {
            format!(
                " {}",
                semantic_muted(&format!("({})", truncate_chars(reason, 200)), color_enabled)
            )
        })
        .unwrap_or_default();
    format!("  {status}{reason}")
}

pub fn format_answer(answer: &str, cols: usize, color_enabled: bool) -> Option<String> {
    let answer = answer.trim();
    if answer.is_empty() {
        return None;
    }

    let wrap_width = cols.saturating_sub(4).max(40);
    let lines = render_markdown_lines(answer, wrap_width, color_enabled);
    let box_width = lines
        .iter()
        .map(|line| display_width(line))
        .max()
        .unwrap_or(0)
        .max(40)
        .min(wrap_width);
    let bar = H_LINE.repeat(box_width + 2);
    let top = format!("{TL_CORNER}{bar}{TR_CORNER}");
    let bottom = format!("{BL_CORNER}{bar}{BR_CORNER}");
    let green = |s: &str| semantic_style(s, &[ANSI_BRIGHT_GREEN], color_enabled);
    let mut out = String::new();
    out.push('\n');
    out.push_str(&green(&top));
    out.push('\n');
    for (idx, line) in lines.iter().enumerate() {
        if idx > 0 {
            out.push('\n');
        }
        let padding = " ".repeat(box_width.saturating_sub(display_width(line)));
        out.push_str(&green(V_LINE));
        out.push(' ');
        out.push_str(line);
        out.push_str(&padding);
        out.push(' ');
        out.push_str(&green(V_LINE));
    }
    out.push('\n');
    out.push_str(&green(&bottom));
    Some(out)
}

pub fn format_usage_summary(usage: &UsageSummary, color_enabled: bool) -> String {
    semantic_muted(
        &format!(
            "{} calls {V_LINE} {} tokens {V_LINE} ${:.4}",
            usage.calls,
            format_number(Some(usage.tokens)),
            usage.cost
        ),
        color_enabled,
    )
}

pub fn format_usage_table(
    history: &[UsageCall],
    breakdown: bool,
    color_enabled: bool,
) -> Option<String> {
    if history.is_empty() {
        return None;
    }

    let n = history.len();
    let latencies = history
        .iter()
        .filter_map(|call| call.latency_ms)
        .collect::<Vec<_>>();
    let total_ms: i64 = latencies.iter().sum();
    let total_s = (total_ms as f64) / 1000.0;
    let avg_ms = if latencies.is_empty() {
        0.0
    } else {
        (total_ms as f64) / (latencies.len() as f64)
    };
    let cache_hits = history
        .iter()
        .filter(|call| call.cache_read_tokens > 0)
        .count();
    let avg_in = history
        .iter()
        .map(usage_non_cached_input_tokens)
        .sum::<i64>() as f64
        / (n as f64);
    let avg_out = history.iter().map(|call| call.output_tokens).sum::<i64>() as f64 / (n as f64);

    let all_attrib = usage_attrib_rows(history);
    let any_attrib = all_attrib.iter().any(|attrib| attrib.total_in > 0);
    let show_attrib = breakdown && any_attrib;
    let show_ctx = show_attrib && all_attrib.iter().any(|attrib| attrib.ctx > 0);
    let groups = usage_group_calls_by_agent(history);
    let multi_group = groups
        .iter()
        .filter(|group| group.label.as_ref().is_some_and(|label| !label.is_empty()))
        .count()
        > 1;
    let show_agent = !multi_group
        && history.iter().any(|call| {
            call.agent_instance_id
                .as_ref()
                .is_some_and(|id| !id.is_empty())
        });

    let base_header = format!(
        "  {}  {}  {}  {}  {}  {}  {}  {}  {}",
        semantic_muted(&format!("{:>3}", "#"), color_enabled),
        semantic_muted(&format!("{:>4}", "Turn"), color_enabled),
        semantic_muted(&format!("{:>4}", "Iter"), color_enabled),
        semantic_muted(&format!("{:>9}", "Latency"), color_enabled),
        semantic_muted(&format!("{:>6}", "In"), color_enabled),
        semantic_muted(&format!("{:>6}", "CacheR"), color_enabled),
        semantic_muted(&format!("{:>6}", "CacheW"), color_enabled),
        semantic_muted(&format!("{:>6}", "Out"), color_enabled),
        semantic_muted(&format!("{:>9}", "Cost"), color_enabled)
    );
    let attrib_header = if show_attrib {
        let mut header = format!(
            "  {}  {}  {}  ",
            semantic_muted(&format!("{:>7}", "System"), color_enabled),
            semantic_muted(&format!("{:>6}", "Sig"), color_enabled),
            semantic_muted(&format!("{:>7}", "UserMsg"), color_enabled)
        );
        if show_ctx {
            header.push_str(&semantic_muted(&format!("{:>6}", "SysCtx"), color_enabled));
            header.push_str("  ");
        }
        header.push_str(&semantic_muted(&format!("{:>7}", "TotalIn"), color_enabled));
        header
    } else {
        String::new()
    };
    let agent_header = if show_agent {
        format!(
            "  {}",
            semantic_muted(&format!("{:<20}", "Agent"), color_enabled)
        )
    } else {
        String::new()
    };
    let model_header = format!("  {}", semantic_muted("Model", color_enabled));
    let header_line = format!("{base_header}{attrib_header}{agent_header}{model_header}");

    let body = groups
        .iter()
        .map(|group| {
            let group_calls = group
                .indices
                .iter()
                .map(|idx| history[*idx].clone())
                .collect::<Vec<_>>();
            let group_attrib = usage_attrib_rows(&group_calls);
            let rows = group
                .indices
                .iter()
                .enumerate()
                .map(|(row_idx, call_idx)| {
                    usage_format_row(
                        row_idx,
                        &history[*call_idx],
                        &group_attrib[row_idx],
                        show_attrib,
                        show_ctx,
                        show_agent,
                    )
                })
                .collect::<Vec<_>>()
                .join("\n");
            let group_header = if multi_group {
                group.label.as_ref().map(|label| {
                    let group_calls = group
                        .indices
                        .iter()
                        .map(|idx| &history[*idx])
                        .collect::<Vec<_>>();
                    let tokens = group_calls
                        .iter()
                        .map(|call| call.input_tokens + call.output_tokens)
                        .sum::<i64>();
                    let cost = group_calls.iter().map(|call| call.total_cost).sum::<f64>();
                    format!(
                        "  {}{}{}",
                        semantic_muted("── ", color_enabled),
                        semantic_style(&normalize_name(label), &[ANSI_BOLD], color_enabled),
                        semantic_muted(
                            &format!(
                                " · {} calls · {} tok · ${:.4} ──",
                                group.indices.len(),
                                format_number(Some(tokens)),
                                cost
                            ),
                            color_enabled
                        )
                    )
                })
            } else {
                None
            };
            if let Some(group_header) = group_header {
                format!("{group_header}\n\n{header_line}\n{rows}")
            } else {
                format!("{header_line}\n{rows}")
            }
        })
        .collect::<Vec<_>>()
        .join("\n\n");

    let latest = history.last();
    let user_tokens = all_attrib
        .iter()
        .map(|attrib| attrib.user)
        .filter(|tokens| *tokens > 0)
        .collect::<Vec<_>>();
    let growth = if show_attrib && user_tokens.len() > 1 {
        let first = user_tokens[0];
        let last = *user_tokens.last().unwrap_or(&first);
        let pct = if first > 0 {
            ((100.0 * ((last - first) as f64) / (first as f64)) as i64).to_string()
        } else {
            "0".to_string()
        };
        let plus = pct.parse::<i64>().ok().is_some_and(|value| value > 0);
        format!(
            "\n\n  {} User message {} → {} tokens ({}{}%) over {} calls",
            semantic_muted("Growth:", color_enabled),
            format_number(Some(first)),
            format_number(Some(last)),
            if plus { "+" } else { "" },
            pct,
            user_tokens.len()
        )
    } else {
        String::new()
    };
    let breakdown_tail = if show_attrib {
        let mut tail = String::new();
        if let Some(call) = latest {
            if let Some(bd) = call.input_token_breakdown.as_ref() {
                tail.push_str(&usage_format_parts(
                    &bd.system_prompt,
                    "System Prompt Parts (latest):",
                    color_enabled,
                ));
                tail.push_str(&usage_format_parts(
                    &bd.user_message,
                    "User Message Parts (latest):",
                    color_enabled,
                ));
            }
        }
        tail.push_str(&growth);
        tail
    } else {
        String::new()
    };

    Some(format!(
        "{}\n\n{}\n\n{}\n{}{}",
        semantic_header(
            &format!(
                "Usage ({} calls, {:.1}s total, avg {:.1}s/call)",
                n,
                total_s,
                avg_ms / 1000.0
            ),
            color_enabled
        ),
        body,
        semantic_muted(
            &format!(
                "  Cache hit rate:   {}",
                if n > 0 {
                    format!(
                        "{}% ({}/{n} calls)",
                        (100.0 * (cache_hits as f64) / (n as f64)) as i64,
                        cache_hits
                    )
                } else {
                    "N/A".to_string()
                }
            ),
            color_enabled
        ),
        semantic_muted(
            &format!(
                "  Avg input tokens: {}  |  Avg output tokens: {}",
                format_number(Some(avg_in as i64)),
                format_number(Some(avg_out as i64))
            ),
            color_enabled
        ),
        breakdown_tail
    ))
}

pub fn format_conversation_message(message: &ConversationMessage, color_enabled: bool) -> String {
    let role = normalize_name(&message.role);
    let role_str = match role.as_str() {
        "user" => user_text("You", color_enabled),
        "assistant" => {
            if let Some(agent_id) = message
                .agent_id
                .as_deref()
                .map(normalize_name)
                .filter(|agent_id| !agent_id.is_empty())
            {
                semantic_style(
                    &format!("Agent ({agent_id})"),
                    &[ANSI_BOLD, ANSI_MAGENTA],
                    color_enabled,
                )
            } else {
                semantic_style("Agent", &[ANSI_BOLD, ANSI_MAGENTA], color_enabled)
            }
        }
        "system" => semantic_muted("System", color_enabled),
        "tool" => tool_name("Tool", color_enabled),
        _ => semantic_muted(&message.role, color_enabled),
    };
    format!("{role_str}: {}", truncate_chars(&message.content, 500))
}

pub fn format_conversation_history(
    messages: &[ConversationMessage],
    last_n: Option<usize>,
    color_enabled: bool,
) -> Option<String> {
    if messages.is_empty() {
        return None;
    }
    let start = last_n
        .and_then(|last_n| messages.len().checked_sub(last_n))
        .unwrap_or(0);
    Some(
        messages[start..]
            .iter()
            .map(|message| format_conversation_message(message, color_enabled))
            .collect::<Vec<_>>()
            .join("\n"),
    )
}

pub fn format_status_summary(summary: &StatusSummary, color_enabled: bool) -> String {
    let status = normalize_name(&summary.status);
    let status_color = match status.as_str() {
        "running" => ANSI_BRIGHT_GREEN,
        "idle" => ANSI_BRIGHT_YELLOW,
        "cancelled" | "stopped" => ANSI_BRIGHT_RED,
        _ => ANSI_WHITE,
    };
    let mut out = String::new();
    out.push_str(&semantic_header("Agent Status", color_enabled));
    out.push('\n');
    out.push_str("  Agent:      ");
    out.push_str(&semantic_style(
        &summary.agent_id,
        &[ANSI_BOLD, ANSI_CYAN],
        color_enabled,
    ));
    out.push('\n');
    out.push_str("  Status:     ");
    out.push_str(&semantic_style(&status, &[status_color], color_enabled));
    out.push('\n');
    out.push_str("  Iteration:  ");
    out.push_str(&summary.iteration.unwrap_or(0).to_string());
    if let Some(max_iterations) = summary.max_iterations {
        out.push_str(" / ");
        out.push_str(&max_iterations.to_string());
    }
    out.push('\n');
    if let Some(todo_progress) = summary.todo_progress.as_deref() {
        out.push_str("  TODO:       ");
        out.push_str(todo_progress);
        out.push('\n');
    }
    if let Some(goal_achieved) = summary.goal_achieved {
        out.push_str("  Goal:       ");
        if goal_achieved {
            out.push_str(&semantic_success("achieved", color_enabled));
        } else {
            out.push_str(&semantic_muted("in progress", color_enabled));
        }
        out.push('\n');
    }
    out
}

pub fn format_welcome_banner(banner: &WelcomeBanner, color_enabled: bool) -> String {
    let sep = semantic_style(" · ", &[ANSI_DIM], color_enabled);
    let title = semantic_style(
        "Brainyard TUI",
        &[ANSI_BOLD, ANSI_BRIGHT_CYAN],
        color_enabled,
    );
    let agent = semantic_style(
        &normalize_name(banner.agent_id.as_deref().unwrap_or("unknown")),
        &[ANSI_BOLD, ANSI_CYAN],
        color_enabled,
    );
    let model = banner.lm_provider.as_deref().map(|provider| {
        semantic_style(
            &format!(
                "{}{}",
                normalize_name(provider),
                banner
                    .lm_model
                    .as_deref()
                    .map(|model| format!("/{model}"))
                    .unwrap_or_default()
            ),
            &[ANSI_BRIGHT_MAGENTA],
            color_enabled,
        )
    });
    let session = banner.session_id.as_deref().map(|session_id| {
        semantic_style(&format!("session {session_id}"), &[ANSI_DIM], color_enabled)
    });
    let mut head = format!(
        "{title}{}{}",
        semantic_style(" — ", &[ANSI_DIM], color_enabled),
        agent
    );
    if let Some(model) = model {
        head.push_str(&sep);
        head.push_str(&model);
    }
    if let Some(session) = session {
        head.push_str(&sep);
        head.push_str(&session);
    }
    let hint = semantic_style(
        "Type /help for commands. AI output may be inaccurate.",
        &[ANSI_DIM],
        color_enabled,
    );
    let inner = display_width(&head).max(display_width(&hint));
    let row = |value: &str| {
        format!(
            "{} {}{} {}",
            semantic_style(V_LINE, &[ANSI_DIM], color_enabled),
            value,
            " ".repeat(inner.saturating_sub(display_width(value))),
            semantic_style(V_LINE, &[ANSI_DIM], color_enabled)
        )
    };
    let bar = H_LINE.repeat(inner + 2);
    let top = semantic_style(
        &format!("{TL_CORNER}{bar}{TR_CORNER}"),
        &[ANSI_DIM],
        color_enabled,
    );
    let bottom = semantic_style(
        &format!("{BL_CORNER}{bar}{BR_CORNER}"),
        &[ANSI_DIM],
        color_enabled,
    );
    format!("\n{top}\n{}\n{}\n{bottom}\n", row(&head), row(&hint))
}

pub fn format_analytics_display(
    analytics: &AnalyticsDisplay,
    cols: usize,
    color_enabled: bool,
) -> Option<String> {
    if analytics.pqs.is_none() && analytics.waste.is_none() && analytics.cost.is_none() {
        return None;
    }

    let bar = H_LINE.repeat(cols.min(60));
    let section = |title: &str| {
        format!(
            "{}\n{}\n",
            semantic_muted(&bar, color_enabled),
            semantic_header(&format!("  {title}"), color_enabled)
        )
    };
    let mut out = String::new();
    out.push('\n');
    out.push_str(&semantic_style(&bar, &[ANSI_BRIGHT_CYAN], color_enabled));
    out.push('\n');
    out.push_str(&semantic_style(
        "  Session Analytics",
        &[ANSI_BOLD, ANSI_BRIGHT_CYAN],
        color_enabled,
    ));
    out.push('\n');
    out.push_str(&semantic_style(&bar, &[ANSI_BRIGHT_CYAN], color_enabled));
    out.push('\n');

    out.push_str(&section("Prompt Quality Score (PQS)"));
    let pqs = analytics.pqs.as_ref().cloned().unwrap_or_default();
    let score_color = if pqs.overall_score >= 80 {
        ANSI_BRIGHT_GREEN
    } else if pqs.overall_score >= 50 {
        ANSI_BRIGHT_YELLOW
    } else {
        ANSI_BRIGHT_RED
    };
    out.push_str("  Overall: ");
    out.push_str(&semantic_style(
        &format!("{}/100", pqs.overall_score),
        &[ANSI_BOLD, score_color],
        color_enabled,
    ));
    out.push('\n');
    if let Some(dimensions) = pqs.dimensions {
        out.push_str(&semantic_muted(
            &format!("  Specificity:          {:>2}/25", dimensions.specificity),
            color_enabled,
        ));
        out.push('\n');
        out.push_str(&semantic_muted(
            &format!(
                "  Task Atomicity:       {:>2}/25",
                dimensions.task_atomicity
            ),
            color_enabled,
        ));
        out.push('\n');
        out.push_str(&semantic_muted(
            &format!(
                "  Context Completeness: {:>2}/20",
                dimensions.context_completeness
            ),
            color_enabled,
        ));
        out.push('\n');
        out.push_str(&semantic_muted(
            &format!(
                "  Acceptance Criteria:  {:>2}/20",
                dimensions.acceptance_criteria
            ),
            color_enabled,
        ));
        out.push('\n');
        out.push_str(&semantic_muted(
            &format!("  Clarity:              {:>2}/10", dimensions.clarity),
            color_enabled,
        ));
        out.push('\n');
    }
    if !pqs.recommendations.is_empty() {
        out.push_str(&semantic_muted("  Recommendations:", color_enabled));
        out.push('\n');
        for recommendation in pqs.recommendations {
            out.push_str("    ");
            out.push_str(&semantic_style(ARROW, &[ANSI_BRIGHT_YELLOW], color_enabled));
            out.push(' ');
            out.push_str(&semantic_muted(&recommendation, color_enabled));
            out.push('\n');
        }
    }

    out.push_str(&section("Waste Detection"));
    let waste = analytics.waste.as_ref().cloned().unwrap_or_default();
    if waste.patterns.is_empty() {
        out.push_str(&semantic_success(
            "  No waste patterns detected.",
            color_enabled,
        ));
        out.push('\n');
    } else {
        out.push_str(&semantic_warning(
            &format!(
                "  Waste: {} tokens (${:.4}, {:.1}%)",
                format_number(Some(waste.total_waste_tokens)),
                waste.total_waste_cost,
                waste.waste_percentage
            ),
            color_enabled,
        ));
        out.push('\n');
        for pattern in waste.patterns {
            let severity = normalize_name(&pattern.severity);
            let severity_upper = severity.to_ascii_uppercase();
            let color = match severity.as_str() {
                "high" => ANSI_BRIGHT_RED,
                "medium" => ANSI_BRIGHT_YELLOW,
                _ => ANSI_DIM,
            };
            out.push_str("  ");
            out.push_str(&semantic_style(
                &format!("[{severity_upper}]"),
                &[color],
                color_enabled,
            ));
            out.push(' ');
            out.push_str(&normalize_name(&pattern.pattern_id));
            out.push_str(": ");
            out.push_str(&semantic_muted(&pattern.detail, color_enabled));
            out.push('\n');
        }
    }

    out.push_str(&section("Cost Analysis"));
    let cost = analytics.cost.as_ref().cloned().unwrap_or_default();
    out.push_str("  Actual cost: ");
    out.push_str(&semantic_style(
        &format!("${:.4}", cost.actual.total_cost),
        &[ANSI_BOLD, ANSI_BRIGHT_WHITE],
        color_enabled,
    ));
    out.push_str(&semantic_muted(
        &format!(
            " ({} tokens, {} calls)",
            format_number(Some(cost.actual.total_tokens)),
            cost.actual.call_count
        ),
        color_enabled,
    ));
    out.push('\n');
    if cost.savings_potential > 0.0 {
        out.push_str("  Potential savings: ");
        out.push_str(&semantic_style(
            &format!("${:.4}", cost.savings_potential),
            &[ANSI_BOLD, ANSI_BRIGHT_GREEN],
            color_enabled,
        ));
        out.push('\n');
    }
    if let Some(throughput) = cost.throughput {
        if throughput.output_tokens_per_sec > 0.0 {
            out.push_str(&semantic_muted(
                &format!(
                    "  Throughput: {} tok/s, avg {}ms",
                    format_float_compact(throughput.output_tokens_per_sec),
                    format_float_compact(throughput.avg_latency_ms)
                ),
                color_enabled,
            ));
            out.push('\n');
        }
    }

    Some(out)
}

pub fn format_trace(entry: &TraceEntry, color_enabled: bool) -> String {
    let indent = " ".repeat(entry.depth.saturating_mul(2));
    let prefix = entry
        .agent_id
        .as_deref()
        .map(|agent_id| {
            format!(
                "{} ",
                semantic_muted(&format!("[{agent_id}]"), color_enabled)
            )
        })
        .unwrap_or_default();
    format!(
        "{}{}{}{}",
        semantic_muted("  [trace] ", color_enabled),
        indent,
        prefix,
        semantic_muted(&entry.content, color_enabled)
    )
}

pub fn format_mulog_event(event: &MulogEvent, color_enabled: bool) -> Option<String> {
    let suffix = event
        .event_name
        .as_deref()
        .map(normalize_name)
        .filter(|suffix| !suffix.is_empty())?;
    let line = match suffix.as_str() {
        "chat-completion" => {
            let mut line = format!("chat model={}", event.model.as_deref().unwrap_or(""));
            if let Some(tokens) = event.total_tokens {
                line.push_str(&format!(" tokens={tokens}"));
            }
            if let Some(cost) = event.cost {
                line.push_str(&format!(" cost=${cost:.4}"));
            }
            line
        }
        "openai-api-call-result" => {
            let mut line = format!("openai-result {}", event.model.as_deref().unwrap_or(""));
            append_duration(&mut line, event.duration_ms);
            if let Some(tokens) = event.prompt_tokens {
                line.push_str(&format!(" in={tokens}"));
            }
            if let Some(tokens) = event.completion_tokens {
                line.push_str(&format!(" out={tokens}"));
            }
            line
        }
        "anthropic-api-call-result" => {
            let mut line = format!("anthropic-result {}", event.model.as_deref().unwrap_or(""));
            append_duration(&mut line, event.duration_ms);
            if let Some(tokens) = event.input_tokens {
                line.push_str(&format!(" in={tokens}"));
            }
            if let Some(tokens) = event.output_tokens {
                line.push_str(&format!(" out={tokens}"));
            }
            line
        }
        "rlm-turn-start" => format!(
            "rlm-start turns={} prev-turns={} compaction={} budget={}",
            option_i64_text(event.conversation_turns),
            option_i64_text(event.previous_turns_count),
            option_bool_text(event.enable_compaction),
            option_bool_text(event.enable_budget)
        ),
        "rlm-turn-complete" => format!(
            "rlm-done terminated={} iters={} compactions={}",
            event.terminated_by.as_deref().unwrap_or(""),
            option_i64_text(event.total_iterations),
            option_i64_text(event.compaction_count)
        ),
        "agent-trace" | "agent-conversation" => return None,
        "rlm-iteration" => {
            let mut line = format!(
                "iter={} blocks={}",
                option_i64_text(event.iteration),
                event.code_block_count
            );
            if event.terminated {
                line.push_str(" FINAL");
            }
            line
        }
        "anthropic-api-call" => {
            let mut line = format!("anthropic {}", event.model.as_deref().unwrap_or(""));
            if event.stream {
                line.push_str(" stream");
            }
            line
        }
        "openai-api-call" => {
            let mut line = format!("openai {}", event.model.as_deref().unwrap_or(""));
            if event.stream {
                line.push_str(" stream");
            }
            line
        }
        "create-embedding" => format!("embed model={}", event.model.as_deref().unwrap_or("")),
        "create-embeddings" => format!(
            "embed model={} n={}",
            event.model.as_deref().unwrap_or(""),
            option_i64_text(event.count)
        ),
        "cli-call" => {
            let mut line = format!("cli-call {}", event.model.as_deref().unwrap_or("?"));
            if event.stream {
                line.push_str(" stream");
            }
            if let Some(prompt_len) = event.prompt_len {
                line.push_str(&format!(" prompt={prompt_len}chars"));
            }
            line
        }
        "cli-call-result" => {
            let mut line = format!("cli-result {}", event.model.as_deref().unwrap_or("?"));
            if event.stream {
                line.push_str(" stream");
            }
            append_duration(&mut line, event.duration_ms);
            if let Some(tokens) = event.input_tokens {
                line.push_str(&format!(" in={tokens}"));
            }
            if let Some(tokens) = event.output_tokens {
                line.push_str(&format!(" out={tokens}"));
            }
            if let Some(cost) = event.cli_cost {
                line.push_str(&format!(" cost=${cost:.4}"));
            }
            if let Some(turns) = event.num_turns {
                line.push_str(&format!(" turns={turns}"));
            }
            line
        }
        _ => {
            let mut line = suffix;
            if !event.fallback_pairs.is_empty() {
                line.push(' ');
                line.push_str(
                    &event
                        .fallback_pairs
                        .iter()
                        .map(|(key, value)| format!("{}={}", normalize_name(key), value))
                        .collect::<Vec<_>>()
                        .join(" "),
                );
            }
            line
        }
    };
    Some(semantic_muted(&format!("  [mulog] {line}"), color_enabled))
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct TextSegment {
    code: bool,
    text: String,
}

fn split_fenced_segments(text: &str) -> Vec<TextSegment> {
    let mut segments = Vec::new();
    let mut in_code = false;
    let mut buffer = Vec::<String>::new();
    for line in text.lines() {
        if line.starts_with("```") {
            if in_code {
                segments.push(TextSegment {
                    code: true,
                    text: buffer.join("\n").trim().to_string(),
                });
                buffer.clear();
                in_code = false;
            } else {
                let prose = buffer.join("\n").trim().to_string();
                if !prose.is_empty() {
                    segments.push(TextSegment {
                        code: false,
                        text: prose,
                    });
                }
                buffer.clear();
                in_code = true;
            }
        } else {
            buffer.push(line.to_string());
        }
    }
    let text = buffer.join("\n").trim().to_string();
    if !text.is_empty() {
        segments.push(TextSegment {
            code: in_code,
            text,
        });
    }
    segments
}

fn format_code_segment(code: &str, indent: &str, cols: usize, color_enabled: bool) -> String {
    let max_width = cols.saturating_sub(indent.len() + 2).max(1);
    let mut lines = Vec::new();
    lines.push(format!("{indent}{}", semantic_muted("┌─", color_enabled)));
    for line in code.lines() {
        lines.push(format!(
            "{indent}{}{}",
            semantic_muted("│ ", color_enabled),
            semantic_style(
                &truncate_chars(line, max_width),
                &[ANSI_DIM, ANSI_CYAN],
                color_enabled
            )
        ));
    }
    lines.push(format!("{indent}{}", semantic_muted("└─", color_enabled)));
    lines.join("\n")
}

fn format_named_box(
    label: &str,
    body_lines: &[String],
    indent: &str,
    color_enabled: bool,
) -> String {
    let mut lines = Vec::new();
    lines.push(format!(
        "  {}",
        semantic_muted(&format!("{BULLET} {label}:"), color_enabled)
    ));
    lines.push(format!("{indent}{}", semantic_muted("┌─", color_enabled)));
    for line in body_lines {
        lines.push(format!(
            "{indent}{}{}",
            semantic_muted("│ ", color_enabled),
            line
        ));
    }
    lines.push(format!("{indent}{}", semantic_muted("└─", color_enabled)));
    lines.join("\n")
}

fn status_label(status: &str) -> &str {
    let status = status.trim();
    if status.is_empty() {
        "idle"
    } else {
        status
    }
}

fn separator(cols: usize) -> String {
    H_LINE.repeat(cols)
}

fn fit(text: &str, cols: usize) -> String {
    text.chars().take(cols).collect()
}

fn skip_ansi_seq(text: &str, i: usize) -> usize {
    let mut j = i + 1;
    while j < text.len() {
        let ch = text[j..].chars().next().expect("valid char boundary");
        j += ch.len_utf8();
        if ch.is_alphabetic() {
            return j;
        }
    }
    j
}

fn emoji_vs16_next(text: &str, next_index: usize) -> bool {
    next_index < text.len()
        && text[next_index..]
            .chars()
            .next()
            .is_some_and(|ch| ch as u32 == 0xFE0F)
}

fn zero_width_codepoint(cp: u32) -> bool {
    cp == 0x200B
        || cp == 0x200C
        || cp == 0x200D
        || (0xFE00..=0xFE0F).contains(&cp)
        || cp == 0xFEFF
        || (0xE0020..=0xE007F).contains(&cp)
}

fn wide_codepoint(cp: u32) -> bool {
    (0x1100..=0x115F).contains(&cp)
        || (0x2E80..=0x303F).contains(&cp)
        || (0x3040..=0x33FF).contains(&cp)
        || (0x3400..=0x4DBF).contains(&cp)
        || (0x4E00..=0x9FFF).contains(&cp)
        || (0xAC00..=0xD7AF).contains(&cp)
        || (0xF900..=0xFAFF).contains(&cp)
        || (0xFE30..=0xFE4F).contains(&cp)
        || (0xFF01..=0xFF60).contains(&cp)
        || (0xFFE0..=0xFFE6).contains(&cp)
        || (0x231A..=0x231B).contains(&cp)
        || (0x23E9..=0x23F3).contains(&cp)
        || (0x23F8..=0x23FA).contains(&cp)
        || (0x25FD..=0x25FE).contains(&cp)
        || (0x2614..=0x2615).contains(&cp)
        || (0x2648..=0x2653).contains(&cp)
        || cp == 0x267F
        || cp == 0x2693
        || cp == 0x26A1
        || (0x26AA..=0x26AB).contains(&cp)
        || (0x26BD..=0x26BE).contains(&cp)
        || (0x26C4..=0x26C5).contains(&cp)
        || cp == 0x26CE
        || cp == 0x26D4
        || cp == 0x26EA
        || (0x26F2..=0x26F3).contains(&cp)
        || cp == 0x26F5
        || cp == 0x26FA
        || cp == 0x26FD
        || cp == 0x2705
        || (0x270A..=0x270B).contains(&cp)
        || cp == 0x2728
        || cp == 0x274C
        || cp == 0x274E
        || (0x2753..=0x2755).contains(&cp)
        || cp == 0x2757
        || (0x2795..=0x2797).contains(&cp)
        || cp == 0x27B0
        || cp == 0x27BF
        || (0x2B1B..=0x2B1C).contains(&cp)
        || cp == 0x2B50
        || cp == 0x2B55
        || cp == 0x3030
        || cp == 0x303D
        || cp == 0x3297
        || cp == 0x3299
        || (0x1F000..=0x1FAFF).contains(&cp)
        || (0x1FC00..=0x1FFFD).contains(&cp)
}

fn last_space_at_or_before(text: &str, byte_limit: usize) -> Option<usize> {
    let end = if byte_limit < text.len() {
        let ch = text[byte_limit..].chars().next()?;
        if ch == ' ' {
            byte_limit + ch.len_utf8()
        } else {
            byte_limit
        }
    } else {
        byte_limit
    };
    text[..end.min(text.len())].rfind(' ')
}

fn sgr_sequence_end(text: &str, i: usize) -> Option<usize> {
    let bytes = text.as_bytes();
    if bytes.get(i) != Some(&0x1b) || bytes.get(i + 1) != Some(&b'[') {
        return None;
    }
    let mut j = i + 2;
    while j < bytes.len() && (bytes[j].is_ascii_digit() || bytes[j] == b';') {
        j += 1;
    }
    if bytes.get(j) == Some(&b'm') {
        Some(j + 1)
    } else {
        None
    }
}

#[derive(Clone, Debug, Default)]
struct UsageAttrib {
    sys: i64,
    sig: i64,
    user: i64,
    ctx: i64,
    total_in: i64,
}

#[derive(Clone, Debug)]
struct UsageGroup {
    label: Option<String>,
    indices: Vec<usize>,
}

fn usage_non_cached_input_tokens(call: &UsageCall) -> i64 {
    (call.input_tokens - call.cache_read_tokens - call.cache_write_tokens).max(0)
}

fn usage_attrib_rows(calls: &[UsageCall]) -> Vec<UsageAttrib> {
    calls
        .iter()
        .map(|call| {
            let Some(bd) = call.input_token_breakdown.as_ref() else {
                return UsageAttrib::default();
            };
            let sys = bd.system_prompt.estimated_tokens;
            let sig = bd.dspy_signature.estimated_tokens;
            let user = bd.user_message.estimated_tokens;
            let ctx = bd.system_context.estimated_tokens;
            UsageAttrib {
                sys,
                sig,
                user,
                ctx,
                total_in: sys + sig + user + ctx,
            }
        })
        .collect()
}

fn usage_group_calls_by_agent(history: &[UsageCall]) -> Vec<UsageGroup> {
    let mut groups: Vec<UsageGroup> = Vec::new();
    for (idx, call) in history.iter().enumerate() {
        let label = call
            .agent_instance_id
            .as_ref()
            .filter(|value| !value.is_empty())
            .cloned();
        if let Some(group) = groups.iter_mut().find(|group| group.label == label) {
            group.indices.push(idx);
        } else {
            groups.push(UsageGroup {
                label,
                indices: vec![idx],
            });
        }
    }
    groups
}

fn usage_format_row(
    idx: usize,
    call: &UsageCall,
    attrib: &UsageAttrib,
    show_attrib: bool,
    show_ctx: bool,
    show_agent: bool,
) -> String {
    let latency = call
        .latency_ms
        .map(|ms| format!("{}ms", format_number(Some(ms))))
        .unwrap_or_else(|| "-".to_string());
    let cost = if call.total_cost > 0.0 {
        format!("${:.4}", call.total_cost)
    } else {
        "-".to_string()
    };
    let turn_id = call.turn_id.as_deref().unwrap_or("-");
    let iteration = call.iteration.as_deref().unwrap_or("-");
    let model = match (call.provider.as_deref(), call.model.as_deref()) {
        (Some(provider), Some(model)) => format!("{}:{model}", normalize_name(provider)),
        (_, Some(model)) => model.to_string(),
        _ => "?".to_string(),
    };
    let mut row = format!(
        "  {:>3}  {:>4}  {:>4}  {:>9}  {:>6}  {:>6}  {:>6}  {:>6}  {:>9}",
        idx + 1,
        turn_id,
        iteration,
        latency,
        format_number(Some(usage_non_cached_input_tokens(call))),
        format_number(Some(call.cache_read_tokens)),
        format_number(Some(call.cache_write_tokens)),
        format_number(Some(call.output_tokens)),
        cost
    );
    if show_attrib {
        row.push_str(&format!(
            "  {:>7}  {:>6}  {:>7}  ",
            format_number(Some(attrib.sys)),
            format_number(Some(attrib.sig)),
            format_number(Some(attrib.user))
        ));
        if show_ctx {
            row.push_str(&format!("{:>6}  ", format_number(Some(attrib.ctx))));
        }
        row.push_str(&format!("{:>7}", format_number(Some(attrib.total_in))));
    }
    if show_agent {
        let agent = call
            .agent_instance_id
            .as_deref()
            .map(normalize_name)
            .filter(|value| !value.is_empty())
            .unwrap_or_else(|| "-".to_string());
        row.push_str(&format!("  {agent:<20}"));
    }
    row.push_str("  ");
    row.push_str(&model);
    row
}

fn usage_format_parts(group: &UsageTokenGroup, label: &str, color_enabled: bool) -> String {
    if group.parts.is_empty() {
        return String::new();
    }
    let total = group.estimated_tokens.max(0);
    let mut parts = group.parts.clone();
    parts.sort_by(|a, b| b.estimated_tokens.cmp(&a.estimated_tokens));
    let rows = parts
        .iter()
        .map(|part| {
            let pct = if total > 0 {
                (100.0 * (part.estimated_tokens as f64) / (total as f64)) as i64
            } else {
                0
            };
            format!(
                "    {:<24}{:>6} tok ({}%)",
                normalize_name(&part.name),
                format_number(Some(part.estimated_tokens)),
                if pct < 1 {
                    "<1".to_string()
                } else {
                    pct.to_string()
                }
            )
        })
        .collect::<Vec<_>>()
        .join("\n");
    format!("\n  {}\n{}", semantic_muted(label, color_enabled), rows)
}

fn semantic_style(text: &str, codes: &[&str], color_enabled: bool) -> String {
    ansi_style(text, codes, color_enabled)
}

fn semantic_header(text: &str, color_enabled: bool) -> String {
    semantic_style(text, &[ANSI_BOLD, ANSI_BRIGHT_WHITE], color_enabled)
}

fn semantic_success(text: &str, color_enabled: bool) -> String {
    semantic_style(text, &[ANSI_BOLD, ANSI_BRIGHT_GREEN], color_enabled)
}

fn semantic_failure(text: &str, color_enabled: bool) -> String {
    semantic_style(text, &[ANSI_BOLD, ANSI_BRIGHT_RED], color_enabled)
}

fn semantic_warning(text: &str, color_enabled: bool) -> String {
    semantic_style(text, &[ANSI_BOLD, ANSI_BRIGHT_YELLOW], color_enabled)
}

fn semantic_muted(text: &str, color_enabled: bool) -> String {
    semantic_style(text, &[ANSI_DIM], color_enabled)
}

fn tool_name(text: &str, color_enabled: bool) -> String {
    semantic_style(text, &[ANSI_BOLD, ANSI_CYAN], color_enabled)
}

fn user_text(text: &str, color_enabled: bool) -> String {
    semantic_style(
        &format!("❯ {text}"),
        &[ANSI_BOLD, ANSI_BRIGHT_CYAN, ANSI_BG_BRIGHT_BLACK],
        color_enabled,
    )
}

fn normalize_name(value: &str) -> String {
    let value = value.trim().trim_start_matches(':');
    value
        .rsplit_once('/')
        .map(|(_, name)| name)
        .unwrap_or(value)
        .to_string()
}

fn format_float_compact(value: f64) -> String {
    if !value.is_finite() {
        return "0".to_string();
    }
    if value.fract() == 0.0 {
        format!("{value:.0}")
    } else {
        let mut rendered = format!("{value:.4}");
        while rendered.contains('.') && rendered.ends_with('0') {
            rendered.pop();
        }
        if rendered.ends_with('.') {
            rendered.pop();
        }
        rendered
    }
}

fn format_duration_ms(ms: i64) -> String {
    format!("{:.1}s", (ms as f64) / 1000.0)
}

fn append_duration(line: &mut String, duration_ms: Option<i64>) {
    if let Some(ms) = duration_ms {
        line.push(' ');
        line.push_str(&format_duration_ms(ms));
    }
}

fn option_i64_text(value: Option<i64>) -> String {
    value.map(|value| value.to_string()).unwrap_or_default()
}

fn option_bool_text(value: Option<bool>) -> String {
    value.map(|value| value.to_string()).unwrap_or_default()
}

fn markdown_header_text(line: &str) -> Option<&str> {
    let hashes = line.chars().take_while(|ch| *ch == '#').count();
    if (1..=6).contains(&hashes) && line.chars().nth(hashes) == Some(' ') {
        Some(line[hashes + 1..].trim())
    } else {
        None
    }
}

fn markdown_hr_line(line: &str) -> bool {
    let trimmed = line.trim();
    trimmed.chars().count() >= 3 && trimmed.chars().all(|ch| matches!(ch, '-' | '*' | '_'))
}

fn unordered_item_text(line: &str) -> Option<&str> {
    ["- ", "* ", "+ "]
        .iter()
        .find_map(|prefix| line.strip_prefix(prefix))
}

fn ordered_item_text(line: &str) -> Option<(String, &str)> {
    let dot = line.find('.')?;
    if dot == 0 || !line[..dot].chars().all(|ch| ch.is_ascii_digit()) {
        return None;
    }
    let rest = line.get(dot + 1..)?;
    let text = rest.strip_prefix(' ')?;
    Some((line[..dot].to_string(), text))
}

fn blockquote_text(line: &str) -> Option<&str> {
    line.strip_prefix('>')
        .map(|text| text.strip_prefix(' ').unwrap_or(text))
}

fn render_code_lines(content: &str, max_width: usize, color_enabled: bool) -> Vec<String> {
    content
        .lines()
        .map(|line| {
            let line = if display_width(line) > max_width {
                let cut = char_index_at_width(line, max_width.saturating_sub(1));
                format!("{}{}", &line[..cut], ELLIPSIS)
            } else {
                line.to_string()
            };
            semantic_style(&line, &[ANSI_DIM, ANSI_CYAN], color_enabled)
        })
        .collect()
}

fn render_markdown_list_item(
    text: &str,
    prefix: &str,
    max_width: usize,
    color_enabled: bool,
) -> Vec<String> {
    let prefix_width = display_width(prefix);
    let styled = convert_inline_markdown(text, color_enabled);
    let wrapped = word_wrap(&styled, max_width.saturating_sub(prefix_width).max(1));
    let indent = " ".repeat(prefix_width);
    wrapped
        .into_iter()
        .enumerate()
        .map(|(idx, line)| {
            if idx == 0 {
                format!("{prefix}{line}")
            } else {
                format!("{indent}{line}")
            }
        })
        .collect()
}

fn convert_inline_markdown(text: &str, color_enabled: bool) -> String {
    let text = replace_delimited(text, "`", "`", |inner| {
        semantic_style(inner, &[ANSI_CYAN], color_enabled)
    });
    let text = replace_delimited(&text, "**", "**", |inner| {
        semantic_header(inner, color_enabled)
    });
    replace_delimited(&text, "*", "*", |inner| {
        semantic_style(inner, &[ANSI_ITALIC], color_enabled)
    })
}

fn replace_delimited<F>(text: &str, start: &str, end: &str, mut convert: F) -> String
where
    F: FnMut(&str) -> String,
{
    let mut out = String::new();
    let mut remaining = text;
    while let Some(open) = remaining.find(start) {
        let after_open = open + start.len();
        let Some(close_rel) = remaining[after_open..].find(end) else {
            break;
        };
        let close = after_open + close_rel;
        if close == after_open {
            break;
        }
        out.push_str(&remaining[..open]);
        out.push_str(&convert(&remaining[after_open..close]));
        remaining = &remaining[close + end.len()..];
    }
    out.push_str(remaining);
    out
}
