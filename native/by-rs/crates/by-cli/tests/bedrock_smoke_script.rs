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

#[test]
fn bedrock_live_smoke_script_opts_into_ask_test_projection_options() {
    let script = native_root().join("scripts/bedrock-live-smoke.sh");
    let temp = tempfile::tempdir().expect("tempdir");
    let fake_bin = temp.path().join("by-rs");
    std::fs::write(
        &fake_bin,
        r#"#!/usr/bin/env bash
printf 'ALLOW=%s\n' "${BY_RS_ALLOW_ASK_TEST_OPTIONS:-}"
printf 'ARGS=%s\n' "$*"
"#,
    )
    .expect("write fake by-rs");

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut permissions = std::fs::metadata(&fake_bin)
            .expect("fake metadata")
            .permissions();
        permissions.set_mode(0o755);
        std::fs::set_permissions(&fake_bin, permissions).expect("chmod fake by-rs");
    }

    let output = Command::new("bash")
        .arg(&script)
        .arg("--dry-run")
        .arg("--bin")
        .arg(&fake_bin)
        .env("BY_NO_DOTENV", "1")
        .output()
        .expect("smoke script should run with fake binary");

    assert!(
        output.status.success(),
        "script failed\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("ALLOW=1"), "{stdout}");
    assert!(stdout.contains("ARGS=ask"), "{stdout}");
    assert!(stdout.contains("--dry-run"), "{stdout}");
    assert!(stdout.contains("--max-tokens"), "{stdout}");
}

#[test]
fn bedrock_live_smoke_script_does_not_pass_removed_ask_live_flag() {
    let script = native_root().join("scripts/bedrock-live-smoke.sh");
    let temp = tempfile::tempdir().expect("tempdir");
    let fake_bin = temp.path().join("by-rs");
    std::fs::write(
        &fake_bin,
        r#"#!/usr/bin/env bash
printf 'ARGS=%s\n' "$*"
"#,
    )
    .expect("write fake by-rs");

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut permissions = std::fs::metadata(&fake_bin)
            .expect("fake metadata")
            .permissions();
        permissions.set_mode(0o755);
        std::fs::set_permissions(&fake_bin, permissions).expect("chmod fake by-rs");
    }

    let output = Command::new("bash")
        .arg(&script)
        .arg("--live")
        .arg("--bin")
        .arg(&fake_bin)
        .env("BY_NO_DOTENV", "1")
        .output()
        .expect("smoke script should run with fake binary");

    assert!(
        output.status.success(),
        "script failed\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    let args_line = stdout
        .lines()
        .find(|line| line.starts_with("ARGS="))
        .expect("fake binary should print args");
    assert!(!args_line.contains("--live"), "{stdout}");
}
