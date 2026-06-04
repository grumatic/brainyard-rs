use regex::Regex;

use std::{
    env,
    ffi::OsStr,
    io::{Read, Write},
    path::{Path, PathBuf},
    process::{Command, Stdio},
    sync::{Mutex, MutexGuard},
    thread,
    time::{Duration, Instant},
};

const ORACLE_ENV: &str = "BY_RS_PARITY_ORACLE";
static PARITY_COMMAND_LOCK: Mutex<()> = Mutex::new(());

#[derive(Debug)]
struct CommandResult {
    status_code: Option<i32>,
    stdout: String,
    stderr: String,
    timed_out: bool,
}

#[test]
fn help_surfaces_match_oracle() {
    let Some(oracle) = oracle_binary() else {
        return;
    };
    let _guard = parity_command_lock();

    for args in [
        &["--help"][..],
        &["run", "--help"],
        &["ask", "--help"],
        &["agents", "--help"],
        &["models", "--help"],
        &["sessions", "--help"],
    ] {
        assert_command_matches_oracle(&oracle, args, "");
    }
}

#[test]
fn run_quit_contract_matches_oracle() {
    let Some(oracle) = oracle_binary() else {
        return;
    };
    let _guard = parity_command_lock();

    assert_command_matches_oracle(&oracle, &["run", "--inline"], "/quit\n");
}

#[test]
fn run_blank_input_contract_matches_oracle() {
    let Some(oracle) = oracle_binary() else {
        return;
    };
    let _guard = parity_command_lock();

    assert_command_matches_oracle(&oracle, &["run", "--inline"], "\n/quit\n");
}

#[test]
fn run_config_listing_matches_oracle() {
    let Some(oracle) = oracle_binary() else {
        return;
    };
    let _guard = parity_command_lock();

    assert_command_matches_oracle(&oracle, &["run", "--inline"], "/config\n/quit\n");
}

#[test]
fn run_memory_and_init_help_match_oracle() {
    let Some(oracle) = oracle_binary() else {
        return;
    };
    let _guard = parity_command_lock();

    assert_command_matches_oracle(
        &oracle,
        &["run", "--inline"],
        "/memory help\n/init help\n/quit\n",
    );
}

#[test]
fn run_init_show_without_docs_matches_oracle() {
    let Some(oracle) = oracle_binary() else {
        return;
    };
    let _guard = parity_command_lock();

    assert_command_matches_oracle(&oracle, &["run", "--inline"], "/init show\n/quit\n");
}

#[test]
fn run_init_show_with_docs_matches_oracle() {
    let Some(oracle) = oracle_binary() else {
        return;
    };
    let _guard = parity_command_lock();

    let oracle_home = tempfile::tempdir().expect("oracle HOME tempdir");
    let rust_home = tempfile::tempdir().expect("by-rs HOME tempdir");
    write_init_show_fixture(oracle_home.path());
    write_init_show_fixture(rust_home.path());

    let expected = run_command(
        &oracle,
        ["run", "--inline"],
        "/init show\n/quit\n",
        oracle_home.path(),
    );
    let by_rs = by_rs_binary();
    let actual = run_command(
        &by_rs,
        ["run", "--inline"],
        "/init show\n/quit\n",
        rust_home.path(),
    );

    assert_eq!(expected.status_code, actual.status_code);
    assert_eq!(expected.timed_out, actual.timed_out);
    assert_eq!(
        normalize_output(&expected.stdout, oracle_home.path(), rust_home.path()),
        normalize_output(&actual.stdout, oracle_home.path(), rust_home.path()),
        "stdout mismatch for non-empty /init show"
    );
    assert_eq!(
        normalize_output(&expected.stderr, oracle_home.path(), rust_home.path()),
        normalize_output(&actual.stderr, oracle_home.path(), rust_home.path()),
        "stderr mismatch for non-empty /init show"
    );
}

#[test]
fn run_init_list_snapshots_empty_matches_oracle() {
    let Some(oracle) = oracle_binary() else {
        return;
    };
    let _guard = parity_command_lock();

    assert_command_matches_oracle(
        &oracle,
        &["run", "--inline"],
        "/init list-snapshots\n/quit\n",
    );
}

