#![forbid(unsafe_code)]

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

fn status_label(status: &str) -> &str {
    let status = status.trim();
    if status.is_empty() {
        "idle"
    } else {
        status
    }
}

fn separator(cols: usize) -> String {
    "─".repeat(cols)
}

fn fit(text: &str, cols: usize) -> String {
    text.chars().take(cols).collect()
}
