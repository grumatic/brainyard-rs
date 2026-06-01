use by_tui::{render_static_frame, StaticFrame};

#[test]
fn renders_legacy_shaped_static_tui_chrome() {
    let frame = render_static_frame(&StaticFrame {
        rows: 12,
        cols: 56,
        agent: "coact-agent".to_string(),
        model: "bedrock:amazon.nova-lite-v1:0".to_string(),
        status: "idle".to_string(),
    });

    let lines: Vec<_> = frame.lines().collect();
    assert_eq!(lines.len(), 12);
    assert!(lines.iter().all(|line| line.chars().count() <= 56));
    assert!(frame.contains("Brainyard by-rs"));
    assert!(frame.contains("agent coact-agent"));
    assert!(frame.contains("model bedrock:amazon.nova-lite-v1:0"));
    assert!(frame.contains("❯ "));
    assert!(frame.contains(" 0:main0*"));
    assert!(frame.contains("idle │ 0 calls │ 0 tokens │ $0.0000"));
}

#[test]
fn clamps_tiny_dimensions_to_a_viable_chrome_snapshot() {
    let frame = render_static_frame(&StaticFrame {
        rows: 2,
        cols: 8,
        agent: "a".to_string(),
        model: "m".to_string(),
        status: "idle".to_string(),
    });

    let lines: Vec<_> = frame.lines().collect();
    assert_eq!(lines.len(), 5);
    assert!(lines.iter().all(|line| line.chars().count() <= 20));
}