#[test]
fn run_init_list_snapshots_non_empty_matches_oracle() {
    let Some(oracle) = oracle_binary() else {
        return;
    };
    let _guard = parity_command_lock();

    let oracle_home = tempfile::tempdir().expect("oracle HOME tempdir");
    let rust_home = tempfile::tempdir().expect("by-rs HOME tempdir");
    write_init_revert_fixture(oracle_home.path());
    write_init_revert_fixture(rust_home.path());

    let expected = run_command(
        &oracle,
        ["run", "--inline"],
        "/init list-snapshots\n/quit\n",
        oracle_home.path(),
    );
    let by_rs = by_rs_binary();
    let actual = run_command(
        &by_rs,
        ["run", "--inline"],
        "/init list-snapshots\n/quit\n",
        rust_home.path(),
    );

    assert_eq!(expected.status_code, actual.status_code);
    assert_eq!(expected.timed_out, actual.timed_out);
    assert_eq!(
        normalize_output(&expected.stdout, oracle_home.path(), rust_home.path()),
        normalize_output(&actual.stdout, oracle_home.path(), rust_home.path()),
        "stdout mismatch for non-empty /init list-snapshots"
    );
    assert_eq!(
        normalize_output(&expected.stderr, oracle_home.path(), rust_home.path()),
        normalize_output(&actual.stderr, oracle_home.path(), rust_home.path()),
        "stderr mismatch for non-empty /init list-snapshots"
    );
}

#[test]
fn run_init_list_snapshots_positional_limit_matches_oracle() {
    let Some(oracle) = oracle_binary() else {
        return;
    };
    let _guard = parity_command_lock();

    let oracle_home = tempfile::tempdir().expect("oracle HOME tempdir");
    let rust_home = tempfile::tempdir().expect("by-rs HOME tempdir");
    write_init_list_snapshots_fixture(oracle_home.path());
    write_init_list_snapshots_fixture(rust_home.path());

    let expected = run_command(
        &oracle,
        ["run", "--inline"],
        "/init list-snapshots 1\n/quit\n",
        oracle_home.path(),
    );
    let by_rs = by_rs_binary();
    let actual = run_command(
        &by_rs,
        ["run", "--inline"],
        "/init list-snapshots 1\n/quit\n",
        rust_home.path(),
    );

    assert_eq!(expected.status_code, actual.status_code);
    assert_eq!(expected.timed_out, actual.timed_out);
    assert_eq!(
        normalize_output(&expected.stdout, oracle_home.path(), rust_home.path()),
        normalize_output(&actual.stdout, oracle_home.path(), rust_home.path()),
        "stdout mismatch for /init list-snapshots positional limit"
    );
    assert_eq!(
        normalize_output(&expected.stderr, oracle_home.path(), rust_home.path()),
        normalize_output(&actual.stderr, oracle_home.path(), rust_home.path()),
        "stderr mismatch for /init list-snapshots positional limit"
    );
}

#[test]
fn run_init_show_scope_before_subcommand_matches_oracle() {
    let Some(oracle) = oracle_binary() else {
        return;
    };
    let _guard = parity_command_lock();

    let oracle_home = tempfile::tempdir().expect("oracle HOME tempdir");
    let rust_home = tempfile::tempdir().expect("by-rs HOME tempdir");
    write_init_show_fixture(oracle_home.path());
    write_init_show_fixture(rust_home.path());

    let expected = run_command(
        &oracle,
        ["run", "--inline"],
        "/init --scope :project show\n/quit\n",
        oracle_home.path(),
    );
    let by_rs = by_rs_binary();
    let actual = run_command(
        &by_rs,
        ["run", "--inline"],
        "/init --scope :project show\n/quit\n",
        rust_home.path(),
    );

    assert_eq!(expected.status_code, actual.status_code);
    assert_eq!(expected.timed_out, actual.timed_out);
    assert_eq!(
        normalize_output(&expected.stdout, oracle_home.path(), rust_home.path()),
        normalize_output(&actual.stdout, oracle_home.path(), rust_home.path()),
        "stdout mismatch for /init --scope :project show"
    );
    assert_eq!(
        normalize_output(&expected.stderr, oracle_home.path(), rust_home.path()),
        normalize_output(&actual.stderr, oracle_home.path(), rust_home.path()),
        "stderr mismatch for /init --scope :project show"
    );
}

