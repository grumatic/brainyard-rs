use std::path::PathBuf;
use std::process::Command;

fn native_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("by-cli should live under crates/")
        .parent()
        .expect("crates/ should live under native/by-rs/")
        .to_path_buf()
}

#[test]
fn bedrock_live_smoke_script_uses_working_global_haiku_default() {
    let script = native_root().join("scripts/bedrock-live-smoke.sh");
    let source = std::fs::read_to_string(&script).expect("smoke script should be readable");

    assert!(source.contains(
        r#"model="${BY_RS_BEDROCK_MODEL:-global.anthropic.claude-haiku-4-5-20251001-v1:0}""#
    ));
    assert!(source.contains(
        "BY_RS_BEDROCK_MODEL=MODEL     Default: global.anthropic.claude-haiku-4-5-20251001-v1:0"
    ));
}

#[test]
fn bedrock_live_smoke_script_keeps_aws_credentials_visible_under_isolated_home() {
    let script = native_root().join("scripts/bedrock-live-smoke.sh");
    let source = std::fs::read_to_string(&script).expect("smoke script should be readable");

    assert!(source.contains("configure_aws_env_for_isolated_home"));
    assert!(source.contains("AWS_CONFIG_FILE"));
    assert!(source.contains("AWS_SHARED_CREDENTIALS_FILE"));
}

#[test]
fn bedrock_live_smoke_script_is_valid_bash() {
    let script = native_root().join("scripts/bedrock-live-smoke.sh");
    let status = Command::new("bash")
        .arg("-n")
        .arg(&script)
        .status()
        .expect("bash -n should run");

    assert!(status.success());
}