#[test]
fn run_init_list_snapshots_scope_before_subcommand_matches_oracle() {
    let Some(oracle) = oracle_binary() else {
        return;
    };
    let _guard = parity_command_lock();

    let oracle_home = tempfile::tempdir().expect("oracle HOME tempdir");
    let rust_home = tempfile::tempdir().expect("by-rs HOME tempdir");
    write_init_list_snapshots_fixture(oracle_home.path());
    write_init_list_snapshots_fixture(rust_home.path());

    let expected = run_command(
        &oracle,
        ["run", "--inline"],
        "/init --scope :both list-snapshots 1\n/quit\n",
        oracle_home.path(),
    );
    let by_rs = by_rs_binary();
    let actual = run_command(
        &by_rs,
        ["run", "--inline"],
        "/init --scope :both list-snapshots 1\n/quit\n",
        rust_home.path(),
    );

    assert_eq!(expected.status_code, actual.status_code);
    assert_eq!(expected.timed_out, actual.timed_out);
    assert_eq!(
        normalize_output(&expected.stdout, oracle_home.path(), rust_home.path()),
        normalize_output(&actual.stdout, oracle_home.path(), rust_home.path()),
        "stdout mismatch for /init list-snapshots with leading scope flag"
    );
    assert_eq!(
        normalize_output(&expected.stderr, oracle_home.path(), rust_home.path()),
        normalize_output(&actual.stderr, oracle_home.path(), rust_home.path()),
        "stderr mismatch for /init list-snapshots with leading scope flag"
    );
}

#[test]
fn run_init_list_snapshots_default_limit_matches_oracle() {
    let Some(oracle) = oracle_binary() else {
        return;
    };
    let _guard = parity_command_lock();

    let oracle_home = tempfile::tempdir().expect("oracle HOME tempdir");
    let rust_home = tempfile::tempdir().expect("by-rs HOME tempdir");
    write_many_init_snapshots_fixture(oracle_home.path(), 11);
    write_many_init_snapshots_fixture(rust_home.path(), 11);

    let expected = run_command(
        &oracle,
        ["run", "--inline"],
        "/init list-snapshots\n/quit\n",
        oracle_home.path(),
    );
    let by_rs = by_rs_binary();
    let actual = run_command(
        &by_rs,
        ["run", "--inline"],
        "/init list-snapshots\n/quit\n",
        rust_home.path(),
    );

    assert_eq!(expected.status_code, actual.status_code);
    assert_eq!(expected.timed_out, actual.timed_out);
    assert_eq!(
        normalize_output(&expected.stdout, oracle_home.path(), rust_home.path()),
        normalize_output(&actual.stdout, oracle_home.path(), rust_home.path()),
        "stdout mismatch for /init list-snapshots default limit"
    );
    assert_eq!(
        normalize_output(&expected.stderr, oracle_home.path(), rust_home.path()),
        normalize_output(&actual.stderr, oracle_home.path(), rust_home.path()),
        "stderr mismatch for /init list-snapshots default limit"
    );
}

#[test]
fn run_init_revert_missing_arg_matches_oracle() {
    let Some(oracle) = oracle_binary() else {
        return;
    };
    let _guard = parity_command_lock();

    assert_command_matches_oracle(&oracle, &["run", "--inline"], "/init revert\n/quit\n");
}

#[test]
fn run_init_revert_missing_snapshot_matches_oracle() {
    let Some(oracle) = oracle_binary() else {
        return;
    };
    let _guard = parity_command_lock();

    assert_command_matches_oracle(
        &oracle,
        &["run", "--inline"],
        "/init revert missing.md\n/quit\n",
    );
}

#[test]
fn run_init_revert_success_matches_oracle() {
    let Some(oracle) = oracle_binary() else {
        return;
    };
    let _guard = parity_command_lock();

    let oracle_home = tempfile::tempdir().expect("oracle HOME tempdir");
    let rust_home = tempfile::tempdir().expect("by-rs HOME tempdir");
    let oracle_snapshot = write_init_revert_fixture(oracle_home.path());
    let rust_snapshot = write_init_revert_fixture(rust_home.path());

    let expected = run_command(
        &oracle,
        ["run", "--inline"],
        &format!("/init revert {}\n/quit\n", oracle_snapshot.display()),
        oracle_home.path(),
    );
    let by_rs = by_rs_binary();
    let actual = run_command(
        &by_rs,
        ["run", "--inline"],
        &format!("/init revert {}\n/quit\n", rust_snapshot.display()),
        rust_home.path(),
    );

    assert_eq!(expected.status_code, actual.status_code);
    assert_eq!(expected.timed_out, actual.timed_out);
    assert_eq!(
        normalize_output(&expected.stdout, oracle_home.path(), rust_home.path()),
        normalize_output(&actual.stdout, oracle_home.path(), rust_home.path()),
        "stdout mismatch for successful /init revert"
    );
    assert_eq!(
        normalize_output(&expected.stderr, oracle_home.path(), rust_home.path()),
        normalize_output(&actual.stderr, oracle_home.path(), rust_home.path()),
        "stderr mismatch for successful /init revert"
    );
    assert_eq!(
        std::fs::read_to_string(oracle_home.path().join(".brainyard/BRAINYARD.md")).unwrap(),
        std::fs::read_to_string(rust_home.path().join(".brainyard/BRAINYARD.md")).unwrap()
    );
}

fn write_init_revert_fixture(home: &Path) -> PathBuf {
    let brainyard_dir = home.join(".brainyard");
    let snapshot_dir = brainyard_dir.join("agents/init-agent/snapshots");
    std::fs::create_dir_all(&snapshot_dir).expect("create init snapshot dir");
    std::fs::write(brainyard_dir.join("BRAINYARD.md"), "# Current\n").expect("write current doc");
    let snapshot = snapshot_dir.join("20260102-030405-project-test-snapshot.md");
    std::fs::write(&snapshot, "# Restored\n").expect("write restore snapshot");
    snapshot
}

fn write_init_show_fixture(home: &Path) {
    let brainyard_dir = home.join(".brainyard");
    std::fs::create_dir_all(&brainyard_dir).expect("create brainyard dir");
    std::fs::write(
        brainyard_dir.join("BRAINYARD.md"),
        "# Brainyard\n\n## Notes\nProject note\n",
    )
    .expect("write brainyard doc");
}

fn write_init_list_snapshots_fixture(home: &Path) {
    let snapshot_dir = home.join(".brainyard/agents/init-agent/snapshots");
    std::fs::create_dir_all(&snapshot_dir).expect("create init snapshot dir");
    std::fs::write(
        snapshot_dir.join("20260102-030405-project-new-snapshot.md"),
        "# New\n",
    )
    .expect("write newer snapshot");
    std::fs::write(
        snapshot_dir.join("20250102-030405-project-old-snapshot.md"),
        "# Old\n",
    )
    .expect("write older snapshot");
}

fn write_many_init_snapshots_fixture(home: &Path, count: usize) {
    let snapshot_dir = home.join(".brainyard/agents/init-agent/snapshots");
    std::fs::create_dir_all(&snapshot_dir).expect("create init snapshot dir");
    for index in 1..=count {
        let filename = format!("202601{:02}-030405-project-snapshot-{:02}.md", index, index);
        std::fs::write(snapshot_dir.join(filename), format!("# Snapshot {index}\n"))
            .expect("write init snapshot");
    }
}

#[test]
fn run_help_command_matches_oracle() {
    let Some(oracle) = oracle_binary() else {
        return;
    };
    let _guard = parity_command_lock();

    assert_command_matches_oracle(&oracle, &["run", "--inline"], "/help\n/quit\n");
}

#[test]
fn run_live_free_slash_commands_match_oracle() {
    let Some(oracle) = oracle_binary() else {
        return;
    };
    let _guard = parity_command_lock();

    for stdin in [
        "/clear\n/quit\n",
        "/history\n/quit\n",
        "/model\n/quit\n",
        "/queue\n/quit\n",
        "/todo\n/quit\n",
        "/usage\n/quit\n",
        "/verbose\n/quit\n",
    ] {
        assert_command_matches_oracle(&oracle, &["run", "--inline"], stdin);
    }
}

#[test]
fn run_argument_slash_commands_match_oracle() {
    let Some(oracle) = oracle_binary() else {
        return;
    };
    let _guard = parity_command_lock();

    assert_command_matches_oracle(
        &oracle,
        &["run", "--inline"],
        "/verbose quiet
/verbose
/verbose verbose
/verbose nope
/usage 5
/usage --breakdown
/usage nope
/history 5
/quit
",
    );
}

#[test]
fn run_static_argument_and_colon_commands_match_oracle() {
    let Some(oracle) = oracle_binary() else {
        return;
    };
    let _guard = parity_command_lock();

    assert_command_matches_oracle(
        &oracle,
        &["run", "--inline"],
        "/help x
/clear x
/queue x
/todo x
:q
/quit
",
    );
}

#[test]

fn run_effort_and_control_slash_commands_match_oracle() {
    let Some(oracle) = oracle_binary() else {
        return;
    };
    let _guard = parity_command_lock();

    assert_command_matches_oracle(
        &oracle,
        &["run", "--inline"],
        concat!(
            "/effort medium\n",
            "/effort\n",
            "/effort low\n",
            "/effort\n",
            "/effort high\n",
            "/effort\n",
            "/effort nope\n",
            "/continue\n",
            "/continue 2\n",
            "/pause\n",
            "/resume\n",
            "/activity\n",
            "/log\n",
            "/compact\n",
            "/compact 0.5\n",
            "/quit\n",
        ),
    );
}

#[test]
fn run_session_slash_commands_match_oracle() {
    let Some(oracle) = oracle_binary() else {
        return;
    };
    let _guard = parity_command_lock();

    assert_command_matches_oracle(
        &oracle,
        &["run", "--inline"],
        "/session\n/session 1\n/quit\n",
    );
}

#[test]
fn run_agent_status_slash_commands_match_oracle() {
    let Some(oracle) = oracle_binary() else {
        return;
    };
    let _guard = parity_command_lock();
    assert_command_matches_oracle(
        &oracle,
        &["run", "--inline"],
        concat!("/agent\n", "/agent status\n", "/status\n", "/quit\n"),
    );
}

#[test]
fn run_more_live_free_slash_commands_match_oracle() {
    let Some(oracle) = oracle_binary() else {
        return;
    };
    let _guard = parity_command_lock();

    assert_command_matches_oracle(
        &oracle,
        &["run", "--inline"],
        concat!(
            "/task\n",
            "/task status\n",
            "/task help\n",
            "/allow-path\n",
            "/capture\n",
            "/sandbox\n",
            "/sandbox status\n",
            "/scrollback\n",
            "/popup\n",
            "/quit\n",
        ),
    );
}

#[test]
fn run_model_switch_slash_commands_match_oracle() {
    let Some(oracle) = oracle_binary() else {
        return;
    };
    let _guard = parity_command_lock();
    assert_command_matches_oracle(
        &oracle,
        &["run", "--inline"],
        concat!("/model 1\n", "/model nope\n", "/model\n", "/quit\n"),
    );
}

#[test]
fn run_mcp_slash_commands_match_oracle() {
    let Some(oracle) = oracle_binary() else {
        return;
    };
    let _guard = parity_command_lock();
    assert_command_matches_oracle(
        &oracle,
        &["run", "--inline"],
        concat!("/mcp\n", "/mcp does-not-exist\n", "/quit\n"),
    );
}

#[test]
fn run_config_unknown_key_matches_oracle() {
    let Some(oracle) = oracle_binary() else {
        return;
    };
    let _guard = parity_command_lock();
    assert_command_matches_oracle(&oracle, &["run", "--inline"], "/config foo\n/quit\n");
}

#[test]
fn run_config_key_lookup_matches_oracle() {
    let Some(oracle) = oracle_binary() else {
        return;
    };
    let _guard = parity_command_lock();
    assert_command_matches_oracle(
        &oracle,
        &["run", "--inline"],
        concat!(
            "/config max-iterations\n",
            "/config show-llm-streaming\n",
            "/quit\n"
        ),
    );
}

#[test]
fn run_unknown_slash_command_matches_oracle() {
    let Some(oracle) = oracle_binary() else {
        return;
    };
    let _guard = parity_command_lock();

    assert_command_matches_oracle(&oracle, &["run", "--inline"], "/nope\n/quit\n");
}

#[test]
fn run_ollama_default_model_matches_oracle() {
    let Some(oracle) = oracle_binary() else {
        return;
    };
    let _guard = parity_command_lock();

    assert_command_matches_oracle(&oracle, &["run", "--inline", "-p", "ollama"], "/quit\n");
}

#[test]
fn run_bedrock_explicit_model_matches_oracle() {
    let Some(oracle) = oracle_binary() else {
        return;
    };
    let _guard = parity_command_lock();

    assert_command_matches_oracle(
        &oracle,
        &[
            "run",
            "--inline",
            "-p",
            "bedrock",
            "-m",
            "global.anthropic.claude-haiku-4-5-20251001-v1:0",
        ],
        "/quit\n",
    );
}

#[test]
fn run_provider_setup_errors_match_oracle_contract() {
    let Some(oracle) = oracle_binary() else {
        return;
    };
    let _guard = parity_command_lock();

    for provider in ["anthropic", "openai", "bedrock"] {
        assert_run_setup_error_matches_oracle(&oracle, provider);
    }
}

fn parity_command_lock() -> MutexGuard<'static, ()> {
    match PARITY_COMMAND_LOCK.lock() {
        Ok(guard) => guard,
        Err(poisoned) => poisoned.into_inner(),
    }
}

fn assert_command_matches_oracle(oracle: &Path, args: &[&str], stdin: &str) {
    let oracle_home = tempfile::tempdir().expect("oracle HOME tempdir");
    let rust_home = tempfile::tempdir().expect("by-rs HOME tempdir");

    let expected = run_command(oracle, args, stdin, oracle_home.path());
    let by_rs = by_rs_binary();
    let actual = run_command(&by_rs, args, stdin, rust_home.path());

    assert_eq!(
        expected.status_code, actual.status_code,
        "status mismatch for args {args:?}",
    );
    assert_eq!(
        expected.timed_out, actual.timed_out,
        "timeout mismatch for args {args:?}",
    );
    assert_eq!(
        normalize_output(&expected.stdout, oracle_home.path(), rust_home.path()),
        normalize_output(&actual.stdout, oracle_home.path(), rust_home.path()),
        "stdout mismatch for args {args:?}",
    );
    assert_eq!(
        normalize_output(&expected.stderr, oracle_home.path(), rust_home.path()),
        normalize_output(&actual.stderr, oracle_home.path(), rust_home.path()),
        "stderr mismatch for args {args:?}",
    );
}

fn assert_run_setup_error_matches_oracle(oracle: &Path, provider: &str) {
    let args = ["run", "--inline", "-p", provider];
    let oracle_home = tempfile::tempdir().expect("oracle HOME tempdir");
    let rust_home = tempfile::tempdir().expect("by-rs HOME tempdir");

    let expected = run_command(oracle, args, "/quit\n", oracle_home.path());
    let by_rs = by_rs_binary();
    let actual = run_command(&by_rs, args, "/quit\n", rust_home.path());

    assert_eq!(
        expected.status_code, actual.status_code,
        "status mismatch for provider {provider}: expected {expected:?}, actual {actual:?}",
    );
    assert_eq!(
        normalize_output(&expected.stdout, oracle_home.path(), rust_home.path()),
        normalize_output(&actual.stdout, oracle_home.path(), rust_home.path()),
        "stdout mismatch for provider {provider}",
    );
    assert_eq!(
        expected.timed_out, actual.timed_out,
        "timeout mismatch for provider {provider}",
    );
    assert_eq!(
        stderr_cause(&expected.stderr),
        stderr_cause(&actual.stderr),
        "stderr cause mismatch for provider {provider}: expected {expected:?}, actual {actual:?}",
    );
    assert!(
        actual.stderr.contains("** ERROR: **"),
        "by-rs must keep the Clojure fatal-error header for provider {provider}: {actual:?}",
    );
}

fn stderr_cause(stderr: &str) -> Option<&str> {
    stderr
        .lines()
        .find_map(|line| line.strip_prefix(" :cause "))
}

fn oracle_binary() -> Option<PathBuf> {
    let Some(path) = env::var_os(ORACLE_ENV).map(PathBuf::from) else {
        eprintln!("skipping by/by-rs parity test: {ORACLE_ENV} is not set");
        return None;
    };
    if path.is_file() {
        Some(path)
    } else {
        panic!(
            "{ORACLE_ENV} points to a non-file oracle binary: {}",
            path.display()
        );
    }
}

fn by_rs_binary() -> PathBuf {
    assert_cmd::cargo::cargo_bin("by-rs")
}

fn run_command<I, S>(binary: &Path, args: I, stdin: &str, home: &Path) -> CommandResult
where
    I: IntoIterator<Item = S>,
    S: AsRef<OsStr>,
{
    let mut child = Command::new(binary)
        .args(args)
        .env_clear()
        .env("HOME", home)
        .env("PATH", env::var_os("PATH").unwrap_or_default())
        .env("BY_NO_DOTENV", "1")
        .env("NO_COLOR", "1")
        .env("TERM", "dumb")
        .env("COLUMNS", "120")
        .env("BRAINYARD_SESSION_ID", "agt-parity")
        .current_dir(home)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn parity command");

    let mut stdout = child.stdout.take().expect("capture command stdout");
    let mut stderr = child.stderr.take().expect("capture command stderr");

    if !stdin.is_empty() {
        child
            .stdin
            .as_mut()
            .expect("open command stdin")
            .write_all(stdin.as_bytes())
            .expect("write command stdin");
    }
    drop(child.stdin.take());

    let deadline = Instant::now() + Duration::from_secs(45);
    let mut timed_out = false;
    let status = loop {
        if let Some(status) = child.try_wait().expect("poll parity command") {
            break status;
        }
        if Instant::now() >= deadline {
            timed_out = true;
            child.kill().expect("kill timed-out parity command");
            break child.wait().expect("wait for killed parity command");
        }
        thread::sleep(Duration::from_millis(25));
    };

    let mut stdout_bytes = Vec::new();
    let mut stderr_bytes = Vec::new();
    stdout
        .read_to_end(&mut stdout_bytes)
        .expect("read command stdout");
    stderr
        .read_to_end(&mut stderr_bytes)
        .expect("read command stderr");

    CommandResult {
        status_code: status.code(),
        stdout: String::from_utf8_lossy(&stdout_bytes).into_owned(),
        stderr: String::from_utf8_lossy(&stderr_bytes).into_owned(),
        timed_out,
    }
}

fn normalize_output(output: &str, oracle_home: &Path, rust_home: &Path) -> String {
    let mut normalized = output.replace("\r\n", "\n");
    normalized = normalized.replace(&oracle_home.display().to_string(), "$HOME");
    normalized = normalized.replace(&rust_home.display().to_string(), "$HOME");
    normalized = normalize_version_lines(&normalized);
    normalized = normalize_agent_instance_ids(&normalized);
    normalized = normalize_init_snapshot_timestamps(&normalized);
    normalized
}

fn normalize_init_snapshot_timestamps(output: &str) -> String {
    Regex::new(r"\d{8}-\d{6}-project-revert-test-snapshot\.md")
        .expect("init snapshot timestamp regex")
        .replace_all(output, "SNAPSHOT_TS-project-revert-test-snapshot.md")
        .into_owned()
}

fn normalize_agent_instance_ids(output: &str) -> String {
    Regex::new(r"coact-agent/[a-z]+-[a-z]+-\d{4}")
        .expect("agent instance id regex")
        .replace_all(output, "coact-agent/$INSTANCE")
        .into_owned()
}

fn normalize_version_lines(output: &str) -> String {
    output
        .lines()
        .map(|line| {
            let trimmed = line.trim_start();
            if trimmed.starts_with('v') || trimmed.starts_with("by v") {
                "$VERSION"
            } else {
                line
            }
        })
        .collect::<Vec<_>>()
        .join("\n")
        + if output.ends_with('\n') { "\n" } else { "" }
}
