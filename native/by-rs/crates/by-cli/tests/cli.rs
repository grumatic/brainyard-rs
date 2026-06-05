use assert_cmd::Command as AssertCommand;
use by_persist::list_sessions;
use predicates::prelude::*;
use rusqlite::Connection;
use std::ffi::OsString;
use std::io::{Read, Write};
use std::net::TcpListener;
use std::path::Path;
use std::process::{Command as StdCommand, Stdio};
use std::thread;
use std::time::Duration;

struct Command(AssertCommand);

impl Command {
    fn cargo_bin<S: AsRef<str>>(name: S) -> Result<Self, assert_cmd::cargo::CargoError> {
        let mut command = AssertCommand::cargo_bin(name.as_ref())?;
        if name.as_ref() == "by-rs" {
            command.env("BY_RS_ALLOW_ASK_TEST_OPTIONS", "1");
        }
        Ok(Self(command))
    }
}

impl std::ops::Deref for Command {
    type Target = AssertCommand;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl std::ops::DerefMut for Command {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.0
    }
}

const TMUX_NEED_SESSION_GUIDANCE: &str =
    "You passed --with-tmux, but you're not currently inside a tmux session.
For tmux side panes (activity, log) and popup dialogs, start a tmux
session and re-run `by` from inside it:

    tmux new -s brainyard
    by --with-tmux

Or drop --with-tmux to run the in-process TUI without tmux integration:

    by\n";
const TMUX_NEED_TMUX_GUIDANCE: &str = "You passed --with-tmux, but `tmux` is not on $PATH.
Install tmux, then re-run from inside a tmux session:

    # macOS
    brew install tmux
    # Debian/Ubuntu
    sudo apt-get install tmux

Or drop --with-tmux to run the in-process TUI without tmux integration:

    by\n";
const TMUX_SERVER_DEAD_GUIDANCE: &str =
    "You passed --with-tmux and $TMUX is set, but the tmux server isn't
responding (it may have been killed or the system was suspended).
Start a fresh tmux session:

    tmux new -s brainyard
    by --with-tmux\n";

fn assert_json_error(args: &[&str], expected: &str) {
    let assert = Command::cargo_bin("by-rs")
        .unwrap()
        .args(args)
        .assert()
        .success();
    let stdout = String::from_utf8(assert.get_output().stdout.clone()).unwrap();
    let value: serde_json::Value = serde_json::from_str(&stdout).unwrap();

    assert_eq!(value["error"], expected);
}

fn link_fake_executable(path: &Path, target: &str) {
    #[cfg(unix)]
    std::os::unix::fs::symlink(target, path).unwrap();

    #[cfg(not(unix))]
    std::fs::copy(target, path).unwrap();
}

fn write_fake_executable(path: &Path, body: &str) {
    std::fs::write(path, body).unwrap();

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut permissions = std::fs::metadata(path).unwrap().permissions();
        permissions.set_mode(0o755);
        std::fs::set_permissions(path, permissions).unwrap();
    }
}

fn prepend_path(path: &Path) -> OsString {
    let mut paths = vec![path.to_path_buf()];
    if let Some(existing) = std::env::var_os("PATH") {
        paths.extend(std::env::split_paths(&existing));
    }
    std::env::join_paths(paths).unwrap()
}

fn system_binary(candidates: &[&'static str]) -> &'static str {
    candidates
        .iter()
        .copied()
        .find(|candidate| Path::new(candidate).exists())
        .expect("expected a system binary for tmux probe fixture")
}

fn write_session_meta(home: &Path, session_id: &str, meta: &str) {
    let session_dir = home.join(".brainyard/sessions").join(session_id);
    std::fs::create_dir_all(&session_dir).unwrap();
    std::fs::write(session_dir.join("meta.edn"), meta).unwrap();
}

fn read_session_meta(home: &Path, session_id: &str) -> String {
    std::fs::read_to_string(
        home.join(".brainyard/sessions")
            .join(session_id)
            .join("meta.edn"),
    )
    .unwrap()
}

fn read_session_messages(home: &Path, session_id: &str) -> String {
    std::fs::read_to_string(
        home.join(".brainyard/sessions")
            .join(session_id)
            .join("messages.log"),
    )
    .unwrap()
}

fn spawn_openai_compatible_fixture(response_text: &str) -> (String, thread::JoinHandle<String>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let base_url = format!("http://{}/v1", listener.local_addr().unwrap());
    let response_text = response_text.to_string();
    let handle = thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        let mut request = Vec::new();
        let mut buffer = [0_u8; 4096];
        let header_end = loop {
            let read = stream.read(&mut buffer).unwrap();
            if read == 0 {
                panic!("openai-compatible fixture connection closed before headers");
            }
            request.extend_from_slice(&buffer[..read]);
            if let Some(header_end) = request.windows(4).position(|window| window == b"\r\n\r\n") {
                break header_end;
            }
        };
        let headers = String::from_utf8_lossy(&request[..header_end]);
        let content_length = headers
            .lines()
            .find_map(|line| {
                let (name, value) = line.split_once(':')?;
                name.eq_ignore_ascii_case("content-length")
                    .then(|| value.trim().parse::<usize>().unwrap())
            })
            .unwrap_or(0);
        let body_start = header_end + 4;
        while request.len() < body_start + content_length {
            let read = stream.read(&mut buffer).unwrap();
            if read == 0 {
                break;
            }
            request.extend_from_slice(&buffer[..read]);
        }
        let request_body =
            String::from_utf8_lossy(&request[body_start..body_start + content_length]).to_string();
        let response_body = serde_json::json!({
            "choices": [{
                "message": {"role": "assistant", "content": response_text},
                "finish_reason": "stop"
            }],
            "usage": {"prompt_tokens": 2, "completion_tokens": 3, "total_tokens": 5}
        })
        .to_string();
        write!(
            stream,
            "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
            response_body.len(),
            response_body
        )
        .unwrap();
        request_body
    });
    (base_url, handle)
}

fn scrub_config_bootstrap_provider_env(cmd: &mut Command) {
    for key in [
        "ANTHROPIC_API_KEY",
        "OPENAI_API_KEY",
        "GOOGLE_API_KEY",
        "AZURE_OPENAI_API_KEY",
        "DEEPSEEK_API_KEY",
        "GROQ_API_KEY",
        "MISTRAL_API_KEY",
        "TOGETHER_API_KEY",
        "FIREWORKS_API_KEY",
        "OPENROUTER_API_KEY",
    ] {
        cmd.env_remove(key);
    }
    cmd.env_remove("BY_ENV_FILE");
}

#[test]
fn top_help_matches_clojure_command_surface() {
    Command::cargo_bin("by-rs")
        .unwrap()
        .arg("--help")
        .assert()
        .success()
        .stderr(predicate::str::contains("NAME:\n by - Brainyard Agent CLI"))
        .stderr(predicate::str::contains(
            "USAGE:\n by [global-options] command [command options] [arguments...]",
        ))
        .stderr(predicate::str::contains("VERSION:"))
        .stderr(predicate::str::contains(
            "run                  Start interactive TUI agent session (default)",
        ))
        .stderr(predicate::str::contains("agents"))
        .stderr(predicate::str::contains("models"))
        .stderr(predicate::str::contains("sessions"))
        .stderr(predicate::str::contains("tools").not())
        .stderr(predicate::str::contains("memory").not())
        .stderr(predicate::str::contains("mcp").not())
        .stderr(predicate::str::contains("tui").not());
}

#[test]
fn top_version_flags_are_not_routed_to_run_command() {
    for flag in ["--version", "-V"] {
        Command::cargo_bin("by-rs")
            .unwrap()
            .arg(flag)
            .assert()
            .success()
            .stdout(predicate::str::starts_with("by "))
            .stderr(predicate::str::is_empty());
    }
}

#[test]
fn short_h_is_not_a_clojure_help_alias() {
    for args in [vec!["-h"], vec!["run", "-h"]] {
        Command::cargo_bin("by-rs")
            .unwrap()
            .args(args)
            .assert()
            .code(255)
            .stdout(predicate::str::is_empty())
            .stderr(predicate::str::contains("Unknown option: \"-h\""))
            .stderr(predicate::str::contains("NAME:\n by"));
    }
}

#[test]
fn run_help_matches_clojure_command_surface() {
    Command::cargo_bin("by-rs")
        .unwrap()
        .args(["run", "--help"])
        .assert()
        .success()
        .stderr(predicate::str::contains(
            "NAME:\n by run - Start interactive TUI agent session (default)",
        ))
        .stderr(predicate::str::contains("-u, --user-id S"))
        .stderr(predicate::str::contains("--[no-]inline"))
        .stderr(predicate::str::contains("--[no-]with-tmux"))
        .stderr(predicate::str::contains("--[no-]select-resume"));
}

#[test]
fn no_args_defaults_to_run_command() {
    let home = tempfile::tempdir().unwrap();

    Command::cargo_bin("by-rs")
        .unwrap()
        .env("HOME", home.path())
        .env("BRAINYARD_SESSION_ID", "agt-default")
        .write_stdin("/quit\n")
        .assert()
        .success()
        .stdout(predicate::str::contains("Brainyard TUI"))
        .stdout(predicate::str::contains("coact-agent"))
        .stdout(predicate::str::contains("claude-code/opus"))
        .stdout(predicate::str::contains("TUI session ended."))
        .stderr(predicate::str::is_empty());

    let meta = read_session_meta(home.path(), "agt-default");
    assert!(meta.contains(r#":user-id ""#));
    assert!(meta.contains(":agent-id :coact-agent"));
    assert!(meta.contains(":defagent-id :coact-agent"));
    assert!(meta.contains(":started-at "));
    assert!(meta.contains(":last-attached-at "));
}

#[test]
fn run_loop_reads_until_quit_command() {
    let home = tempfile::tempdir().unwrap();

    Command::cargo_bin("by-rs")
        .unwrap()
        .env("HOME", home.path())
        .env("BRAINYARD_SESSION_ID", "agt-run-loop-quit")
        .env("BY_NO_DOTENV", "1")
        .args(["run", "--inline"])
        .write_stdin("/quit\n")
        .assert()
        .success()
        .stdout(predicate::str::contains("Brainyard TUI"))
        .stdout(predicate::str::contains("TUI session ended."))
        .stderr(predicate::str::is_empty());

    let meta = read_session_meta(home.path(), "agt-run-loop-quit");
    assert!(meta.contains(":agent-id :coact-agent"));
}

#[test]
fn run_config_slash_lists_runtime_config() {
    let home = tempfile::tempdir().unwrap();
    let project = tempfile::tempdir().unwrap();

    let assert = Command::cargo_bin("by-rs")
        .unwrap()
        .current_dir(project.path())
        .env("HOME", home.path())
        .env("BRAINYARD_PROJECT_DIR", project.path())
        .env("BRAINYARD_SESSION_ID", "agt-config-list")
        .env("BY_NO_DOTENV", "1")
        .args(["run", "--inline"])
        .write_stdin("/config\n/quit\n")
        .assert()
        .success()
        .stderr(predicate::str::is_empty());

    let stdout = String::from_utf8(assert.get_output().stdout.clone()).unwrap();
    assert!(stdout.contains("Runtime Config"));
    assert!(stdout.contains("acp-backend"));
    assert!(stdout.contains("max-iterations"));
    assert!(stdout.contains("show-llm-streaming"));
    assert!(stdout.contains("working-dir"));
    assert!(!stdout.contains("not available in by-rs yet"));
}

#[test]
fn run_memory_and_init_help_slash_print_static_help() {
    let home = tempfile::tempdir().unwrap();
    let project = tempfile::tempdir().unwrap();

    let assert = Command::cargo_bin("by-rs")
        .unwrap()
        .current_dir(project.path())
        .env("HOME", home.path())
        .env("BRAINYARD_PROJECT_DIR", project.path())
        .env("BRAINYARD_SESSION_ID", "agt-static-help")
        .env("BY_NO_DOTENV", "1")
        .args(["run", "--inline"])
        .write_stdin("/memory help\n/init help\n/quit\n")
        .assert()
        .success()
        .stderr(predicate::str::is_empty());

    let stdout = String::from_utf8(assert.get_output().stdout.clone()).unwrap();
    assert!(stdout.contains("/memory stats"));
    assert!(stdout.contains("/memory remember <content>"));
    assert!(stdout.contains("/init show"));
    assert!(stdout.contains("/init --scope :user|:project|:both"));
    assert!(!stdout.contains("Unknown slash command: /memory"));
    assert!(!stdout.contains("Unknown slash command: /init"));
}

#[test]
fn run_init_show_slash_reads_project_and_lists_user_scope() {
    let home = tempfile::tempdir().unwrap();
    let project = tempfile::tempdir().unwrap();
    let project_brainyard = project.path().join(".brainyard/BRAINYARD.md");
    std::fs::create_dir_all(project_brainyard.parent().unwrap()).unwrap();
    std::fs::write(
        &project_brainyard,
        "# Project Brainyard\n\n## Notes\nProject note\n",
    )
    .unwrap();

    let assert = Command::cargo_bin("by-rs")
        .unwrap()
        .current_dir(project.path())
        .env("HOME", home.path())
        .env("BRAINYARD_PROJECT_DIR", project.path())
        .env("BRAINYARD_SESSION_ID", "agt-init-show")
        .env("BY_NO_DOTENV", "1")
        .args(["run", "--inline"])
        .write_stdin("/init show\n/quit\n")
        .assert()
        .success()
        .stderr(predicate::str::is_empty());

    let stdout = String::from_utf8(assert.get_output().stdout.clone()).unwrap();
    assert!(stdout.contains("── PROJECT"));
    assert!(stdout.contains("Project note"));
    assert!(stdout.contains("── USER"));
    assert!(stdout.contains("sections: 2"));
    assert!(!stdout.contains("Unknown slash command: /init"));
}

#[test]
fn run_init_list_snapshots_slash_renders_project_snapshot_records() {
    let home = tempfile::tempdir().unwrap();
    let project = tempfile::tempdir().unwrap();
    let snapshot = project
        .path()
        .join(".brainyard/agents/init-agent/snapshots/20260102-030405-project-test-snapshot.md");
    std::fs::create_dir_all(snapshot.parent().unwrap()).unwrap();
    std::fs::write(&snapshot, "# Prior Brainyard\n").unwrap();

    let assert = Command::cargo_bin("by-rs")
        .unwrap()
        .current_dir(project.path())
        .env("HOME", home.path())
        .env("BRAINYARD_PROJECT_DIR", project.path())
        .env("BRAINYARD_SESSION_ID", "agt-init-list-snapshots")
        .env("BY_NO_DOTENV", "1")
        .args(["run", "--inline"])
        .write_stdin("/init list-snapshots\n/quit\n")
        .assert()
        .success()
        .stderr(predicate::str::is_empty());

    let stdout = String::from_utf8(assert.get_output().stdout.clone()).unwrap();
    assert!(stdout.contains("20260102-030405"));
    assert!(stdout.contains("project"));
    assert!(stdout.contains("test-snapshot"));
    assert!(stdout.contains(snapshot.to_string_lossy().as_ref()));
    assert!(!stdout.contains("Unknown slash command: /init"));
}

#[test]
fn run_init_list_snapshots_both_lists_cross_scope_records_from_project_base() {
    let home = tempfile::tempdir().unwrap();
    let project = tempfile::tempdir().unwrap();
    let snapshots_dir = project
        .path()
        .join(".brainyard/agents/init-agent/snapshots");
    std::fs::create_dir_all(&snapshots_dir).unwrap();

    let project_snapshot = snapshots_dir.join("20260102-030405-project-project-only.md");
    let user_snapshot = snapshots_dir.join("20260103-030405-user-user-inside-project-base.md");
    std::fs::write(&project_snapshot, "# Project snapshot\n").unwrap();
    std::fs::write(&user_snapshot, "# User snapshot stored in project base\n").unwrap();

    let assert = Command::cargo_bin("by-rs")
        .unwrap()
        .current_dir(project.path())
        .env("HOME", home.path())
        .env("BRAINYARD_PROJECT_DIR", project.path())
        .env("BRAINYARD_SESSION_ID", "agt-init-list-both-cross-scope")
        .env("BY_NO_DOTENV", "1")
        .env("NO_COLOR", "1")
        .env("TERM", "dumb")
        .env("COLUMNS", "120")
        .args(["run", "--inline"])
        .write_stdin("/init list-snapshots\n/quit\n")
        .assert()
        .success()
        .stderr(predicate::str::is_empty());

    let stdout = String::from_utf8(assert.get_output().stdout.clone()).unwrap();
    assert!(stdout.contains("project-only"), "stdout: {stdout}");
    assert!(
        stdout.contains("user-inside-project-base"),
        "stdout: {stdout}"
    );
}

#[test]
fn run_init_revert_without_snapshot_prints_usage() {
    let home = tempfile::tempdir().unwrap();

    Command::cargo_bin("by-rs")
        .unwrap()
        .env("HOME", home.path())
        .env("BRAINYARD_SESSION_ID", "agt-init-revert-usage")
        .env("BY_NO_DOTENV", "1")
        .args(["run", "--inline"])
        .write_stdin("/init revert\n/quit\n")
        .assert()
        .success()
        .stdout(predicate::str::contains(
            "Usage: /init revert <snapshot-path>  (use /init list-snapshots first)",
        ))
        .stdout(predicate::str::contains("Unknown slash command: /init").not())
        .stderr(predicate::str::is_empty());
}

#[test]
fn run_init_revert_restores_snapshot_and_writes_pre_revert_snapshot() {
    let home = tempfile::tempdir().unwrap();
    let project = tempfile::tempdir().unwrap();
    let brainyard_dir = project.path().join(".brainyard");
    let snapshot_dir = brainyard_dir.join("agents/init-agent/snapshots");
    std::fs::create_dir_all(&snapshot_dir).unwrap();
    let brainyard_file = brainyard_dir.join("BRAINYARD.md");
    std::fs::write(&brainyard_file, "# Current\n").unwrap();
    let snapshot = snapshot_dir.join("20260102-030405-project-test-snapshot.md");
    std::fs::write(&snapshot, "# Restored\n").unwrap();

    let assert = Command::cargo_bin("by-rs")
        .unwrap()
        .current_dir(project.path())
        .env("HOME", home.path())
        .env("BRAINYARD_SESSION_ID", "agt-init-revert")
        .env("BY_NO_DOTENV", "1")
        .args(["run", "--inline"])
        .write_stdin(format!("/init revert {}\n/quit\n", snapshot.display()))
        .assert()
        .success()
        .stdout(predicate::str::contains("{:ok? true"))
        .stdout(predicate::str::contains(":scope :project"))
        .stdout(predicate::str::contains(":restored-from"))
        .stdout(predicate::str::contains(":pre-revert-snapshot"))
        .stdout(predicate::str::contains(":dest"))
        .stdout(predicate::str::contains("Unknown slash command: /init").not())
        .stderr(predicate::str::is_empty());

    let stdout = String::from_utf8(assert.get_output().stdout.clone()).unwrap();
    assert!(stdout.contains(&snapshot.display().to_string()));
    assert_eq!(
        std::fs::read_to_string(&brainyard_file).unwrap(),
        "# Restored\n"
    );

    let mut pre_revert_snapshots = std::fs::read_dir(&snapshot_dir)
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .filter(|path| path != &snapshot)
        .collect::<Vec<_>>();
    pre_revert_snapshots.sort();
    assert_eq!(pre_revert_snapshots.len(), 1);
    let pre_revert = &pre_revert_snapshots[0];
    assert!(pre_revert
        .file_name()
        .unwrap()
        .to_string_lossy()
        .contains("-project-revert-test-snapshot.md"));
    assert_eq!(std::fs::read_to_string(pre_revert).unwrap(), "# Current\n");
}

#[test]
fn run_with_closed_stdin_stays_alive_like_tui() {
    let home = tempfile::tempdir().unwrap();
    let binary = assert_cmd::cargo::cargo_bin("by-rs");
    let mut child = StdCommand::new(binary)
        .env("HOME", home.path())
        .env("BRAINYARD_SESSION_ID", "agt-closed-stdin")
        .env("BY_NO_DOTENV", "1")
        .args(["run", "--inline"])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();

    thread::sleep(Duration::from_secs(2));
    let observed_status = child.try_wait().unwrap();
    assert!(
        observed_status.is_none(),
        "run should stay alive when launched without an input stream"
    );

    child.kill().unwrap();
    let _ = child.wait().unwrap();
}

#[test]
fn run_loop_invokes_ollama_live_and_persists_messages() {
    let home = tempfile::tempdir().unwrap();
    let (ollama_base_url, request_handle) =
        spawn_openai_compatible_fixture("Ollama fixture answer");

    Command::cargo_bin("by-rs")
        .unwrap()
        .env("HOME", home.path())
        .env("BRAINYARD_SESSION_ID", "agt-run-loop-ollama")
        .env("BY_NO_DOTENV", "1")
        .env("BY_RS_OLLAMA_BASE_URL", &ollama_base_url)
        .args(["run", "--inline", "-p", "ollama"])
        .write_stdin("Hello Ollama\n/quit\n")
        .assert()
        .success()
        .stdout(predicate::str::contains("Brainyard TUI"))
        .stdout(predicate::str::contains("ollama/glm-5:cloud"))
        .stdout(predicate::str::contains("Ollama fixture answer"))
        .stdout(predicate::str::contains("TUI session ended."))
        .stderr(predicate::str::is_empty());

    let request_body = request_handle.join().unwrap();
    assert!(request_body.contains(r#""model":"glm-5:cloud""#));
    assert!(request_body.contains("Hello Ollama"));

    let messages = read_session_messages(home.path(), "agt-run-loop-ollama");
    assert!(messages.contains("Hello Ollama"));
    assert!(messages.contains("Ollama fixture answer"));
}

#[test]
fn run_loop_invokes_claude_code_live_and_persists_messages() {
    let home = tempfile::tempdir().unwrap();
    let path_dir = tempfile::tempdir().unwrap();
    let capture_path = home.path().join("claude-stdin.txt");
    write_fake_executable(
        &path_dir.path().join("claude"),
        r#"#!/usr/bin/env bash
cat > "$CLAUDE_CAPTURE_STDIN"
printf '%s\n' '{"type":"result","subtype":"success","is_error":false,"result":"Claude fixture answer","usage":{"input_tokens":3,"output_tokens":4}}'
"#,
    );

    let assert = Command::cargo_bin("by-rs")
        .unwrap()
        .env("HOME", home.path())
        .env("BRAINYARD_SESSION_ID", "agt-run-claude-code")
        .env("BY_NO_DOTENV", "1")
        .env("PATH", prepend_path(path_dir.path()))
        .env("CLAUDE_CAPTURE_STDIN", &capture_path)
        .args(["run", "--inline", "-p", "claude-code", "-m", "haiku"])
        .write_stdin("Hello Claude\n/quit\n")
        .assert()
        .success()
        .stdout(predicate::str::contains("Claude fixture answer"))
        .stdout(predicate::str::contains("TUI session ended."))
        .stderr(predicate::str::is_empty());
    let _ = assert;

    assert_eq!(
        std::fs::read_to_string(capture_path).unwrap(),
        "Hello Claude"
    );
    let messages = read_session_messages(home.path(), "agt-run-claude-code");
    assert!(messages.contains(r#":role "user""#));
    assert!(messages.contains(r#":content "Hello Claude""#));
    assert!(messages.contains(r#":role "assistant""#));
    assert!(messages.contains(r#":content "Claude fixture answer""#));
}

#[test]
fn run_claude_code_live_resume_spools_large_system_prompt_file() {
    let home = tempfile::tempdir().unwrap();
    let path_dir = tempfile::tempdir().unwrap();
    let session_id = "agt-run-claude-code-spooled";
    let capture_stdin = home.path().join("claude-spooled-stdin.txt");
    let capture_system = home.path().join("claude-spooled-system.txt");
    let system_prompt = "S".repeat(262_145);

    write_fake_executable(
        &path_dir.path().join("claude"),
        r#"#!/usr/bin/env bash
system_file=""
prev=""
for arg in "$@"; do
  if [ "$prev" = "--system-prompt-file" ]; then
    system_file="$arg"
  fi
  prev="$arg"
done
if [ -z "$system_file" ] || [ ! -f "$system_file" ]; then
  echo "missing system prompt file: $system_file" >&2
  exit 9
fi
cat "$system_file" > "$CLAUDE_SYSTEM_CAPTURE"
cat > "$CLAUDE_CAPTURE_STDIN"
printf '%s\n' '{"type":"result","subtype":"success","is_error":false,"result":"Spooled system answer","usage":{"input_tokens":7,"output_tokens":8}}'
"#,
    );

    write_session_meta(
        home.path(),
        session_id,
        r#"{:agent-id :coact-agent
            :defagent-id :coact-agent
            :started-at 1000
            :last-attached-at 1000}"#,
    );
    let session_dir = home.path().join(".brainyard/sessions").join(session_id);
    let original_messages = format!(
        r#"{{:t 1 :kind :message :payload {{:role "system" :content "{}"}}}}
"#,
        system_prompt
    );
    std::fs::write(session_dir.join("messages.log"), original_messages).unwrap();

    Command::cargo_bin("by-rs")
        .unwrap()
        .env("HOME", home.path())
        .env("BY_NO_DOTENV", "1")
        .env("PATH", prepend_path(path_dir.path()))
        .env("CLAUDE_CAPTURE_STDIN", &capture_stdin)
        .env("CLAUDE_SYSTEM_CAPTURE", &capture_system)
        .args([
            "run",
            "--inline",
            "--resume",
            session_id,
            "-p",
            "claude-code",
            "-m",
            "haiku",
        ])
        .write_stdin("Hello with system\n/quit\n")
        .assert()
        .success()
        .stdout(predicate::str::contains("Spooled system answer"))
        .stderr(predicate::str::is_empty());

    assert_eq!(
        std::fs::read_to_string(capture_stdin).unwrap(),
        "Hello with system"
    );
    assert_eq!(
        std::fs::read_to_string(capture_system).unwrap(),
        system_prompt
    );
    let messages = read_session_messages(home.path(), session_id);
    assert!(messages.contains(r#":content "Spooled system answer""#));
}

#[test]
fn root_level_run_flags_are_routed_to_run_command() {
    let home = tempfile::tempdir().unwrap();

    Command::cargo_bin("by-rs")
        .unwrap()
        .env("HOME", home.path())
        .env("BRAINYARD_SESSION_ID", "agt-root")
        .args([
            "--inline",
            "--no-inline",
            "--verbose",
            "--no-verbose",
            "--with-tmux",
            "--no-with-tmux",
            "--select-resume",
            "--no-select-resume",
            "--new",
            "--no-new",
            "--max-iterations",
            "3",
            "--agent",
            "coact-agent",
            "--provider",
            "bedrock",
            "--model",
            "amazon.nova-lite-v1:0",
            "--user-id",
            "alice",
        ])
        .write_stdin("/quit\n")
        .assert()
        .success()
        .stdout(predicate::str::contains("Brainyard TUI"))
        .stdout(predicate::str::contains("coact-agent"))
        .stdout(predicate::str::contains("bedrock/amazon.nova-lite-v1:0"))
        .stdout(predicate::str::contains("TUI session ended."))
        .stderr(predicate::str::contains("unexpected argument").not())
        .stderr(predicate::str::is_empty());

    let meta = read_session_meta(home.path(), "agt-root");
    assert!(meta.contains(r#":user-id "alice""#));
    assert!(meta.contains(":agent-id :coact-agent"));
    assert!(meta.contains(":defagent-id :coact-agent"));
    assert!(meta.contains(":working-dir "));
}

#[test]
fn bare_agent_id_is_routed_to_run_command() {
    let home = tempfile::tempdir().unwrap();

    Command::cargo_bin("by-rs")
        .unwrap()
        .env("HOME", home.path())
        .env("BRAINYARD_SESSION_ID", "agt-bare")
        .arg("coact-agent")
        .write_stdin("/quit\n")
        .assert()
        .success()
        .stdout(predicate::str::contains("Brainyard TUI"))
        .stdout(predicate::str::contains("coact-agent"))
        .stdout(predicate::str::contains("TUI session ended."))
        .stderr(predicate::str::contains("unrecognized subcommand").not())
        .stderr(predicate::str::is_empty());

    assert!(home
        .path()
        .join(".brainyard/sessions/agt-bare/meta.edn")
        .is_file());
}

#[test]
fn run_accepts_bare_resume_flag_like_clojure() {
    let home = tempfile::tempdir().unwrap();

    Command::cargo_bin("by-rs")
        .unwrap()
        .env("HOME", home.path())
        .env("BRAINYARD_SESSION_ID", "agt-bare-resume")
        .args(["run", "--resume"])
        .write_stdin("/quit\n")
        .assert()
        .success()
        .stdout(predicate::str::contains("Brainyard TUI"))
        .stdout(predicate::str::contains("TUI session ended."))
        .stderr(predicate::str::contains("a value is required").not())
        .stderr(predicate::str::is_empty());

    assert!(home
        .path()
        .join(".brainyard/sessions/agt-bare-resume/meta.edn")
        .is_file());
}

#[test]
fn run_explicit_missing_resume_matches_clojure_error() {
    let home = tempfile::tempdir().unwrap();

    Command::cargo_bin("by-rs")
        .unwrap()
        .env("HOME", home.path())
        .args(["run", "--resume", "missing"])
        .assert()
        .failure()
        .code(1)
        .stdout(predicate::str::is_empty())
        .stderr(predicate::str::contains(
            "Error: no persisted session named 'missing'.",
        ))
        .stderr(predicate::str::contains("by-rs run is not implemented yet").not());

    assert!(!home.path().join(".brainyard").exists());
}

#[test]
fn run_explicit_existing_resume_reaches_preview_tui() {
    let home = tempfile::tempdir().unwrap();
    let session_dir = home.path().join(".brainyard/sessions/alpha");
    std::fs::create_dir_all(&session_dir).unwrap();

    Command::cargo_bin("by-rs")
        .unwrap()
        .env("HOME", home.path())
        .args(["run", "--resume", "alpha"])
        .write_stdin("/quit\n")
        .assert()
        .success()
        .stdout(predicate::str::contains("Brainyard TUI"))
        .stdout(predicate::str::contains("session alpha"))
        .stderr(predicate::str::contains("no persisted session named").not())
        .stderr(predicate::str::is_empty());

    let sessions = list_sessions(home.path().join(".brainyard/sessions")).unwrap();
    assert_eq!(sessions[0].id, "alpha");
    assert!(sessions[0].started_at_millis.is_some());
    assert!(sessions[0].last_attached_at_millis.is_some());
}

#[test]
fn run_explicit_resume_uses_session_dir_when_meta_id_is_stale() {
    let home = tempfile::tempdir().unwrap();
    write_session_meta(
        home.path(),
        "alpha",
        r#"{:id "stale-meta-id"
            :label "Alpha session"
            :started-at 1000
            :last-attached-at 2000}"#,
    );

    Command::cargo_bin("by-rs")
        .unwrap()
        .env("HOME", home.path())
        .args(["run", "--resume", "alpha"])
        .write_stdin("/quit\n")
        .assert()
        .success()
        .stdout(predicate::str::contains("Brainyard TUI"))
        .stdout(predicate::str::contains("session alpha"))
        .stderr(predicate::str::contains("no persisted session named").not())
        .stderr(predicate::str::is_empty());

    let sessions = list_sessions(home.path().join(".brainyard/sessions")).unwrap();
    assert_eq!(sessions[0].id, "alpha");
    assert_eq!(sessions[0].label.as_deref(), Some("Alpha session"));
    assert_eq!(sessions[0].started_at_millis, Some(1000));
    assert!(sessions[0].last_attached_at_millis.unwrap() >= 2000);
}

#[test]
fn run_explicit_resume_preview_reports_persisted_message_count() {
    let home = tempfile::tempdir().unwrap();
    write_session_meta(
        home.path(),
        "alpha",
        r#"{:agent-id :coact-agent
            :defagent-id :coact-agent
            :started-at 1000
            :last-attached-at 1000}"#,
    );
    std::fs::write(
        home.path()
            .join(".brainyard/sessions/alpha")
            .join("messages.log"),
        r#"{:t 1 :kind :message :payload {:role "user" :content "Hello"}}
{:t 2 :kind :message :payload {:role "assistant" :content "Hi"}}
"#,
    )
    .unwrap();

    Command::cargo_bin("by-rs")
        .unwrap()
        .env("HOME", home.path())
        .env("BRAINYARD_SESSION_ID", "ignored-for-resume")
        .args(["run", "--resume", "alpha"])
        .write_stdin("/quit\n")
        .assert()
        .success()
        .stdout(predicate::str::contains("session alpha"))
        .stderr(predicate::str::is_empty());
}

#[test]
fn run_select_resume_without_sessions_reaches_preview_tui_without_prompt() {
    let home = tempfile::tempdir().unwrap();

    Command::cargo_bin("by-rs")
        .unwrap()
        .env("HOME", home.path())
        .env("BRAINYARD_SESSION_ID", "agt-select-empty")
        .args(["run", "--select-resume"])
        .write_stdin("/quit\n")
        .assert()
        .success()
        .stdout(predicate::str::contains("Brainyard TUI"))
        .stdout(predicate::str::contains("TUI session ended."))
        .stderr(predicate::str::contains("no persisted session named").not())
        .stderr(predicate::str::is_empty());

    assert!(home
        .path()
        .join(".brainyard/sessions/agt-select-empty/meta.edn")
        .is_file());
}

#[test]
fn run_select_resume_takes_precedence_over_explicit_missing_resume() {
    let home = tempfile::tempdir().unwrap();

    Command::cargo_bin("by-rs")
        .unwrap()
        .env("HOME", home.path())
        .env("BRAINYARD_SESSION_ID", "agt-select-new")
        .args(["run", "--select-resume", "--resume", "missing"])
        .write_stdin("/quit\n")
        .assert()
        .success()
        .stdout(predicate::str::contains("Brainyard TUI"))
        .stdout(predicate::str::contains("TUI session ended."))
        .stderr(predicate::str::contains("no persisted session named").not())
        .stderr(predicate::str::is_empty());

    assert!(home
        .path()
        .join(".brainyard/sessions/agt-select-new/meta.edn")
        .is_file());
}

#[test]
fn run_select_resume_prints_clojure_style_picker_before_tui() {
    let home = tempfile::tempdir().unwrap();
    write_session_meta(
        home.path(),
        "older",
        r#"{:id "older"
            :label "Old"
            :defagent-id :coact-agent
            :started-at 1000
            :last-attached-at 2000}"#,
    );
    write_session_meta(
        home.path(),
        "newer",
        r#"{:id "newer"
            :label "New"
            :agent-id :main-agent
            :started-at 1000
            :last-attached-at 3000}"#,
    );

    let assert = Command::cargo_bin("by-rs")
        .unwrap()
        .env("HOME", home.path())
        .env("BRAINYARD_SESSION_ID", "agt-picked-new")
        .args(["run", "--select-resume"])
        .write_stdin("N\n/quit\n")
        .assert()
        .success()
        .stderr(predicate::str::is_empty());

    let stdout = String::from_utf8(assert.get_output().stdout.clone()).unwrap();
    assert!(
        stdout.contains("2 persisted session(s) — pick one to resume, or [N] for a new session:")
    );
    assert!(stdout.contains("Choice [1- 2 ] / (N)ew: "));
    assert!(stdout.contains("newer"));
    assert!(stdout.contains("older"));
    assert!(
        stdout.find("newer").unwrap() < stdout.find("older").unwrap(),
        "newest session should be listed first: {stdout}"
    );
    assert!(home
        .path()
        .join(".brainyard/sessions/agt-picked-new/meta.edn")
        .is_file());
}

#[test]
fn run_with_tmux_without_tmux_binary_matches_clojure_guidance() {
    let path_dir = tempfile::tempdir().unwrap();

    let assert = Command::cargo_bin("by-rs")
        .unwrap()
        .env("PATH", path_dir.path())
        .env_remove("TMUX")
        .args(["run", "--with-tmux"])
        .assert()
        .failure()
        .code(1)
        .stdout(predicate::str::is_empty())
        .stderr(predicate::str::contains("by-rs run is not implemented yet").not());

    assert_eq!(
        String::from_utf8(assert.get_output().stderr.clone()).unwrap(),
        TMUX_NEED_TMUX_GUIDANCE
    );
}

#[test]
fn run_with_tmux_without_tmux_env_matches_clojure_guidance() {
    let path_dir = tempfile::tempdir().unwrap();
    link_fake_executable(
        &path_dir.path().join("tmux"),
        system_binary(&["/usr/bin/true", "/bin/true"]),
    );

    let assert = Command::cargo_bin("by-rs")
        .unwrap()
        .env("PATH", path_dir.path())
        .env_remove("TMUX")
        .args(["run", "--with-tmux"])
        .assert()
        .failure()
        .code(1)
        .stdout(predicate::str::is_empty())
        .stderr(predicate::str::contains("by-rs run is not implemented yet").not());

    assert_eq!(
        String::from_utf8(assert.get_output().stderr.clone()).unwrap(),
        TMUX_NEED_SESSION_GUIDANCE
    );
}

#[test]
fn run_with_tmux_dead_server_matches_clojure_guidance() {
    let path_dir = tempfile::tempdir().unwrap();
    link_fake_executable(
        &path_dir.path().join("tmux"),
        system_binary(&["/usr/bin/false", "/bin/false"]),
    );

    let assert = Command::cargo_bin("by-rs")
        .unwrap()
        .env("PATH", path_dir.path())
        .env("TMUX", "/tmp/dead,123,0")
        .args(["run", "--with-tmux"])
        .assert()
        .failure()
        .code(1)
        .stdout(predicate::str::is_empty())
        .stderr(predicate::str::contains("by-rs run is not implemented yet").not());

    assert_eq!(
        String::from_utf8(assert.get_output().stderr.clone()).unwrap(),
        TMUX_SERVER_DEAD_GUIDANCE
    );
}

#[test]
fn run_with_tmux_live_server_reaches_preview_tui() {
    let home = tempfile::tempdir().unwrap();
    let path_dir = tempfile::tempdir().unwrap();
    link_fake_executable(
        &path_dir.path().join("tmux"),
        system_binary(&["/usr/bin/true", "/bin/true"]),
    );

    Command::cargo_bin("by-rs")
        .unwrap()
        .env("HOME", home.path())
        .env("BRAINYARD_SESSION_ID", "agt-tmux")
        .env("PATH", path_dir.path())
        .env("TMUX", "/tmp/live,123,0")
        .args(["run", "--with-tmux"])
        .write_stdin("/quit\n")
        .assert()
        .success()
        .stdout(predicate::str::contains("Brainyard TUI"))
        .stdout(predicate::str::contains("TUI session ended."))
        .stderr(predicate::str::contains("You passed --with-tmux").not())
        .stderr(predicate::str::is_empty());

    assert!(home
        .path()
        .join(".brainyard/sessions/agt-tmux/meta.edn")
        .is_file());
}

#[test]
fn run_no_with_tmux_does_not_trigger_tmux_preflight() {
    let home = tempfile::tempdir().unwrap();
    let path_dir = tempfile::tempdir().unwrap();

    Command::cargo_bin("by-rs")
        .unwrap()
        .env("HOME", home.path())
        .env("BRAINYARD_SESSION_ID", "agt-no-tmux")
        .env("PATH", path_dir.path())
        .env_remove("TMUX")
        .args(["run", "--no-with-tmux"])
        .write_stdin("/quit\n")
        .assert()
        .success()
        .stdout(predicate::str::contains("Brainyard TUI"))
        .stdout(predicate::str::contains("TUI session ended."))
        .stderr(predicate::str::contains("You passed --with-tmux").not())
        .stderr(predicate::str::is_empty());

    assert!(home
        .path()
        .join(".brainyard/sessions/agt-no-tmux/meta.edn")
        .is_file());
}

#[test]
fn run_bedrock_dry_run_one_turn_prepares_request_and_session_meta() {
    let home = tempfile::tempdir().unwrap();

    Command::cargo_bin("by-rs")
        .unwrap()
        .env("HOME", home.path())
        .env("BRAINYARD_SESSION_ID", "agt-run-dry")
        .env("BY_NO_DOTENV", "1")
        .env_remove("AWS_REGION")
        .env_remove("AWS_DEFAULT_REGION")
        .env_remove("AWS_PROFILE")
        .env_remove("AWS_DEFAULT_PROFILE")
        .args([
            "run",
            "--provider",
            "bedrock",
            "--model",
            "amazon.nova-lite-v1:0",
            "--dry-run",
            "--region",
            "ap-northeast-2",
            "--aws-profile",
            "sandbox",
            "--max-tokens",
            "64",
            "--no-prompt-cache",
            "What",
            "is",
            "2+2?",
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("\"provider\": \"bedrock\""))
        .stdout(predicate::str::contains("\"operation\": \"Converse\""))
        .stdout(predicate::str::contains("\"network\": false"))
        .stdout(predicate::str::contains("\"session_id\": \"agt-run-dry\""))
        .stdout(predicate::str::contains("\"region\": \"ap-northeast-2\""))
        .stdout(predicate::str::contains("\"aws_profile\": \"sandbox\""))
        .stdout(predicate::str::contains(
            "\"modelId\": \"amazon.nova-lite-v1:0\"",
        ))
        .stdout(predicate::str::contains("\"maxTokens\": 64"))
        .stdout(predicate::str::contains("\"text\": \"What is 2+2?\""))
        .stdout(predicate::str::contains("cachePoint").not())
        .stderr(predicate::str::is_empty());

    let meta = read_session_meta(home.path(), "agt-run-dry");
    assert!(meta.contains(":agent-id :coact-agent"));
    assert!(!home
        .path()
        .join(".brainyard/sessions/agt-run-dry/messages.log")
        .exists());
    assert!(!home
        .path()
        .join(".brainyard/sessions/agt-run-dry/session.edn")
        .exists());
    assert!(!home
        .path()
        .join(".brainyard/sessions/agt-run-dry/usage-tracker.edn")
        .exists());
    assert!(!home
        .path()
        .join(".brainyard/sessions/agt-run-dry/input-history.edn")
        .exists());
}

#[test]
fn run_bedrock_fixture_one_turn_persists_restore_compatible_messages() {
    let home = tempfile::tempdir().unwrap();
    let fixture =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/bedrock/converse-response.json");

    Command::cargo_bin("by-rs")
        .unwrap()
        .env("HOME", home.path())
        .env("BRAINYARD_SESSION_ID", "agt-run-fixture")
        .env("BY_NO_DOTENV", "1")
        .args([
            "run",
            "--provider",
            "bedrock",
            "--model",
            "amazon.nova-lite-v1:0",
            "--fixture-response",
        ])
        .arg(fixture)
        .arg("What is 2+2?")
        .assert()
        .success()
        .stdout(predicate::str::contains("Hello world"))
        .stderr(predicate::str::is_empty());

    let messages = read_session_messages(home.path(), "agt-run-fixture");
    let lines = messages.lines().collect::<Vec<_>>();
    assert_eq!(lines.len(), 2);
    assert!(lines[0].contains(r#":kind :message"#));
    assert!(lines[0].contains(r#":role "user""#));
    assert!(lines[0].contains(r#":content "What is 2+2?""#));
    assert!(lines[1].contains(r#":kind :message"#));
    assert!(lines[1].contains(r#":role "assistant""#));
    assert!(lines[1].contains(r#":content "Hello world""#));

    let restored =
        by_persist::restore_session(home.path().join(".brainyard/sessions"), "agt-run-fixture")
            .unwrap();
    assert_eq!(restored.total_turns, Some(1));
    assert_eq!(restored.agent_activity_seq, Some(1));
    assert!(restored.usage_tracker.is_some());
    let session_dir = home.path().join(".brainyard/sessions/agt-run-fixture");
    let usage = std::fs::read_to_string(session_dir.join("usage-tracker.edn")).unwrap();
    assert!(usage.contains(r#""amazon.nova-lite-v1:0""#));
    assert!(usage.contains(":input-tokens 7"));
    assert!(usage.contains(":output-tokens 2"));
    assert!(usage.contains(":total-tokens 9"));
    assert!(usage.contains(":cache-read-tokens 3"));
    assert!(usage.contains(":cache-write-tokens 4"));
    assert!(usage.contains(":call-count 1"));
    let input_history = std::fs::read_to_string(session_dir.join("input-history.edn")).unwrap();
    assert_eq!(input_history.trim(), r#"["What is 2+2?"]"#);
}

#[test]
fn run_bedrock_fixture_resume_appends_new_exchange_after_existing_history() {
    let home = tempfile::tempdir().unwrap();
    let session_id = "agt-run-fixture-resume";
    let fixture =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/bedrock/converse-response.json");
    write_session_meta(
        home.path(),
        session_id,
        r#"{:agent-id :coact-agent
            :defagent-id :coact-agent
            :started-at 1000
            :last-attached-at 1000}"#,
    );
    let session_dir = home.path().join(".brainyard/sessions").join(session_id);
    let original_messages = r#"{:t 1 :kind :message :payload {:role "user" :content "First turn"}}
{:t 2 :kind :message :payload {:role "assistant" :content "Intermediate answer"}}
"#;
    std::fs::write(session_dir.join("messages.log"), original_messages).unwrap();
    std::fs::write(
        session_dir.join("usage-tracker.edn"),
        r#"{:totals {:input-tokens 10
                    :output-tokens 5
                    :total-tokens 15
                    :cache-read-tokens 1
                    :cache-write-tokens 2
                    :total-cost 0.0
                    :call-count 1}
            :by-model {"amazon.nova-lite-v1:0" {:input-tokens 10
                                                :output-tokens 5
                                                :total-tokens 15
                                                :cache-read-tokens 1
                                                :cache-write-tokens 2
                                                :total-cost 0.0
                                                :call-count 1}
                       "other-model" {:input-tokens 3
                                      :output-tokens 4
                                      :total-tokens 7
                                      :cache-read-tokens 0
                                      :cache-write-tokens 0
                                      :total-cost 0.0
                                      :call-count 1}}
            :history [{:model "amazon.nova-lite-v1:0"
                       :input-tokens 10
                       :output-tokens 5
                       :total-tokens 15
                       :cache-read-tokens 1
                       :cache-write-tokens 2
                       :total-cost 0.0}]
            :history-cap 2}"#,
    )
    .unwrap();

    Command::cargo_bin("by-rs")
        .unwrap()
        .env("HOME", home.path())
        .env("BY_NO_DOTENV", "1")
        .args([
            "run",
            "--resume",
            session_id,
            "--provider",
            "bedrock",
            "--model",
            "amazon.nova-lite-v1:0",
            "--fixture-response",
        ])
        .arg(fixture)
        .arg("Second turn")
        .assert()
        .success()
        .stdout(predicate::str::contains("Hello world"))
        .stderr(predicate::str::is_empty());

    let messages = read_session_messages(home.path(), session_id);
    let lines = messages.lines().collect::<Vec<_>>();
    assert_eq!(lines.len(), 4);
    assert!(lines[0].contains(r#":content "First turn""#));
    assert!(lines[1].contains(r#":content "Intermediate answer""#));
    assert!(lines[2].contains(r#":role "user""#));
    assert!(lines[2].contains(r#":content "Second turn""#));
    assert!(lines[3].contains(r#":role "assistant""#));
    assert!(lines[3].contains(r#":content "Hello world""#));

    let meta = read_session_meta(home.path(), session_id);
    assert!(meta.contains(":started-at 1000"));
    assert!(!meta.contains(":last-attached-at 1000"));

    let restored =
        by_persist::restore_session(home.path().join(".brainyard/sessions"), session_id).unwrap();
    assert_eq!(restored.total_turns, Some(2));
    let input_history = std::fs::read_to_string(session_dir.join("input-history.edn")).unwrap();
    assert_eq!(input_history.trim(), r#"["Second turn"]"#);
    let usage = std::fs::read_to_string(session_dir.join("usage-tracker.edn")).unwrap();
    assert!(usage.contains(":input-tokens 17"));
    assert!(usage.contains(":output-tokens 7"));
    assert!(usage.contains(":total-tokens 24"));
    assert!(usage.contains(":cache-read-tokens 4"));
    assert!(usage.contains(":cache-write-tokens 6"));
    assert!(usage.contains(":call-count 2"));
    assert!(usage.contains(r#""other-model""#));
    assert!(usage.contains(":history-cap 2"));
    let usage_value = by_persist::read_session_snapshot(
        home.path().join(".brainyard/sessions"),
        session_id,
        by_persist::SessionSnapshotKind::UsageTracker,
    )
    .unwrap()
    .unwrap();
    let by_contracts::EdnValue::Map(usage_map) = usage_value else {
        panic!("expected usage tracker map");
    };
    let Some(by_contracts::EdnValue::Vector(history)) = usage_map.get("history") else {
        panic!("expected usage history vector");
    };
    assert_eq!(history.len(), 2);
}

#[test]
fn run_bedrock_dry_run_resume_includes_persisted_message_history() {
    let home = tempfile::tempdir().unwrap();
    let session_id = "agt-run-resume-history";
    write_session_meta(
        home.path(),
        session_id,
        r#"{:agent-id :coact-agent
            :defagent-id :coact-agent
            :started-at 1000
            :last-attached-at 1000}"#,
    );
    let session_dir = home.path().join(".brainyard/sessions").join(session_id);
    let original_messages = r#"{:t 1 :kind :agent.ask/pre :payload {:input "ignored"}}
{:t 2 :kind :message :payload {:role "user" :content "First turn"}}
{:t 3 :kind :message :payload {:role "assistant" :content "Intermediate answer"}}
"#;
    std::fs::write(session_dir.join("messages.log"), original_messages).unwrap();

    let assert = Command::cargo_bin("by-rs")
        .unwrap()
        .env("HOME", home.path())
        .env("BY_NO_DOTENV", "1")
        .args([
            "run",
            "--resume",
            session_id,
            "--provider",
            "bedrock",
            "--model",
            "amazon.nova-lite-v1:0",
            "--dry-run",
            "--no-prompt-cache",
            "Second turn",
        ])
        .assert()
        .success()
        .stderr(predicate::str::is_empty());
    let stdout = String::from_utf8(assert.get_output().stdout.clone()).unwrap();
    let value: serde_json::Value = serde_json::from_str(&stdout).unwrap();

    assert_eq!(value["resume"], true);
    assert_eq!(value["session_id"], session_id);
    let messages = value
        .pointer("/request/messages")
        .and_then(serde_json::Value::as_array)
        .expect("dry-run should include Bedrock messages");
    assert_eq!(messages.len(), 3);
    assert_eq!(messages[0]["role"], "user");
    assert_eq!(messages[0]["content"][0]["text"], "First turn");
    assert_eq!(messages[1]["role"], "assistant");
    assert_eq!(messages[1]["content"][0]["text"], "Intermediate answer");
    assert_eq!(messages[2]["role"], "user");
    assert_eq!(messages[2]["content"][0]["text"], "Second turn");

    assert_eq!(
        std::fs::read_to_string(session_dir.join("messages.log")).unwrap(),
        original_messages
    );
    let meta = read_session_meta(home.path(), session_id);
    assert!(meta.contains(":started-at 1000"));
    assert!(!meta.contains(":last-attached-at 1000"));
}

#[test]
fn run_openai_dry_run_one_turn_prepares_request_without_messages_write() {
    let home = tempfile::tempdir().unwrap();

    let assert = Command::cargo_bin("by-rs")
        .unwrap()
        .env("HOME", home.path())
        .env("BRAINYARD_SESSION_ID", "agt-run-openai-dry")
        .env("BY_NO_DOTENV", "1")
        .args([
            "run",
            "--provider",
            "openai",
            "--model",
            "gpt-5",
            "--dry-run",
            "--max-tokens",
            "64",
            "What is 2+2?",
        ])
        .assert()
        .success()
        .stderr(predicate::str::is_empty());
    let stdout = String::from_utf8(assert.get_output().stdout.clone()).unwrap();
    let value: serde_json::Value = serde_json::from_str(&stdout).unwrap();

    assert_eq!(value["provider"], "openai");
    assert_eq!(value["operation"], "chat/completions");
    assert_eq!(value["network"], false);
    assert_eq!(value["session_id"], "agt-run-openai-dry");
    assert_eq!(value["resume"], false);
    assert_eq!(value["request"]["model"], "gpt-5");
    assert_eq!(value["request"]["max_tokens"], 64);
    assert!(value["request"].get("temperature").is_none());
    assert_eq!(value["request"]["messages"][0]["role"], "user");
    assert_eq!(value["request"]["messages"][0]["content"], "What is 2+2?");

    let session_dir = home.path().join(".brainyard/sessions/agt-run-openai-dry");
    assert!(session_dir.join("meta.edn").is_file());
    assert!(!session_dir.join("messages.log").exists());
    assert!(!session_dir.join("session.edn").exists());
    assert!(!session_dir.join("usage-tracker.edn").exists());
    assert!(!session_dir.join("input-history.edn").exists());
}

#[test]
fn run_claude_code_dry_run_one_turn_prepares_request_without_messages_write() {
    let home = tempfile::tempdir().unwrap();

    let assert = Command::cargo_bin("by-rs")
        .unwrap()
        .env("HOME", home.path())
        .env("BRAINYARD_SESSION_ID", "agt-run-claude-code-dry")
        .env("BY_NO_DOTENV", "1")
        .args([
            "run",
            "--provider",
            "claude-code",
            "--model",
            "claude-sonnet-4-6",
            "--dry-run",
            "--max-tokens",
            "32",
            "What is 2+2?",
        ])
        .assert()
        .success()
        .stderr(predicate::str::is_empty());
    let stdout = String::from_utf8(assert.get_output().stdout.clone()).unwrap();
    let value: serde_json::Value = serde_json::from_str(&stdout).unwrap();

    assert_eq!(value["provider"], "claude-code");
    assert_eq!(value["operation"], "claude-code/subprocess");
    assert_eq!(value["network"], false);
    assert_eq!(value["session_id"], "agt-run-claude-code-dry");
    assert_eq!(value["request"]["stdin"], "What is 2+2?");
    assert_eq!(value["request"]["system_prompt_spooled"], false);
    let argv = value["request"]["argv"]
        .as_array()
        .expect("claude-code dry-run should expose argv");
    assert!(argv.iter().any(|value| value == "--strict-mcp-config"));
    assert!(argv.iter().any(|value| value == "--disable-slash-commands"));
    assert!(argv.iter().any(|value| value == "claude-sonnet-4-6"));
    assert!(argv.iter().any(|value| value == "32"));

    let session_dir = home
        .path()
        .join(".brainyard/sessions/agt-run-claude-code-dry");
    assert!(session_dir.join("meta.edn").is_file());
    assert!(!session_dir.join("messages.log").exists());
    assert!(!session_dir.join("session.edn").exists());
    assert!(!session_dir.join("usage-tracker.edn").exists());
    assert!(!session_dir.join("input-history.edn").exists());
}

#[test]
fn run_acp_dry_run_one_turn_prepares_prompt_request_without_messages_write() {
    let home = tempfile::tempdir().unwrap();

    let assert = Command::cargo_bin("by-rs")
        .unwrap()
        .env("HOME", home.path())
        .env("BRAINYARD_SESSION_ID", "agt-run-acp-dry")
        .env("BY_NO_DOTENV", "1")
        .args([
            "run",
            "--provider",
            "acp",
            "--model",
            "stub",
            "--dry-run",
            "What is 2+2?",
        ])
        .assert()
        .success()
        .stderr(predicate::str::is_empty());
    let stdout = String::from_utf8(assert.get_output().stdout.clone()).unwrap();
    let value: serde_json::Value = serde_json::from_str(&stdout).unwrap();

    assert_eq!(value["provider"], "acp");
    assert_eq!(value["operation"], "acp/session-prompt");
    assert_eq!(value["network"], false);
    assert_eq!(value["session_id"], "agt-run-acp-dry");
    assert_eq!(value["request"]["backend"], "stub");
    assert_eq!(value["request"]["prompt"][0]["text"], "What is 2+2?");
    assert_eq!(value["request"]["timeout_ms"], 600000);

    let session_dir = home.path().join(".brainyard/sessions/agt-run-acp-dry");
    assert!(session_dir.join("meta.edn").is_file());
    assert!(!session_dir.join("messages.log").exists());
    assert!(!session_dir.join("session.edn").exists());
    assert!(!session_dir.join("usage-tracker.edn").exists());
    assert!(!session_dir.join("input-history.edn").exists());
}

#[test]
fn run_anthropic_fixture_one_turn_persists_restore_compatible_messages() {
    let home = tempfile::tempdir().unwrap();
    let fixture = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../fixtures/anthropic/messages-response.json");

    Command::cargo_bin("by-rs")
        .unwrap()
        .env("HOME", home.path())
        .env("BRAINYARD_SESSION_ID", "agt-run-anthropic-fixture")
        .env("BY_NO_DOTENV", "1")
        .args([
            "run",
            "--provider",
            "anthropic",
            "--model",
            "claude-sonnet-4-6",
            "--fixture-response",
        ])
        .arg(fixture)
        .arg("What is 2+2?")
        .assert()
        .success()
        .stdout(predicate::str::contains("Hello from Anthropic fixture"))
        .stderr(predicate::str::is_empty());

    let messages = read_session_messages(home.path(), "agt-run-anthropic-fixture");
    let lines = messages.lines().collect::<Vec<_>>();
    assert_eq!(lines.len(), 2);
    assert!(lines[0].contains(r#":role "user""#));
    assert!(lines[0].contains(r#":content "What is 2+2?""#));
    assert!(lines[1].contains(r#":role "assistant""#));
    assert!(lines[1].contains(r#":content "Hello from Anthropic fixture""#));

    let restored = by_persist::restore_session(
        home.path().join(".brainyard/sessions"),
        "agt-run-anthropic-fixture",
    )
    .unwrap();
    assert_eq!(restored.total_turns, Some(1));
    assert_eq!(restored.agent_activity_seq, Some(1));
    let session_dir = home
        .path()
        .join(".brainyard/sessions/agt-run-anthropic-fixture");
    let usage = std::fs::read_to_string(session_dir.join("usage-tracker.edn")).unwrap();
    assert!(usage.contains(r#""claude-sonnet-4-6""#));
    assert!(usage.contains(":input-tokens 13"));
    assert!(usage.contains(":output-tokens 6"));
    assert!(usage.contains(":total-tokens 19"));
    assert!(usage.contains(":cache-read-tokens 3"));
    assert!(usage.contains(":cache-write-tokens 2"));
    assert!(usage.contains(":call-count 1"));
    let input_history = std::fs::read_to_string(session_dir.join("input-history.edn")).unwrap();
    assert_eq!(input_history.trim(), r#"["What is 2+2?"]"#);
}

#[test]
fn run_openai_fixture_one_turn_persists_restore_compatible_messages() {
    let home = tempfile::tempdir().unwrap();
    let fixture = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../fixtures/openai/chat-completion-response.json");

    Command::cargo_bin("by-rs")
        .unwrap()
        .env("HOME", home.path())
        .env("BRAINYARD_SESSION_ID", "agt-run-openai-fixture")
        .env("BY_NO_DOTENV", "1")
        .args([
            "run",
            "--provider",
            "openai",
            "--model",
            "gpt-5-mini",
            "--fixture-response",
        ])
        .arg(fixture)
        .arg("What is 2+2?")
        .assert()
        .success()
        .stdout(predicate::str::contains("Hello from OpenAI fixture"))
        .stderr(predicate::str::is_empty());

    let restored = by_persist::restore_session(
        home.path().join(".brainyard/sessions"),
        "agt-run-openai-fixture",
    )
    .unwrap();
    assert_eq!(restored.total_turns, Some(1));
    assert_eq!(restored.agent_activity_seq, Some(1));
    let session_dir = home
        .path()
        .join(".brainyard/sessions/agt-run-openai-fixture");
    let usage = std::fs::read_to_string(session_dir.join("usage-tracker.edn")).unwrap();
    assert!(usage.contains(r#""gpt-5-mini""#));
    assert!(usage.contains(":input-tokens 11"));
    assert!(usage.contains(":output-tokens 5"));
    assert!(usage.contains(":total-tokens 16"));
    assert!(usage.contains(":call-count 1"));
    let input_history = std::fs::read_to_string(session_dir.join("input-history.edn")).unwrap();
    assert_eq!(input_history.trim(), r#"["What is 2+2?"]"#);
}

#[test]
fn run_claude_code_fixture_one_turn_persists_restore_compatible_messages() {
    let home = tempfile::tempdir().unwrap();
    let fixture =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/claude-code/result-events.json");

    Command::cargo_bin("by-rs")
        .unwrap()
        .env("HOME", home.path())
        .env("BRAINYARD_SESSION_ID", "agt-run-claude-code-fixture")
        .env("BY_NO_DOTENV", "1")
        .args([
            "run",
            "--provider",
            "claude-code",
            "--model",
            "claude-sonnet-4-6",
            "--fixture-response",
        ])
        .arg(fixture)
        .arg("What is 2+2?")
        .assert()
        .success()
        .stdout(predicate::str::contains("Hello from Claude Code fixture"))
        .stderr(predicate::str::is_empty());

    let restored = by_persist::restore_session(
        home.path().join(".brainyard/sessions"),
        "agt-run-claude-code-fixture",
    )
    .unwrap();
    assert_eq!(restored.total_turns, Some(1));
    assert_eq!(restored.agent_activity_seq, Some(1));
    let session_dir = home
        .path()
        .join(".brainyard/sessions/agt-run-claude-code-fixture");
    let usage = std::fs::read_to_string(session_dir.join("usage-tracker.edn")).unwrap();
    assert!(usage.contains(r#""claude-sonnet-4-6""#));
    assert!(usage.contains(":input-tokens 17"));
    assert!(usage.contains(":output-tokens 4"));
    assert!(usage.contains(":total-tokens 21"));
    assert!(usage.contains(":cache-read-tokens 1"));
    assert!(usage.contains(":cache-write-tokens 2"));
    assert!(usage.contains(":call-count 1"));
    let input_history = std::fs::read_to_string(session_dir.join("input-history.edn")).unwrap();
    assert_eq!(input_history.trim(), r#"["What is 2+2?"]"#);
}

#[test]
fn run_acp_fixture_one_turn_persists_restore_compatible_messages() {
    let home = tempfile::tempdir().unwrap();
    let fixture = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../fixtures/acp/session-prompt-response.json");

    Command::cargo_bin("by-rs")
        .unwrap()
        .env("HOME", home.path())
        .env("BRAINYARD_SESSION_ID", "agt-run-acp-fixture")
        .env("BY_NO_DOTENV", "1")
        .args([
            "run",
            "--provider",
            "acp",
            "--model",
            "stub",
            "--fixture-response",
        ])
        .arg(fixture)
        .arg("What is 2+2?")
        .assert()
        .success()
        .stdout(predicate::str::contains("Hello from ACP fixture"))
        .stderr(predicate::str::is_empty());

    let restored = by_persist::restore_session(
        home.path().join(".brainyard/sessions"),
        "agt-run-acp-fixture",
    )
    .unwrap();
    assert_eq!(restored.total_turns, Some(1));
    assert_eq!(restored.agent_activity_seq, Some(1));
    let session_dir = home.path().join(".brainyard/sessions/agt-run-acp-fixture");
    let usage = std::fs::read_to_string(session_dir.join("usage-tracker.edn")).unwrap();
    assert!(usage.contains(r#""stub""#));
    assert!(usage.contains(":input-tokens 0"));
    assert!(usage.contains(":output-tokens 0"));
    assert!(usage.contains(":total-tokens 0"));
    assert!(usage.contains(":call-count 1"));
    let input_history = std::fs::read_to_string(session_dir.join("input-history.edn")).unwrap();
    assert_eq!(input_history.trim(), r#"["What is 2+2?"]"#);
}

#[test]
fn run_fixture_one_turn_rejects_locked_session_before_messages_write() {
    let home = tempfile::tempdir().unwrap();
    let fixture =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/bedrock/converse-response.json");
    let session_dir = home.path().join(".brainyard/sessions/agt-run-locked");
    std::fs::create_dir_all(&session_dir).unwrap();
    std::fs::write(
        session_dir.join("by-host.lock"),
        format!("{}\n", std::process::id()),
    )
    .unwrap();

    Command::cargo_bin("by-rs")
        .unwrap()
        .env("HOME", home.path())
        .env("BRAINYARD_SESSION_ID", "agt-run-locked")
        .env("BY_NO_DOTENV", "1")
        .args([
            "run",
            "--provider",
            "bedrock",
            "--model",
            "amazon.nova-lite-v1:0",
            "--fixture-response",
            fixture.to_str().unwrap(),
            "What is 2+2?",
        ])
        .assert()
        .failure()
        .stderr(predicate::str::contains(
            "Session is locked by another process: agt-run-locked",
        ));

    assert!(!session_dir.join("messages.log").exists());
    assert_eq!(
        std::fs::read_to_string(session_dir.join("by-host.lock")).unwrap(),
        format!("{}\n", std::process::id())
    );
}

#[test]
fn run_one_turn_mode_rejects_non_bedrock_before_network() {
    let home = tempfile::tempdir().unwrap();

    Command::cargo_bin("by-rs")
        .unwrap()
        .env("HOME", home.path())
        .env("BRAINYARD_SESSION_ID", "agt-run-openai")
        .env("BY_NO_DOTENV", "1")
        .args([
            "run",
            "--provider",
            "openai",
            "--model",
            "gpt-5",
            "--live",
            "What is 2+2?",
        ])
        .assert()
        .failure()
        .stderr(predicate::str::contains(
            "by-rs run live one-turn currently supports providers 'bedrock', 'claude-code', and 'ollama' only",
        ));

    assert!(!home
        .path()
        .join(".brainyard/sessions/agt-run-openai")
        .exists());
}

#[test]
fn run_one_turn_mode_rejects_missing_prompt_before_session_write() {
    let home = tempfile::tempdir().unwrap();

    Command::cargo_bin("by-rs")
        .unwrap()
        .env("HOME", home.path())
        .env("BRAINYARD_SESSION_ID", "agt-run-missing-prompt")
        .env("BY_NO_DOTENV", "1")
        .args([
            "run",
            "--provider",
            "bedrock",
            "--model",
            "amazon.nova-lite-v1:0",
            "--dry-run",
        ])
        .assert()
        .failure()
        .stderr(predicate::str::contains(
            "by-rs run one-turn mode requires a prompt",
        ));

    assert!(!home
        .path()
        .join(".brainyard/sessions/agt-run-missing-prompt")
        .exists());
}

#[test]
fn agents_help_matches_clojure_command_surface() {
    Command::cargo_bin("by-rs")
        .unwrap()
        .args(["agents", "--help"])
        .assert()
        .success()
        .stderr(predicate::str::contains(
            "NAME:\n by agents - List available agents",
        ))
        .stderr(predicate::str::contains(
            "USAGE:\n by agents [command options] [arguments...]",
        ))
        .stderr(predicate::str::contains("--fixture").not());
}

#[test]
fn agents_command_reads_registry_fixture() {
    let dir = tempfile::tempdir().unwrap();
    let fixture = dir.path().join("registry.json");
    std::fs::write(
        &fixture,
        r#"{"agents":[{"id":"coder","name":"Coder","description":"Writes code"}],"models":[]}"#,
    )
    .unwrap();

    Command::cargo_bin("by-rs")
        .unwrap()
        .args(["agents", "--fixture"])
        .arg(&fixture)
        .assert()
        .success()
        .stdout(predicate::str::contains("1 agent(s) available:"))
        .stdout(predicate::str::contains("AGENT"))
        .stdout(predicate::str::contains("coder"))
        .stdout(predicate::str::contains("Writes code"));
}

#[test]
fn agent_registry_instances_is_live_free_json_projection() {
    let agents = r#"{
        :agents [
          {:agent-id "root" :turn 4 :iter 2 :status :running}
          {:id "child" :state {:status :waiting :parent-id "root"}
           :memory-init {:turn-id 1}
           :bt-st-memory {:iteration-count 3}}
        ]
        :total-turns 9
    }"#;
    let assert = Command::cargo_bin("by-rs")
        .unwrap()
        .args(["agent-registry", "instances", "--agents-json", agents])
        .assert()
        .success()
        .stderr(predicate::str::is_empty());
    let stdout = String::from_utf8(assert.get_output().stdout.clone()).unwrap();
    let instances: serde_json::Value = serde_json::from_str(&stdout).unwrap();
    assert_eq!(
        instances["projection"],
        "common.commands/agent-registry$instances"
    );
    assert_eq!(instances["live-skipped?"], true);
    assert_eq!(instances["total"], 2);
    assert_eq!(instances["total-turns"], 9);
    assert_eq!(instances["agents"][0]["agent-id"], "root");
    assert_eq!(instances["agents"][0]["turn"], 4);
    assert_eq!(instances["agents"][0]["iter"], 2);
    assert_eq!(instances["agents"][0]["parent-id"], serde_json::Value::Null);
    assert_eq!(instances["agents"][0]["status"], "running");
    assert_eq!(instances["agents"][1]["agent-id"], "child");
    assert_eq!(instances["agents"][1]["turn"], 1);
    assert_eq!(instances["agents"][1]["iter"], 3);
    assert_eq!(instances["agents"][1]["parent-id"], "root");
    assert_eq!(instances["agents"][1]["status"], "waiting");
}

#[test]
fn agents_command_truncates_multiline_descriptions_like_clojure() {
    let dir = tempfile::tempdir().unwrap();
    let fixture = dir.path().join("registry.json");
    let registry = serde_json::json!({
        "agents": [{
            "id": "coder",
            "name": "Coder",
            "description": "First line\n    second line\nthird line"
        }],
        "models": []
    });
    std::fs::write(&fixture, registry.to_string()).unwrap();

    let assert = Command::cargo_bin("by-rs")
        .unwrap()
        .args(["agents", "--fixture"])
        .arg(&fixture)
        .assert()
        .success();
    let stdout = String::from_utf8(assert.get_output().stdout.clone()).unwrap();

    assert!(stdout.contains("  coder       First line\n              second line ..."));
    assert!(!stdout.contains("third line"));
}

#[test]
fn agents_command_uses_embedded_registry_by_default() {
    Command::cargo_bin("by-rs")
        .unwrap()
        .arg("agents")
        .assert()
        .success()
        .stdout(predicate::str::contains("agent(s) available:"))
        .stdout(predicate::str::contains("coact-agent"))
        .stdout(predicate::str::contains("main-agent"))
        .stdout(predicate::str::contains("tool-agent"));
}

#[test]
fn models_help_matches_clojure_command_surface() {
    Command::cargo_bin("by-rs")
        .unwrap()
        .args(["models", "--help"])
        .assert()
        .success()
        .stderr(predicate::str::contains(
            "NAME:\n by models - List available LLM models (provider/model)",
        ))
        .stderr(predicate::str::contains(
            "-p, --provider S  Filter to a single provider",
        ))
        .stderr(predicate::str::contains("--fixture").not());
}

#[test]
fn models_command_reads_registry_fixture() {
    let dir = tempfile::tempdir().unwrap();
    let fixture = dir.path().join("registry.json");
    std::fs::write(
        &fixture,
        r#"{"agents":[],"models":[{"provider":"bedrock","id":"amazon.nova-lite-v1:0"}]}"#,
    )
    .unwrap();

    Command::cargo_bin("by-rs")
        .unwrap()
        .args(["models", "--fixture"])
        .arg(&fixture)
        .assert()
        .success()
        .stdout(predicate::str::contains("PROVIDER"))
        .stdout(predicate::str::contains("MODEL"))
        .stdout(predicate::str::contains("bedrock"))
        .stdout(predicate::str::contains("amazon.nova-lite-v1:0"))
        .stdout(predicate::str::contains("1 model(s) listed."));
}

#[test]
fn models_command_uses_embedded_registry_by_default() {
    Command::cargo_bin("by-rs")
        .unwrap()
        .arg("models")
        .assert()
        .success()
        .stdout(predicate::str::contains("PROVIDER"))
        .stdout(predicate::str::contains("MODEL"))
        .stdout(predicate::str::contains("bedrock"))
        .stdout(predicate::str::contains("amazon.nova-lite-v1:0"))
        .stdout(predicate::str::contains("model(s) listed."));
}

#[test]
fn sessions_help_matches_clojure_command_surface() {
    Command::cargo_bin("by-rs")
        .unwrap()
        .args(["sessions", "--help"])
        .assert()
        .success()
        .stderr(predicate::str::contains(
            "NAME:\n by sessions - List or prune persisted agent sessions",
        ))
        .stderr(predicate::str::contains(
            "USAGE:\n by sessions [global-options] command [command options] [arguments...]",
        ))
        .stderr(predicate::str::contains(
            "list                 List all persisted sessions",
        ))
        .stderr(predicate::str::contains(
            "prune                Delete a persisted session",
        ));
}

#[test]
fn sessions_list_help_matches_clojure_command_surface() {
    Command::cargo_bin("by-rs")
        .unwrap()
        .args(["sessions", "list", "--help"])
        .assert()
        .success()
        .stdout(predicate::str::is_empty())
        .stderr(predicate::str::contains(
            "NAME:\n by sessions list - List all persisted sessions",
        ))
        .stderr(predicate::str::contains(
            "USAGE:\n by sessions list [command options] [arguments...]",
        ))
        .stderr(predicate::str::contains("OPTIONS:\n   -?, --help"));
}

#[test]
fn sessions_prune_help_matches_clojure_command_surface() {
    Command::cargo_bin("by-rs")
        .unwrap()
        .args(["sessions", "prune", "--help"])
        .assert()
        .success()
        .stderr(predicate::str::contains(
            "NAME:\n by sessions prune - Delete a persisted session",
        ))
        .stderr(predicate::str::contains("-s, --session-id S  Session ID"))
        .stderr(predicate::str::contains("--root").not());
}

#[test]
fn sessions_list_reads_fixture_root_without_writing() {
    let root = tempfile::tempdir().unwrap();
    let session = root.path().join("alpha");
    std::fs::create_dir_all(&session).unwrap();
    std::fs::write(
        session.join("meta.edn"),
        r#"{:id "stale-meta-id"
            :label "Alpha session"
            :defagent-id :coact-agent
            :started-at 1780290000000
            :last-attached-at 1780290100000}"#,
    )
    .unwrap();
    std::fs::write(session.join("messages.log"), "hello").unwrap();

    Command::cargo_bin("by-rs")
        .unwrap()
        .args(["sessions", "list", "--root"])
        .arg(root.path())
        .assert()
        .success()
        .stdout(predicate::str::contains("session-id"))
        .stdout(predicate::str::contains("alpha"))
        .stdout(predicate::str::contains("stale-meta-id").not())
        .stdout(predicate::str::contains("Alpha session"))
        .stdout(predicate::str::contains("coact-agent"))
        .stdout(predicate::str::contains("B"));
}

#[test]
fn sessions_list_keeps_corrupt_meta_visible_and_warns_on_stderr() {
    let root = tempfile::tempdir().unwrap();
    let session = root.path().join("broken");
    std::fs::create_dir_all(&session).unwrap();
    std::fs::write(session.join("meta.edn"), "{:id ").unwrap();
    std::fs::write(session.join("messages.log"), "hello").unwrap();

    Command::cargo_bin("by-rs")
        .unwrap()
        .args(["sessions", "list", "--root"])
        .arg(root.path())
        .assert()
        .success()
        .stdout(predicate::str::contains("broken"))
        .stdout(predicate::str::contains("session-id"))
        .stderr(predicate::str::contains(
            "[persist] skipping unreadable meta.edn for broken:",
        ));
}

#[test]
fn sessions_inspect_reports_restore_and_snapshot_status_without_writing() {
    let root = tempfile::tempdir().unwrap();
    let session = root.path().join("alpha");
    std::fs::create_dir_all(&session).unwrap();
    std::fs::write(
        session.join("meta.edn"),
        r#"{:user-id "alice" :defagent-id :coact-agent :started-at 1000}"#,
    )
    .unwrap();
    std::fs::write(
        session.join("session.edn"),
        r#"{:user-id "bob" :agent-id :session-agent :total-turns 2 :agent-activity-seq 9}"#,
    )
    .unwrap();
    std::fs::write(
        session.join("messages.log"),
        r#"{:t 1 :kind :message :payload {:role "user" :content "First turn"}}
{:t 2 :kind :message :payload {:role "assistant" :content "Answer"}}
"#,
    )
    .unwrap();
    std::fs::write(
        session.join("usage-tracker.edn"),
        r#"{:totals {:call-count 1} :history []}"#,
    )
    .unwrap();
    std::fs::write(session.join("by-host.lock"), "1234").unwrap();

    let assert = Command::cargo_bin("by-rs")
        .unwrap()
        .args(["sessions", "inspect", "--root"])
        .arg(root.path())
        .arg("alpha")
        .assert()
        .success()
        .stderr(predicate::str::is_empty());
    let stdout = String::from_utf8(assert.get_output().stdout.clone()).unwrap();
    let value: serde_json::Value = serde_json::from_str(&stdout).unwrap();

    assert_eq!(value["session_id"], "alpha");
    assert_eq!(value["restore_status"], "ok");
    assert_eq!(value["user_id"], "bob");
    assert_eq!(value["agent"], "session-agent");
    assert_eq!(value["total_turns"], 2);
    assert_eq!(value["agent_activity_seq"], 9);
    assert_eq!(value["message_count"], 2);
    assert_eq!(value["usage_tracker_present"], true);
    assert_eq!(value["lock"]["status"], "present");
    assert_eq!(value["lock"]["bytes"], 4);
    let snapshots = value["snapshots"].as_array().unwrap();
    let session_status = snapshots
        .iter()
        .find(|snapshot| snapshot["file"] == "session.edn")
        .unwrap();
    assert_eq!(session_status["status"], "ok");
    let layout_status = snapshots
        .iter()
        .find(|snapshot| snapshot["file"] == "layout.edn")
        .unwrap();
    assert_eq!(layout_status["status"], "missing");
}

#[test]
fn sessions_audit_reports_unsupported_artifacts_without_writing() {
    let root = tempfile::tempdir().unwrap();
    let session = root.path().join("alpha");
    std::fs::create_dir_all(session.join("custom-dir")).unwrap();
    std::fs::write(
        session.join("meta.edn"),
        r#"{:user-id "alice" :defagent-id :coact-agent :started-at 1000}"#,
    )
    .unwrap();
    std::fs::write(
        session.join("messages.log"),
        r#"{:t 1 :kind :message :payload {:role "user" :content "First turn"}}"#,
    )
    .unwrap();
    std::fs::write(session.join("scrollback.stream.1.txt"), "rotated").unwrap();
    std::fs::write(session.join("custom.edn"), "{:extra true}").unwrap();
    std::fs::write(session.join("custom-dir/note.txt"), "nested").unwrap();

    let assert = Command::cargo_bin("by-rs")
        .unwrap()
        .args(["sessions", "audit", "--root"])
        .arg(root.path())
        .assert()
        .success()
        .stderr(predicate::str::is_empty());
    let stdout = String::from_utf8(assert.get_output().stdout.clone()).unwrap();
    let value: serde_json::Value = serde_json::from_str(&stdout).unwrap();

    assert_eq!(value["session_count"], 1);
    assert_eq!(value["ready_count"], 0);
    assert_eq!(value["unsupported_file_count"], 2);
    let session_report = &value["sessions"][0];
    assert_eq!(session_report["session_id"], "alpha");
    assert_eq!(session_report["restore_status"], "ok");
    assert_eq!(session_report["message_count"], 1);
    assert_eq!(session_report["migration_ready"], false);
    let unsupported = session_report["unsupported_files"].as_array().unwrap();
    assert!(unsupported
        .iter()
        .any(|entry| entry["path"] == "custom.edn" && entry["kind"] == "file"));
    assert!(unsupported
        .iter()
        .any(|entry| entry["path"] == "custom-dir" && entry["kind"] == "directory"));
    assert!(session.join("custom.edn").exists());
    assert!(session.join("custom-dir/note.txt").exists());
}

#[test]
fn sessions_audit_reports_restore_errors_but_keeps_scanning_artifacts() {
    let root = tempfile::tempdir().unwrap();
    let session = root.path().join("broken");
    std::fs::create_dir_all(&session).unwrap();
    std::fs::write(session.join("meta.edn"), "{:user-id ").unwrap();
    std::fs::write(session.join("mystery.log"), "still visible").unwrap();

    let assert = Command::cargo_bin("by-rs")
        .unwrap()
        .args(["sessions", "audit", "--root"])
        .arg(root.path())
        .arg("broken")
        .assert()
        .success()
        .stderr(predicate::str::is_empty());
    let stdout = String::from_utf8(assert.get_output().stdout.clone()).unwrap();
    let value: serde_json::Value = serde_json::from_str(&stdout).unwrap();
    let session_report = &value["sessions"][0];

    assert_eq!(value["session_count"], 1);
    assert_eq!(session_report["session_id"], "broken");
    assert_eq!(session_report["restore_status"], "error");
    assert!(session_report["restore_error"]
        .as_str()
        .unwrap()
        .contains("failed to parse session meta"));
    assert_eq!(session_report["migration_ready"], false);
    assert_eq!(session_report["unsupported_file_count"], 1);
    assert_eq!(
        session_report["unsupported_files"][0]["path"],
        "mystery.log"
    );
}

#[test]
fn sessions_audit_empty_root_is_json_empty_report() {
    let root = tempfile::tempdir().unwrap();

    let assert = Command::cargo_bin("by-rs")
        .unwrap()
        .args(["sessions", "audit", "--root"])
        .arg(root.path().join("missing"))
        .assert()
        .success()
        .stderr(predicate::str::is_empty());
    let stdout = String::from_utf8(assert.get_output().stdout.clone()).unwrap();
    let value: serde_json::Value = serde_json::from_str(&stdout).unwrap();

    assert_eq!(value["session_count"], 0);
    assert_eq!(value["ready_count"], 0);
    assert_eq!(value["unsupported_file_count"], 0);
    assert_eq!(value["sessions"].as_array().unwrap().len(), 0);
}

#[test]
fn sessions_prune_deletes_fixture_session_by_positional_id() {
    let root = tempfile::tempdir().unwrap();
    let session = root.path().join("doomed");
    std::fs::create_dir_all(session.join("nested")).unwrap();
    std::fs::write(session.join("nested/messages.log"), "goodbye").unwrap();

    Command::cargo_bin("by-rs")
        .unwrap()
        .args(["sessions", "prune", "--root"])
        .arg(root.path())
        .arg("doomed")
        .assert()
        .success()
        .stdout(predicate::str::contains("Deleted session: doomed"));

    assert!(!session.exists());
}

#[test]
fn sessions_prune_reports_missing_session_without_creating_it() {
    let root = tempfile::tempdir().unwrap();

    Command::cargo_bin("by-rs")
        .unwrap()
        .args(["sessions", "prune", "--root"])
        .arg(root.path())
        .args(["--session-id", "missing"])
        .assert()
        .success()
        .stdout(predicate::str::contains("Session not found: missing"));

    assert!(!root.path().join("missing").exists());
}

#[test]
fn sessions_prune_without_session_id_matches_clojure_usage() {
    let root = tempfile::tempdir().unwrap();

    Command::cargo_bin("by-rs")
        .unwrap()
        .args(["sessions", "prune", "--root"])
        .arg(root.path())
        .assert()
        .failure()
        .stdout(predicate::str::is_empty())
        .stderr(predicate::str::contains(
            "Usage: by sessions prune <session-id>",
        ))
        .stderr(predicate::str::contains("by-rs sessions prune").not());
}

#[test]
fn models_command_filters_by_provider() {
    let dir = tempfile::tempdir().unwrap();
    let fixture = dir.path().join("registry.json");
    std::fs::write(
        &fixture,
        r#"{"agents":[],"models":[{"provider":"bedrock","id":"amazon.nova-lite-v1:0"},{"provider":"openai","id":"gpt-5"}]}"#,
    )
    .unwrap();

    Command::cargo_bin("by-rs")
        .unwrap()
        .args(["models", "--fixture"])
        .arg(&fixture)
        .args(["--provider", "bedrock"])
        .assert()
        .success()
        .stdout(predicate::str::contains("amazon.nova-lite-v1:0"))
        .stdout(predicate::str::contains(
            "1 model(s) listed. (filtered to bedrock)",
        ))
        .stdout(predicate::str::contains("gpt-5").not())
        .stdout(predicate::str::contains("openai").not());
}

#[test]
fn models_command_accepts_positional_provider_like_clojure() {
    Command::cargo_bin("by-rs")
        .unwrap()
        .args(["models", "claude-code"])
        .assert()
        .success()
        .stderr(predicate::str::is_empty())
        .stdout(predicate::str::contains("claude-code"))
        .stdout(predicate::str::contains("anthropic"))
        .stdout(predicate::str::contains("filtered to claude-code").not());
}

#[test]
fn models_command_accepts_short_provider_flag_like_clojure() {
    let dir = tempfile::tempdir().unwrap();
    let fixture = dir.path().join("registry.json");
    std::fs::write(
        &fixture,
        r#"{"agents":[],"models":[{"provider":"bedrock","id":"amazon.nova-lite-v1:0"},{"provider":"openai","id":"gpt-5"}]}"#,
    )
    .unwrap();

    Command::cargo_bin("by-rs")
        .unwrap()
        .args(["models", "--fixture"])
        .arg(&fixture)
        .args(["-p", "bedrock"])
        .assert()
        .success()
        .stdout(predicate::str::contains("amazon.nova-lite-v1:0"))
        .stdout(predicate::str::contains(
            "1 model(s) listed. (filtered to bedrock)",
        ))
        .stdout(predicate::str::contains("gpt-5").not());
}

#[test]
fn models_command_displays_region_when_present() {
    let dir = tempfile::tempdir().unwrap();
    let fixture = dir.path().join("registry.json");
    std::fs::write(
        &fixture,
        r#"{"agents":[],"models":[{"provider":"bedrock","id":"openai.gpt-oss-120b-1:0","region":"us-east-1"}]}"#,
    )
    .unwrap();

    Command::cargo_bin("by-rs")
        .unwrap()
        .args(["models", "--fixture"])
        .arg(&fixture)
        .assert()
        .success()
        .stdout(predicate::str::contains("bedrock"))
        .stdout(predicate::str::contains(
            "openai.gpt-oss-120b-1:0 (us-east-1)",
        ));
}

#[test]
fn llm_list_models_is_live_free_json_projection() {
    let dir = tempfile::tempdir().unwrap();
    let fixture = dir.path().join("registry.json");
    std::fs::write(
        &fixture,
        r#"{"agents":[],"models":[
            {"provider":"bedrock","id":"amazon.nova-lite-v1:0","description":"Nova Lite"},
            {"provider":"bedrock","id":"openai.gpt-oss-120b-1:0","description":"gpt-oss","region":"us-east-1"},
            {"provider":"openai","id":"gpt-5","description":"GPT-5"}
        ]}"#,
    )
    .unwrap();

    let assert = Command::cargo_bin("by-rs")
        .unwrap()
        .args([
            "llm",
            "list-models",
            "--fixture",
            fixture.to_str().unwrap(),
            "--provider",
            ":bedrock",
            "--limit",
            "1",
        ])
        .assert()
        .success()
        .stderr(predicate::str::is_empty());
    let stdout = String::from_utf8(assert.get_output().stdout.clone()).unwrap();
    let value: serde_json::Value = serde_json::from_str(&stdout).unwrap();

    assert_eq!(value["projection"], "common.commands/llm$list-models");
    assert_eq!(value["provider"], "bedrock");
    assert_eq!(value["count"], 1);
    assert_eq!(value["total"], 3);
    assert_eq!(value["providers"][0], "bedrock");
    assert_eq!(value["providers"][1], "openai");
    assert_eq!(value["models"][0]["provider"], "bedrock");
    assert_eq!(value["models"][0]["model"], "amazon.nova-lite-v1:0");
    assert_eq!(value["models"][0]["description"], "Nova Lite");
    assert_eq!(value["network-skipped?"], true);

    let assert = Command::cargo_bin("by-rs")
        .unwrap()
        .args([
            "llm",
            "list-models",
            "--fixture",
            fixture.to_str().unwrap(),
            "--provider",
            "bedrock",
            "--limit",
            "0",
        ])
        .assert()
        .success()
        .stderr(predicate::str::is_empty());
    let stdout = String::from_utf8(assert.get_output().stdout.clone()).unwrap();
    let uncapped: serde_json::Value = serde_json::from_str(&stdout).unwrap();
    assert_eq!(uncapped["count"], 2);
    assert_eq!(uncapped["models"][1]["region"], "us-east-1");
}

#[test]
fn models_command_truncates_long_descriptions_like_clojure() {
    let dir = tempfile::tempdir().unwrap();
    let fixture = dir.path().join("registry.json");
    let registry = serde_json::json!({
        "agents": [],
        "models": [{
            "provider": "bedrock",
            "id": "amazon.nova-lite-v1:0",
            "description": "abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789"
        }]
    });
    std::fs::write(&fixture, registry.to_string()).unwrap();

    Command::cargo_bin("by-rs")
        .unwrap()
        .args(["models", "--fixture"])
        .arg(&fixture)
        .assert()
        .success()
        .stdout(predicate::str::contains(
            "abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ01234...",
        ))
        .stdout(
            predicate::str::contains("abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ012345")
                .not(),
        );
}

#[test]
fn tools_command_reads_registry_fixture_and_filters_by_type() {
    let dir = tempfile::tempdir().unwrap();
    let fixture = dir.path().join("registry.json");
    std::fs::write(
        &fixture,
        r#"{"tools":[
             {"id":"grep","type":"tool","description":"Search files"},
             {"id":"code$eval","type":"command","description":"Evaluate code"}
           ],"agents":[],"models":[]}"#,
    )
    .unwrap();

    Command::cargo_bin("by-rs")
        .unwrap()
        .args(["tools", "--fixture"])
        .arg(&fixture)
        .args(["--type", "command"])
        .assert()
        .success()
        .stdout(predicate::str::contains("TOOL"))
        .stdout(predicate::str::contains("TYPE"))
        .stdout(predicate::str::contains("code$eval"))
        .stdout(predicate::str::contains("command"))
        .stdout(predicate::str::contains("Evaluate code"))
        .stdout(predicate::str::contains(
            "1 tool(s) listed. (filtered to type command)",
        ))
        .stdout(predicate::str::contains("grep").not());
}

#[test]
fn tools_command_reads_standalone_tools_fixture_and_filters_by_id() {
    let dir = tempfile::tempdir().unwrap();
    let fixture = dir.path().join("tools.json");
    std::fs::write(
        &fixture,
        r#"[
             {"id":"grep","type":"tool","description":"Search files"},
             {"id":"memory$recall","type":"command","description":"Recall memory"}
           ]"#,
    )
    .unwrap();

    Command::cargo_bin("by-rs")
        .unwrap()
        .args(["tools", "--fixture"])
        .arg(&fixture)
        .args(["--id", "grep"])
        .assert()
        .success()
        .stdout(predicate::str::contains("grep"))
        .stdout(predicate::str::contains("Search files"))
        .stdout(predicate::str::contains(
            "1 tool(s) listed. (filtered to id grep)",
        ))
        .stdout(predicate::str::contains("memory$recall").not());
}

#[test]
fn tools_command_reads_refreshed_oracle_user_tool_commands() {
    let fixture = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/oracle/tools.json");

    Command::cargo_bin("by-rs")
        .unwrap()
        .args(["tools", "--fixture"])
        .arg(&fixture)
        .args(["--id", "tools$create"])
        .assert()
        .success()
        .stdout(predicate::str::contains("tools$create"))
        .stdout(predicate::str::contains("command"))
        .stdout(predicate::str::contains("PERSISTENT tool"))
        .stdout(predicate::str::contains(
            "1 tool(s) listed. (filtered to id tools$create)",
        ))
        .stdout(predicate::str::contains("tools$list").not());

    Command::cargo_bin("by-rs")
        .unwrap()
        .args(["tools", "--fixture"])
        .arg(&fixture)
        .args(["--id", "tools$validate"])
        .assert()
        .success()
        .stdout(predicate::str::contains("tools$validate"))
        .stdout(predicate::str::contains("command"))
        .stdout(predicate::str::contains("valid"))
        .stdout(predicate::str::contains(
            "1 tool(s) listed. (filtered to id tools$validate)",
        ))
        .stdout(predicate::str::contains("tools$create").not());
}

#[test]
fn mcp_servers_hidden_command_projects_clojure_list_shape() {
    let dir = tempfile::tempdir().unwrap();
    let fixture = dir.path().join("registry.json");
    std::fs::write(
        &fixture,
        r#"{"agents":[],"models":[],"mcpServers":[
             {"name":"filesystem","transport":"stdio","config":{"command":"npx","args":["-y","server"]},"enabled":false,"autoRegisterTools":true},
             {"name":"api-server","transport":"http","config":{"url":"https://api.example.com"},"enabled":false,"autoRegisterTools":true}
           ]}"#,
    )
    .unwrap();

    let assert = Command::cargo_bin("by-rs")
        .unwrap()
        .args(["mcp", "servers", "--fixture"])
        .arg(&fixture)
        .assert()
        .success();
    let stdout = String::from_utf8(assert.get_output().stdout.clone()).unwrap();
    let value: serde_json::Value = serde_json::from_str(&stdout).unwrap();

    assert_eq!(value["result"]["total"], 2);
    assert_eq!(value["result"]["connected"], 0);
    assert_eq!(value["result"]["servers"][0]["name"], "api-server");
    assert_eq!(value["result"]["servers"][0]["connected"], false);
    assert_eq!(value["result"]["servers"][0]["transport"], "http");
    assert_eq!(value["result"]["servers"][1]["name"], "filesystem");
    assert_eq!(value["result"]["servers"][1]["transport"], "stdio");
}

#[test]
fn mcp_config_hidden_command_projects_clojure_config_shape() {
    let dir = tempfile::tempdir().unwrap();
    let fixture = dir.path().join("mcp-servers.json");
    std::fs::write(
        &fixture,
        r#"{
          "gmail": {
            "transport": "stdio",
            "config": {"command": "bash", "args": ["-c", "npx -y mcp-remote https://gmailmcp.googleapis.com/mcp/v1"]},
            "enabled": false,
            "auto-register-tools": true
          }
        }"#,
    )
    .unwrap();

    let assert = Command::cargo_bin("by-rs")
        .unwrap()
        .args(["mcp", "config", "--fixture"])
        .arg(&fixture)
        .arg("gmail")
        .assert()
        .success();
    let stdout = String::from_utf8(assert.get_output().stdout.clone()).unwrap();
    let value: serde_json::Value = serde_json::from_str(&stdout).unwrap();

    assert_eq!(value["result"]["name"], "gmail");
    assert_eq!(value["result"]["config"]["transport"], "stdio");
    assert_eq!(value["result"]["config"]["enabled"], false);
    assert_eq!(value["result"]["config"]["auto-register-tools"], true);
    assert_eq!(value["result"]["config"]["config"]["command"], "bash");
}

#[test]
fn mcp_config_hidden_command_projects_missing_server_to_error_shape() {
    let dir = tempfile::tempdir().unwrap();
    let fixture = dir.path().join("mcp-servers.json");
    std::fs::write(
        &fixture,
        r#"{
          "gmail": {
            "transport": "stdio",
            "config": {"command": "bash", "args": ["-c", "npx -y server"]},
            "enabled": false,
            "auto-register-tools": true
          }
        }"#,
    )
    .unwrap();

    let assert = Command::cargo_bin("by-rs")
        .unwrap()
        .args(["mcp", "config", "--fixture"])
        .arg(&fixture)
        .arg("missing")
        .assert()
        .success();
    let stdout = String::from_utf8(assert.get_output().stdout.clone()).unwrap();
    let value: serde_json::Value = serde_json::from_str(&stdout).unwrap();

    assert_eq!(
        value["error"],
        "MCP server 'missing' not found in configuration"
    );
}

#[test]
fn mcp_config_hidden_command_projects_blank_server_to_error_shape() {
    let dir = tempfile::tempdir().unwrap();
    let fixture = dir.path().join("mcp-servers.json");
    std::fs::write(&fixture, r#"{}"#).unwrap();

    let assert = Command::cargo_bin("by-rs")
        .unwrap()
        .args(["mcp", "config", "--fixture"])
        .arg(&fixture)
        .arg("")
        .assert()
        .success();
    let stdout = String::from_utf8(assert.get_output().stdout.clone()).unwrap();
    let value: serde_json::Value = serde_json::from_str(&stdout).unwrap();

    assert_eq!(value["error"], "server-name is required");
}

#[test]
fn mcp_info_hidden_command_projects_blank_server_to_error_shape() {
    assert_json_error(
        &[
            "mcp",
            "info",
            "--server-name",
            "",
            "--fixture-response",
            "/definitely/missing/initialize-response.json",
        ],
        "server-name is required",
    );
}

#[test]
fn mcp_info_hidden_command_projects_initialize_fixture_to_clojure_shape() {
    let dir = tempfile::tempdir().unwrap();
    let fixture = dir.path().join("initialize-response.json");
    std::fs::write(
        &fixture,
        r#"{"jsonrpc":"2.0","id":47,"result":{"protocolVersion":"2024-11-05","serverInfo":{"name":"filesystem","version":"1.0.0"},"capabilities":{"resources":{},"tools":{}}}}"#,
    )
    .unwrap();

    let assert = Command::cargo_bin("by-rs")
        .unwrap()
        .args([
            "mcp",
            "info",
            "--server-name",
            "filesystem",
            "--fixture-response",
        ])
        .arg(&fixture)
        .args(["--request-id", "47"])
        .assert()
        .success();
    let stdout = String::from_utf8(assert.get_output().stdout.clone()).unwrap();
    let value: serde_json::Value = serde_json::from_str(&stdout).unwrap();

    assert_eq!(value["result"]["name"], "filesystem");
    assert_eq!(
        value["result"]["server-info"]["serverInfo"]["name"],
        "filesystem"
    );
    assert_eq!(
        value["result"]["server-info"]["capabilities"]["resources"],
        serde_json::json!({})
    );
}

#[test]
fn mcp_info_hidden_command_projects_error_fixture_to_command_result() {
    let dir = tempfile::tempdir().unwrap();
    let fixture = dir.path().join("initialize-error.json");
    std::fs::write(
        &fixture,
        r#"{"jsonrpc":"2.0","id":52,"error":{"code":-32000,"message":"info failed"}}"#,
    )
    .unwrap();

    let assert = Command::cargo_bin("by-rs")
        .unwrap()
        .args([
            "mcp",
            "info",
            "--server-name",
            "filesystem",
            "--fixture-response",
        ])
        .arg(&fixture)
        .args(["--request-id", "52"])
        .assert()
        .success();
    let stdout = String::from_utf8(assert.get_output().stdout.clone()).unwrap();
    let value: serde_json::Value = serde_json::from_str(&stdout).unwrap();

    assert_eq!(
        value["error"],
        "Failed to get server info for 'filesystem': info failed"
    );
}

#[test]
fn mcp_capabilities_hidden_command_projects_initialize_fixture_to_clojure_shape() {
    let dir = tempfile::tempdir().unwrap();
    let fixture = dir.path().join("initialize-response.json");
    std::fs::write(
        &fixture,
        r#"{"jsonrpc":"2.0","id":48,"result":{"protocolVersion":"2024-11-05","serverInfo":{"name":"filesystem","version":"1.0.0"},"capabilities":{"resources":{"subscribe":true},"prompts":{},"tools":{}}}}"#,
    )
    .unwrap();

    let assert = Command::cargo_bin("by-rs")
        .unwrap()
        .args([
            "mcp",
            "capabilities",
            "--server-name",
            "filesystem",
            "--fixture-response",
        ])
        .arg(&fixture)
        .args(["--request-id", "48"])
        .assert()
        .success();
    let stdout = String::from_utf8(assert.get_output().stdout.clone()).unwrap();
    let value: serde_json::Value = serde_json::from_str(&stdout).unwrap();

    assert_eq!(value["result"]["name"], "filesystem");
    assert_eq!(
        value["result"]["capabilities"]["resources"]["subscribe"],
        true
    );
    assert_eq!(
        value["result"]["capabilities"]["serverInfo"],
        serde_json::Value::Null
    );
}

#[test]
fn mcp_capabilities_hidden_command_projects_error_fixture_to_command_result() {
    let dir = tempfile::tempdir().unwrap();
    let fixture = dir.path().join("capabilities-error.json");
    std::fs::write(
        &fixture,
        r#"{"jsonrpc":"2.0","id":53,"error":{"code":-32000,"message":"capabilities failed"}}"#,
    )
    .unwrap();

    let assert = Command::cargo_bin("by-rs")
        .unwrap()
        .args([
            "mcp",
            "capabilities",
            "--server-name",
            "filesystem",
            "--fixture-response",
        ])
        .arg(&fixture)
        .args(["--request-id", "53"])
        .assert()
        .success();
    let stdout = String::from_utf8(assert.get_output().stdout.clone()).unwrap();
    let value: serde_json::Value = serde_json::from_str(&stdout).unwrap();

    assert_eq!(
        value["error"],
        "Failed to get capabilities for 'filesystem': capabilities failed"
    );
}

#[test]
fn mcp_capabilities_hidden_command_projects_blank_server_to_error_shape() {
    assert_json_error(
        &[
            "mcp",
            "capabilities",
            "--server-name",
            "",
            "--fixture-response",
            "/definitely/missing/capabilities-response.json",
        ],
        "server-name is required",
    );
}

#[test]
fn mcp_health_hidden_command_projects_ping_request_without_network() {
    let assert = Command::cargo_bin("by-rs")
        .unwrap()
        .args([
            "mcp",
            "health",
            "--server-name",
            "filesystem",
            "--request-id",
            "49",
        ])
        .assert()
        .success();
    let stdout = String::from_utf8(assert.get_output().stdout.clone()).unwrap();
    let value: serde_json::Value = serde_json::from_str(&stdout).unwrap();

    assert_eq!(value["jsonrpc"], "2.0");
    assert_eq!(value["id"], 49);
    assert_eq!(value["method"], "ping");
    assert_eq!(value["params"], serde_json::json!({}));
}

#[test]
fn mcp_health_hidden_command_projects_blank_server_to_error_shape() {
    assert_json_error(
        &["mcp", "health", "--server-name", "", "--request-id", "49"],
        "server-name is required",
    );
}

#[test]
fn mcp_health_hidden_command_projects_ping_fixture_to_clojure_shape() {
    let dir = tempfile::tempdir().unwrap();
    let fixture = dir.path().join("ping-response.json");
    std::fs::write(&fixture, r#"{"jsonrpc":"2.0","id":50,"result":{}}"#).unwrap();

    let assert = Command::cargo_bin("by-rs")
        .unwrap()
        .args([
            "mcp",
            "health",
            "--server-name",
            "filesystem",
            "--fixture-response",
        ])
        .arg(&fixture)
        .args(["--request-id", "50", "--timestamp-ms", "1700000000000"])
        .assert()
        .success();
    let stdout = String::from_utf8(assert.get_output().stdout.clone()).unwrap();
    let value: serde_json::Value = serde_json::from_str(&stdout).unwrap();

    assert_eq!(value["result"]["name"], "filesystem");
    assert_eq!(value["result"]["status"], "healthy");
    assert_eq!(value["result"]["timestamp"], 1_700_000_000_000_u64);
}

#[test]
fn mcp_health_hidden_command_projects_ping_error_fixture_to_unhealthy_shape() {
    let dir = tempfile::tempdir().unwrap();
    let fixture = dir.path().join("ping-error-response.json");
    std::fs::write(
        &fixture,
        r#"{"jsonrpc":"2.0","id":51,"error":{"code":-32000,"message":"ping failed"}}"#,
    )
    .unwrap();

    let assert = Command::cargo_bin("by-rs")
        .unwrap()
        .args([
            "mcp",
            "health",
            "--server-name",
            "filesystem",
            "--fixture-response",
        ])
        .arg(&fixture)
        .args(["--request-id", "51", "--timestamp-ms", "1700000000001"])
        .assert()
        .success();
    let stdout = String::from_utf8(assert.get_output().stdout.clone()).unwrap();
    let value: serde_json::Value = serde_json::from_str(&stdout).unwrap();

    assert_eq!(value["result"]["name"], "filesystem");
    assert_eq!(value["result"]["status"], "unhealthy");
    assert_eq!(value["result"]["error"], "ping failed");
    assert_eq!(value["result"]["timestamp"], 1_700_000_000_001_u64);
}

#[test]
fn mcp_health_hidden_command_accepts_sse_error_fixture_to_unhealthy_shape() {
    let dir = tempfile::tempdir().unwrap();
    let fixture = dir.path().join("ping-error-response.sse");
    std::fs::write(
        &fixture,
        r#": keepalive
event: message
data: {"jsonrpc":"2.0","method":"notifications/message","params":{"level":"info"}}

event: message
data: {"jsonrpc":"2.0","id":52,"error":{"code":-32000,"message":"ping failed over sse"}}
"#,
    )
    .unwrap();

    let assert = Command::cargo_bin("by-rs")
        .unwrap()
        .args([
            "mcp",
            "health",
            "--server-name",
            "filesystem",
            "--fixture-response",
        ])
        .arg(&fixture)
        .args(["--request-id", "52", "--timestamp-ms", "1700000000002"])
        .assert()
        .success();
    let stdout = String::from_utf8(assert.get_output().stdout.clone()).unwrap();
    let value: serde_json::Value = serde_json::from_str(&stdout).unwrap();

    assert_eq!(value["result"]["name"], "filesystem");
    assert_eq!(value["result"]["status"], "unhealthy");
    assert_eq!(value["result"]["error"], "ping failed over sse");
    assert_eq!(value["result"]["timestamp"], 1_700_000_000_002_u64);
}

#[test]
fn mcp_disconnected_hidden_command_projects_clojure_shape() {
    let assert = Command::cargo_bin("by-rs")
        .unwrap()
        .args(["mcp", "disconnected", "--server-name", "filesystem"])
        .assert()
        .success();
    let stdout = String::from_utf8(assert.get_output().stdout.clone()).unwrap();
    let value: serde_json::Value = serde_json::from_str(&stdout).unwrap();

    assert_eq!(value["result"]["name"], "filesystem");
    assert_eq!(value["result"]["status"], "disconnected");
    assert_eq!(value["result"]["message"], "Server is not connected");
}

#[test]
fn mcp_lifecycle_hidden_command_projects_success_shape() {
    let assert = Command::cargo_bin("by-rs")
        .unwrap()
        .args([
            "mcp",
            "lifecycle",
            "--op",
            "restart",
            "--server-name",
            "filesystem",
        ])
        .assert()
        .success();
    let stdout = String::from_utf8(assert.get_output().stdout.clone()).unwrap();
    let value: serde_json::Value = serde_json::from_str(&stdout).unwrap();

    assert_eq!(
        value["result"],
        "MCP server 'filesystem' restarted successfully"
    );
}

#[test]
fn mcp_lifecycle_hidden_command_projects_blank_server_to_error_shape() {
    assert_json_error(
        &["mcp", "lifecycle", "--op", "restart", "--server-name", ""],
        "server-name is required",
    );
}

#[test]
fn mcp_tools_hidden_command_projects_clojure_list_shape() {
    let dir = tempfile::tempdir().unwrap();
    let fixture = dir.path().join("tools-list-response.json");
    std::fs::write(
        &fixture,
        r#"{"jsonrpc":"2.0","id":42,"result":{"tools":[
          {"name":"read_file","description":"Read a file","inputSchema":{"type":"object","properties":{"path":{"type":"string"}},"required":["path"]}},
          {"name":"list-dir","inputSchema":{"type":"object","properties":{"root":{"type":"string"}}}}
        ]}}"#,
    )
    .unwrap();

    let assert = Command::cargo_bin("by-rs")
        .unwrap()
        .args([
            "mcp",
            "tools",
            "--server-name",
            "filesystem",
            "--fixture-response",
        ])
        .arg(&fixture)
        .args(["--request-id", "42"])
        .assert()
        .success();
    let stdout = String::from_utf8(assert.get_output().stdout.clone()).unwrap();
    let value: serde_json::Value = serde_json::from_str(&stdout).unwrap();

    assert_eq!(value["result"]["total"], 2);
    assert_eq!(value["result"]["tools"][0]["server-name"], "filesystem");
    assert_eq!(value["result"]["tools"][0]["name"], "read_file");
    assert_eq!(value["result"]["tools"][0]["description"], "Read a file");
    assert_eq!(
        value["result"]["tools"][0]["parameters"]["properties"]["path"]["type"],
        "string"
    );
    assert_eq!(value["result"]["tools"][1]["name"], "list-dir");
    assert_eq!(
        value["result"]["tools"][1]["description"],
        serde_json::Value::Null
    );
}

#[test]
fn mcp_tools_hidden_command_accepts_sse_response_fixture() {
    let dir = tempfile::tempdir().unwrap();
    let fixture = dir.path().join("tools-list-response.sse");
    std::fs::write(
        &fixture,
        r#": keepalive
event: message
data: {"jsonrpc":"2.0","method":"notifications/tools/list_changed","params":{}}

event: message
data: {"jsonrpc":"2.0","id":41,"result":{"tools":[]}}

event: message
data: {"jsonrpc":"2.0","id":42,"result":{"tools":[{"name":"search","description":"Search docs","inputSchema":{"type":"object"}}]}}
"#,
    )
    .unwrap();

    let assert = Command::cargo_bin("by-rs")
        .unwrap()
        .args([
            "mcp",
            "tools",
            "--server-name",
            "filesystem",
            "--fixture-response",
        ])
        .arg(&fixture)
        .args(["--request-id", "42"])
        .assert()
        .success();
    let stdout = String::from_utf8(assert.get_output().stdout.clone()).unwrap();
    let value: serde_json::Value = serde_json::from_str(&stdout).unwrap();

    assert_eq!(value["result"]["total"], 1);
    assert_eq!(value["result"]["tools"][0]["server-name"], "filesystem");
    assert_eq!(value["result"]["tools"][0]["name"], "search");
    assert_eq!(value["result"]["tools"][0]["description"], "Search docs");
    assert_eq!(value["result"]["tools"][0]["parameters"]["type"], "object");
}

#[test]
fn mcp_tools_hidden_command_projects_error_fixture_to_command_result() {
    let dir = tempfile::tempdir().unwrap();
    let fixture = dir.path().join("tools-list-error.json");
    std::fs::write(
        &fixture,
        r#"{"jsonrpc":"2.0","id":43,"error":{"code":-32000,"message":"tools failed"}}"#,
    )
    .unwrap();

    let assert = Command::cargo_bin("by-rs")
        .unwrap()
        .args([
            "mcp",
            "tools",
            "--server-name",
            "filesystem",
            "--fixture-response",
        ])
        .arg(&fixture)
        .args(["--request-id", "43"])
        .assert()
        .success();
    let stdout = String::from_utf8(assert.get_output().stdout.clone()).unwrap();
    let value: serde_json::Value = serde_json::from_str(&stdout).unwrap();

    assert_eq!(value["error"], "Failed to list MCP tools: tools failed");
}

#[test]
fn mcp_tools_hidden_command_accepts_raw_result_fixture() {
    let dir = tempfile::tempdir().unwrap();
    let fixture = dir.path().join("tools-list-result.json");
    std::fs::write(
        &fixture,
        r#"{"tools":[{"name":"search","description":"Search docs","parameters":{"type":"object"}}]}"#,
    )
    .unwrap();

    let assert = Command::cargo_bin("by-rs")
        .unwrap()
        .args([
            "mcp",
            "tools",
            "--server-name",
            "linear",
            "--fixture-response",
        ])
        .arg(&fixture)
        .assert()
        .success();
    let stdout = String::from_utf8(assert.get_output().stdout.clone()).unwrap();
    let value: serde_json::Value = serde_json::from_str(&stdout).unwrap();

    assert_eq!(value["result"]["total"], 1);
    assert_eq!(value["result"]["tools"][0]["server-name"], "linear");
    assert_eq!(value["result"]["tools"][0]["parameters"]["type"], "object");
}

#[test]
fn mcp_resources_hidden_command_projects_request_without_network() {
    let assert = Command::cargo_bin("by-rs")
        .unwrap()
        .args([
            "mcp",
            "resources",
            "--server-name",
            "filesystem",
            "--request-id",
            "43",
        ])
        .assert()
        .success();
    let stdout = String::from_utf8(assert.get_output().stdout.clone()).unwrap();
    let value: serde_json::Value = serde_json::from_str(&stdout).unwrap();

    assert_eq!(value["jsonrpc"], "2.0");
    assert_eq!(value["id"], 43);
    assert_eq!(value["method"], "resources/list");
    assert_eq!(value["params"], serde_json::json!({}));
}

#[test]
fn mcp_resources_hidden_command_projects_blank_server_to_error_shape() {
    assert_json_error(
        &[
            "mcp",
            "resources",
            "--server-name",
            "",
            "--request-id",
            "43",
        ],
        "server-name is required",
    );
}

#[test]
fn mcp_resources_hidden_command_projects_clojure_server_shape() {
    let dir = tempfile::tempdir().unwrap();
    let fixture = dir.path().join("resources-list-response.json");
    std::fs::write(
        &fixture,
        r#"{"jsonrpc":"2.0","id":44,"result":{"resources":[{"uri":"file:///tmp/a.txt","name":"a.txt","mimeType":"text/plain"}]}}"#,
    )
    .unwrap();

    let assert = Command::cargo_bin("by-rs")
        .unwrap()
        .args([
            "mcp",
            "resources",
            "--server-name",
            "filesystem",
            "--fixture-response",
        ])
        .arg(&fixture)
        .args(["--request-id", "44"])
        .assert()
        .success();
    let stdout = String::from_utf8(assert.get_output().stdout.clone()).unwrap();
    let value: serde_json::Value = serde_json::from_str(&stdout).unwrap();

    assert_eq!(value["result"]["name"], "filesystem");
    assert_eq!(
        value["result"]["resources"]["resources"][0]["uri"],
        "file:///tmp/a.txt"
    );
    assert_eq!(
        value["result"]["resources"]["resources"][0]["mimeType"],
        "text/plain"
    );
}

#[test]
fn mcp_resources_hidden_command_projects_error_fixture_to_command_result() {
    let dir = tempfile::tempdir().unwrap();
    let fixture = dir.path().join("resources-list-error.json");
    std::fs::write(
        &fixture,
        r#"{"jsonrpc":"2.0","id":54,"error":{"code":-32000,"message":"resources failed"}}"#,
    )
    .unwrap();

    let assert = Command::cargo_bin("by-rs")
        .unwrap()
        .args([
            "mcp",
            "resources",
            "--server-name",
            "filesystem",
            "--fixture-response",
        ])
        .arg(&fixture)
        .args(["--request-id", "54"])
        .assert()
        .success();
    let stdout = String::from_utf8(assert.get_output().stdout.clone()).unwrap();
    let value: serde_json::Value = serde_json::from_str(&stdout).unwrap();

    assert_eq!(
        value["error"],
        "Failed to list resources for 'filesystem': resources failed"
    );
}

#[test]
fn mcp_prompts_hidden_command_projects_request_without_network() {
    let assert = Command::cargo_bin("by-rs")
        .unwrap()
        .args([
            "mcp",
            "prompts",
            "--server-name",
            "linear",
            "--request-id",
            "45",
        ])
        .assert()
        .success();
    let stdout = String::from_utf8(assert.get_output().stdout.clone()).unwrap();
    let value: serde_json::Value = serde_json::from_str(&stdout).unwrap();

    assert_eq!(value["jsonrpc"], "2.0");
    assert_eq!(value["id"], 45);
    assert_eq!(value["method"], "prompts/list");
    assert_eq!(value["params"], serde_json::json!({}));
}

#[test]
fn mcp_prompts_hidden_command_projects_blank_server_to_error_shape() {
    assert_json_error(
        &["mcp", "prompts", "--server-name", "", "--request-id", "45"],
        "server-name is required",
    );
}

#[test]
fn mcp_prompts_hidden_command_projects_clojure_server_shape() {
    let dir = tempfile::tempdir().unwrap();
    let fixture = dir.path().join("prompts-list-response.json");
    std::fs::write(
        &fixture,
        r#"{"jsonrpc":"2.0","id":46,"result":{"prompts":[{"name":"summarize","description":"Summarize work","arguments":[{"name":"topic","required":true}]}]}}"#,
    )
    .unwrap();

    let assert = Command::cargo_bin("by-rs")
        .unwrap()
        .args([
            "mcp",
            "prompts",
            "--server-name",
            "linear",
            "--fixture-response",
        ])
        .arg(&fixture)
        .args(["--request-id", "46"])
        .assert()
        .success();
    let stdout = String::from_utf8(assert.get_output().stdout.clone()).unwrap();
    let value: serde_json::Value = serde_json::from_str(&stdout).unwrap();

    assert_eq!(value["result"]["name"], "linear");
    assert_eq!(
        value["result"]["prompts"]["prompts"][0]["name"],
        "summarize"
    );
    assert_eq!(
        value["result"]["prompts"]["prompts"][0]["arguments"][0]["name"],
        "topic"
    );
}

#[test]
fn mcp_prompts_hidden_command_projects_error_fixture_to_command_result() {
    let dir = tempfile::tempdir().unwrap();
    let fixture = dir.path().join("prompts-list-error.json");
    std::fs::write(
        &fixture,
        r#"{"jsonrpc":"2.0","id":55,"error":{"code":-32000,"message":"prompts failed"}}"#,
    )
    .unwrap();

    let assert = Command::cargo_bin("by-rs")
        .unwrap()
        .args([
            "mcp",
            "prompts",
            "--server-name",
            "linear",
            "--fixture-response",
        ])
        .arg(&fixture)
        .args(["--request-id", "55"])
        .assert()
        .success();
    let stdout = String::from_utf8(assert.get_output().stdout.clone()).unwrap();
    let value: serde_json::Value = serde_json::from_str(&stdout).unwrap();

    assert_eq!(
        value["error"],
        "Failed to list prompts for 'linear': prompts failed"
    );
}

#[test]
fn mcp_registered_tools_hidden_command_projects_clojure_auto_registration_shape() {
    let dir = tempfile::tempdir().unwrap();
    let fixture = dir.path().join("tools-list-response.json");
    std::fs::write(
        &fixture,
        r#"{"jsonrpc":"2.0","id":9,"result":{"tools":[
          {"name":"read_file","description":"Read a file","inputSchema":{"type":"object","properties":{"path":{"type":"string","description":"Path to read"},"limit":{"type":"integer","default":10}},"required":["path"]}},
          {"name":"bad/name","description":"Skipped","inputSchema":{"type":"object"}}
        ]}}"#,
    )
    .unwrap();

    let assert = Command::cargo_bin("by-rs")
        .unwrap()
        .args([
            "mcp",
            "registered-tools",
            "--server-name",
            "filesystem",
            "--fixture-response",
        ])
        .arg(&fixture)
        .args(["--request-id", "9"])
        .assert()
        .success();
    let stdout = String::from_utf8(assert.get_output().stdout.clone()).unwrap();
    let value: serde_json::Value = serde_json::from_str(&stdout).unwrap();

    assert_eq!(value["result"]["total"], 1);
    assert_eq!(
        value["result"]["tools"][0]["id"],
        "mcp$filesystem$read_file"
    );
    assert_eq!(value["result"]["tools"][0]["type"], "tool");
    assert_eq!(value["result"]["tools"][0]["description"], "Read a file");
    assert_eq!(value["result"]["tools"][0]["mcp-server"], "filesystem");
    assert_eq!(value["result"]["tools"][0]["mcp-tool"], "read_file");
    assert_eq!(
        value["result"]["tools"][0]["input-schema"],
        serde_json::json!([
            "map",
            ["limit", {"optional": true}, ["int", {"default": 10}]],
            ["path", ["string", {"desc": "Path to read"}]]
        ])
    );
    assert_eq!(
        value["result"]["tools"][0]["output-schema"],
        serde_json::json!(["map"])
    );
}

#[test]
fn mcp_call_tool_hidden_command_projects_request_without_network() {
    let assert = Command::cargo_bin("by-rs")
        .unwrap()
        .args([
            "mcp",
            "call-tool",
            "--server-name",
            "filesystem",
            "--tool-name",
            "read_file",
            "--tool-args",
            r#"[{"name":"path","value":"/tmp/a.txt"}]"#,
            "--request-id",
            "23",
        ])
        .assert()
        .success();
    let stdout = String::from_utf8(assert.get_output().stdout.clone()).unwrap();
    let value: serde_json::Value = serde_json::from_str(&stdout).unwrap();

    assert_eq!(value["jsonrpc"], "2.0");
    assert_eq!(value["id"], 23);
    assert_eq!(value["method"], "tools/call");
    assert_eq!(value["params"]["name"], "read_file");
    assert_eq!(value["params"]["arguments"]["path"], "/tmp/a.txt");
}

#[test]
fn mcp_call_tool_hidden_command_projects_blank_server_to_validation_result() {
    let assert = Command::cargo_bin("by-rs")
        .unwrap()
        .args([
            "mcp",
            "call-tool",
            "--server-name",
            "",
            "--tool-name",
            "read_file",
            "--tool-args",
            r#"{"path":"/tmp/a.txt"}"#,
        ])
        .assert()
        .success();
    let stdout = String::from_utf8(assert.get_output().stdout.clone()).unwrap();
    let value: serde_json::Value = serde_json::from_str(&stdout).unwrap();

    assert_eq!(value["result"]["total"], 1);
    assert_eq!(value["result"]["tool-results"][0]["server-name"], "");
    assert_eq!(value["result"]["tool-results"][0]["tool-name"], "read_file");
    assert_eq!(
        value["result"]["tool-results"][0]["tool-args"]["path"],
        "/tmp/a.txt"
    );
    assert_eq!(
        value["result"]["tool-results"][0]["tool-result"]["error"],
        "server-name is required"
    );
    assert_eq!(
        value["result"]["tool-results"][0]["tool-result"]["success"],
        serde_json::Value::Null
    );
}

#[test]
fn mcp_call_tool_hidden_command_projects_blank_tool_to_validation_result() {
    let assert = Command::cargo_bin("by-rs")
        .unwrap()
        .args([
            "mcp",
            "call-tool",
            "--server-name",
            "filesystem",
            "--tool-name",
            "",
            "--tool-args",
            r#"{"path":"/tmp/a.txt"}"#,
        ])
        .assert()
        .success();
    let stdout = String::from_utf8(assert.get_output().stdout.clone()).unwrap();
    let value: serde_json::Value = serde_json::from_str(&stdout).unwrap();

    assert_eq!(value["result"]["total"], 1);
    assert_eq!(
        value["result"]["tool-results"][0]["server-name"],
        "filesystem"
    );
    assert_eq!(value["result"]["tool-results"][0]["tool-name"], "");
    assert_eq!(
        value["result"]["tool-results"][0]["tool-result"]["error"],
        "tool-name is required"
    );
    assert_eq!(
        value["result"]["tool-results"][0]["tool-result"]["success"],
        serde_json::Value::Null
    );
}

#[test]
fn mcp_call_tool_hidden_command_projects_response_fixture_to_command_result() {
    let dir = tempfile::tempdir().unwrap();
    let fixture = dir.path().join("tools-call-response.json");
    std::fs::write(
        &fixture,
        r#"{"jsonrpc":"2.0","id":24,"result":{"content":[{"type":"text","text":"ok"}]}}"#,
    )
    .unwrap();

    let assert = Command::cargo_bin("by-rs")
        .unwrap()
        .args([
            "mcp",
            "call-tool",
            "--server-name",
            "filesystem",
            "--tool-name",
            "read_file",
            "--tool-args",
            r#"{"path":"/tmp/a.txt"}"#,
            "--fixture-response",
        ])
        .arg(&fixture)
        .args(["--request-id", "24"])
        .assert()
        .success();
    let stdout = String::from_utf8(assert.get_output().stdout.clone()).unwrap();
    let value: serde_json::Value = serde_json::from_str(&stdout).unwrap();

    assert_eq!(value["result"]["total"], 1);
    assert_eq!(
        value["result"]["tool-results"][0]["server-name"],
        "filesystem"
    );
    assert_eq!(value["result"]["tool-results"][0]["tool-name"], "read_file");
    assert_eq!(
        value["result"]["tool-results"][0]["tool-args"]["path"],
        "/tmp/a.txt"
    );
    assert_eq!(
        value["result"]["tool-results"][0]["tool-result"]["success"],
        true
    );
    assert_eq!(
        value["result"]["tool-results"][0]["tool-result"]["result"]["content"][0]["text"],
        "ok"
    );
}

#[test]
fn mcp_call_tool_hidden_command_projects_error_fixture_to_command_result() {
    let dir = tempfile::tempdir().unwrap();
    let fixture = dir.path().join("tools-call-error-response.json");
    std::fs::write(
        &fixture,
        r#"{"jsonrpc":"2.0","id":25,"error":{"code":-32000,"message":"tool failed"}}"#,
    )
    .unwrap();

    let assert = Command::cargo_bin("by-rs")
        .unwrap()
        .args([
            "mcp",
            "call-tool",
            "--server-name",
            "filesystem",
            "--tool-name",
            "read_file",
            "--tool-args",
            r#"{"path":"/tmp/a.txt"}"#,
            "--fixture-response",
        ])
        .arg(&fixture)
        .args(["--request-id", "25"])
        .assert()
        .success();
    let stdout = String::from_utf8(assert.get_output().stdout.clone()).unwrap();
    let value: serde_json::Value = serde_json::from_str(&stdout).unwrap();

    assert_eq!(value["result"]["total"], 1);
    assert_eq!(
        value["result"]["tool-results"][0]["tool-result"]["success"],
        false
    );
    assert_eq!(
        value["result"]["tool-results"][0]["tool-result"]["error"],
        "tool failed"
    );
}

#[test]
fn mcp_read_resource_hidden_command_projects_request_without_network() {
    let assert = Command::cargo_bin("by-rs")
        .unwrap()
        .args([
            "mcp",
            "read-resource",
            "--server-name",
            "filesystem",
            "--resource-uri",
            "file:///tmp/a.txt",
            "--request-id",
            "31",
        ])
        .assert()
        .success();
    let stdout = String::from_utf8(assert.get_output().stdout.clone()).unwrap();
    let value: serde_json::Value = serde_json::from_str(&stdout).unwrap();

    assert_eq!(value["jsonrpc"], "2.0");
    assert_eq!(value["id"], 31);
    assert_eq!(value["method"], "resources/read");
    assert_eq!(value["params"]["uri"], "file:///tmp/a.txt");
}

#[test]
fn mcp_read_resource_hidden_command_projects_blank_server_to_error_shape() {
    let assert = Command::cargo_bin("by-rs")
        .unwrap()
        .args([
            "mcp",
            "read-resource",
            "--server-name",
            "",
            "--resource-uri",
            "file:///tmp/a.txt",
        ])
        .assert()
        .success();
    let stdout = String::from_utf8(assert.get_output().stdout.clone()).unwrap();
    let value: serde_json::Value = serde_json::from_str(&stdout).unwrap();

    assert_eq!(value["error"], "server-name is required");
}

#[test]
fn mcp_read_resource_hidden_command_projects_blank_uri_to_error_shape() {
    let assert = Command::cargo_bin("by-rs")
        .unwrap()
        .args([
            "mcp",
            "read-resource",
            "--server-name",
            "filesystem",
            "--resource-uri",
            "",
        ])
        .assert()
        .success();
    let stdout = String::from_utf8(assert.get_output().stdout.clone()).unwrap();
    let value: serde_json::Value = serde_json::from_str(&stdout).unwrap();

    assert_eq!(value["error"], "resource-uri is required");
}

#[test]
fn mcp_read_resource_hidden_command_projects_response_fixture_to_command_result() {
    let dir = tempfile::tempdir().unwrap();
    let fixture = dir.path().join("resources-read-response.json");
    std::fs::write(
        &fixture,
        r#"{"jsonrpc":"2.0","id":32,"result":{"contents":[{"uri":"file:///tmp/a.txt","mimeType":"text/plain","text":"hello"}]}}"#,
    )
    .unwrap();

    let assert = Command::cargo_bin("by-rs")
        .unwrap()
        .args([
            "mcp",
            "read-resource",
            "--server-name",
            "filesystem",
            "--resource-uri",
            "file:///tmp/a.txt",
            "--fixture-response",
        ])
        .arg(&fixture)
        .args(["--request-id", "32"])
        .assert()
        .success();
    let stdout = String::from_utf8(assert.get_output().stdout.clone()).unwrap();
    let value: serde_json::Value = serde_json::from_str(&stdout).unwrap();

    assert_eq!(value["result"]["name"], "filesystem");
    assert_eq!(value["result"]["uri"], "file:///tmp/a.txt");
    assert_eq!(
        value["result"]["resource"]["contents"][0]["mimeType"],
        "text/plain"
    );
    assert_eq!(value["result"]["resource"]["contents"][0]["text"], "hello");
}

#[test]
fn mcp_read_resource_hidden_command_projects_error_fixture_to_command_result() {
    let dir = tempfile::tempdir().unwrap();
    let fixture = dir.path().join("resources-read-error.json");
    std::fs::write(
        &fixture,
        r#"{"jsonrpc":"2.0","id":35,"error":{"code":-32000,"message":"resource failed"}}"#,
    )
    .unwrap();

    let assert = Command::cargo_bin("by-rs")
        .unwrap()
        .args([
            "mcp",
            "read-resource",
            "--server-name",
            "filesystem",
            "--resource-uri",
            "file:///tmp/a.txt",
            "--fixture-response",
        ])
        .arg(&fixture)
        .args(["--request-id", "35"])
        .assert()
        .success();
    let stdout = String::from_utf8(assert.get_output().stdout.clone()).unwrap();
    let value: serde_json::Value = serde_json::from_str(&stdout).unwrap();

    assert_eq!(
        value["error"],
        "Failed to read resource 'file:///tmp/a.txt' from 'filesystem': resource failed"
    );
}

#[test]
fn mcp_get_prompt_hidden_command_projects_request_without_network() {
    let assert = Command::cargo_bin("by-rs")
        .unwrap()
        .args([
            "mcp",
            "get-prompt",
            "--server-name",
            "linear",
            "--prompt-name",
            "summarize",
            "--arguments",
            r#"{"topic":"mcp"}"#,
            "--request-id",
            "33",
        ])
        .assert()
        .success();
    let stdout = String::from_utf8(assert.get_output().stdout.clone()).unwrap();
    let value: serde_json::Value = serde_json::from_str(&stdout).unwrap();

    assert_eq!(value["jsonrpc"], "2.0");
    assert_eq!(value["id"], 33);
    assert_eq!(value["method"], "prompts/get");
    assert_eq!(value["params"]["name"], "summarize");
    assert_eq!(value["params"]["arguments"]["topic"], "mcp");
}

#[test]
fn mcp_get_prompt_hidden_command_projects_blank_server_to_error_shape() {
    let assert = Command::cargo_bin("by-rs")
        .unwrap()
        .args([
            "mcp",
            "get-prompt",
            "--server-name",
            "",
            "--prompt-name",
            "summarize",
        ])
        .assert()
        .success();
    let stdout = String::from_utf8(assert.get_output().stdout.clone()).unwrap();
    let value: serde_json::Value = serde_json::from_str(&stdout).unwrap();

    assert_eq!(value["error"], "server-name is required");
}

#[test]
fn mcp_get_prompt_hidden_command_projects_blank_prompt_to_error_shape() {
    let assert = Command::cargo_bin("by-rs")
        .unwrap()
        .args([
            "mcp",
            "get-prompt",
            "--server-name",
            "linear",
            "--prompt-name",
            "",
        ])
        .assert()
        .success();
    let stdout = String::from_utf8(assert.get_output().stdout.clone()).unwrap();
    let value: serde_json::Value = serde_json::from_str(&stdout).unwrap();

    assert_eq!(value["error"], "prompt-name is required");
}

#[test]
fn mcp_get_prompt_hidden_command_projects_response_fixture_to_command_result() {
    let dir = tempfile::tempdir().unwrap();
    let fixture = dir.path().join("prompts-get-response.json");
    std::fs::write(
        &fixture,
        r#"{"jsonrpc":"2.0","id":34,"result":{"messages":[{"role":"user","content":{"type":"text","text":"hello"}}]}}"#,
    )
    .unwrap();

    let assert = Command::cargo_bin("by-rs")
        .unwrap()
        .args([
            "mcp",
            "get-prompt",
            "--server-name",
            "linear",
            "--prompt-name",
            "summarize",
            "--fixture-response",
        ])
        .arg(&fixture)
        .args(["--request-id", "34"])
        .assert()
        .success();
    let stdout = String::from_utf8(assert.get_output().stdout.clone()).unwrap();
    let value: serde_json::Value = serde_json::from_str(&stdout).unwrap();

    assert_eq!(value["result"]["name"], "linear");
    assert_eq!(value["result"]["prompt-name"], "summarize");
    assert_eq!(value["result"]["prompt"]["messages"][0]["role"], "user");
    assert_eq!(
        value["result"]["prompt"]["messages"][0]["content"]["text"],
        "hello"
    );
}

#[test]
fn mcp_get_prompt_hidden_command_projects_error_fixture_to_command_result() {
    let dir = tempfile::tempdir().unwrap();
    let fixture = dir.path().join("prompts-get-error.json");
    std::fs::write(
        &fixture,
        r#"{"jsonrpc":"2.0","id":36,"error":{"code":-32000,"message":"prompt failed"}}"#,
    )
    .unwrap();

    let assert = Command::cargo_bin("by-rs")
        .unwrap()
        .args([
            "mcp",
            "get-prompt",
            "--server-name",
            "linear",
            "--prompt-name",
            "summarize",
            "--fixture-response",
        ])
        .arg(&fixture)
        .args(["--request-id", "36"])
        .assert()
        .success();
    let stdout = String::from_utf8(assert.get_output().stdout.clone()).unwrap();
    let value: serde_json::Value = serde_json::from_str(&stdout).unwrap();

    assert_eq!(
        value["error"],
        "Failed to get prompt 'summarize' from 'linear': prompt failed"
    );
}

#[test]
fn config_help_matches_clojure_bootstrap_surface() {
    Command::cargo_bin("by-rs")
        .unwrap()
        .args(["config", "--help"])
        .assert()
        .success()
        .stderr(predicate::str::contains(
            "NAME:\n by config - Bootstrap pipeline (detect → ladder → handoff)",
        ))
        .stderr(predicate::str::contains(
            "USAGE:\n by config [command options] [arguments...]",
        ))
        .stderr(predicate::str::contains("--[no-]auto"))
        .stderr(predicate::str::contains("--profile S"))
        .stderr(predicate::str::contains("--[no-]dry-run"))
        .stderr(predicate::str::contains("show").not());
}

#[test]
fn config_requires_auto_when_stdin_is_non_interactive() {
    Command::cargo_bin("by-rs")
        .unwrap()
        .args(["config", "--dry-run"])
        .assert()
        .code(2)
        .stdout(predicate::str::is_empty())
        .stderr(predicate::str::contains(
            "Non-interactive stdin detected. Use --auto for non-interactive runs.",
        ));
}

#[test]
fn config_rejects_unknown_profile_like_clojure() {
    Command::cargo_bin("by-rs")
        .unwrap()
        .args(["config", "--auto", "--profile", "bogus", "--dry-run"])
        .assert()
        .code(2)
        .stdout(predicate::str::is_empty())
        .stderr(predicate::str::contains("Unknown profile: :bogus"))
        .stderr(predicate::str::contains(
            "Known profiles: ci, cloud, dev, offline",
        ));
}

#[test]
fn config_bootstrap_dry_run_prints_clojure_style_human_summary_by_default() {
    let home = tempfile::tempdir().unwrap();
    let path_dir = tempfile::tempdir().unwrap();
    let mut cmd = Command::cargo_bin("by-rs").unwrap();
    scrub_config_bootstrap_provider_env(&mut cmd);

    let assert = cmd
        .env("HOME", home.path())
        .env("PATH", path_dir.path())
        .env("BY_NO_DOTENV", "1")
        .args(["config", "--auto", "--profile", "dev", "--dry-run"])
        .assert()
        .success()
        .stderr(predicate::str::is_empty())
        .stdout(predicate::str::contains("Brainyard Environment Bootstrap"))
        .stdout(predicate::str::contains("--auto mode (dev profile)"))
        .stdout(predicate::str::contains(
            "(No existing config found — starting fresh.)",
        ))
        .stdout(predicate::str::contains("Detecting environment..."))
        .stdout(predicate::str::contains("Detected providers:"))
        .stdout(predicate::str::contains("Bootstrap rung (g): Stop"))
        .stdout(predicate::str::contains(
            "--dry-run: not writing config.edn.",
        ))
        .stdout(predicate::str::contains("\"operation\": \"config\"").not());

    let stdout = String::from_utf8(assert.get_output().stdout.clone()).unwrap();
    assert!(!stdout.trim_start().starts_with('{'));
    assert!(!home.path().join(".brainyard").exists());
}

#[test]
fn config_bootstrap_dry_run_prints_projected_config_edn_like_clojure() {
    let home = tempfile::tempdir().unwrap();
    let path_dir = tempfile::tempdir().unwrap();
    let mut cmd = Command::cargo_bin("by-rs").unwrap();
    scrub_config_bootstrap_provider_env(&mut cmd);

    cmd.env("HOME", home.path())
        .env("PATH", path_dir.path())
        .env("BY_NO_DOTENV", "1")
        .args(["config", "--auto", "--profile", "dev", "--dry-run"])
        .assert()
        .success()
        .stderr(predicate::str::is_empty())
        .stdout(predicate::str::contains(
            "--dry-run: not writing config.edn.",
        ))
        .stdout(predicate::str::contains("{:bootstrap"))
        .stdout(predicate::str::contains(":rung :g"))
        .stdout(predicate::str::contains(":incomplete true"))
        .stdout(predicate::str::contains(":next-steps"))
        .stdout(predicate::str::contains("\"projected_config_delta\"").not());

    assert!(!home.path().join(".brainyard").exists());
}

#[test]
fn config_bootstrap_projection_stays_read_only() {
    let home = tempfile::tempdir().unwrap();
    let path_dir = tempfile::tempdir().unwrap();
    let mut cmd = Command::cargo_bin("by-rs").unwrap();
    scrub_config_bootstrap_provider_env(&mut cmd);

    let assert = cmd
        .env("HOME", home.path())
        .env("PATH", path_dir.path())
        .env("BY_NO_DOTENV", "1")
        .env("BY_RS_CONFIG_BOOTSTRAP_JSON", "1")
        .args(["config", "--auto", "--profile", "dev", "--dry-run"])
        .assert()
        .success()
        .stdout(predicate::str::contains("\"operation\": \"config\""))
        .stdout(predicate::str::contains("\"status\": \"planned\""))
        .stdout(predicate::str::contains("\"network\": false"))
        .stdout(predicate::str::contains("\"writes\": false"))
        .stdout(predicate::str::contains("\"auto\": true"))
        .stdout(predicate::str::contains("\"profile\": \"dev\""))
        .stdout(predicate::str::contains("\"dry_run\": true"));
    let stdout = String::from_utf8(assert.get_output().stdout.clone()).unwrap();
    let value: serde_json::Value = serde_json::from_str(&stdout).unwrap();

    assert_eq!(value["chosen"]["rung"], "g");
    assert_eq!(
        value["projected_config_delta"]["bootstrap"]["incomplete"],
        true
    );
    assert!(value["actions"].as_array().unwrap().is_empty());
    assert!(!home.path().join(".brainyard").exists());
}

#[test]
fn config_bootstrap_projection_reports_clojure_permission_defaults_without_writing() {
    let project = tempfile::tempdir().unwrap();
    let home = tempfile::tempdir().unwrap();
    let nested = project.path().join("subdir");
    std::fs::create_dir_all(project.path().join(".git")).unwrap();
    std::fs::create_dir_all(&nested).unwrap();

    let assert = Command::cargo_bin("by-rs")
        .unwrap()
        .current_dir(&nested)
        .env("HOME", home.path())
        .env("BY_RS_CONFIG_BOOTSTRAP_JSON", "1")
        .env_remove("BRAINYARD_PROJECT_DIR")
        .args(["config", "--auto", "--profile", "dev", "--dry-run"])
        .assert()
        .success();
    let stdout = String::from_utf8(assert.get_output().stdout.clone()).unwrap();
    let value: serde_json::Value = serde_json::from_str(&stdout).unwrap();
    let expected_working_dir = std::fs::canonicalize(&nested).unwrap();
    let expected_project_dir = std::fs::canonicalize(project.path()).unwrap();

    assert_eq!(value["network"], false);
    assert_eq!(value["writes"], false);
    assert_eq!(
        value["dirs"]["working_dir"],
        expected_working_dir.display().to_string()
    );
    assert_eq!(
        value["dirs"]["project_dir"],
        expected_project_dir.display().to_string()
    );
    assert_eq!(value["dirs"]["user_dir"], home.path().display().to_string());
    assert_eq!(
        value["dirs"]["project_config_dir"],
        expected_project_dir
            .join(".brainyard")
            .display()
            .to_string()
    );
    assert_eq!(
        value["dirs"]["user_config_dir"],
        home.path().join(".brainyard").display().to_string()
    );
    assert_eq!(value["defaults"]["permissions"]["mode"], "ask-each-time");
    assert_eq!(
        value["defaults"]["permissions"]["allowed_dirs"],
        serde_json::json!([
            "/tmp",
            expected_project_dir.display().to_string(),
            home.path().join(".brainyard").display().to_string(),
        ])
    );
    assert!(!project.path().join(".brainyard").exists());
    assert!(!home.path().join(".brainyard").exists());
}

#[test]
fn config_bootstrap_projection_chooses_api_key_rung_from_dotenv_without_exposing_secret() {
    let project = tempfile::tempdir().unwrap();
    let home = tempfile::tempdir().unwrap();
    let path_dir = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(project.path().join(".git")).unwrap();
    std::fs::write(
        project.path().join(".env"),
        "OPENAI_API_KEY=sk-openai-lower-priority\nANTHROPIC_API_KEY=sk-anthropic-secret-7890\n",
    )
    .unwrap();
    let mut cmd = Command::cargo_bin("by-rs").unwrap();
    scrub_config_bootstrap_provider_env(&mut cmd);

    let assert = cmd
        .current_dir(project.path())
        .env("HOME", home.path())
        .env("PATH", path_dir.path())
        .env("BY_RS_CONFIG_BOOTSTRAP_JSON", "1")
        .env_remove("BY_NO_DOTENV")
        .env_remove("BRAINYARD_PROJECT_DIR")
        .args(["config", "--auto", "--profile", "ci", "--dry-run"])
        .assert()
        .success();
    let stdout = String::from_utf8(assert.get_output().stdout.clone()).unwrap();
    let value: serde_json::Value = serde_json::from_str(&stdout).unwrap();

    assert_eq!(value["chosen"]["rung"], "b");
    assert_eq!(value["chosen"]["provider"], "anthropic");
    assert_eq!(value["chosen"]["model"], "claude-opus-4-7");
    assert_eq!(
        value["chosen"]["available_providers"],
        serde_json::json!(["anthropic", "openai"])
    );
    assert_eq!(
        value["projected_config_delta"]["llm"]["default_provider"],
        "anthropic"
    );
    assert!(stdout.contains("sk-ant...****7890"));
    assert!(!stdout.contains("sk-anthropic-secret-7890"));
    assert_eq!(value["network"], false);
    assert_eq!(value["writes"], false);
    assert!(!project.path().join(".brainyard").exists());
    assert!(!home.path().join(".brainyard").exists());
}

#[test]
fn config_bootstrap_projection_uses_existing_reachable_config_unless_rebootstrap() {
    let project = tempfile::tempdir().unwrap();
    let home = tempfile::tempdir().unwrap();
    let path_dir = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(project.path().join(".git")).unwrap();
    std::fs::create_dir_all(project.path().join(".brainyard")).unwrap();
    std::fs::write(
        project.path().join(".brainyard/config.edn"),
        r#"{:llm {:default-provider :claude-code
                 :default-model "sonnet"
                 :available-providers [:claude-code]}}"#,
    )
    .unwrap();
    link_fake_executable(
        &path_dir.path().join("claude"),
        system_binary(&["/bin/sh", "/usr/bin/true"]),
    );
    let mut cmd = Command::cargo_bin("by-rs").unwrap();
    scrub_config_bootstrap_provider_env(&mut cmd);

    let assert = cmd
        .current_dir(project.path())
        .env("HOME", home.path())
        .env("PATH", path_dir.path())
        .env("BY_NO_DOTENV", "1")
        .env("BY_RS_CONFIG_BOOTSTRAP_JSON", "1")
        .env_remove("BRAINYARD_PROJECT_DIR")
        .args(["config", "--auto", "--profile", "dev", "--dry-run"])
        .assert()
        .success();
    let stdout = String::from_utf8(assert.get_output().stdout.clone()).unwrap();
    let value: serde_json::Value = serde_json::from_str(&stdout).unwrap();

    assert_eq!(value["chosen"]["rung"], "a");
    assert_eq!(value["chosen"]["provider"], "claude-code");
    assert_eq!(value["chosen"]["model"], "sonnet");

    let mut rebootstrap = Command::cargo_bin("by-rs").unwrap();
    scrub_config_bootstrap_provider_env(&mut rebootstrap);
    let assert = rebootstrap
        .current_dir(project.path())
        .env("HOME", home.path())
        .env("PATH", path_dir.path())
        .env("BY_NO_DOTENV", "1")
        .env("BY_RS_CONFIG_BOOTSTRAP_JSON", "1")
        .env_remove("BRAINYARD_PROJECT_DIR")
        .args([
            "config",
            "--auto",
            "--profile",
            "dev",
            "--re-bootstrap",
            "--dry-run",
        ])
        .assert()
        .success();
    let stdout = String::from_utf8(assert.get_output().stdout.clone()).unwrap();
    let value: serde_json::Value = serde_json::from_str(&stdout).unwrap();

    assert_eq!(value["chosen"]["rung"], "c");
    assert_eq!(value["chosen"]["provider"], "claude-code");
    assert_eq!(value["chosen"]["model"], "opus");
}

#[test]
fn config_show_reads_llm_defaults_without_writing() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("config.edn");
    std::fs::write(
        &path,
        r#"{:agent {:default-agent :coact-agent
                    :config {:max-iterations 30}}
            :llm {:default-provider :bedrock
                 :default-model "amazon.nova-lite-v1:0"
                 :available-providers [:bedrock :claude-code]}
            :permissions {:mode :auto-approve
                          :allowed-dirs ["/tmp" "/workspace"]}}"#,
    )
    .unwrap();

    Command::cargo_bin("by-rs")
        .unwrap()
        .args(["config", "show", "--path"])
        .arg(&path)
        .assert()
        .success()
        .stdout(predicate::str::contains("agent.default-agent\tcoact-agent"))
        .stdout(predicate::str::contains("agent.max-iterations\t30"))
        .stdout(predicate::str::contains("permissions.mode\tauto-approve"))
        .stdout(predicate::str::contains(
            "permissions.allowed-dirs\t/tmp,/workspace",
        ))
        .stdout(predicate::str::contains("llm.default-provider\tbedrock"))
        .stdout(predicate::str::contains(
            "llm.default-model\tamazon.nova-lite-v1:0",
        ))
        .stdout(predicate::str::contains(
            "llm.available-providers\tbedrock,claude-code",
        ));
}

#[test]
fn config_show_prefers_project_config_over_user_config_by_default() {
    let project = tempfile::tempdir().unwrap();
    let home = tempfile::tempdir().unwrap();
    let nested = project.path().join("subdir");
    std::fs::create_dir_all(project.path().join(".git")).unwrap();
    std::fs::create_dir_all(project.path().join(".brainyard")).unwrap();
    std::fs::create_dir_all(home.path().join(".brainyard")).unwrap();
    std::fs::create_dir_all(&nested).unwrap();
    std::fs::write(
        project.path().join(".brainyard/config.edn"),
        r#"{:llm {:default-provider :bedrock
                 :default-model "project-model"}}"#,
    )
    .unwrap();
    std::fs::write(
        home.path().join(".brainyard/config.edn"),
        r#"{:llm {:default-provider :claude-code
                 :default-model "user-model"}}"#,
    )
    .unwrap();

    Command::cargo_bin("by-rs")
        .unwrap()
        .current_dir(&nested)
        .env("HOME", home.path())
        .env_remove("BRAINYARD_PROJECT_DIR")
        .args(["config", "show"])
        .assert()
        .success()
        .stdout(predicate::str::contains("llm.default-provider\tbedrock"))
        .stdout(predicate::str::contains("llm.default-model\tproject-model"))
        .stdout(predicate::str::contains("user-model").not());
}

#[test]
fn config_show_missing_default_config_prints_empty_defaults_without_writing() {
    let cwd = tempfile::tempdir().unwrap();
    let home = tempfile::tempdir().unwrap();

    Command::cargo_bin("by-rs")
        .unwrap()
        .current_dir(cwd.path())
        .env("HOME", home.path())
        .env_remove("BRAINYARD_PROJECT_DIR")
        .args(["config", "show"])
        .assert()
        .success()
        .stdout(predicate::str::contains("agent.default-agent\t"))
        .stdout(predicate::str::contains("agent.max-iterations\t"))
        .stdout(predicate::str::contains("permissions.mode\t"))
        .stdout(predicate::str::contains("permissions.allowed-dirs\t"))
        .stdout(predicate::str::contains("llm.default-provider\t"))
        .stdout(predicate::str::contains("llm.default-model\t"))
        .stdout(predicate::str::contains("llm.available-providers\t"));

    assert!(!cwd.path().join(".brainyard").exists());
    assert!(!home.path().join(".brainyard").exists());
}

#[test]
fn config_hidden_read_diff_snapshots_and_frontmatter_are_read_only() {
    let project = tempfile::tempdir().unwrap();
    let home = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(
        project
            .path()
            .join(".brainyard/agents/config-agent/snapshots"),
    )
    .unwrap();
    std::fs::write(
        project.path().join(".brainyard/config.edn"),
        r#"{:llm {:default-provider :bedrock
                 :default-model "amazon.nova-lite-v1:0"}
            :agent {:max-iterations 5}}"#,
    )
    .unwrap();
    let snapshot = project
        .path()
        .join(".brainyard/agents/config-agent/snapshots/20260603-120000-before-change.edn");
    std::fs::write(&snapshot, "{:llm {:default-provider :bedrock}}\n").unwrap();

    let assert = Command::cargo_bin("by-rs")
        .unwrap()
        .env("HOME", home.path())
        .args([
            "config",
            "slug",
            "--reason",
            "Improve Bedrock config",
            "--max-chars",
            "30",
        ])
        .assert()
        .success();
    let stdout = String::from_utf8(assert.get_output().stdout.clone()).unwrap();
    let value: serde_json::Value = serde_json::from_str(&stdout).unwrap();
    assert_eq!(value["slug"], "improve-bedrock-config");

    let assert = Command::cargo_bin("by-rs")
        .unwrap()
        .env("HOME", home.path())
        .args([
            "config",
            "read",
            "--project-dir",
            project.path().to_str().unwrap(),
            "--scope",
            "project",
            "--section",
            "llm",
        ])
        .assert()
        .success();
    let stdout = String::from_utf8(assert.get_output().stdout.clone()).unwrap();
    let value: serde_json::Value = serde_json::from_str(&stdout).unwrap();
    assert_eq!(value["section"], "llm");
    assert_eq!(value["scope"], "project");
    assert_eq!(value["requested-scope"], "project");
    assert_eq!(value["runtime-source"], "persisted-projection");
    assert_eq!(value["persisted"]["llm"]["default-provider"], "bedrock");
    assert_eq!(
        value["persisted"]["llm"]["default-model"],
        "amazon.nova-lite-v1:0"
    );

    let assert = Command::cargo_bin("by-rs")
        .unwrap()
        .env("HOME", home.path())
        .args([
            "config",
            "diff",
            "--project-dir",
            project.path().to_str().unwrap(),
            "--scope",
            "project",
            "--proposed-edn",
            r#"{:llm {:default-model "anthropic.claude-v3"}}"#,
        ])
        .assert()
        .success();
    let stdout = String::from_utf8(assert.get_output().stdout.clone()).unwrap();
    let value: serde_json::Value = serde_json::from_str(&stdout).unwrap();
    assert!(value["diff"]
        .as_str()
        .unwrap()
        .contains("anthropic.claude-v3"));
    assert_eq!(
        value["structural"]["changes"]["llm"]["after"]["default-model"],
        "anthropic.claude-v3"
    );

    let assert = Command::cargo_bin("by-rs")
        .unwrap()
        .env("HOME", home.path())
        .args([
            "config",
            "list-snapshots",
            "--project-dir",
            project.path().to_str().unwrap(),
            "--scope",
            "project",
            "--limit",
            "1",
        ])
        .assert()
        .success();
    let stdout = String::from_utf8(assert.get_output().stdout.clone()).unwrap();
    let value: serde_json::Value = serde_json::from_str(&stdout).unwrap();
    assert_eq!(
        value["snapshots"][0]["filename"],
        "20260603-120000-before-change.edn"
    );
    assert_eq!(value["snapshots"][0]["reason"], "before-change");

    let assert = Command::cargo_bin("by-rs")
        .unwrap()
        .env("HOME", home.path())
        .args([
            "config",
            "snapshot-preview",
            "--project-dir",
            project.path().to_str().unwrap(),
            "--scope",
            "project",
            "--reason",
            "manual edit",
            "--timestamp",
            "20260603-130000",
        ])
        .assert()
        .success();
    let stdout = String::from_utf8(assert.get_output().stdout.clone()).unwrap();
    let value: serde_json::Value = serde_json::from_str(&stdout).unwrap();
    assert_eq!(value["projection"], "common.config/config$snapshot-preview");
    assert_eq!(value["ok?"], true);
    assert_eq!(value["write-skipped?"], true);
    assert_eq!(value["rotation-skipped?"], true);
    assert_eq!(value["scope"], "project");
    assert!(value["content"]
        .as_str()
        .unwrap()
        .contains("amazon.nova-lite-v1:0"));
    assert!(value["path"]
        .as_str()
        .unwrap()
        .ends_with("20260603-130000-manual-edit.edn"));
    assert!(!Path::new(value["path"].as_str().unwrap()).exists());

    let assert = Command::cargo_bin("by-rs")
        .unwrap()
        .env("HOME", home.path())
        .args([
            "config",
            "revert-preview",
            "--project-dir",
            project.path().to_str().unwrap(),
            "--scope",
            "project",
            "--snapshot-path",
            snapshot.to_str().unwrap(),
            "--timestamp",
            "20260603-140000",
        ])
        .assert()
        .success();
    let stdout = String::from_utf8(assert.get_output().stdout.clone()).unwrap();
    let value: serde_json::Value = serde_json::from_str(&stdout).unwrap();
    assert_eq!(value["projection"], "common.config/config$revert-preview");
    assert_eq!(value["ok?"], true);
    assert_eq!(value["write-skipped?"], true);
    assert_eq!(value["snapshot-skipped?"], true);
    assert_eq!(value["restored-from"], snapshot.display().to_string());
    assert_eq!(
        value["dest"],
        project
            .path()
            .join(".brainyard/config.edn")
            .display()
            .to_string()
    );
    assert_eq!(
        value["pre-revert-snapshot"],
        project
            .path()
            .join(
                ".brainyard/agents/config-agent/snapshots/20260603-140000-revert-before-change.edn"
            )
            .display()
            .to_string()
    );
    assert_eq!(value["content"], "{:llm {:default-provider :bedrock}}\n");

    let assert = Command::cargo_bin("by-rs")
        .unwrap()
        .env("HOME", home.path())
        .args([
            "config",
            "frontmatter",
            "--slug",
            "improve-bedrock-config",
            "--question",
            "Improve Bedrock config",
            "--session-id",
            "agt-1",
            "--config-path",
            ".brainyard/config.edn",
            "--snapshot",
            "snap.edn",
            "--writes",
            "1",
            "--reverts",
            "0",
            "--started",
            "2026-06-03T12:00:00Z",
            "--next-step",
            "Run smoke",
        ])
        .assert()
        .success();
    let stdout = String::from_utf8(assert.get_output().stdout.clone()).unwrap();
    let value: serde_json::Value = serde_json::from_str(&stdout).unwrap();
    let frontmatter = value["frontmatter"].as_str().unwrap();
    assert!(frontmatter.contains("agent: config-agent"));
    assert!(frontmatter.contains("session-id: \"agt-1\""));
    assert!(frontmatter.contains("slug: improve-bedrock-config"));
    assert!(frontmatter.contains("snapshots: [snap.edn]"));
    assert!(frontmatter.contains("next-steps: [\"Run smoke\"]"));

    assert!(project.path().join(".brainyard/config.edn").exists());
    assert!(snapshot.exists());
    assert!(!home.path().join(".brainyard").exists());
}

#[test]
fn config_validate_persisted_and_secret_scan_are_live_free_projections() {
    let assert = Command::cargo_bin("by-rs")
        .unwrap()
        .args([
            "config",
            "validate-persisted",
            "--proposed-edn",
            r#"{:agent {:config {:max-iterations 10
                                 :enable-context-budget true
                                 :acp-backend :stub}}
                :llm {:available-providers [:bedrock]}}"#,
        ])
        .assert()
        .success()
        .stderr(predicate::str::is_empty());
    let stdout = String::from_utf8(assert.get_output().stdout.clone()).unwrap();
    let valid: serde_json::Value = serde_json::from_str(&stdout).unwrap();
    assert_eq!(valid["projection"], "common.config/validate-persisted");
    assert_eq!(valid["ok?"], true);
    assert_eq!(valid["secret-detected?"], false);
    assert_eq!(valid["sensitive?"], false);
    assert!(valid["errors"].as_array().unwrap().is_empty());

    let assert = Command::cargo_bin("by-rs")
        .unwrap()
        .args([
            "config",
            "validate-persisted",
            "--proposed-edn",
            r#"{:llm {:default-provider :bedrock}
                :agent {:config {:max-iterations "ten"
                                 :unknown-key true}}
                :permissions {:mode :open
                              :allowed-dirs [:tmp]}}"#,
        ])
        .assert()
        .success()
        .stderr(predicate::str::is_empty());
    let stdout = String::from_utf8(assert.get_output().stdout.clone()).unwrap();
    let invalid: serde_json::Value = serde_json::from_str(&stdout).unwrap();
    assert_eq!(invalid["ok?"], false);
    assert_eq!(invalid["sensitive?"], true);
    let errors = invalid["errors"].as_array().unwrap();
    assert!(errors.iter().any(|error| {
        error["type"] == "allowlist-violation"
            && error["path"] == serde_json::json!(["llm", "default-provider"])
    }));
    assert!(errors.iter().any(|error| {
        error["type"] == "schema-violation"
            && error["path"] == serde_json::json!(["agent", "config", "max-iterations"])
            && error["reason"]
                .as_str()
                .unwrap()
                .contains("schema type integer")
    }));
    assert!(errors.iter().any(|error| {
        error["type"] == "schema-violation"
            && error["path"] == serde_json::json!(["agent", "config", "unknown-key"])
            && error["reason"]
                .as_str()
                .unwrap()
                .contains("not a known config-schema key")
    }));
    assert!(errors
        .iter()
        .any(|error| { error["path"] == serde_json::json!(["permissions", "allowed-dirs"]) }));
    assert!(errors
        .iter()
        .any(|error| error["path"] == serde_json::json!(["permissions", "mode"])));

    let assert = Command::cargo_bin("by-rs")
        .unwrap()
        .args([
            "config",
            "validate-persisted",
            "--proposed-edn",
            r#"{:mcp {:servers {:demo {:env {:OPENAI_API_KEY "sk-ABCDEFGHIJKLMNOPQRST"}}}}}"#,
        ])
        .assert()
        .success()
        .stderr(predicate::str::is_empty());
    let stdout = String::from_utf8(assert.get_output().stdout.clone()).unwrap();
    let secret: serde_json::Value = serde_json::from_str(&stdout).unwrap();
    assert_eq!(secret["ok?"], true);
    assert_eq!(secret["secret-detected?"], true);
    assert_eq!(secret["matches"][0], "sk-ABCDE…");

    let assert = Command::cargo_bin("by-rs")
        .unwrap()
        .args([
            "config",
            "secret-scan",
            "--text",
            "token ghp_ABCDEFGHIJKLMNOPQRST should stay redacted",
        ])
        .assert()
        .success()
        .stderr(predicate::str::is_empty());
    let stdout = String::from_utf8(assert.get_output().stdout.clone()).unwrap();
    let scan: serde_json::Value = serde_json::from_str(&stdout).unwrap();
    assert_eq!(scan["projection"], "common.config/secret-scan");
    assert_eq!(scan["secret-detected?"], true);
    assert_eq!(scan["matches"][0], "ghp_ABCD…");

    let project = tempfile::tempdir().unwrap();
    let base = project.path().join(".brainyard");
    std::fs::create_dir_all(base.join("agents/config-agent")).unwrap();
    std::fs::write(base.join("agents/config-agent/INDEX.md"), "existing\n").unwrap();
    let long_summary = format!("{} {}", "change".repeat(40), "reason");
    let assert = Command::cargo_bin("by-rs")
        .unwrap()
        .args([
            "config",
            "index-append-preview",
            "--path",
            ".brainyard/agents/config-agent/dossiers/20260603-bedrock.md",
            "--slug",
            "bedrock-config",
            "--summary",
            &long_summary,
            "--created",
            "2026-06-03 12:36",
            "--base-dir",
        ])
        .arg(&base)
        .assert()
        .success()
        .stderr(predicate::str::is_empty());
    let stdout = String::from_utf8(assert.get_output().stdout.clone()).unwrap();
    let index: serde_json::Value = serde_json::from_str(&stdout).unwrap();
    assert_eq!(
        index["projection"],
        "common.config/config$index-append-preview"
    );
    assert_eq!(index["appended"], true);
    assert_eq!(index["write-skipped?"], true);
    assert_eq!(index["prepend?"], true);
    assert_eq!(index["existing?"], true);
    let line = index["line"].as_str().unwrap();
    assert!(
        line.starts_with("- 2026-06-03 12:36 [bedrock-config](dossiers/20260603-bedrock.md) — *")
    );
    assert!(line.ends_with("*\n"));
    assert!(line.contains('…'));
    assert!(line.len() < long_summary.len() + 80);
}

#[test]
fn agent_runtime_config_projects_read_and_set_without_agent_or_writes() {
    let project = tempfile::tempdir().unwrap();
    let base = project.path().join(".brainyard");
    std::fs::create_dir_all(&base).unwrap();
    let config_path = base.join("config.edn");
    std::fs::write(
        &config_path,
        r#"{:agent {:config {:max-iterations 5
                            :enable-context-budget false}}
            :permissions {:mode :auto-approve
                          :allowed-dirs ["/tmp"]}}"#,
    )
    .unwrap();

    let assert = Command::cargo_bin("by-rs")
        .unwrap()
        .args(["agent-runtime", "config", "--project-dir"])
        .arg(project.path())
        .assert()
        .success()
        .stderr(predicate::str::is_empty());
    let stdout = String::from_utf8(assert.get_output().stdout.clone()).unwrap();
    let read: serde_json::Value = serde_json::from_str(&stdout).unwrap();
    assert_eq!(read["projection"], "common.commands/agent-runtime$config");
    assert_eq!(read["ok?"], true);
    assert_eq!(read["mode"], "read");
    assert_eq!(read["live-skipped?"], true);
    assert_eq!(read["current-agent-skipped?"], true);
    assert_eq!(read["runtime-source"], "persisted-projection");
    assert_eq!(read["scope"], "project");
    assert_eq!(read["config"]["max-iterations"], 5);
    assert_eq!(read["config"]["enable-context-budget"], false);
    assert_eq!(read["config"]["permission-mode"], "auto-approve");
    assert_eq!(read["config"]["allowed-dirs"][0], "/tmp");
    assert_eq!(read["config"]["acp-backend"], "stub");
    assert_eq!(read["config"]["auto-background-timeout-ms"], 120000);
    assert!(read["config"].get("working-dir").is_none());

    let assert = Command::cargo_bin("by-rs")
        .unwrap()
        .args(["agent-runtime", "config", "--project-dir"])
        .arg(project.path())
        .args(["--key", "max-iterations", "--value", "7"])
        .assert()
        .success()
        .stderr(predicate::str::is_empty());
    let stdout = String::from_utf8(assert.get_output().stdout.clone()).unwrap();
    let set: serde_json::Value = serde_json::from_str(&stdout).unwrap();
    assert_eq!(set["mode"], "set-preview");
    assert_eq!(set["ok?"], true);
    assert_eq!(set["type"], "integer");
    assert_eq!(set["value"], 7);
    assert_eq!(set["config"]["max-iterations"], 7);
    assert_eq!(set["persisted"]["agent"]["config"]["max-iterations"], 5);
    assert_eq!(set["proposed"]["agent"]["config"]["max-iterations"], 7);
    assert_eq!(
        set["would-write"][0]["path"],
        config_path.display().to_string()
    );
    assert_eq!(
        set["would-write"][0]["path-vector"],
        serde_json::json!(["agent", "config", "max-iterations"])
    );
    assert_eq!(set["write-skipped?"], true);
    assert_eq!(set["runtime-write-skipped?"], true);
    assert_eq!(set["persisted-valid?"], true);
    assert!(set["diff"].as_str().unwrap().contains("max-iterations"));
    assert_eq!(
        set["structural"]["changes"]["agent"]["after"]["config"]["max-iterations"],
        7
    );
    let unchanged = std::fs::read_to_string(&config_path).unwrap();
    assert!(unchanged.contains(":max-iterations 5"));
    assert!(!unchanged.contains(":max-iterations 7"));

    let assert = Command::cargo_bin("by-rs")
        .unwrap()
        .args(["agent-runtime", "config", "--project-dir"])
        .arg(project.path())
        .args(["--key", "acp-backend", "--value", ":bedrock"])
        .assert()
        .success()
        .stderr(predicate::str::is_empty());
    let stdout = String::from_utf8(assert.get_output().stdout.clone()).unwrap();
    let keyword: serde_json::Value = serde_json::from_str(&stdout).unwrap();
    assert_eq!(keyword["value"], "bedrock");
    assert_eq!(keyword["config"]["acp-backend"], "bedrock");
    assert!(keyword["result"].as_str().unwrap().contains(":bedrock"));
    assert_eq!(keyword["persisted-valid?"], true);

    let assert = Command::cargo_bin("by-rs")
        .unwrap()
        .args(["agent-runtime", "config", "--key", "max-iterations"])
        .assert()
        .success()
        .stderr(predicate::str::is_empty());
    let stdout = String::from_utf8(assert.get_output().stdout.clone()).unwrap();
    let partial: serde_json::Value = serde_json::from_str(&stdout).unwrap();
    assert_eq!(partial["ok?"], false);
    assert_eq!(
        partial["error-message"],
        "Both 'key' and 'value' are required to set config; omit both to read"
    );

    let assert = Command::cargo_bin("by-rs")
        .unwrap()
        .args([
            "agent-runtime",
            "config",
            "--key",
            "not-a-config-key",
            "--value",
            "true",
        ])
        .assert()
        .success()
        .stderr(predicate::str::is_empty());
    let stdout = String::from_utf8(assert.get_output().stdout.clone()).unwrap();
    let invalid: serde_json::Value = serde_json::from_str(&stdout).unwrap();
    assert_eq!(invalid["ok?"], false);
    assert!(invalid["error-message"]
        .as_str()
        .unwrap()
        .contains("Invalid config key 'not-a-config-key'. Valid:"));
}

#[test]
fn config_apply_preflight_projects_apply_gates_without_writes() {
    let assert = Command::cargo_bin("by-rs")
        .unwrap()
        .args([
            "config",
            "apply-preflight",
            "--proposed-edn",
            "{:llm {:default-provider :openai}}",
            "--reason",
            "Change provider",
        ])
        .assert()
        .success()
        .stderr(predicate::str::is_empty());
    let stdout = String::from_utf8(assert.get_output().stdout.clone()).unwrap();
    let invalid: serde_json::Value = serde_json::from_str(&stdout).unwrap();
    assert_eq!(invalid["projection"], "common.config/apply-preflight");
    assert_eq!(invalid["ok?"], false);
    assert_eq!(invalid["stage"], "validate");
    assert!(invalid["hint"]
        .as_str()
        .unwrap()
        .contains("bootstrap$re-run-rung"));
    assert_eq!(invalid["write-skipped?"], true);
    assert_eq!(invalid["snapshot-skipped?"], true);
    assert_eq!(invalid["smoke-test-skipped?"], true);

    let assert = Command::cargo_bin("by-rs")
        .unwrap()
        .args([
            "config",
            "apply-preflight",
            "--proposed-edn",
            r#"{:mcp {:servers {:demo {:env {:OPENAI_API_KEY "sk-ABCDEFGHIJKLMNOPQRST"}}}}}"#,
            "--reason",
            "Add MCP",
            "--confirm",
        ])
        .assert()
        .success()
        .stderr(predicate::str::is_empty());
    let stdout = String::from_utf8(assert.get_output().stdout.clone()).unwrap();
    let secret: serde_json::Value = serde_json::from_str(&stdout).unwrap();
    assert_eq!(secret["ok?"], false);
    assert_eq!(secret["stage"], "secret-detected");
    assert_eq!(secret["matches"][0], "sk-ABCDE…");

    let assert = Command::cargo_bin("by-rs")
        .unwrap()
        .args([
            "config",
            "apply-preflight",
            "--current-edn",
            "{:agent {:default-agent :coact}}",
            "--proposed-edn",
            "{:agent {:default-agent :react}}",
            "--reason",
            "Switch agent",
            "--target-path",
            "/tmp/config.edn",
        ])
        .assert()
        .success()
        .stderr(predicate::str::is_empty());
    let stdout = String::from_utf8(assert.get_output().stdout.clone()).unwrap();
    let unconfirmed: serde_json::Value = serde_json::from_str(&stdout).unwrap();
    assert_eq!(unconfirmed["ok?"], false);
    assert_eq!(unconfirmed["stage"], "unconfirmed");
    assert_eq!(unconfirmed["path"], "/tmp/config.edn");
    assert!(unconfirmed["diff"].as_str().unwrap().contains("react"));
    assert!(unconfirmed["structural"]["changes"]["agent"]["after"]["default-agent"] == "react");

    let assert = Command::cargo_bin("by-rs")
        .unwrap()
        .args([
            "config",
            "apply-preflight",
            "--proposed-edn",
            "{:permissions {:mode :auto-approve}}",
            "--reason",
            "Loosen permissions",
            "--auto",
        ])
        .assert()
        .success()
        .stderr(predicate::str::is_empty());
    let stdout = String::from_utf8(assert.get_output().stdout.clone()).unwrap();
    let sensitive_auto: serde_json::Value = serde_json::from_str(&stdout).unwrap();
    assert_eq!(sensitive_auto["ok?"], false);
    assert_eq!(sensitive_auto["stage"], "unconfirmed");
    assert_eq!(sensitive_auto["sensitive?"], true);
    assert!(sensitive_auto["hint"]
        .as_str()
        .unwrap()
        .contains("--auto cannot self-confirm"));

    let assert = Command::cargo_bin("by-rs")
        .unwrap()
        .args([
            "config",
            "apply-preflight",
            "--proposed-edn",
            "{:agent {:default-agent :react}}",
            "--reason",
            "Switch agent",
            "--confirm",
            "--expected-mtime",
            "1",
            "--current-mtime",
            "2",
        ])
        .assert()
        .success()
        .stderr(predicate::str::is_empty());
    let stdout = String::from_utf8(assert.get_output().stdout.clone()).unwrap();
    let conflict: serde_json::Value = serde_json::from_str(&stdout).unwrap();
    assert_eq!(conflict["ok?"], false);
    assert_eq!(conflict["stage"], "mtime-conflict");
    assert_eq!(conflict["expected"], 1);
    assert_eq!(conflict["actual"], 2);

    let assert = Command::cargo_bin("by-rs")
        .unwrap()
        .args([
            "config",
            "apply-preflight",
            "--current-edn",
            "{:agent {:default-agent :coact}}",
            "--proposed-edn",
            "{:agent {:default-agent :react}}",
            "--reason",
            "Switch agent",
            "--confirm",
            "--expected-mtime",
            "2",
            "--current-mtime",
            "2",
        ])
        .assert()
        .success()
        .stderr(predicate::str::is_empty());
    let stdout = String::from_utf8(assert.get_output().stdout.clone()).unwrap();
    let ready: serde_json::Value = serde_json::from_str(&stdout).unwrap();
    assert_eq!(ready["ok?"], true);
    assert_eq!(ready["stage"], "ready-to-write");
    assert_eq!(ready["write-skipped?"], true);
    assert_eq!(ready["snapshot-skipped?"], true);
    assert_eq!(ready["smoke-test-skipped?"], true);
    assert_eq!(ready["updated-at-would-change?"], true);
}

#[test]
fn ask_dry_run_uses_project_config_defaults_before_user_config() {
    let project = tempfile::tempdir().unwrap();
    let home = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(project.path().join(".git")).unwrap();
    std::fs::create_dir_all(project.path().join(".brainyard")).unwrap();
    std::fs::create_dir_all(home.path().join(".brainyard")).unwrap();
    std::fs::write(
        project.path().join(".brainyard/config.edn"),
        r#"{:llm {:default-provider :bedrock
                 :default-model "amazon.nova-lite-v1:0"}}"#,
    )
    .unwrap();
    std::fs::write(
        home.path().join(".brainyard/config.edn"),
        r#"{:llm {:default-provider :bedrock
                 :default-model "user-model"}}"#,
    )
    .unwrap();

    Command::cargo_bin("by-rs")
        .unwrap()
        .current_dir(project.path())
        .env("HOME", home.path())
        .env_remove("BRAINYARD_PROJECT_DIR")
        .args(["ask", "--dry-run", "What is 2+2?"])
        .assert()
        .success()
        .stdout(predicate::str::contains(
            "\"modelId\": \"amazon.nova-lite-v1:0\"",
        ))
        .stdout(predicate::str::contains("user-model").not());
}

#[test]
fn ask_dry_run_resolves_default_agent_from_project_config() {
    let project = tempfile::tempdir().unwrap();
    let home = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(project.path().join(".git")).unwrap();
    std::fs::create_dir_all(project.path().join(".brainyard")).unwrap();
    std::fs::write(
        project.path().join(".brainyard/config.edn"),
        r#"{:agent {:default-agent :main-agent}
            :llm {:default-provider :bedrock
                  :default-model "amazon.nova-lite-v1:0"}}"#,
    )
    .unwrap();

    Command::cargo_bin("by-rs")
        .unwrap()
        .current_dir(project.path())
        .env("HOME", home.path())
        .env_remove("BRAINYARD_PROJECT_DIR")
        .args(["ask", "--dry-run", "What is 2+2?"])
        .assert()
        .success()
        .stdout(predicate::str::contains("\"agent_id\": \"main-agent\""))
        .stdout(predicate::str::contains(
            "\"modelId\": \"amazon.nova-lite-v1:0\"",
        ));
}

#[test]
fn ask_dry_run_explicit_agent_wins_over_config_default() {
    let project = tempfile::tempdir().unwrap();
    let home = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(project.path().join(".git")).unwrap();
    std::fs::create_dir_all(project.path().join(".brainyard")).unwrap();
    std::fs::write(
        project.path().join(".brainyard/config.edn"),
        r#"{:agent {:default-agent :main-agent}
            :llm {:default-provider :bedrock
                  :default-model "amazon.nova-lite-v1:0"}}"#,
    )
    .unwrap();

    Command::cargo_bin("by-rs")
        .unwrap()
        .current_dir(project.path())
        .env("HOME", home.path())
        .env_remove("BRAINYARD_PROJECT_DIR")
        .args(["ask", "-a", "research-agent", "--dry-run", "What is 2+2?"])
        .assert()
        .success()
        .stdout(predicate::str::contains("\"agent_id\": \"research-agent\""))
        .stdout(predicate::str::contains("\"agent_id\": \"main-agent\"").not());
}

#[test]
fn ask_dry_run_exposes_agent_registry_max_iterations_default() {
    let dir = tempfile::tempdir().unwrap();

    Command::cargo_bin("by-rs")
        .unwrap()
        .current_dir(dir.path())
        .env("HOME", dir.path())
        .env_remove("BRAINYARD_PROJECT_DIR")
        .args([
            "ask",
            "-a",
            "debug-agent",
            "-p",
            "bedrock",
            "-m",
            "amazon.nova-lite-v1:0",
            "--dry-run",
            "What is 2+2?",
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("\"agent_id\": \"debug-agent\""))
        .stdout(predicate::str::contains("\"max_iterations\": 30"));
}

#[test]
fn ask_dry_run_exposes_config_schema_max_iterations_fallback() {
    let dir = tempfile::tempdir().unwrap();

    Command::cargo_bin("by-rs")
        .unwrap()
        .current_dir(dir.path())
        .env("HOME", dir.path())
        .env_remove("BRAINYARD_PROJECT_DIR")
        .args([
            "ask",
            "-p",
            "bedrock",
            "-m",
            "amazon.nova-lite-v1:0",
            "--dry-run",
            "What is 2+2?",
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("\"agent_id\": \"coact-agent\""))
        .stdout(predicate::str::contains("\"max_iterations\": 100"))
        .stdout(predicate::str::contains("\"max_iterations\": 30").not());
}

#[test]
fn ask_dry_run_exposes_explicit_max_iterations_override() {
    Command::cargo_bin("by-rs")
        .unwrap()
        .args([
            "ask",
            "-a",
            "debug-agent",
            "-p",
            "bedrock",
            "-m",
            "amazon.nova-lite-v1:0",
            "-n",
            "7",
            "--dry-run",
            "What is 2+2?",
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("\"max_iterations\": 7"))
        .stdout(predicate::str::contains("\"max_iterations\": 30").not());
}

#[test]
fn help_exposes_memory_search_command() {
    Command::cargo_bin("by-rs")
        .unwrap()
        .args(["memory", "--help"])
        .assert()
        .success()
        .stdout(predicate::str::contains("inspect"))
        .stdout(predicate::str::contains("search"));
}

#[test]
fn memory_search_reads_sqlite_fts_without_writing() {
    let dir = tempfile::tempdir().unwrap();
    let db_path = dir.path().join("memory.db");
    let conn = Connection::open(&db_path).unwrap();
    create_memory_schema(&conn);
    conn.execute(
        "INSERT INTO episodes (session_id, user_id, episode_type, role, content)
         VALUES ('s1', 'u1', 'conversation', 'assistant', 'blue deploy note')",
        [],
    )
    .unwrap();
    conn.execute(
        "INSERT INTO semantic_facts (user_id, fact_type, content, confidence)
         VALUES ('u1', 'preference', 'green release preference', 0.9)",
        [],
    )
    .unwrap();
    drop(conn);

    Command::cargo_bin("by-rs")
        .unwrap()
        .args(["memory", "search", "--db"])
        .arg(&db_path)
        .args(["--query", "blue green"])
        .assert()
        .success()
        .stdout(predicate::str::contains(
            "l2\tconversation\tblue deploy note",
        ))
        .stdout(predicate::str::contains(
            "l3\tpreference\tgreen release preference",
        ));
}

#[test]
fn memory_search_uses_default_user_db_path_from_environment_without_project_writes() {
    let project = tempfile::tempdir().unwrap();
    let home = tempfile::tempdir().unwrap();
    let db_path = home.path().join(".brainyard/memory/env-user.db");
    std::fs::create_dir_all(db_path.parent().unwrap()).unwrap();
    let conn = Connection::open(&db_path).unwrap();
    create_memory_schema(&conn);
    conn.execute(
        "INSERT INTO episodes (session_id, user_id, episode_type, role, content)
         VALUES ('s1', 'env-user', 'conversation', 'assistant', 'blue default-path note')",
        [],
    )
    .unwrap();
    drop(conn);

    Command::cargo_bin("by-rs")
        .unwrap()
        .current_dir(project.path())
        .env("HOME", home.path())
        .env("BY_USER_ID", "env-user")
        .env_remove("BY_ENV_FILE")
        .env_remove("BY_NO_DOTENV")
        .args(["memory", "search", "--query", "blue"])
        .assert()
        .success()
        .stdout(predicate::str::contains(
            "l2\tconversation\tblue default-path note",
        ));

    assert!(!project.path().join(".brainyard").exists());
}

#[test]
fn memory_search_uses_default_user_db_path_from_project_dotenv() {
    let project = tempfile::tempdir().unwrap();
    let home = tempfile::tempdir().unwrap();
    let nested = project.path().join("nested/work");
    std::fs::create_dir_all(&nested).unwrap();
    std::fs::write(project.path().join(".env"), "BY_USER_ID=dotenv-user\n").unwrap();

    let db_path = home.path().join(".brainyard/memory/dotenv-user.db");
    std::fs::create_dir_all(db_path.parent().unwrap()).unwrap();
    let conn = Connection::open(&db_path).unwrap();
    create_memory_schema(&conn);
    conn.execute(
        "INSERT INTO episodes (session_id, user_id, episode_type, role, content)
         VALUES ('s1', 'dotenv-user', 'conversation', 'assistant', 'dotenv default-path note')",
        [],
    )
    .unwrap();
    drop(conn);

    Command::cargo_bin("by-rs")
        .unwrap()
        .current_dir(&nested)
        .env("HOME", home.path())
        .env_remove("BY_USER_ID")
        .env_remove("BY_ENV_FILE")
        .env_remove("BY_NO_DOTENV")
        .args(["memory", "search", "--query", "dotenv"])
        .assert()
        .success()
        .stdout(predicate::str::contains(
            "l2\tconversation\tdotenv default-path note",
        ));

    assert!(!project.path().join(".brainyard").exists());
}

#[test]
fn memory_inspect_reports_schema_and_counts_without_writing() {
    let dir = tempfile::tempdir().unwrap();
    let db_path = dir.path().join("memory.db");
    let conn = Connection::open(&db_path).unwrap();
    create_memory_schema(&conn);
    conn.execute(
        "INSERT OR REPLACE INTO memory_metadata (key, value)
         VALUES ('schema_version', '2.0.0')",
        [],
    )
    .unwrap();
    conn.execute(
        "INSERT INTO episodes (session_id, user_id, episode_type, role, content)
         VALUES ('s1', 'u1', 'conversation', 'assistant', 'blue deploy note')",
        [],
    )
    .unwrap();
    conn.execute(
        "INSERT INTO semantic_facts (user_id, fact_type, content, confidence)
         VALUES ('u1', 'preference', 'green release preference', 0.9)",
        [],
    )
    .unwrap();
    conn.execute(
        "INSERT INTO memory_audit
         (user_id, session_id, agent_id, turn_id, total_turns, entry_id, layer, byte_cost)
         VALUES ('u1', 's1', 'coact-agent', 1, 1, 'entry-1', 'l2', 42)",
        [],
    )
    .unwrap();
    drop(conn);

    Command::cargo_bin("by-rs")
        .unwrap()
        .args(["memory", "inspect", "--db"])
        .arg(&db_path)
        .assert()
        .success()
        .stdout(predicate::str::contains("schema-version: 2.0.0"))
        .stdout(predicate::str::contains("sqlite-user-version: 0"))
        .stdout(predicate::str::contains("journal-mode:"))
        .stdout(predicate::str::contains("memory_metadata"))
        .stdout(predicate::str::contains("episodes"))
        .stdout(predicate::str::contains("episodes_fts"))
        .stdout(predicate::str::contains("semantic_facts"))
        .stdout(predicate::str::contains("semantic_fts"))
        .stdout(predicate::str::contains("memory_audit"));
}

#[test]
fn memory_inspect_uses_user_id_flag_for_default_db_path() {
    let project = tempfile::tempdir().unwrap();
    let home = tempfile::tempdir().unwrap();
    let db_path = home.path().join(".brainyard/memory/alice.db");
    std::fs::create_dir_all(db_path.parent().unwrap()).unwrap();
    let conn = Connection::open(&db_path).unwrap();
    create_memory_schema(&conn);
    conn.execute(
        "INSERT OR REPLACE INTO memory_metadata (key, value)
         VALUES ('schema_version', '2.0.0')",
        [],
    )
    .unwrap();
    drop(conn);

    Command::cargo_bin("by-rs")
        .unwrap()
        .current_dir(project.path())
        .env("HOME", home.path())
        .env("BY_USER_ID", "env-user")
        .env_remove("BY_ENV_FILE")
        .env_remove("BY_NO_DOTENV")
        .args(["memory", "inspect", "--user-id", "alice"])
        .assert()
        .success()
        .stdout(predicate::str::contains("Memory database:"))
        .stdout(predicate::str::contains("alice.db"))
        .stdout(predicate::str::contains("schema-version: 2.0.0"))
        .stdout(predicate::str::contains("env-user").not());
}

#[test]
fn memory_stats_and_keywords_are_live_free_json_projections() {
    let dir = tempfile::tempdir().unwrap();
    let db_path = dir.path().join("memory.db");
    let conn = Connection::open(&db_path).unwrap();
    create_memory_schema(&conn);
    conn.execute(
        "INSERT OR REPLACE INTO memory_metadata (key, value)
         VALUES ('schema_version', '2.0.0')",
        [],
    )
    .unwrap();
    conn.execute(
        "INSERT INTO episodes (session_id, user_id, episode_type, role, content, keep_flag)
         VALUES ('s1', 'u1', 'conversation', 'assistant', 'blue deploy note', 1)",
        [],
    )
    .unwrap();
    conn.execute(
        "INSERT INTO episodes (session_id, user_id, episode_type, role, content, tombstoned_flag)
         VALUES ('s2', 'u1', 'conversation', 'assistant', 'deleted note', 1)",
        [],
    )
    .unwrap();
    conn.execute(
        "INSERT INTO semantic_facts (user_id, fact_type, content, confidence)
         VALUES ('u1', 'preference', 'green release preference', 0.9)",
        [],
    )
    .unwrap();
    conn.execute(
        "INSERT INTO memory_audit
         (user_id, session_id, agent_id, turn_id, total_turns, entry_id, layer, byte_cost)
         VALUES ('u1', 's1', 'coact-agent', 1, 1, 'entry-1', 'l2', 42)",
        [],
    )
    .unwrap();
    drop(conn);

    let assert = Command::cargo_bin("by-rs")
        .unwrap()
        .args(["memory", "stats", "--db"])
        .arg(&db_path)
        .args(["--user-id", "u1", "--session-id", "s1"])
        .assert()
        .success();
    let stdout = String::from_utf8(assert.get_output().stdout.clone()).unwrap();
    let stats: serde_json::Value = serde_json::from_str(&stdout).unwrap();
    assert_eq!(stats["stats"]["l2"]["total"], 1);
    assert_eq!(stats["stats"]["l2"]["current-session"], 1);
    assert_eq!(stats["stats"]["l2"]["tombstoned"], 1);
    assert_eq!(stats["stats"]["l3"]["by-kind"]["preference"], 1);
    assert_eq!(stats["stats"]["audit"]["rows"], 1);

    let assert = Command::cargo_bin("by-rs")
        .unwrap()
        .args(["memory", "status", "--db"])
        .arg(&db_path)
        .args(["--user-id", "u1", "--session-id", "s1"])
        .assert()
        .success();
    let stdout = String::from_utf8(assert.get_output().stdout.clone()).unwrap();
    let status: serde_json::Value = serde_json::from_str(&stdout).unwrap();
    assert_eq!(status["projection"], "common.commands/memory$status");
    assert_eq!(status["live-skipped?"], true);
    assert_eq!(status["source"], "local-sqlite");
    assert_eq!(status["user-id"], "u1");
    assert_eq!(status["session-id"], "s1");
    assert_eq!(status["schema-version"], "2.0.0");
    assert_eq!(status["capture-running?"], false);
    assert_eq!(status["l1"]["session-entries"], 0);
    assert_eq!(status["l2"]["total"], 1);
    assert_eq!(status["l2"]["session"], 1);
    assert_eq!(status["l3"]["total"], 1);
    assert_eq!(status["audit"]["turns"], 1);
    assert_eq!(status["audit"]["total-prompt-bytes"], 42);

    let assert = Command::cargo_bin("by-rs")
        .unwrap()
        .args([
            "memory",
            "keywords",
            "--text",
            "AWS EC2 costs are high, need to optimize EC2 spending",
        ])
        .assert()
        .success();
    let stdout = String::from_utf8(assert.get_output().stdout.clone()).unwrap();
    let keywords: serde_json::Value = serde_json::from_str(&stdout).unwrap();
    assert!(keywords["keywords"]
        .as_array()
        .unwrap()
        .iter()
        .any(|keyword| keyword == "ec2"));
    assert!(!keywords["keywords"]
        .as_array()
        .unwrap()
        .iter()
        .any(|keyword| keyword == "need"));
}

#[test]
fn memory_recall_projects_common_command_shape_without_live_agent() {
    let dir = tempfile::tempdir().unwrap();
    let db_path = dir.path().join("memory.db");
    let conn = Connection::open(&db_path).unwrap();
    create_memory_schema(&conn);
    conn.execute(
        "INSERT INTO episodes
         (session_id, user_id, episode_type, role, content, tags, entry_id)
         VALUES
         ('s1', 'u1', 'conversation', 'assistant', 'remember the blue deploy plan',
          '[\"blue\"]', 'ep-blue')",
        [],
    )
    .unwrap();
    conn.execute(
        "INSERT INTO episodes
         (session_id, user_id, episode_type, role, content, entry_id)
         VALUES
         ('s2', 'u1', 'conversation', 'assistant', 'blue deploy other session', 'ep-other')",
        [],
    )
    .unwrap();
    conn.execute(
        "INSERT INTO semantic_facts
         (user_id, fact_type, content, confidence, tags, entry_id)
         VALUES
         ('u1', 'preference', 'user prefers green releases', 0.9,
          '[\"green\"]', 'fact-green')",
        [],
    )
    .unwrap();
    conn.execute(
        "INSERT INTO semantic_facts
         (user_id, fact_type, content, confidence, tombstoned_flag, entry_id)
         VALUES ('u1', 'preference', 'green tombstoned fact', 1.0, 1, 'fact-tombstoned')",
        [],
    )
    .unwrap();
    drop(conn);

    let assert = Command::cargo_bin("by-rs")
        .unwrap()
        .args(["memory", "recall", "--db"])
        .arg(&db_path)
        .args([
            "--user-id",
            "u1",
            "--session-id",
            "s1",
            "--query",
            "blue green",
            "--match",
            "or",
        ])
        .assert()
        .success();
    let stdout = String::from_utf8(assert.get_output().stdout.clone()).unwrap();
    let recall: serde_json::Value = serde_json::from_str(&stdout).unwrap();
    assert_eq!(recall["projection"], "common.commands/memory$recall");
    assert_eq!(recall["source"], "local-sqlite");
    assert_eq!(recall["live-skipped?"], true);
    assert_eq!(recall["current-agent-skipped?"], true);
    assert_eq!(recall["user-id"], "u1");
    assert_eq!(recall["session-id"], "s1");
    assert_eq!(recall["query"], "blue green");
    assert_eq!(recall["requested-layer"], serde_json::Value::Null);
    assert_eq!(recall["limit"], 10);
    assert_eq!(recall["layer"], "combined");
    assert_eq!(recall["count"], 2);
    assert_eq!(recall["entries"][0]["_layer"], "l3");
    assert_eq!(recall["entries"][0]["kind"], "preference");
    assert!(recall["entries"].as_array().unwrap().iter().any(|entry| {
        entry["_layer"] == "l2"
            && entry["id"] == "ep-blue"
            && entry["session-id"] == "s1"
            && entry["tags"].as_array().is_some_and(|tags| tags.len() == 1)
    }));

    let assert = Command::cargo_bin("by-rs")
        .unwrap()
        .args(["memory", "recall", "--db"])
        .arg(&db_path)
        .args([
            "--user-id",
            "u1",
            "--session-id",
            "s1",
            "--layer",
            "l1",
            "--query",
            "blue",
        ])
        .assert()
        .success();
    let l1: serde_json::Value =
        serde_json::from_slice(&assert.get_output().stdout).expect("l1 recall json");
    assert_eq!(l1["layer"], "l1");
    assert_eq!(l1["count"], 0);
    assert_eq!(l1["l1-persisted?"], false);
    assert_eq!(l1["l1-live-skipped?"], true);
}

#[test]
fn memory_read_projects_memory_agent_shape_without_live_agent() {
    let dir = tempfile::tempdir().unwrap();
    let db_path = dir.path().join("memory.db");
    let conn = Connection::open(&db_path).unwrap();
    create_memory_schema(&conn);
    conn.execute(
        "INSERT INTO episodes
         (session_id, user_id, episode_type, role, content, tags, entry_id)
         VALUES
         ('s1', 'u1', 'conversation', 'assistant', 'remember the blue deploy plan',
          '[\"blue\"]', 'ep-blue')",
        [],
    )
    .unwrap();
    conn.execute(
        "INSERT INTO episodes
         (session_id, user_id, episode_type, role, content, archived_flag, entry_id)
         VALUES
         ('s1', 'u1', 'conversation', 'assistant', 'archived blue deploy', 1, 'ep-archived')",
        [],
    )
    .unwrap();
    conn.execute(
        "INSERT INTO semantic_facts
         (user_id, fact_type, content, confidence, tags, entry_id)
         VALUES
         ('u1', 'preference', 'user prefers green releases', 0.9,
          '[\"green\"]', 'fact-green')",
        [],
    )
    .unwrap();
    drop(conn);

    let assert = Command::cargo_bin("by-rs")
        .unwrap()
        .args(["memory", "read", "--db"])
        .arg(&db_path)
        .args([
            "--user-id",
            "u1",
            "--layer",
            "l2",
            "--query",
            "{\"id\":\"ep-blue\"}",
        ])
        .assert()
        .success();
    let read: serde_json::Value =
        serde_json::from_slice(&assert.get_output().stdout).expect("memory read json");
    assert_eq!(read["projection"], "memory-agent/memory$read");
    assert_eq!(read["source"], "local-sqlite");
    assert_eq!(read["live-skipped?"], true);
    assert_eq!(read["current-agent-skipped?"], true);
    assert_eq!(read["user-id"], "u1");
    assert_eq!(read["requested-layer"], "l2");
    assert_eq!(read["query"]["id"], "ep-blue");
    assert_eq!(read["limit"], 20);
    assert_eq!(read["include-archived?"], false);
    assert_eq!(read["layer"], "l2");
    assert_eq!(read["count"], 1);
    assert_eq!(read["entries"][0]["id"], "ep-blue");
    assert_eq!(read["entries"][0]["kind"], "conversation");
    assert_eq!(read["entries"][0]["session-id"], "s1");

    let assert = Command::cargo_bin("by-rs")
        .unwrap()
        .args(["memory", "read", "--db"])
        .arg(&db_path)
        .args([
            "--user-id",
            "u1",
            "--layer",
            "l2",
            "--query",
            "{\"id\":\"ep-archived\"}",
            "--include-archived",
        ])
        .assert()
        .success();
    let archived: serde_json::Value =
        serde_json::from_slice(&assert.get_output().stdout).expect("archived read json");
    assert_eq!(archived["count"], 1);
    assert_eq!(archived["entries"][0]["archived"], true);

    let assert = Command::cargo_bin("by-rs")
        .unwrap()
        .args(["memory", "read", "--db"])
        .arg(&db_path)
        .args([
            "--user-id",
            "u1",
            "--layer",
            "l3",
            "--query",
            "{:text \"green releases\" :fact-type :preference :min-confidence 0.8 :match :phrase}",
            "--limit",
            "5",
        ])
        .assert()
        .success();
    let l3: serde_json::Value =
        serde_json::from_slice(&assert.get_output().stdout).expect("l3 read json");
    assert_eq!(l3["query"]["fact-type"], "preference");
    assert_eq!(l3["limit"], 5);
    assert_eq!(l3["layer"], "l3");
    assert_eq!(l3["count"], 1);
    assert_eq!(l3["entries"][0]["id"], "fact-green");
    assert_eq!(l3["entries"][0]["confidence"], 0.9);

    let assert = Command::cargo_bin("by-rs")
        .unwrap()
        .args(["memory", "read", "--db"])
        .arg(&db_path)
        .args([
            "--user-id",
            "u1",
            "--layer",
            "l1",
            "--query",
            "{\"session-id\":\"s1\"}",
        ])
        .assert()
        .success();
    let l1: serde_json::Value =
        serde_json::from_slice(&assert.get_output().stdout).expect("l1 read json");
    assert_eq!(l1["layer"], "l1");
    assert_eq!(l1["count"], 0);
    assert_eq!(l1["l1-persisted?"], false);
    assert_eq!(l1["l1-live-skipped?"], true);
}

#[test]
fn memory_policy_toggles_project_memory_agent_write_primitives_without_live_agent() {
    let dir = tempfile::tempdir().unwrap();
    let db_path = dir.path().join("memory.db");
    let conn = Connection::open(&db_path).unwrap();
    create_memory_schema(&conn);
    conn.execute(
        "INSERT INTO episodes
         (session_id, user_id, episode_type, role, content, tags, entry_id)
         VALUES
         ('s1', 'u1', 'conversation', 'assistant', 'remember the blue deploy plan',
          '[\"blue\"]', 'ep-blue')",
        [],
    )
    .unwrap();
    conn.execute(
        "INSERT INTO semantic_facts
         (user_id, fact_type, content, confidence, tags, entry_id)
         VALUES
         ('u1', 'preference', 'user prefers green releases', 0.9,
          '[\"green\"]', 'fact-green')",
        [],
    )
    .unwrap();
    drop(conn);

    let assert = Command::cargo_bin("by-rs")
        .unwrap()
        .args(["memory", "keep", "--db"])
        .arg(&db_path)
        .args(["--user-id", "u1", "--layer", "l2", "--entry-id", "ep-blue"])
        .assert()
        .success();
    let keep: serde_json::Value =
        serde_json::from_slice(&assert.get_output().stdout).expect("memory keep json");
    assert_eq!(keep["projection"], "memory-agent/memory$keep!");
    assert_eq!(keep["source"], "local-sqlite");
    assert_eq!(keep["live-skipped?"], true);
    assert_eq!(keep["current-agent-skipped?"], true);
    assert_eq!(keep["ok"], true);
    assert_eq!(keep["layer"], "l2");
    assert_eq!(keep["entry-id"], "ep-blue");
    assert_eq!(keep["value"], true);

    let assert = Command::cargo_bin("by-rs")
        .unwrap()
        .args(["memory", "archive!", "--db"])
        .arg(&db_path)
        .args([
            "--user-id",
            "u1",
            "--layer",
            "l3",
            "--entry-id",
            "fact-green",
            "--value",
            "true",
        ])
        .assert()
        .success();
    let archive: serde_json::Value =
        serde_json::from_slice(&assert.get_output().stdout).expect("memory archive json");
    assert_eq!(archive["projection"], "memory-agent/memory$archive!");
    assert_eq!(archive["ok"], true);
    assert_eq!(archive["layer"], "l3");
    assert_eq!(archive["entry-id"], "fact-green");
    assert_eq!(archive["value"], true);

    let assert = Command::cargo_bin("by-rs")
        .unwrap()
        .args(["memory", "read", "--db"])
        .arg(&db_path)
        .args([
            "--user-id",
            "u1",
            "--layer",
            "l3",
            "--query",
            "{\"id\":\"fact-green\"}",
        ])
        .assert()
        .success();
    let archived_hidden: serde_json::Value =
        serde_json::from_slice(&assert.get_output().stdout).expect("archived hidden json");
    assert_eq!(archived_hidden["count"], 0);

    let assert = Command::cargo_bin("by-rs")
        .unwrap()
        .args(["memory", "forget", "--db"])
        .arg(&db_path)
        .args([
            "--user-id",
            "u1",
            "--layer",
            ":l2",
            "--entry-id",
            "ep-blue",
            "--reason",
            "test cleanup",
        ])
        .assert()
        .success();
    let forget: serde_json::Value =
        serde_json::from_slice(&assert.get_output().stdout).expect("memory forget json");
    assert_eq!(forget["projection"], "memory-agent/memory$forget");
    assert_eq!(forget["ok"], true);
    assert_eq!(forget["layer"], "l2");
    assert_eq!(forget["entry-id"], "ep-blue");
    assert_eq!(forget["reason"], "test cleanup");

    let assert = Command::cargo_bin("by-rs")
        .unwrap()
        .args(["memory", "read", "--db"])
        .arg(&db_path)
        .args([
            "--user-id",
            "u1",
            "--layer",
            "l2",
            "--query",
            "{\"id\":\"ep-blue\"}",
            "--include-archived",
        ])
        .assert()
        .success();
    let tombstoned_hidden: serde_json::Value =
        serde_json::from_slice(&assert.get_output().stdout).expect("tombstoned hidden json");
    assert_eq!(tombstoned_hidden["count"], 0);
}

#[test]
fn memory_write_and_remember_project_persisted_entries_without_live_agent() {
    let dir = tempfile::tempdir().unwrap();
    let db_path = dir.path().join("memory.db");
    let conn = Connection::open(&db_path).unwrap();
    create_memory_schema(&conn);
    drop(conn);

    let assert = Command::cargo_bin("by-rs")
        .unwrap()
        .args(["memory", "write", "--db"])
        .arg(&db_path)
        .args([
            "--user-id",
            "u1",
            "--layer",
            "l2",
            "--entry",
            "{:session-id \"s-write\" :kind :observation :role \"assistant\" :content \"remember blue deploy\" :tags [\"blue\"] :data {:plan 1}}",
        ])
        .assert()
        .success();
    let write: serde_json::Value =
        serde_json::from_slice(&assert.get_output().stdout).expect("memory write json");
    assert_eq!(write["projection"], "memory-agent/memory$write");
    assert_eq!(write["source"], "local-sqlite");
    assert_eq!(write["live-skipped?"], true);
    assert_eq!(write["current-agent-skipped?"], true);
    assert_eq!(write["requested-layer"], "l2");
    assert_eq!(write["layer"], "l2");
    assert_eq!(write["entry"]["session-id"], "s-write");
    assert_eq!(write["entry"]["kind"], "observation");
    assert_eq!(write["entry"]["data"]["plan"], 1);
    assert!(write["entry-id"].as_str().unwrap().starts_with("l2/"));

    let remember_content = "green release preference";
    let expected_remember_id =
        by_memory::entry_id_for("l3", remember_content).expect("remember entry id");
    let assert = Command::cargo_bin("by-rs")
        .unwrap()
        .args(["memory", "remember", "--db"])
        .arg(&db_path)
        .args([
            "--user-id",
            "u1",
            "--layer",
            "l3",
            "--kind",
            "preference",
            "--content",
            remember_content,
            "--tag",
            "green",
            "--confidence",
            "0.75",
        ])
        .assert()
        .success();
    let remember: serde_json::Value =
        serde_json::from_slice(&assert.get_output().stdout).expect("memory remember json");
    assert_eq!(remember["projection"], "common.commands/memory$remember");
    assert_eq!(remember["source"], "local-sqlite");
    assert_eq!(remember["live-skipped?"], true);
    assert_eq!(remember["entry-id"], expected_remember_id);
    assert_eq!(remember["layer"], "l3");
    assert_eq!(remember["entry"]["kind"], "preference");
    assert_eq!(remember["entry"]["tags"][0], "green");
    assert_eq!(remember["entry"]["confidence"], 0.75);
    assert!(remember["result"]
        .as_str()
        .unwrap()
        .contains("Stored in l3 (kind: preference"));

    let assert = Command::cargo_bin("by-rs")
        .unwrap()
        .args(["memory", "read", "--db"])
        .arg(&db_path)
        .args([
            "--user-id",
            "u1",
            "--layer",
            "l3",
            "--query",
            &format!("{{\"id\":\"{expected_remember_id}\"}}"),
        ])
        .assert()
        .success();
    let read: serde_json::Value =
        serde_json::from_slice(&assert.get_output().stdout).expect("memory read json");
    assert_eq!(read["count"], 1);
    assert_eq!(read["entries"][0]["content"], remember_content);
}

#[test]
fn memory_promote_projects_persisted_cross_layer_copy_without_live_agent() {
    let dir = tempfile::tempdir().unwrap();
    let db_path = dir.path().join("memory.db");
    let conn = Connection::open(&db_path).unwrap();
    create_memory_schema(&conn);
    drop(conn);

    let assert = Command::cargo_bin("by-rs")
        .unwrap()
        .args(["memory", "promote", "--db"])
        .arg(&db_path)
        .args([
            "--user-id",
            "u1",
            "--from",
            "l2",
            "--to",
            "l3",
            "--new-entry-id",
            "fact-promoted",
            "--entry",
            r#"{:id "ep-promote" :kind :observation :content "blue deploy should become semantic memory" :user-id "u1" :session-id "s1" :role :assistant :tags ["deploy"]}"#,
        ])
        .assert()
        .success();

    let output = String::from_utf8(assert.get_output().stdout.clone()).unwrap();
    let value: serde_json::Value = serde_json::from_str(&output).unwrap();
    assert_eq!(value["projection"], "memory-agent/memory$promote");
    assert_eq!(value["source"], "local-sqlite");
    assert_eq!(value["live-skipped?"], true);
    assert_eq!(value["current-agent-skipped?"], true);
    assert_eq!(value["requested-from"], "l2");
    assert_eq!(value["requested-to"], "l3");
    assert_eq!(value["requested-layer"], "l3");
    assert_eq!(value["entry-id"], "fact-promoted");
    assert_eq!(value["from"], "l2");
    assert_eq!(value["to"], "l3");
    assert_eq!(value["entry"]["sources"][0]["type"], "promotion");
    assert_eq!(value["entry"]["sources"][0]["id"], "ep-promote");

    let read = Command::cargo_bin("by-rs")
        .unwrap()
        .args(["memory", "read", "--db"])
        .arg(&db_path)
        .args([
            "--user-id",
            "u1",
            "--layer",
            "l3",
            "--query",
            r#"{:id "fact-promoted"}"#,
        ])
        .assert()
        .success();
    let read_output = String::from_utf8(read.get_output().stdout.clone()).unwrap();
    let read_value: serde_json::Value = serde_json::from_str(&read_output).unwrap();
    assert_eq!(read_value["entries"][0]["id"], "fact-promoted");
}

#[test]
fn memory_sweep_l2_projects_local_ttl_tombstone_without_live_agent() {
    let dir = tempfile::tempdir().unwrap();
    let db_path = dir.path().join("memory.db");
    let conn = Connection::open(&db_path).unwrap();
    create_memory_schema(&conn);
    conn.execute(
        "INSERT INTO episodes
         (session_id, user_id, timestamp, episode_type, role, content, entry_id)
         VALUES ('s-old', 'u1', datetime('now', '-31 days'), 'conversation', 'assistant',
                 'old blue episode', 'ep-old')",
        [],
    )
    .unwrap();
    conn.execute(
        "INSERT INTO episodes
         (session_id, user_id, timestamp, episode_type, role, content, entry_id, keep_flag)
         VALUES ('s-keep', 'u1', datetime('now', '-31 days'), 'conversation', 'assistant',
                 'kept old episode', 'ep-keep', 1)",
        [],
    )
    .unwrap();
    drop(conn);

    let assert = Command::cargo_bin("by-rs")
        .unwrap()
        .args(["memory", "sweep-l2", "--db"])
        .arg(&db_path)
        .args(["--user-id", "u1", "--retention-days", "30"])
        .assert()
        .success();

    let output = String::from_utf8(assert.get_output().stdout.clone()).unwrap();
    let value: serde_json::Value = serde_json::from_str(&output).unwrap();
    assert_eq!(value["projection"], "memory-agent/memory$sweep-l2");
    assert_eq!(value["source"], "local-sqlite");
    assert_eq!(value["live-skipped?"], true);
    assert_eq!(value["current-agent-skipped?"], true);
    assert_eq!(value["requested-layer"], "l2");
    assert_eq!(value["user-id"], "u1");
    assert_eq!(value["tombstoned"], 1);
    assert_eq!(value["retention-days"], 30);

    let conn = Connection::open(&db_path).unwrap();
    let old_tombstoned: i64 = conn
        .query_row(
            "SELECT tombstoned_flag FROM episodes WHERE entry_id = 'ep-old'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    let keep_tombstoned: i64 = conn
        .query_row(
            "SELECT tombstoned_flag FROM episodes WHERE entry_id = 'ep-keep'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(old_tombstoned, 1);
    assert_eq!(keep_tombstoned, 0);
}

#[test]
fn memory_consolidate_projects_local_l2_to_l3_reduction_without_live_agent() {
    let dir = tempfile::tempdir().unwrap();
    let db_path = dir.path().join("memory.db");
    let conn = Connection::open(&db_path).unwrap();
    create_memory_schema(&conn);
    for (entry_id, timestamp, content) in [
        ("ep-c1", "2026-01-01 00:00:00", "blue deploy started"),
        ("ep-c2", "2026-01-01 00:01:00", "blue deploy checked health"),
        ("ep-c3", "2026-01-01 00:02:00", "blue deploy finished"),
    ] {
        conn.execute(
            "INSERT INTO episodes
             (session_id, user_id, timestamp, episode_type, role, content, tags, entry_id)
             VALUES (?1, 'u1', ?2, 'conversation', 'assistant', ?3,
                     '[\"deploy\",\"event:turn\",\"kind:message\"]', ?4)",
            ("s-consolidate", timestamp, content, entry_id),
        )
        .unwrap();
    }
    drop(conn);

    let assert = Command::cargo_bin("by-rs")
        .unwrap()
        .args(["memory", "consolidate", "--db"])
        .arg(&db_path)
        .args([
            "--user-id",
            "u1",
            "--session-id",
            "s-consolidate",
            "--min-batch",
            "3",
        ])
        .assert()
        .success();

    let output = String::from_utf8(assert.get_output().stdout.clone()).unwrap();
    let value: serde_json::Value = serde_json::from_str(&output).unwrap();
    assert_eq!(value["projection"], "memory-agent/memory$consolidate");
    assert_eq!(value["source"], "local-sqlite");
    assert_eq!(value["live-skipped?"], true);
    assert_eq!(value["current-agent-skipped?"], true);
    assert_eq!(value["requested-layer"], "l2");
    assert_eq!(value["user-id"], "u1");
    assert_eq!(value["session-id"], "s-consolidate");
    assert_eq!(value["report"]["produced"], 1);
    assert_eq!(value["report"]["consumed"], 3);
    assert_eq!(value["report"]["auto-kept"], 3);
    assert_eq!(
        value["report"]["batches"][0]["tags"],
        serde_json::json!(["deploy"])
    );

    let conn = Connection::open(&db_path).unwrap();
    let fact_count: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM semantic_facts WHERE user_id = 'u1' AND fact_type = 'summary'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    let kept_count: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM episodes WHERE user_id = 'u1' AND keep_flag = 1",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(fact_count, 1);
    assert_eq!(kept_count, 3);
}

#[test]
fn memory_state_read_projects_working_area_slot_without_live_agent() {
    let project = tempfile::tempdir().unwrap();
    let slot_path = project
        .path()
        .join(".brainyard/agents/memory-agent/u1/pending/verify-queue.edn");
    std::fs::create_dir_all(slot_path.parent().unwrap()).unwrap();
    std::fs::write(&slot_path, "{:items [1 2] :ok? true}").unwrap();

    let assert = Command::cargo_bin("by-rs")
        .unwrap()
        .args([
            "memory",
            "state-read",
            "--user-id",
            "u1",
            "--slot",
            "pending/verify-queue.edn",
            "--project-dir",
        ])
        .arg(project.path())
        .assert()
        .success();
    let state: serde_json::Value =
        serde_json::from_slice(&assert.get_output().stdout).expect("state read json");
    assert_eq!(state["projection"], "memory-agent/memory$state-read");
    assert_eq!(state["source"], "local-working-area");
    assert_eq!(state["live-skipped?"], true);
    assert_eq!(state["current-agent-skipped?"], true);
    assert_eq!(state["user-id"], "u1");
    assert_eq!(state["slot"], "pending/verify-queue.edn");
    assert_eq!(state["content"]["items"][0], 1);
    assert_eq!(state["content"]["items"][1], 2);
    assert_eq!(state["content"]["ok?"], true);
    assert!(state["path"]
        .as_str()
        .unwrap()
        .ends_with("pending/verify-queue.edn"));

    let assert = Command::cargo_bin("by-rs")
        .unwrap()
        .args([
            "memory",
            "state-read",
            "--user-id",
            "u1",
            "--slot",
            "../config.edn",
            "--project-dir",
        ])
        .arg(project.path())
        .assert()
        .success();
    let disallowed: serde_json::Value =
        serde_json::from_slice(&assert.get_output().stdout).expect("disallowed state read json");
    assert_eq!(disallowed["projection"], "memory-agent/memory$state-read");
    assert!(disallowed["error"]
        .as_str()
        .unwrap()
        .contains("slot not allowed"));
}

#[test]
fn memory_state_write_projects_working_area_slot_without_live_agent() {
    let project = tempfile::tempdir().unwrap();

    let assert = Command::cargo_bin("by-rs")
        .unwrap()
        .args([
            "memory",
            "state-write",
            "--user-id",
            "u1",
            "--slot",
            "stats.edn",
            "--content",
            "{:count 2 :ok? true :items [1 nil]}",
            "--project-dir",
        ])
        .arg(project.path())
        .assert()
        .success();
    let state: serde_json::Value =
        serde_json::from_slice(&assert.get_output().stdout).expect("state write json");
    assert_eq!(state["projection"], "memory-agent/memory$state-write");
    assert_eq!(state["source"], "local-working-area");
    assert_eq!(state["live-skipped?"], true);
    assert_eq!(state["current-agent-skipped?"], true);
    assert_eq!(state["user-id"], "u1");
    assert_eq!(state["slot"], "stats.edn");
    assert_eq!(state["written?"], true);
    assert!(state["path"].as_str().unwrap().ends_with("stats.edn"));

    let written = std::fs::read_to_string(
        project
            .path()
            .join(".brainyard/agents/memory-agent/u1/stats.edn"),
    )
    .unwrap();
    assert!(written.contains(":count 2"));
    assert!(written.contains(":ok? true"));
    assert!(written.contains("nil"));

    let assert = Command::cargo_bin("by-rs")
        .unwrap()
        .args([
            "memory",
            "state-read",
            "--user-id",
            "u1",
            "--slot",
            "stats.edn",
            "--project-dir",
        ])
        .arg(project.path())
        .assert()
        .success();
    let readback: serde_json::Value =
        serde_json::from_slice(&assert.get_output().stdout).expect("state readback json");
    assert_eq!(readback["content"]["count"], 2);
    assert_eq!(readback["content"]["ok?"], true);
    assert_eq!(readback["content"]["items"][0], 1);
    assert_eq!(readback["content"]["items"][1], serde_json::Value::Null);

    let assert = Command::cargo_bin("by-rs")
        .unwrap()
        .args([
            "memory",
            "state-write",
            "--user-id",
            "u1",
            "--slot",
            "../config.edn",
            "--content",
            "{:ok? true}",
            "--project-dir",
        ])
        .arg(project.path())
        .assert()
        .success();
    let disallowed: serde_json::Value =
        serde_json::from_slice(&assert.get_output().stdout).expect("disallowed state write json");
    assert_eq!(disallowed["projection"], "memory-agent/memory$state-write");
    assert!(disallowed["error"]
        .as_str()
        .unwrap()
        .contains("slot not allowed"));

    let assert = Command::cargo_bin("by-rs")
        .unwrap()
        .args([
            "memory",
            "state-write",
            "--user-id",
            "u1",
            "--slot",
            "pending/verify-queue.edn",
            "--content",
            "{:token \"AKIA1234567890ABCDEF\"}",
            "--project-dir",
        ])
        .arg(project.path())
        .assert()
        .success();
    let secret: serde_json::Value =
        serde_json::from_slice(&assert.get_output().stdout).expect("secret guard json");
    assert_eq!(secret["projection"], "memory-agent/memory$state-write");
    assert_eq!(secret["stage"], "secret-detected");
    assert_eq!(secret["source"], "local-working-area");
    assert!(!project
        .path()
        .join(".brainyard/agents/memory-agent/u1/pending/verify-queue.edn")
        .exists());
}

#[test]
fn memory_essence_append_projects_ndjson_audit_without_live_agent() {
    let project = tempfile::tempdir().unwrap();

    let assert = Command::cargo_bin("by-rs")
        .unwrap()
        .args([
            "memory",
            "essence-append",
            "--user-id",
            "u1",
            "--turn-id",
            "7",
            "--agent-id",
            "memory-agent",
            "--essences",
            "[{:kind :preference :text \"ship blue\"}]",
            "--project-dir",
        ])
        .arg(project.path())
        .assert()
        .success();
    let report: serde_json::Value =
        serde_json::from_slice(&assert.get_output().stdout).expect("essence append json");
    assert_eq!(report["projection"], "memory-agent/memory$essence-append");
    assert_eq!(report["source"], "local-working-area");
    assert_eq!(report["live-skipped?"], true);
    assert_eq!(report["current-agent-skipped?"], true);
    assert_eq!(report["user-id"], "u1");
    assert_eq!(report["appended?"], true);
    assert_eq!(report["record"]["turn-id"], 7);
    assert_eq!(report["record"]["agent-id"], "memory-agent");
    assert_eq!(report["record"]["essences"][0]["kind"], "preference");
    assert!(report["record"]["at"].as_i64().unwrap() > 0);

    Command::cargo_bin("by-rs")
        .unwrap()
        .args([
            "memory",
            "essence-append",
            "--user-id",
            "u1",
            "--turn-id",
            "8",
            "--agent-id",
            "memory-agent",
            "--project-dir",
        ])
        .arg(project.path())
        .assert()
        .success();

    let log_path = project
        .path()
        .join(".brainyard/agents/memory-agent/u1/essence.log");
    let lines = std::fs::read_to_string(&log_path).unwrap();
    let records = lines
        .lines()
        .map(|line| serde_json::from_str::<serde_json::Value>(line).unwrap())
        .collect::<Vec<_>>();
    assert_eq!(records.len(), 2);
    assert_eq!(records[0]["turn-id"], 7);
    assert_eq!(records[0]["essences"][0]["text"], "ship blue");
    assert_eq!(records[1]["turn-id"], 8);
    assert_eq!(records[1]["essences"].as_array().unwrap().len(), 0);

    let assert = Command::cargo_bin("by-rs")
        .unwrap()
        .args([
            "memory",
            "essence-append",
            "--user-id",
            "u1",
            "--turn-id",
            "9",
            "--agent-id",
            "memory-agent",
            "--essences",
            "{:not :a-vector}",
            "--project-dir",
        ])
        .arg(project.path())
        .assert()
        .success();
    let invalid: serde_json::Value =
        serde_json::from_slice(&assert.get_output().stdout).expect("invalid essence json");
    assert_eq!(invalid["projection"], "memory-agent/memory$essence-append");
    assert_eq!(invalid["appended?"], false);
    assert!(invalid["error"].as_str().unwrap().contains("vector"));
}

#[test]
fn memory_essence_extract_projects_live_sub_lm_skip_without_live_agent() {
    let assert = Command::cargo_bin("by-rs")
        .unwrap()
        .args([
            "memory",
            "essence-extract",
            "--user-id",
            "u1",
            "--turn-summary",
            "User prefers blue deploys.",
            "--turn-messages",
            "user: ship blue\nassistant: noted",
            "--recent-episodes",
            "ep1: blue deploy preference",
        ])
        .assert()
        .success();

    let report: serde_json::Value =
        serde_json::from_slice(&assert.get_output().stdout).expect("essence extract json");
    assert_eq!(report["projection"], "memory-agent/memory$essence-extract");
    assert_eq!(report["source"], "live-sub-lm-skipped");
    assert_eq!(report["live-skipped?"], true);
    assert_eq!(report["current-agent-skipped?"], true);
    assert_eq!(report["input-contract-only?"], true);
    assert_eq!(report["sub-lm-live-skipped?"], true);
    assert_eq!(report["user-id"], "u1");
    assert_eq!(report["essences"].as_array().unwrap().len(), 0);
    assert!(report["error"]
        .as_str()
        .unwrap()
        .contains("requires a live sub-LM"));
    assert!(report["turn-summary-bytes"].as_u64().unwrap() > 0);
    assert!(report["turn-messages-bytes"].as_u64().unwrap() > 0);
    assert!(report["recent-episodes-bytes"].as_u64().unwrap() > 0);
}

#[test]
fn memory_verify_fact_projects_live_sub_lm_skip_without_live_agent() {
    let assert = Command::cargo_bin("by-rs")
        .unwrap()
        .args([
            "memory",
            "verify-fact",
            "--fact",
            r#"{"id":"fact-blue","content":"User prefers blue deploys.","confidence":0.7,"tags":["deploy"]}"#,
            "--fresh-recall",
            "Recent recall agrees with the blue deploy preference.",
            "--evidence",
            "User explicitly said ship blue.",
        ])
        .assert()
        .success();

    let report: serde_json::Value =
        serde_json::from_slice(&assert.get_output().stdout).expect("verify fact json");
    assert_eq!(report["projection"], "memory-agent/memory$verify-fact");
    assert_eq!(report["source"], "live-sub-lm-skipped");
    assert_eq!(report["live-skipped?"], true);
    assert_eq!(report["current-agent-skipped?"], true);
    assert_eq!(report["input-contract-only?"], true);
    assert_eq!(report["sub-lm-live-skipped?"], true);
    assert!(report["verdict"].is_null());
    assert_eq!(report["refined-content"], "");
    assert!((report["new-confidence"].as_f64().unwrap() - 0.7).abs() < f64::EPSILON);
    assert_eq!(report["fact"]["id"], "fact-blue");
    assert!(report["fresh-recall-bytes"].as_u64().unwrap() > 0);
    assert!(report["evidence-bytes"].as_u64().unwrap() > 0);
    assert!(report["error"]
        .as_str()
        .unwrap()
        .contains("requires a live sub-LM"));
}

#[test]
fn memory_llm_consolidate_projects_live_sub_lm_skip_without_live_agent() {
    let assert = Command::cargo_bin("by-rs")
        .unwrap()
        .args([
            "memory",
            "llm-consolidate",
            "--user-id",
            "u1",
            "--episodes",
            r#"[{"id":"ep1","content":"blue deploy started","tags":["deploy"],"created-at":"2026-01-01"}]"#,
            "--window-desc",
            "session s1 first window",
            "--existing-l3-hits",
            "fact: previous deploy preference",
        ])
        .assert()
        .success();

    let report: serde_json::Value =
        serde_json::from_slice(&assert.get_output().stdout).expect("llm consolidate json");
    assert_eq!(report["projection"], "memory-agent/memory$llm-consolidate");
    assert_eq!(report["source"], "live-sub-lm-skipped");
    assert_eq!(report["live-skipped?"], true);
    assert_eq!(report["current-agent-skipped?"], true);
    assert_eq!(report["input-contract-only?"], true);
    assert_eq!(report["sub-lm-live-skipped?"], true);
    assert_eq!(report["user-id"], "u1");
    assert_eq!(report["facts"].as_array().unwrap().len(), 0);
    assert_eq!(report["episode-count"], 1);
    assert_eq!(report["episodes"][0]["id"], "ep1");
    assert_eq!(report["window-desc"], "session s1 first window");
    assert!(report["existing-l3-hits-bytes"].as_u64().unwrap() > 0);
    assert!(report["error"]
        .as_str()
        .unwrap()
        .contains("requires a live sub-LM"));
}

#[test]
fn memory_essence_extract_bedrock_dry_run_prepares_sub_lm_request_without_network() {
    let assert = Command::cargo_bin("by-rs")
        .unwrap()
        .args([
            "memory",
            "essence-extract",
            "--dry-run",
            "--model",
            "global.anthropic.claude-haiku-4-5-20251001-v1:0",
            "--region",
            "ap-northeast-2",
            "--aws-profile",
            "grumatic",
            "--max-tokens",
            "64",
            "--user-id",
            "u1",
            "--turn-summary",
            "User prefers blue deploys.",
            "--turn-messages",
            "user: deploy color should be blue",
            "--recent-episodes",
            "ep1 user-context blue deploy preference",
        ])
        .assert()
        .success();
    let report: serde_json::Value = serde_json::from_slice(&assert.get_output().stdout).unwrap();
    assert_eq!(report["projection"], "memory-agent/memory$essence-extract");
    assert_eq!(report["source"], "bedrock-dry-run");
    assert_eq!(report["network"], false);
    assert_eq!(report["live-skipped?"], false);
    assert_eq!(report["input"]["user-id"], "u1");
    assert_eq!(
        report["request"]["modelId"],
        "global.anthropic.claude-haiku-4-5-20251001-v1:0"
    );
    assert_eq!(report["request"]["inferenceConfig"]["maxTokens"], 64);
    assert_eq!(report["request"]["messages"].as_array().unwrap().len(), 1);
    assert!(!report["request"]["system"].as_array().unwrap().is_empty());
}

#[test]
fn memory_verify_fact_bedrock_dry_run_prepares_sub_lm_request_without_network() {
    let assert = Command::cargo_bin("by-rs")
        .unwrap()
        .args([
            "memory",
            "verify-fact",
            "--dry-run",
            "--model",
            "global.anthropic.claude-haiku-4-5-20251001-v1:0",
            "--region",
            "ap-northeast-2",
            "--aws-profile",
            "grumatic",
            "--max-tokens",
            "80",
            "--fact",
            r#"{"id":"fact-blue","content":"User likes blue deploys","confidence":0.7}"#,
            "--fresh-recall",
            "User confirmed blue deploys today.",
            "--evidence",
            "direct user message",
        ])
        .assert()
        .success();
    let report: serde_json::Value = serde_json::from_slice(&assert.get_output().stdout).unwrap();
    assert_eq!(report["projection"], "memory-agent/memory$verify-fact");
    assert_eq!(report["source"], "bedrock-dry-run");
    assert_eq!(report["network"], false);
    assert_eq!(report["input"]["fact"]["id"], "fact-blue");
    assert_eq!(report["request"]["inferenceConfig"]["maxTokens"], 80);
    assert_eq!(report["request"]["messages"].as_array().unwrap().len(), 1);
    assert!(!report["request"]["system"].as_array().unwrap().is_empty());
}

#[test]
fn memory_llm_consolidate_bedrock_dry_run_prepares_sub_lm_request_without_network() {
    let assert = Command::cargo_bin("by-rs")
        .unwrap()
        .args([
            "memory",
            "llm-consolidate",
            "--dry-run",
            "--model",
            "global.anthropic.claude-haiku-4-5-20251001-v1:0",
            "--region",
            "ap-northeast-2",
            "--aws-profile",
            "grumatic",
            "--max-tokens",
            "96",
            "--episodes",
            r#"[{"id":"ep1","content":"User asked to keep deploys blue","tags":["deploy"],"created-at":1}]"#,
            "--window-desc",
            "session s1 first window",
            "--existing-l3-hits",
            "fact-blue old blue preference",
            "--user-id",
            "u1",
        ])
        .assert()
        .success();
    let report: serde_json::Value = serde_json::from_slice(&assert.get_output().stdout).unwrap();
    assert_eq!(report["projection"], "memory-agent/memory$llm-consolidate");
    assert_eq!(report["source"], "bedrock-dry-run");
    assert_eq!(report["network"], false);
    assert_eq!(report["input"]["user-id"], "u1");
    assert_eq!(report["input"]["episodes"].as_array().unwrap().len(), 1);
    assert_eq!(report["request"]["inferenceConfig"]["maxTokens"], 96);
    assert_eq!(report["request"]["messages"].as_array().unwrap().len(), 1);
    assert!(!report["request"]["system"].as_array().unwrap().is_empty());
}

#[test]
fn memory_purge_plan_projects_candidate_report_without_live_registry() {
    let dir = tempfile::tempdir().unwrap();
    let db_path = dir.path().join("memory.db");
    let sessions_root = dir.path().join("sessions");
    std::fs::create_dir_all(sessions_root.join("s-live")).unwrap();
    let conn = Connection::open(&db_path).unwrap();
    create_memory_schema(&conn);
    conn.execute(
        "INSERT INTO episodes
         (session_id, user_id, episode_type, role, content, entry_id)
         VALUES ('s-orphan', 'u1', 'conversation', 'assistant',
                 'orphan blue episode content',
                 'ep-orphan')",
        [],
    )
    .unwrap();
    conn.execute(
        "INSERT INTO episodes
         (session_id, user_id, episode_type, role, content, entry_id)
         VALUES ('s-live', 'u1', 'conversation', 'assistant',
                 'live blue episode content',
                 'ep-live')",
        [],
    )
    .unwrap();
    conn.execute(
        "INSERT INTO semantic_facts
         (user_id, fact_type, content, confidence, last_accessed, entry_id)
         VALUES ('u1', 'preference', 'stale low confidence fact', 0.4,
                 '2000-01-01 00:00:00', 'fact-stale')",
        [],
    )
    .unwrap();
    conn.execute(
        "INSERT INTO semantic_facts
         (user_id, fact_type, content, confidence, last_accessed, entry_id)
         VALUES ('u1', 'preference', 'old but high confidence fact', 0.9,
                 '2000-01-01 00:00:00', 'fact-strong')",
        [],
    )
    .unwrap();
    drop(conn);

    let assert = Command::cargo_bin("by-rs")
        .unwrap()
        .args(["memory", "purge-plan", "--db"])
        .arg(&db_path)
        .args(["--user-id", "u1", "--sessions-root"])
        .arg(&sessions_root)
        .args(["--cap", "10", "--stale-days", "60"])
        .assert()
        .success();
    let purge: serde_json::Value =
        serde_json::from_slice(&assert.get_output().stdout).expect("purge plan json");
    assert_eq!(purge["projection"], "memory-agent/memory$purge-plan");
    assert_eq!(purge["source"], "local-sqlite");
    assert_eq!(purge["live-skipped?"], true);
    assert_eq!(purge["current-agent-skipped?"], true);
    assert_eq!(purge["user-id"], "u1");
    assert_eq!(purge["cap"], 10);
    assert_eq!(purge["stale-days"], 60);
    assert_eq!(purge["l2-orphan-sessions"], serde_json::json!(["s-orphan"]));
    assert_eq!(purge["counts"]["l2-orphan-episodes"], 1);
    assert_eq!(purge["l2-orphan-episodes"][0]["entry-id"], "ep-orphan");
    assert_eq!(purge["counts"]["l3-stale"], 1);
    assert_eq!(purge["l3-stale-facts"][0]["entry-id"], "fact-stale");
    assert_eq!(purge["registry-live-skipped?"], true);
}

#[test]
fn memory_explain_reports_hydrated_audit_json_without_writing() {
    let dir = tempfile::tempdir().unwrap();
    let db_path = dir.path().join("memory.db");
    let conn = Connection::open(&db_path).unwrap();
    create_memory_schema(&conn);
    seed_memory_explain_rows(&conn);
    drop(conn);

    Command::cargo_bin("by-rs")
        .unwrap()
        .args(["memory", "explain", "--db"])
        .arg(&db_path)
        .args([
            "--session-id",
            "s1",
            "--agent-id",
            "coact-agent",
            "--turn-id",
            "3",
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("\"session-id\": \"s1\""))
        .stdout(predicate::str::contains("\"prompt-bytes\": 51"))
        .stdout(predicate::str::contains("\"entry_id\": \"ep-explain\""))
        .stdout(predicate::str::contains(
            "\"content\": \"explain blue episode\"",
        ))
        .stdout(predicate::str::contains("\"archived\": true"))
        .stdout(predicate::str::contains(
            "\"content\": \"explain green fact\"",
        ))
        .stdout(predicate::str::contains("\"tombstoned\": true"));
}

#[test]
fn memory_explain_session_reports_grouped_turns() {
    let dir = tempfile::tempdir().unwrap();
    let db_path = dir.path().join("memory.db");
    let conn = Connection::open(&db_path).unwrap();
    create_memory_schema(&conn);
    seed_memory_explain_rows(&conn);
    conn.execute(
        "INSERT INTO memory_audit
         (user_id, session_id, agent_id, turn_id, total_turns, entry_id, layer, byte_cost)
         VALUES ('u1', 's1', 'sub-agent', 1, 1, 'ep-explain', 'l2', 11)",
        [],
    )
    .unwrap();
    drop(conn);

    Command::cargo_bin("by-rs")
        .unwrap()
        .args(["memory", "explain-session", "--db"])
        .arg(&db_path)
        .args(["--session-id", "s1"])
        .assert()
        .success()
        .stdout(predicate::str::contains("\"session-id\": \"s1\""))
        .stdout(predicate::str::contains("\"agent-id\": \"sub-agent\""))
        .stdout(predicate::str::contains("\"turn-id\": 1"))
        .stdout(predicate::str::contains("\"agent-id\": \"coact-agent\""))
        .stdout(predicate::str::contains("\"prompt-bytes\": 51"));
}

#[test]
fn memory_explain_reports_common_projection_metadata_and_limits_entries() {
    let dir = tempfile::tempdir().unwrap();
    let db_path = dir.path().join("memory.db");
    let conn = Connection::open(&db_path).unwrap();
    create_memory_schema(&conn);
    seed_memory_explain_rows(&conn);
    drop(conn);

    let turn_assert = Command::cargo_bin("by-rs")
        .unwrap()
        .args(["memory", "explain", "--db"])
        .arg(&db_path)
        .args([
            "--user-id",
            "u1",
            "--session-id",
            "s1",
            "--agent-id",
            "coact-agent",
            "--turn-id",
            "3",
            "--limit",
            "1",
        ])
        .assert()
        .success();
    let turn_stdout = String::from_utf8(turn_assert.get_output().stdout.clone()).unwrap();
    let turn: serde_json::Value = serde_json::from_str(&turn_stdout).unwrap();
    assert_eq!(turn["projection"], "common.commands/memory$explain");
    assert_eq!(turn["source"], "local-sqlite");
    assert_eq!(turn["live-skipped?"], true);
    assert_eq!(turn["current-agent-skipped?"], true);
    assert_eq!(turn["all-turns"], false);
    assert_eq!(turn["limit"], 1);
    assert_eq!(turn["count"], 2);
    assert_eq!(turn["user-id"], "u1");
    assert_eq!(turn["entries"].as_array().unwrap().len(), 1);

    let session_assert = Command::cargo_bin("by-rs")
        .unwrap()
        .args(["memory", "explain-session", "--db"])
        .arg(&db_path)
        .args(["--user-id", "u1", "--session-id", "s1", "--limit", "1"])
        .assert()
        .success();
    let session_stdout = String::from_utf8(session_assert.get_output().stdout.clone()).unwrap();
    let session: serde_json::Value = serde_json::from_str(&session_stdout).unwrap();
    assert_eq!(session["projection"], "common.commands/memory$explain");
    assert_eq!(session["all-turns"], true);
    assert_eq!(session["limit"], 1);
    assert_eq!(session["count"], 1);
    assert_eq!(session["turns"][0]["count"], 2);
    assert_eq!(session["turns"][0]["entries"].as_array().unwrap().len(), 1);
}

#[test]
fn logs_query_pretty_mulog_file_without_writing() {
    let dir = tempfile::tempdir().unwrap();
    let log_path = dir.path().join("agent-tui-app.log");
    std::fs::write(
        &log_path,
        r#"{:mulog/event-name :ai.brainyard.agent/coact-init
 :mulog/timestamp 1000
 :mulog/trace-id #mulog/flake "trace-1"
 :user-id "u1"
 :session-id "s1"
 :agent-id "coact-agent"
 :turn-id 1
 :total-turns 1
 :previous-turns-count 0
 :system-context-chars 10
 :user-context-chars 20
 :budget-tokens 100}

{:mulog/event-name :ai.brainyard.agent/store-results
 :mulog/timestamp 1500
 :user-id "u1"
 :session-id "s1"
 :agent-id "coact-agent"
 :turn-id 1
 :total-iterations 2
 :terminated-by :done
 :answer-length 42}

{:mulog/event-name :ai.brainyard.agent/agent-conversation
 :mulog/timestamp 1600
 :user-id "u1"
 :session-id "s1"
 :agent-id "research-agent"
 :turn-id 2
 :request "Find migration gaps"
 :reply "Answer text"}"#,
    )
    .unwrap();

    let assert = Command::cargo_bin("by-rs")
        .unwrap()
        .args(["logs", "turns", "--log"])
        .arg(&log_path)
        .args(["--session-id", "s1"])
        .assert()
        .success();
    let stdout = String::from_utf8(assert.get_output().stdout.clone()).unwrap();
    let turns: serde_json::Value = serde_json::from_str(&stdout).unwrap();
    assert_eq!(turns["turns"][0]["turn-id"], 1);
    assert_eq!(turns["turns"][0]["event-count"], 2);
    assert_eq!(turns["turns"][0]["first-ts"], 1000);
    assert_eq!(turns["turns"][0]["last-ts"], 1500);
    assert_eq!(turns["turns"][0]["categories"][0], "turn-start");
    assert_eq!(turns["turns"][0]["categories"][1], "turn-complete");
    assert_eq!(turns["turns"][1]["turn-id"], 2);

    let assert = Command::cargo_bin("by-rs")
        .unwrap()
        .args(["logs", "events", "--log"])
        .arg(&log_path)
        .args([
            "--session-id",
            "s1",
            "--turn-id",
            "1",
            "--type",
            "turn-complete",
        ])
        .assert()
        .success();
    let stdout = String::from_utf8(assert.get_output().stdout.clone()).unwrap();
    let events: serde_json::Value = serde_json::from_str(&stdout).unwrap();
    assert_eq!(events["events"].as_array().unwrap().len(), 1);
    assert_eq!(events["events"][0]["category"], "turn-complete");
    assert_eq!(events["events"][0]["terminated-by"], "done");
    assert_eq!(events["events"][0]["answer-length"], 42);

    let assert = Command::cargo_bin("by-rs")
        .unwrap()
        .args(["logs", "search", "--log"])
        .arg(&log_path)
        .args(["--session-id", "s1", "--query", "migration"])
        .assert()
        .success();
    let stdout = String::from_utf8(assert.get_output().stdout.clone()).unwrap();
    let search: serde_json::Value = serde_json::from_str(&stdout).unwrap();
    assert_eq!(search["events"].as_array().unwrap().len(), 1);
    assert_eq!(search["events"][0]["category"], "conversation");
    assert_eq!(search["events"][0]["request"], "Find migration gaps");
}

#[test]
fn logs_missing_file_is_empty_and_read_only() {
    let dir = tempfile::tempdir().unwrap();
    let missing = dir.path().join("missing.log");

    let assert = Command::cargo_bin("by-rs")
        .unwrap()
        .args(["logs", "turns", "--log"])
        .arg(&missing)
        .assert()
        .success();
    let stdout = String::from_utf8(assert.get_output().stdout.clone()).unwrap();
    let value: serde_json::Value = serde_json::from_str(&stdout).unwrap();

    assert!(value["turns"].as_array().unwrap().is_empty());
    assert!(!missing.exists());
}

#[test]
fn tasks_list_and_detail_read_persisted_artifacts_without_manager() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("tasks");
    let task_dir = root.join("task-1");
    std::fs::create_dir_all(&task_dir).unwrap();
    std::fs::write(
        task_dir.join("meta.edn"),
        r#"{:id :task-1
 :name "build fixture"
 :job-type :bash
 :status :completed
 :created-at 100
 :started-at 120
 :completed-at 150
 :result {:exit-code 0}}"#,
    )
    .unwrap();
    std::fs::write(task_dir.join("output.log"), "first\nsecond\nthird\n").unwrap();

    let assert = Command::cargo_bin("by-rs")
        .unwrap()
        .args(["tasks", "list", "--root"])
        .arg(&root)
        .args(["--status", "completed"])
        .assert()
        .success();
    let stdout = String::from_utf8(assert.get_output().stdout.clone()).unwrap();
    let list: serde_json::Value = serde_json::from_str(&stdout).unwrap();
    assert_eq!(list["total"], 1);
    assert_eq!(list["tasks"][0]["id"], "task-1");
    assert_eq!(list["tasks"][0]["name"], "build fixture");
    assert_eq!(list["tasks"][0]["status"], "completed");
    assert_eq!(list["tasks"][0]["job-type"], "bash");
    assert_eq!(list["tasks"][0]["output-lines"], 3);

    let assert = Command::cargo_bin("by-rs")
        .unwrap()
        .args(["tasks", "detail", "--root"])
        .arg(&root)
        .args(["task-1", "--last-n", "2"])
        .assert()
        .success();
    let stdout = String::from_utf8(assert.get_output().stdout.clone()).unwrap();
    let detail: serde_json::Value = serde_json::from_str(&stdout).unwrap();
    assert_eq!(detail["id"], "task-1");
    assert_eq!(detail["result"]["exit-code"], 0);
    assert_eq!(detail["total-lines"], 3);
    assert_eq!(detail["lines"][0], "second");
    assert_eq!(detail["lines"][1], "third");
    assert!(detail["output-file"]
        .as_str()
        .unwrap()
        .ends_with("output.log"));
}

#[test]
fn tasks_missing_root_is_empty_and_read_only() {
    let dir = tempfile::tempdir().unwrap();
    let missing = dir.path().join("missing-tasks");

    let assert = Command::cargo_bin("by-rs")
        .unwrap()
        .args(["tasks", "list", "--root"])
        .arg(&missing)
        .assert()
        .success();
    let stdout = String::from_utf8(assert.get_output().stdout.clone()).unwrap();
    let value: serde_json::Value = serde_json::from_str(&stdout).unwrap();

    assert_eq!(value["total"], 0);
    assert!(value["tasks"].as_array().unwrap().is_empty());
    assert!(!missing.exists());
}

#[test]
fn tasks_sweep_reports_retention_candidates_without_deleting() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("tasks");
    for (id, status, output) in [
        ("task-1", ":completed", "old\n"),
        ("task-2", ":completed", "new\n"),
        ("task-3", ":running", "active\n"),
    ] {
        let task_dir = root.join(id);
        std::fs::create_dir_all(&task_dir).unwrap();
        std::fs::write(
            task_dir.join("meta.edn"),
            format!(r#"{{:id :{id} :name "{id}" :status {status}}}"#),
        )
        .unwrap();
        std::fs::write(task_dir.join("output.log"), output).unwrap();
        std::thread::sleep(std::time::Duration::from_millis(20));
    }

    let assert = Command::cargo_bin("by-rs")
        .unwrap()
        .args(["tasks", "sweep", "--root"])
        .arg(&root)
        .args([
            "--retention-count",
            "1",
            "--retention-days",
            "0",
            "--dry-run",
        ])
        .assert()
        .success();
    let stdout = String::from_utf8(assert.get_output().stdout.clone()).unwrap();
    let value: serde_json::Value = serde_json::from_str(&stdout).unwrap();

    assert_eq!(value["dry-run?"], true);
    assert_eq!(value["total-deleted"], 1);
    assert_eq!(value["total-bytes-freed"], 0);
    assert_eq!(value["results"][0]["scanned"], 3);
    assert_eq!(value["results"][0]["deleted"], 1);
    assert_eq!(value["results"][0]["kept"], 2);
    assert_eq!(value["results"][0]["bytes-freed"], 0);
    assert_eq!(value["results"][0]["candidates"][0]["id"], "task-1");
    assert!(value["results"][0]["candidate-bytes"].as_u64().unwrap() > 0);
    assert!(root.join("task-1").exists());
    assert!(root.join("task-2").exists());
    assert!(root.join("task-3").exists());
}

#[test]
fn task_live_manager_commands_project_persisted_and_preflight_state_without_manager() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("tasks");
    let running_dir = root.join("task-running");
    std::fs::create_dir_all(&running_dir).unwrap();
    std::fs::write(
        running_dir.join("meta.edn"),
        r#"{:id :task-running
 :name "running bash"
 :job-type :bash
 :status :running
 :created-at 100
 :started-at 110}"#,
    )
    .unwrap();
    std::fs::write(running_dir.join("output.log"), "\u{1b}[31mred\u{1b}[0m\n").unwrap();

    let completed_dir = root.join("task-done");
    std::fs::create_dir_all(&completed_dir).unwrap();
    std::fs::write(
        completed_dir.join("meta.edn"),
        r#"{:id :task-done
 :name "done bash"
 :job-type :bash
 :status :completed
 :created-at 100
 :started-at 110
 :completed-at 120
 :result {:exit-code 0}}"#,
    )
    .unwrap();
    std::fs::write(completed_dir.join("output.log"), "done\n").unwrap();

    let assert = Command::cargo_bin("by-rs")
        .unwrap()
        .args([
            "tasks",
            "run",
            "--job-type",
            "bash",
            "--command",
            "echo hi",
            "--sync",
            "--timeout",
            "50",
            "--on-timeout",
            "kill",
        ])
        .assert()
        .success();
    let stdout = String::from_utf8(assert.get_output().stdout.clone()).unwrap();
    let run: serde_json::Value = serde_json::from_str(&stdout).unwrap();
    assert_eq!(run["projection"], "task.commands/task$run");
    assert_eq!(run["source"], "live-free-preflight");
    assert_eq!(run["live-skipped?"], true);
    assert_eq!(run["current-agent-skipped?"], true);
    assert_eq!(run["valid?"], true);
    assert_eq!(run["error"], "Task manager not initialized");
    assert_eq!(run["would-create"]["job-type"], "bash");
    assert_eq!(run["would-create"]["name"], "bash: echo hi");
    assert_eq!(run["timeout-ms"], 50);
    assert_eq!(run["on-timeout"], "kill");

    let assert = Command::cargo_bin("by-rs")
        .unwrap()
        .args(["tasks", "wait", "--root"])
        .arg(&root)
        .args(["task-running", "--timeout-ms", "25"])
        .assert()
        .success();
    let stdout = String::from_utf8(assert.get_output().stdout.clone()).unwrap();
    let running_wait: serde_json::Value = serde_json::from_str(&stdout).unwrap();
    assert_eq!(running_wait["projection"], "task.commands/task$wait");
    assert_eq!(running_wait["source"], "local-task-artifacts");
    assert_eq!(running_wait["status"], "still-running");
    assert_eq!(running_wait["task-id"], "task-running");
    assert_eq!(running_wait["timeout-ms"], 25);
    assert_eq!(running_wait["output"], "red\n");
    assert!(running_wait["message"]
        .as_str()
        .unwrap()
        .contains("without blocking"));

    let assert = Command::cargo_bin("by-rs")
        .unwrap()
        .args(["tasks", "wait", "--root"])
        .arg(&root)
        .args(["task-done", "--timeout-ms", "25"])
        .assert()
        .success();
    let stdout = String::from_utf8(assert.get_output().stdout.clone()).unwrap();
    let completed_wait: serde_json::Value = serde_json::from_str(&stdout).unwrap();
    assert_eq!(completed_wait["status"], "completed");
    assert_eq!(completed_wait["result"]["exit-code"], 0);
    assert_eq!(completed_wait["output"], "done\n");

    let assert = Command::cargo_bin("by-rs")
        .unwrap()
        .args(["tasks", "cancel", "--root"])
        .arg(&root)
        .arg("task-running")
        .assert()
        .success();
    let stdout = String::from_utf8(assert.get_output().stdout.clone()).unwrap();
    let cancel: serde_json::Value = serde_json::from_str(&stdout).unwrap();
    assert_eq!(cancel["projection"], "task.commands/task$cancel");
    assert_eq!(cancel["persisted-status"], "running");
    assert_eq!(cancel["error"], "Task manager not initialized");

    let assert = Command::cargo_bin("by-rs")
        .unwrap()
        .args(["tasks", "wakeup", "--root"])
        .arg(&root)
        .args(["--task-id", "task-done", "--note", "resume later"])
        .assert()
        .success();
    let stdout = String::from_utf8(assert.get_output().stdout.clone()).unwrap();
    let wakeup: serde_json::Value = serde_json::from_str(&stdout).unwrap();
    assert_eq!(wakeup["projection"], "task.commands/task$wakeup");
    assert_eq!(wakeup["persisted-status"], "completed");
    assert_eq!(wakeup["parked"], false);
    assert_eq!(wakeup["error"], "task$wakeup requires an active agent");
}

#[test]
fn dossiers_slug_read_frontmatter_and_handoff_are_live_free() {
    let dir = tempfile::tempdir().unwrap();
    let dossier_path = dir.path().join("plan.md");
    std::fs::write(
        &dossier_path,
        r#"---
slug: sample-plan
agent: plan-agent
created: 2026-01-01T00:00:00Z
plan_path: docs/plan.md
plan_status: completed

pre:
  verdict: go
  checks: [c1, "c two"]

post:
  verdict: pass

handoff:
  next_agent: todo-agent
  next_call: "(todo-agent {})"
---
body
"#,
    )
    .unwrap();

    let assert = Command::cargo_bin("by-rs")
        .unwrap()
        .args(["dossiers", "slug", "--agent", "todo", "--question"])
        .arg("Spawn a todo for the Rust native port")
        .args(["--max-chars", "80"])
        .assert()
        .success();
    let stdout = String::from_utf8(assert.get_output().stdout.clone()).unwrap();
    let slug: serde_json::Value = serde_json::from_str(&stdout).unwrap();
    assert_eq!(slug["agent"], "todo-agent");
    assert_eq!(slug["slug"], "rust-native-port");

    let assert = Command::cargo_bin("by-rs")
        .unwrap()
        .args(["dossiers", "read", "--path"])
        .arg(&dossier_path)
        .assert()
        .success();
    let stdout = String::from_utf8(assert.get_output().stdout.clone()).unwrap();
    let read: serde_json::Value = serde_json::from_str(&stdout).unwrap();
    assert_eq!(read["dossier"]["agent"], "plan-agent");
    assert_eq!(read["dossier"]["plan_path"], "docs/plan.md");
    assert_eq!(read["dossier"]["pre"]["verdict"], "go");
    assert_eq!(read["dossier"]["pre"]["checks"][1], "c two");
    assert_eq!(read["dossier"]["post"]["verdict"], "pass");

    for (command, agent_id, slug_projection, read_projection) in [
        (
            "plan",
            "plan-agent",
            "common.plan/plan$dossier-slug",
            "common.plan/plan$read-dossier",
        ),
        (
            "todo",
            "todo-agent",
            "common.todo/todo$dossier-slug",
            "common.todo/todo$read-dossier",
        ),
        (
            "exec",
            "exec-agent",
            "common.exec/exec$dossier-slug",
            "common.exec/exec$read-dossier",
        ),
        (
            "eval",
            "eval-agent",
            "common.eval/eval$dossier-slug",
            "common.eval/eval$read-dossier",
        ),
    ] {
        let slug_command = format!("{command}$dossier-slug");
        let assert = Command::cargo_bin("by-rs")
            .unwrap()
            .arg("dossiers")
            .arg(&slug_command)
            .arg("--question")
            .arg(format!("Spawn {command} work for the Rust native port"))
            .args(["--max-chars", "80"])
            .assert()
            .success()
            .stderr(predicate::str::is_empty());
        let stdout = String::from_utf8(assert.get_output().stdout.clone()).unwrap();
        let slug: serde_json::Value = serde_json::from_str(&stdout).unwrap();
        assert_eq!(slug["agent"], agent_id);
        assert_eq!(slug["projection"], slug_projection);

        let alias_path = dir.path().join(format!("{command}.md"));
        std::fs::write(
            &alias_path,
            format!(
                "---\nslug: {command}-slug\nagent: {agent_id}\ncreated: 2026-01-01T00:00:00Z\n---\nbody\n"
            ),
        )
        .unwrap();
        let read_command = format!("{command}$read-dossier");
        let assert = Command::cargo_bin("by-rs")
            .unwrap()
            .arg("dossiers")
            .arg(&read_command)
            .arg("--path")
            .arg(&alias_path)
            .assert()
            .success()
            .stderr(predicate::str::is_empty());
        let stdout = String::from_utf8(assert.get_output().stdout.clone()).unwrap();
        let read: serde_json::Value = serde_json::from_str(&stdout).unwrap();
        assert_eq!(read["projection"], read_projection);
        assert_eq!(read["dossier"]["agent"], agent_id);
        assert_eq!(read["dossier"]["slug"], format!("{command}-slug"));
    }

    let assert = Command::cargo_bin("by-rs")
        .unwrap()
        .args([
            "dossiers",
            "frontmatter",
            "--agent",
            "eval",
            "--slug",
            "eval-rust",
            "--created",
            "2026-01-01T00:00:00Z",
            "--verdict-path",
            ".brainyard/agents/eval-agent/verdicts/v.md",
            "--score",
            "{:verdict :achieved :confidence :high :criteria [{:class :c1 :status :pass}]}",
            "--handoff",
            "{:next_agent \"user\" :next_call \"done\"}",
        ])
        .assert()
        .success();
    let stdout = String::from_utf8(assert.get_output().stdout.clone()).unwrap();
    let frontmatter: serde_json::Value = serde_json::from_str(&stdout).unwrap();
    assert_eq!(
        frontmatter["projection"],
        "common.eval/eval$dossier-frontmatter"
    );
    let frontmatter = frontmatter["frontmatter"].as_str().unwrap();
    assert!(frontmatter.contains("agent: eval-agent"));
    assert!(frontmatter.contains("verdict_path: .brainyard/agents/eval-agent/verdicts/v.md"));
    assert!(frontmatter.contains("score:"));
    assert!(frontmatter.contains("verdict: achieved"));

    let plan_dossiers_dir = dir.path().join(".brainyard/agents/plan-agent/dossiers");
    std::fs::create_dir_all(&plan_dossiers_dir).unwrap();
    std::fs::write(
        plan_dossiers_dir.join("20260101-000000-rust-port.md"),
        "---\nslug: rust-port\n---\nbody\n",
    )
    .unwrap();
    let content = "---\nslug: rust-port\nagent: plan-agent\n---\n\n# Body\n";
    let assert = Command::cargo_bin("by-rs")
        .unwrap()
        .args([
            "dossiers",
            "write-preview",
            "--agent",
            "plan",
            "--slug",
            "rust-port",
            "--content",
            content,
            "--base-dir",
        ])
        .arg(dir.path())
        .args(["--ts", "20260102-030405"])
        .assert()
        .success();
    let stdout = String::from_utf8(assert.get_output().stdout.clone()).unwrap();
    let write_preview: serde_json::Value = serde_json::from_str(&stdout).unwrap();
    assert_eq!(
        write_preview["projection"],
        "common.plan/plan$dossier-write"
    );
    assert_eq!(write_preview["write-skipped?"], true);
    assert_eq!(write_preview["slug"], "rust-port-2");
    assert_eq!(write_preview["requested-slug"], "rust-port");
    assert_eq!(write_preview["collision?"], true);
    assert_eq!(
        write_preview["rel-path"],
        ".brainyard/agents/plan-agent/dossiers/20260102-030405-rust-port-2.md"
    );
    assert_eq!(write_preview["content"], content);
    assert!(!dir
        .path()
        .join(".brainyard/agents/plan-agent/dossiers/20260102-030405-rust-port-2.md")
        .exists());

    let assert = Command::cargo_bin("by-rs")
        .unwrap()
        .args([
            "dossiers",
            "index-append-preview",
            "--agent",
            "plan",
            "--path",
            ".brainyard/agents/plan-agent/dossiers/20260102-030405-rust-port-2.md",
            "--slug",
            "rust-port-2",
            "--pre-verdict",
            "revise",
            "--post-verdict",
            "revise",
            "--next-agent",
            "todo-agent",
            "--base-dir",
        ])
        .arg(dir.path())
        .args(["--created", "2026-01-02 03:04"])
        .assert()
        .success();
    let stdout = String::from_utf8(assert.get_output().stdout.clone()).unwrap();
    let index_preview: serde_json::Value = serde_json::from_str(&stdout).unwrap();
    assert_eq!(
        index_preview["projection"],
        "common.plan/plan$dossier-index-append"
    );
    assert_eq!(index_preview["write-skipped?"], true);
    assert_eq!(index_preview["appended"], true);
    assert_eq!(index_preview["prepend?"], true);
    assert_eq!(index_preview["existing?"], false);
    assert_eq!(index_preview["pre-verdict"], "n-a");
    assert_eq!(index_preview["post-verdict"], "n-a");
    assert_eq!(index_preview["next-agent"], "todo-agent");
    assert_eq!(
        index_preview["line"],
        "- 2026-01-02 03:04 [rust-port-2](dossiers/20260102-030405-rust-port-2.md) — pre:n-a · post:n-a · → todo-agent\n"
    );

    let assert = Command::cargo_bin("by-rs")
        .unwrap()
        .args([
            "dossiers",
            "index-append-preview",
            "--agent",
            "exec",
            "--path",
            ".brainyard/agents/exec-agent/dossiers/20260102-030405-run.md",
            "--slug",
            "run",
            "--pre-verdict",
            "go",
            "--post-verdict",
            "pass",
            "--next-agent",
            "eval-agent",
            "--advanced",
            "2",
            "--pending",
            "1",
            "--base-dir",
        ])
        .arg(dir.path())
        .args(["--created", "2026-01-02 03:05"])
        .assert()
        .success();
    let stdout = String::from_utf8(assert.get_output().stdout.clone()).unwrap();
    let exec_index: serde_json::Value = serde_json::from_str(&stdout).unwrap();
    assert_eq!(
        exec_index["line"],
        "- 2026-01-02 03:05 [run](dossiers/20260102-030405-run.md) — pre:go · post:pass · [+2 / -1] · → eval-agent\n"
    );

    let assert = Command::cargo_bin("by-rs")
        .unwrap()
        .args([
            "dossiers",
            "index-append-preview",
            "--agent",
            "eval",
            "--path",
            ".brainyard/agents/eval-agent/dossiers/20260102-030405-score.md",
            "--slug",
            "score",
            "--pre-verdict",
            "go",
            "--score-verdict",
            "achieved",
            "--confidence",
            "high",
            "--post-verdict",
            "pass",
            "--next-agent",
            "user",
            "--base-dir",
        ])
        .arg(dir.path())
        .args(["--created", "2026-01-02 03:06"])
        .assert()
        .success();
    let stdout = String::from_utf8(assert.get_output().stdout.clone()).unwrap();
    let eval_index: serde_json::Value = serde_json::from_str(&stdout).unwrap();
    assert_eq!(
        eval_index["line"],
        "- 2026-01-02 03:06 [score](dossiers/20260102-030405-score.md) — pre:go · verdict:ACHIEVED (conf:high) · post:pass · → user\n"
    );

    let assert = Command::cargo_bin("by-rs")
        .unwrap()
        .args([
            "dossiers",
            "next-handoff",
            "--agent",
            "plan",
            "--pre",
            "{:verdict :go}",
            "--post",
            "{:verdict :pass}",
            "--dossier-path",
        ])
        .arg(&dossier_path)
        .assert()
        .success();
    let stdout = String::from_utf8(assert.get_output().stdout.clone()).unwrap();
    let plan_handoff: serde_json::Value = serde_json::from_str(&stdout).unwrap();
    assert_eq!(plan_handoff["projection"], "common.plan/plan$next-handoff");
    assert_eq!(plan_handoff["next-agent"], "todo-agent");
    assert!(plan_handoff["next-call"]
        .as_str()
        .unwrap()
        .contains(":agent-context"));

    let assert = Command::cargo_bin("by-rs")
        .unwrap()
        .args([
            "dossiers",
            "next-handoff",
            "--agent",
            "exec",
            "--post",
            "{:verdict :pass}",
            "--items-pending-after",
            "[1]",
        ])
        .assert()
        .success();
    let stdout = String::from_utf8(assert.get_output().stdout.clone()).unwrap();
    let exec_continue: serde_json::Value = serde_json::from_str(&stdout).unwrap();
    assert_eq!(exec_continue["next-agent"], "exec-agent");

    let assert = Command::cargo_bin("by-rs")
        .unwrap()
        .args([
            "dossiers",
            "next-handoff",
            "--agent",
            "exec",
            "--post",
            "{:verdict :pass}",
            "--items-pending-after",
            "[]",
        ])
        .assert()
        .success();
    let stdout = String::from_utf8(assert.get_output().stdout.clone()).unwrap();
    let exec_done: serde_json::Value = serde_json::from_str(&stdout).unwrap();
    assert_eq!(exec_done["next-agent"], "eval-agent");

    let assert = Command::cargo_bin("by-rs")
        .unwrap()
        .args([
            "dossiers",
            "next-handoff",
            "--agent",
            "eval",
            "--score",
            "{:verdict :not-achieved}",
            "--slug",
            "rust-port",
            "--dossier-path",
            "d.md",
        ])
        .assert()
        .success();
    let stdout = String::from_utf8(assert.get_output().stdout.clone()).unwrap();
    let eval_handoff: serde_json::Value = serde_json::from_str(&stdout).unwrap();
    assert_eq!(eval_handoff["projection"], "common.eval/eval$next-handoff");
    assert_eq!(eval_handoff["next-agent"], "plan-agent");
}

#[test]
fn dossiers_eval_verdict_preview_and_find_are_live_free() {
    let dir = tempfile::tempdir().unwrap();
    let verdicts_dir = dir.path().join(".brainyard/agents/eval-agent/verdicts");
    std::fs::create_dir_all(&verdicts_dir).unwrap();
    std::fs::write(verdicts_dir.join("20260101-000000-rust-port.md"), "# old\n").unwrap();

    let assert = Command::cargo_bin("by-rs")
        .unwrap()
        .args([
            "dossiers",
            "verdict-write-preview",
            "--slug",
            "rust-port",
            "--content",
            "# Verdict\npass\n",
            "--base-dir",
        ])
        .arg(dir.path())
        .args(["--ts", "20260102-030405"])
        .assert()
        .success();
    let stdout = String::from_utf8(assert.get_output().stdout.clone()).unwrap();
    let preview: serde_json::Value = serde_json::from_str(&stdout).unwrap();
    assert_eq!(
        preview["projection"],
        "common.eval/eval$verdict-write-preview"
    );
    assert_eq!(preview["write-skipped?"], true);
    assert_eq!(preview["slug"], "rust-port-2");
    assert_eq!(preview["requested-slug"], "rust-port");
    assert_eq!(preview["collision?"], true);
    assert_eq!(
        preview["rel-path"],
        ".brainyard/agents/eval-agent/verdicts/20260102-030405-rust-port-2.md"
    );
    assert_eq!(preview["content"], "# Verdict\npass\n");
    assert!(!dir
        .path()
        .join(".brainyard/agents/eval-agent/verdicts/20260102-030405-rust-port-2.md")
        .exists());

    let exec_dir = dir.path().join(".brainyard/agents/exec-agent/dossiers");
    std::fs::create_dir_all(&exec_dir).unwrap();
    std::fs::write(
        exec_dir.join("20260101-000000-rust-port.md"),
        r#"---
slug: rust-port
created: 2026-01-01T00:00:00Z
pre:
  verdict: gather
execute:
  items_advanced: 0
  items_pending_after: 3
post:
  verdict: hold
---
old
"#,
    )
    .unwrap();
    std::fs::write(
        exec_dir.join("20260103-010203-rust-port-2.md"),
        r#"---
slug: rust-port-2
created: 2026-01-03T01:02:03Z
pre:
  verdict: go
execute:
  items_advanced: 2
  items_pending_after: 1
post:
  verdict: pass
---
new
"#,
    )
    .unwrap();

    let assert = Command::cargo_bin("by-rs")
        .unwrap()
        .args([
            "dossiers",
            "find",
            "--agent",
            "exec",
            "--slug",
            "rust-port",
            "--base-dir",
        ])
        .arg(dir.path())
        .assert()
        .success();
    let stdout = String::from_utf8(assert.get_output().stdout.clone()).unwrap();
    let exec_found: serde_json::Value = serde_json::from_str(&stdout).unwrap();
    assert_eq!(exec_found["projection"], "common.exec/exec$find");
    assert_eq!(exec_found["n-matches"], 2);
    assert_eq!(
        exec_found["matches"][0]["path"],
        ".brainyard/agents/exec-agent/dossiers/20260103-010203-rust-port-2.md"
    );
    assert_eq!(exec_found["matches"][0]["pre_verdict"], "go");
    assert_eq!(exec_found["matches"][0]["post_verdict"], "pass");
    assert_eq!(exec_found["matches"][0]["advanced"], 2);
    assert_eq!(exec_found["matches"][0]["pending"], 1);

    let eval_dir = dir.path().join(".brainyard/agents/eval-agent/dossiers");
    std::fs::create_dir_all(&eval_dir).unwrap();
    std::fs::write(
        eval_dir.join("20260104-010203-rust-port.md"),
        r#"---
slug: rust-port
created: 2026-01-04T01:02:03Z
exec_run_record: records/a.edn
score:
  verdict: achieved
  confidence: high
---
score a
"#,
    )
    .unwrap();
    std::fs::write(
        eval_dir.join("20260105-010203-rust-port-2.md"),
        r#"---
slug: rust-port-2
created: 2026-01-05T01:02:03Z
exec_run_record: records/b.edn
score:
  verdict: not-achieved
  confidence: low
---
score b
"#,
    )
    .unwrap();

    let assert = Command::cargo_bin("by-rs")
        .unwrap()
        .args([
            "dossiers",
            "find",
            "--agent",
            "eval",
            "--slug",
            "rust-port",
            "--run-record",
            "records/a.edn",
            "--base-dir",
        ])
        .arg(dir.path())
        .assert()
        .success();
    let stdout = String::from_utf8(assert.get_output().stdout.clone()).unwrap();
    let eval_found: serde_json::Value = serde_json::from_str(&stdout).unwrap();
    assert_eq!(eval_found["projection"], "common.eval/eval$find");
    assert_eq!(eval_found["n-matches"], 1);
    assert_eq!(eval_found["matches"][0]["score_verdict"], "achieved");
    assert_eq!(eval_found["matches"][0]["confidence"], "high");
    assert_eq!(eval_found["matches"][0]["exec_run_record"], "records/a.edn");
}

#[test]
fn rlm_helpers_are_live_free_json_projections() {
    let assert = Command::cargo_bin("by-rs")
        .unwrap()
        .args([
            "rlm",
            "chunk-text",
            "--text",
            "abcdef",
            "--size",
            "4",
            "--overlap",
            "1",
        ])
        .assert()
        .success();
    let stdout = String::from_utf8(assert.get_output().stdout.clone()).unwrap();
    let chunks: serde_json::Value = serde_json::from_str(&stdout).unwrap();
    assert_eq!(chunks["chunks"][0], "abcd");
    assert_eq!(chunks["chunks"][1], "def");
    assert_eq!(chunks["n-chunks"], 2);

    let dir = tempfile::tempdir().unwrap();
    let one = dir.path().join("one.txt");
    let missing = dir.path().join("missing.txt");
    std::fs::write(&one, "alpha").unwrap();
    let assert = Command::cargo_bin("by-rs")
        .unwrap()
        .args(["rlm", "chunk-files", "--path"])
        .arg(&one)
        .args(["--path"])
        .arg(&missing)
        .args([
            "--group-size",
            "1",
            "--max-bytes",
            "1000",
            "--separator",
            "\n---\n",
        ])
        .assert()
        .success();
    let stdout = String::from_utf8(assert.get_output().stdout.clone()).unwrap();
    let file_chunks: serde_json::Value = serde_json::from_str(&stdout).unwrap();
    assert_eq!(file_chunks["n-chunks"], 1);
    assert!(file_chunks["chunks"][0].as_str().unwrap().contains("=== "));
    assert!(file_chunks["chunks"][0].as_str().unwrap().contains("alpha"));
    assert!(file_chunks["errors"][0]["path"]
        .as_str()
        .unwrap()
        .ends_with("missing.txt"));

    let assert = Command::cargo_bin("by-rs")
        .unwrap()
        .args([
            "rlm",
            "parse-map-results",
            "--result",
            r#"{"category":"bug","count":2}"#,
            "--result",
            "{:category :feature :count 3}",
            "--result",
            "not json edn",
            "--shape",
            "json",
        ])
        .assert()
        .success();
    let stdout = String::from_utf8(assert.get_output().stdout.clone()).unwrap();
    let parsed: serde_json::Value = serde_json::from_str(&stdout).unwrap();
    assert_eq!(parsed["n-parsed"], 2);
    assert_eq!(parsed["n-failed"], 1);
    assert_eq!(parsed["parsed"][0]["category"], "bug");
    assert_eq!(parsed["parsed"][1]["category"], "feature");
    assert_eq!(parsed["failed"][0]["idx"], 2);

    let assert = Command::cargo_bin("by-rs")
        .unwrap()
        .args([
            "rlm",
            "reduce-counts",
            "--parsed-results",
            "[{:category :bug :count 2} {:category :bug :count 1} {:category :feature}]",
        ])
        .assert()
        .success();
    let stdout = String::from_utf8(assert.get_output().stdout.clone()).unwrap();
    let reduced: serde_json::Value = serde_json::from_str(&stdout).unwrap();
    assert_eq!(reduced["total"], 4);
    assert_eq!(reduced["n-categories"], 2);
    assert_eq!(reduced["counts"][0]["key"], "bug");
    assert_eq!(reduced["counts"][0]["count"], 3);
    assert_eq!(reduced["counts"][0]["percent"].as_f64().unwrap(), 75.0);

    let assert = Command::cargo_bin("by-rs")
        .unwrap()
        .args([
            "rlm",
            "conservative-verdict",
            "--parsed-results",
            r#"[{:malicious? false} {:parse-failed true :raw "x"} {:malicious? true :id 1}]"#,
        ])
        .assert()
        .success();
    let stdout = String::from_utf8(assert.get_output().stdout.clone()).unwrap();
    let verdict: serde_json::Value = serde_json::from_str(&stdout).unwrap();
    assert_eq!(verdict["verdict"], true);
    assert_eq!(verdict["positive-key"], "malicious?");
    assert_eq!(verdict["positive-count"], 1);
    assert_eq!(verdict["negative-count"], 1);
    assert_eq!(verdict["skipped"], 1);
    assert_eq!(verdict["evidence"][0]["id"], 1);
}

#[test]
fn doc_list_and_read_plan_todo_without_writes() {
    let project = tempfile::tempdir().unwrap();
    let home = tempfile::tempdir().unwrap();
    let new_plan_dir = project.path().join(".brainyard/agents/plan-agent/plans");
    let legacy_plan_dir = project.path().join(".brainyard/plans");
    let new_todo_dir = project.path().join(".brainyard/agents/todo-agent/todos");
    std::fs::create_dir_all(&new_plan_dir).unwrap();
    std::fs::create_dir_all(&legacy_plan_dir).unwrap();
    std::fs::create_dir_all(&new_todo_dir).unwrap();
    std::fs::write(
        new_plan_dir.join("project-plan.md"),
        r#"---
id: p1
file-type: plan
title: Project Plan
scope: project
status: draft
created: 2026-01-01T00:00:00Z
updated: 2026-01-02T00:00:00Z
---

# Project Plan

## Approach
Port local projections first.
"#,
    )
    .unwrap();
    std::fs::write(
        legacy_plan_dir.join("project-plan.md"),
        r#"---
id: old-p1
file-type: plan
title: Legacy Project Plan
scope: project
status: draft
---

# Legacy Project Plan
"#,
    )
    .unwrap();
    std::fs::write(new_plan_dir.join("README.md"), "# loose note\n").unwrap();
    std::fs::write(
        new_todo_dir.join("project-todo.md"),
        r#"---
id: t1
file-type: todo
title: Project Todo
scope: project
status: in-progress
created: 2026-01-01T00:00:00Z
updated: 2026-01-02T00:00:00Z
---

# Project Todo

## Goal
Finish local compatibility work.

## Todo
- [x] Implement plan docs {via: bash, covers: ["plan-read"]}
- [ ] Verify todo docs
"#,
    )
    .unwrap();

    let assert = Command::cargo_bin("by-rs")
        .unwrap()
        .args(["doc", "list", "--kind", "plan", "--project-dir"])
        .arg(project.path())
        .args(["--user-dir"])
        .arg(home.path())
        .assert()
        .success()
        .stderr(predicate::str::is_empty());
    let stdout = String::from_utf8(assert.get_output().stdout.clone()).unwrap();
    let plans: serde_json::Value = serde_json::from_str(&stdout).unwrap();
    assert_eq!(plans["total"], 1);
    assert_eq!(plans["result"][0]["slug"], "project-plan");
    assert_eq!(plans["result"][0]["title"], "Project Plan");
    assert_eq!(plans["result"][0]["layout"], "new");

    let assert = Command::cargo_bin("by-rs")
        .unwrap()
        .args([
            "doc",
            "read",
            "--kind",
            "plan",
            "--slug",
            "project-plan",
            "--project-dir",
        ])
        .arg(project.path())
        .args(["--user-dir"])
        .arg(home.path())
        .assert()
        .success()
        .stderr(predicate::str::is_empty());
    let stdout = String::from_utf8(assert.get_output().stdout.clone()).unwrap();
    let plan: serde_json::Value = serde_json::from_str(&stdout).unwrap();
    assert_eq!(plan["title"], "Project Plan");
    assert_eq!(plan["scope"], "project");
    assert!(plan["body"].as_str().unwrap().contains("## Approach"));
    assert!(!plan["body"].as_str().unwrap().contains("# Project Plan"));

    let assert = Command::cargo_bin("by-rs")
        .unwrap()
        .args([
            "doc",
            "list",
            "--kind",
            "todo",
            "--status",
            "in-progress",
            "--project-dir",
        ])
        .arg(project.path())
        .args(["--user-dir"])
        .arg(home.path())
        .assert()
        .success()
        .stderr(predicate::str::is_empty());
    let stdout = String::from_utf8(assert.get_output().stdout.clone()).unwrap();
    let todos: serde_json::Value = serde_json::from_str(&stdout).unwrap();
    assert_eq!(todos["total"], 1);
    assert_eq!(todos["result"][0]["item-progress"], "1/2");

    let assert = Command::cargo_bin("by-rs")
        .unwrap()
        .args([
            "doc",
            "read",
            "--kind",
            "todo",
            "--slug",
            "project-todo",
            "--project-dir",
        ])
        .arg(project.path())
        .args(["--user-dir"])
        .arg(home.path())
        .assert()
        .success()
        .stderr(predicate::str::is_empty());
    let stdout = String::from_utf8(assert.get_output().stdout.clone()).unwrap();
    let todo: serde_json::Value = serde_json::from_str(&stdout).unwrap();
    assert_eq!(todo["goal"], "Finish local compatibility work.");
    assert_eq!(todo["items"][0]["done"], true);
    assert_eq!(todo["items"][0]["tags"]["via"], "bash");
    assert_eq!(todo["items"][0]["tags"]["covers"][0], "plan-read");
    assert_eq!(todo["progress"]["completed"], 1);
    assert_eq!(todo["progress"]["pending"], 1);
    assert_eq!(todo["progress"]["percent"].as_f64().unwrap(), 50.0);
    assert_eq!(
        todo["progress"]["next-item"]["description"],
        "Verify todo docs"
    );

    let assert = Command::cargo_bin("by-rs")
        .unwrap()
        .args([
            "doc",
            "read",
            "--kind",
            "todo",
            "--slug",
            "missing",
            "--project-dir",
        ])
        .arg(project.path())
        .args(["--user-dir"])
        .arg(home.path())
        .assert()
        .success()
        .stderr(predicate::str::is_empty());
    let stdout = String::from_utf8(assert.get_output().stdout.clone()).unwrap();
    let missing: serde_json::Value = serde_json::from_str(&stdout).unwrap();
    assert_eq!(missing["not-found"], true);
    assert_eq!(missing["kind"], "todo");
}

#[test]
fn doc_render_parse_exists_and_progress_are_live_free() {
    let project = tempfile::tempdir().unwrap();
    let home = tempfile::tempdir().unwrap();

    let assert = Command::cargo_bin("by-rs")
        .unwrap()
        .args([
            "doc",
            "render",
            "--kind",
            "plan",
            "--id",
            "plan-1",
            "--title",
            "Rust Plan",
            "--created",
            "2026-01-01T00:00:00Z",
            "--updated",
            "2026-01-02T00:00:00Z",
            "--body",
            "## Approach\nPort local conversions first.",
        ])
        .assert()
        .success()
        .stderr(predicate::str::is_empty());
    let stdout = String::from_utf8(assert.get_output().stdout.clone()).unwrap();
    let rendered_plan: serde_json::Value = serde_json::from_str(&stdout).unwrap();
    let plan_markdown = rendered_plan["markdown"].as_str().unwrap();
    assert!(plan_markdown.contains("file-type: plan"));
    assert!(plan_markdown.contains("# Rust Plan"));
    assert!(plan_markdown.contains("## Approach"));

    let plan_path = project.path().join("plan.md");
    std::fs::write(&plan_path, plan_markdown).unwrap();
    let assert = Command::cargo_bin("by-rs")
        .unwrap()
        .args(["doc", "parse", "--kind", "plan", "--path"])
        .arg(&plan_path)
        .assert()
        .success()
        .stderr(predicate::str::is_empty());
    let stdout = String::from_utf8(assert.get_output().stdout.clone()).unwrap();
    let parsed_plan: serde_json::Value = serde_json::from_str(&stdout).unwrap();
    assert_eq!(parsed_plan["kind"], "plan");
    assert_eq!(parsed_plan["valid?"], true);
    assert_eq!(parsed_plan["title"], "Rust Plan");
    assert!(parsed_plan["body"]
        .as_str()
        .unwrap()
        .contains("Port local conversions first."));

    let assert = Command::cargo_bin("by-rs")
        .unwrap()
        .args([
            "doc",
            "render",
            "--kind",
            "todo",
            "--id",
            "todo-1",
            "--title",
            "Rust Todo",
            "--status",
            "in-progress",
            "--created",
            "2026-01-01T00:00:00Z",
            "--updated",
            "2026-01-02T00:00:00Z",
            "--goal",
            "Finish local doc compatibility.",
            "--items",
            r#"[{:description "Done item" :done true :tags {:via :bash :covers ["doc-parse"]}} {:description "Next item"}]"#,
        ])
        .assert()
        .success()
        .stderr(predicate::str::is_empty());
    let stdout = String::from_utf8(assert.get_output().stdout.clone()).unwrap();
    let rendered_todo: serde_json::Value = serde_json::from_str(&stdout).unwrap();
    let todo_markdown = rendered_todo["markdown"].as_str().unwrap();
    assert!(todo_markdown.contains("file-type: todo"));
    assert!(todo_markdown.contains("- [x] Done item {via: bash, covers: [\"doc-parse\"]}"));
    assert!(todo_markdown.contains("- [ ] Next item"));

    let todo_dir = project.path().join(".brainyard/agents/todo-agent/todos");
    std::fs::create_dir_all(&todo_dir).unwrap();
    let todo_path = todo_dir.join("rendered-todo.md");
    std::fs::write(&todo_path, todo_markdown).unwrap();

    let assert = Command::cargo_bin("by-rs")
        .unwrap()
        .args(["doc", "parse", "--kind", "todo", "--path"])
        .arg(&todo_path)
        .assert()
        .success()
        .stderr(predicate::str::is_empty());
    let stdout = String::from_utf8(assert.get_output().stdout.clone()).unwrap();
    let parsed_todo: serde_json::Value = serde_json::from_str(&stdout).unwrap();
    assert_eq!(parsed_todo["kind"], "todo");
    assert_eq!(parsed_todo["valid?"], true);
    assert_eq!(parsed_todo["goal"], "Finish local doc compatibility.");
    assert_eq!(parsed_todo["items"][0]["done"], true);
    assert_eq!(parsed_todo["items"][0]["tags"]["via"], "bash");
    assert_eq!(parsed_todo["progress"]["completed"], 1);
    assert_eq!(parsed_todo["progress"]["pending"], 1);

    let assert = Command::cargo_bin("by-rs")
        .unwrap()
        .args(["doc", "progress", "--path"])
        .arg(&todo_path)
        .assert()
        .success()
        .stderr(predicate::str::is_empty());
    let stdout = String::from_utf8(assert.get_output().stdout.clone()).unwrap();
    let progress: serde_json::Value = serde_json::from_str(&stdout).unwrap();
    assert_eq!(progress["progress"]["completed"], 1);
    assert_eq!(progress["progress"]["pending"], 1);
    assert_eq!(
        progress["progress"]["next-item"]["description"],
        "Next item"
    );

    let assert = Command::cargo_bin("by-rs")
        .unwrap()
        .args([
            "doc",
            "exists",
            "--kind",
            "todo",
            "--slug",
            "rendered-todo",
            "--project-dir",
        ])
        .arg(project.path())
        .args(["--user-dir"])
        .arg(home.path())
        .assert()
        .success()
        .stderr(predicate::str::is_empty());
    let stdout = String::from_utf8(assert.get_output().stdout.clone()).unwrap();
    let exists: serde_json::Value = serde_json::from_str(&stdout).unwrap();
    assert_eq!(exists["exists?"], true);
    assert_eq!(exists["layout"], "new");
    assert_eq!(exists["scope"], "project");
}

#[test]
fn doc_create_update_delete_previews_are_live_free() {
    let project = tempfile::tempdir().unwrap();
    let home = tempfile::tempdir().unwrap();

    let assert = Command::cargo_bin("by-rs")
        .unwrap()
        .args([
            "doc",
            "create-preview",
            "--kind",
            "plan",
            "--title",
            "Rust Plan",
            "--slug",
            "rust-plan",
            "--id",
            "plan-id",
            "--created",
            "2026-01-01T00:00:00Z",
            "--updated",
            "2026-01-01T00:00:00Z",
            "--body",
            "## Approach\nPort docs.",
            "--project-dir",
        ])
        .arg(project.path())
        .args(["--user-dir"])
        .arg(home.path())
        .assert()
        .success()
        .stderr(predicate::str::is_empty());
    let stdout = String::from_utf8(assert.get_output().stdout.clone()).unwrap();
    let plan_create: serde_json::Value = serde_json::from_str(&stdout).unwrap();
    assert_eq!(plan_create["projection"], "common.doc/doc$create-preview");
    assert_eq!(plan_create["write-skipped?"], true);
    assert_eq!(plan_create["result"]["slug"], "rust-plan");
    assert_eq!(plan_create["result"]["body"], "## Approach\nPort docs.");
    assert!(plan_create["content"]
        .as_str()
        .unwrap()
        .contains("file-type: plan"));
    assert!(!project
        .path()
        .join(".brainyard/agents/plan-agent/plans/rust-plan.md")
        .exists());

    let assert = Command::cargo_bin("by-rs")
        .unwrap()
        .args([
            "doc",
            "create-preview",
            "--kind",
            "todo",
            "--title",
            "Rust Todo",
            "--slug",
            "rust-todo-preview",
            "--id",
            "todo-preview-id",
            "--created",
            "2026-01-01T00:00:00Z",
            "--goal",
            "Finish local docs.",
            "--items",
            r#"[{:description "Already done" :done true :tags {:via :mcp :covers ["doc-preview"]}}]"#,
            "--project-dir",
        ])
        .arg(project.path())
        .args(["--user-dir"])
        .arg(home.path())
        .assert()
        .success()
        .stderr(predicate::str::is_empty());
    let stdout = String::from_utf8(assert.get_output().stdout.clone()).unwrap();
    let todo_create: serde_json::Value = serde_json::from_str(&stdout).unwrap();
    assert_eq!(todo_create["result"]["items"][0]["done"], false);
    assert_eq!(todo_create["result"]["items"][0]["tags"]["via"], "mcp");
    assert!(todo_create["content"]
        .as_str()
        .unwrap()
        .contains("- [ ] Already done {via: mcp, covers: [\"doc-preview\"]}"));

    let plan_dir = project.path().join(".brainyard/agents/plan-agent/plans");
    std::fs::create_dir_all(&plan_dir).unwrap();
    let plan_path = plan_dir.join("rust-plan.md");
    std::fs::write(
        &plan_path,
        r#"---
id: plan-id
file-type: plan
title: Rust Plan
scope: project
status: draft
created: 2026-01-01T00:00:00Z
updated: 2026-01-01T00:00:00Z
---

# Rust Plan

## Approach
Port docs.
"#,
    )
    .unwrap();

    let assert = Command::cargo_bin("by-rs")
        .unwrap()
        .args([
            "doc",
            "update-preview",
            "--kind",
            "plan",
            "--slug",
            "rust-plan",
            "--body",
            "## Revised\nUse Rust.",
            "--updated",
            "2026-01-03T00:00:00Z",
            "--project-dir",
        ])
        .arg(project.path())
        .args(["--user-dir"])
        .arg(home.path())
        .assert()
        .success()
        .stderr(predicate::str::is_empty());
    let stdout = String::from_utf8(assert.get_output().stdout.clone()).unwrap();
    let plan_update: serde_json::Value = serde_json::from_str(&stdout).unwrap();
    assert_eq!(plan_update["projection"], "common.doc/doc$update-preview");
    assert_eq!(plan_update["sub-op"], "body");
    assert_eq!(plan_update["full-result"]["body"], "## Revised\nUse Rust.");
    assert!(plan_update["content"]
        .as_str()
        .unwrap()
        .contains("## Revised"));
    assert!(!std::fs::read_to_string(&plan_path)
        .unwrap()
        .contains("## Revised"));

    let todo_dir = project.path().join(".brainyard/agents/todo-agent/todos");
    std::fs::create_dir_all(&todo_dir).unwrap();
    let todo_path = todo_dir.join("rust-todo.md");
    std::fs::write(
        &todo_path,
        r#"---
id: todo-id
file-type: todo
title: Rust Todo
scope: project
status: draft
created: 2026-01-01T00:00:00Z
updated: 2026-01-01T00:00:00Z
---

# Rust Todo

## Goal
Finish port.

## Todo
- [ ] First
- [ ] Second {via: bash, covers: ["doc-update"]}
"#,
    )
    .unwrap();

    let assert = Command::cargo_bin("by-rs")
        .unwrap()
        .args([
            "doc",
            "update-preview",
            "--kind",
            "todo",
            "--slug",
            "rust-todo",
            "--item-idx",
            "1",
            "--item-done",
            "true",
            "--updated",
            "2026-01-03T00:00:00Z",
            "--project-dir",
        ])
        .arg(project.path())
        .args(["--user-dir"])
        .arg(home.path())
        .assert()
        .success()
        .stderr(predicate::str::is_empty());
    let stdout = String::from_utf8(assert.get_output().stdout.clone()).unwrap();
    let todo_update: serde_json::Value = serde_json::from_str(&stdout).unwrap();
    assert_eq!(todo_update["sub-op"], "item");
    assert_eq!(todo_update["result"]["status"], "in-progress");
    assert_eq!(todo_update["result"]["items"][1]["done"], true);
    assert_eq!(todo_update["result"]["progress"]["completed"], 1);
    assert!(todo_update["content"]
        .as_str()
        .unwrap()
        .contains("- [x] Second {via: bash, covers: [\"doc-update\"]}"));
    assert!(std::fs::read_to_string(&todo_path)
        .unwrap()
        .contains("- [ ] Second"));

    let assert = Command::cargo_bin("by-rs")
        .unwrap()
        .args([
            "doc",
            "delete-preview",
            "--kind",
            "todo",
            "--slug",
            "rust-todo",
            "--project-dir",
        ])
        .arg(project.path())
        .args(["--user-dir"])
        .arg(home.path())
        .assert()
        .success()
        .stderr(predicate::str::is_empty());
    let stdout = String::from_utf8(assert.get_output().stdout.clone()).unwrap();
    let todo_delete: serde_json::Value = serde_json::from_str(&stdout).unwrap();
    assert_eq!(todo_delete["projection"], "common.doc/doc$delete-preview");
    assert_eq!(todo_delete["write-skipped?"], true);
    assert_eq!(todo_delete["deleted"], "rust-todo");
    assert!(todo_path.exists());
}

#[test]
fn explore_helpers_are_live_free_json_projections() {
    let assert = Command::cargo_bin("by-rs")
        .unwrap()
        .args([
            "explore",
            "slug",
            "--question",
            "How can we port Clojure to Rust?",
            "--max-chars",
            "40",
        ])
        .assert()
        .success()
        .stderr(predicate::str::is_empty());
    let stdout = String::from_utf8(assert.get_output().stdout.clone()).unwrap();
    let slug: serde_json::Value = serde_json::from_str(&stdout).unwrap();
    assert_eq!(slug["projection"], "common.explore/explore$slug");
    assert_eq!(slug["slug"], "port-clojure-rust");

    let assert = Command::cargo_bin("by-rs")
        .unwrap()
        .args([
            "explore",
            "frontmatter",
            "--question",
            "Question-only needle for Rust port?",
            "--slug",
            "rust-port",
            "--summary",
            "Rust\nport   summary",
            "--surface",
            "filesystem",
            "--surface",
            "mcp",
            "--file",
            "src/main.rs",
            "--url",
            "https://example.com/notes",
            "--mcp-tool",
            "aws-knowledge",
            "--skill",
            "plan",
            "--created",
            "2026-01-02T03:04:05Z",
            "--turn-id",
            "turn-1",
            "--session-id",
            "session-1",
        ])
        .assert()
        .success()
        .stderr(predicate::str::is_empty());
    let stdout = String::from_utf8(assert.get_output().stdout.clone()).unwrap();
    let frontmatter: serde_json::Value = serde_json::from_str(&stdout).unwrap();
    assert_eq!(
        frontmatter["projection"],
        "common.explore/explore$frontmatter"
    );
    let frontmatter = frontmatter["frontmatter"].as_str().unwrap();
    assert!(frontmatter.contains("agent: explore-agent"));
    assert!(frontmatter.contains("surfaces: [filesystem, mcp]"));
    assert!(frontmatter.contains("summary: >\n  Rust port summary"));

    let project = tempfile::tempdir().unwrap();
    let results_dir = project
        .path()
        .join(".brainyard/agents/explore-agent/results");
    std::fs::create_dir_all(&results_dir).unwrap();
    let result_path = results_dir.join("20260102-rust-port.md");
    std::fs::write(&result_path, format!("{frontmatter}\n# Body\n")).unwrap();
    std::fs::write(
        project.path().join(".brainyard/agents/explore-agent/INDEX.md"),
        "- 2026-01-02 03:04 [rust-port](results/20260102-rust-port.md) — filesystem, mcp · *Rust port summary*\n",
    )
    .unwrap();

    let assert = Command::cargo_bin("by-rs")
        .unwrap()
        .args(["explore", "index-append-preview", "--path"])
        .arg(&result_path)
        .args([
            "--slug",
            "rust-port",
            "--surfaces",
            "[:filesystem :mcp]",
            "--summary",
            "Rust\nport   summary",
            "--created",
            "2026-01-02 03:04",
            "--base-dir",
        ])
        .arg(project.path())
        .assert()
        .success()
        .stderr(predicate::str::is_empty());
    let stdout = String::from_utf8(assert.get_output().stdout.clone()).unwrap();
    let preview: serde_json::Value = serde_json::from_str(&stdout).unwrap();
    assert_eq!(
        preview["projection"],
        "common.explore/explore$index-append-preview"
    );
    assert_eq!(preview["write-skipped?"], true);
    assert_eq!(preview["existing?"], true);
    assert_eq!(preview["surfaces"][0], "filesystem");
    assert_eq!(
        preview["line"],
        "- 2026-01-02 03:04 [rust-port](results/20260102-rust-port.md) — filesystem, mcp · *Rust port summary*\n"
    );

    let assert = Command::cargo_bin("by-rs")
        .unwrap()
        .args(["explore", "read-frontmatter", "--path"])
        .arg(&result_path)
        .assert()
        .success()
        .stderr(predicate::str::is_empty());
    let stdout = String::from_utf8(assert.get_output().stdout.clone()).unwrap();
    let parsed: serde_json::Value = serde_json::from_str(&stdout).unwrap();
    assert_eq!(
        parsed["projection"],
        "common.explore/explore$read-frontmatter"
    );
    assert_eq!(parsed["slug"], "rust-port");
    assert_eq!(parsed["question"], "Question-only needle for Rust port?");
    assert_eq!(parsed["surfaces"][1], "mcp");
    assert_eq!(parsed["entities"]["files"][0], "src/main.rs");
    assert_eq!(parsed["entities"]["mcp_tools"][0], "aws-knowledge");
    assert_eq!(parsed["summary"], "Rust port summary");
    assert_eq!(parsed["turn_id"], "turn-1");

    let assert = Command::cargo_bin("by-rs")
        .unwrap()
        .args(["explore", "find", "--query", "summary", "--base-dir"])
        .arg(project.path())
        .assert()
        .success()
        .stderr(predicate::str::is_empty());
    let stdout = String::from_utf8(assert.get_output().stdout.clone()).unwrap();
    let from_index: serde_json::Value = serde_json::from_str(&stdout).unwrap();
    assert_eq!(from_index["projection"], "common.explore/explore$find");
    assert_eq!(from_index["n-matches"], 1);
    assert_eq!(
        from_index["matches"][0]["path"],
        ".brainyard/agents/explore-agent/results/20260102-rust-port.md"
    );
    assert_eq!(from_index["matches"][0]["surfaces"][1], "mcp");

    let assert = Command::cargo_bin("by-rs")
        .unwrap()
        .args(["explore", "find", "--query", "question-only", "--base-dir"])
        .arg(project.path())
        .assert()
        .success()
        .stderr(predicate::str::is_empty());
    let stdout = String::from_utf8(assert.get_output().stdout.clone()).unwrap();
    let from_scan: serde_json::Value = serde_json::from_str(&stdout).unwrap();
    assert_eq!(from_scan["projection"], "common.explore/explore$find");
    assert_eq!(from_scan["n-matches"], 1);
    assert_eq!(from_scan["matches"][0]["slug"], "rust-port");
    assert_eq!(from_scan["matches"][0]["summary"], "Rust port summary");
}

#[test]
fn reference_helpers_read_glob_batch_and_grep_locally() {
    let project = tempfile::tempdir().unwrap();
    let docs = project.path().join("docs");
    let src = project.path().join("src");
    std::fs::create_dir_all(&docs).unwrap();
    std::fs::create_dir_all(&src).unwrap();
    std::fs::write(docs.join("one.md"), "alpha\nbeta needle\ncharlie\n").unwrap();
    std::fs::write(
        src.join("main.rs"),
        "fn main() {\n    println!(\"needle\");\n}\n",
    )
    .unwrap();

    let assert = Command::cargo_bin("by-rs")
        .unwrap()
        .args([
            "reference",
            "read-file",
            "--path",
            "@docs/one.md",
            "--base-dir",
        ])
        .arg(project.path())
        .args(["--lines", "[2, 2]"])
        .assert()
        .success()
        .stderr(predicate::str::is_empty());
    let stdout = String::from_utf8(assert.get_output().stdout.clone()).unwrap();
    let read_file: serde_json::Value = serde_json::from_str(&stdout).unwrap();
    assert_eq!(read_file["content"], "beta needle");
    assert_eq!(read_file["lines-range"][0], 2);
    assert_eq!(read_file["has-more"], true);

    let assert = Command::cargo_bin("by-rs")
        .unwrap()
        .args([
            "reference",
            "list-files",
            "--pattern",
            "**/*.md",
            "--base-dir",
        ])
        .arg(project.path())
        .assert()
        .success()
        .stderr(predicate::str::is_empty());
    let stdout = String::from_utf8(assert.get_output().stdout.clone()).unwrap();
    let listed: serde_json::Value = serde_json::from_str(&stdout).unwrap();
    assert_eq!(listed["count"], 1);
    assert_eq!(listed["files"][0]["path"], "docs/one.md");

    let assert = Command::cargo_bin("by-rs")
        .unwrap()
        .args([
            "reference",
            "read-glob",
            "--pattern",
            "docs/*.md",
            "--base-dir",
        ])
        .arg(project.path())
        .assert()
        .success()
        .stderr(predicate::str::is_empty());
    let stdout = String::from_utf8(assert.get_output().stdout.clone()).unwrap();
    let globbed: serde_json::Value = serde_json::from_str(&stdout).unwrap();
    assert_eq!(globbed["count"], 1);
    assert!(globbed["files"][0]["content"]
        .as_str()
        .unwrap()
        .contains("beta needle"));

    let assert = Command::cargo_bin("by-rs")
        .unwrap()
        .args([
            "reference",
            "read-files-batch",
            "--path",
            "docs/one.md",
            "--path",
            "missing.md",
            "--base-dir",
        ])
        .arg(project.path())
        .assert()
        .success()
        .stderr(predicate::str::is_empty());
    let stdout = String::from_utf8(assert.get_output().stdout.clone()).unwrap();
    let batch: serde_json::Value = serde_json::from_str(&stdout).unwrap();
    assert_eq!(batch["count"], 2);
    assert!(batch["files"][0]["content"]
        .as_str()
        .unwrap()
        .contains("alpha"));
    assert!(batch["files"][1]["error"]
        .as_str()
        .unwrap()
        .contains("File not found"));

    let assert = Command::cargo_bin("by-rs")
        .unwrap()
        .args([
            "reference",
            "grep",
            "--pattern",
            "need(le)?",
            "--path",
            ".",
            "--include-ext",
            ".rs",
            "--base-dir",
        ])
        .arg(project.path())
        .assert()
        .success()
        .stderr(predicate::str::is_empty());
    let stdout = String::from_utf8(assert.get_output().stdout.clone()).unwrap();
    let grep: serde_json::Value = serde_json::from_str(&stdout).unwrap();
    assert_eq!(grep["count"], 1);
    assert_eq!(grep["matches"][0]["line"], 2);
    assert!(grep["matches"][0]["file"]
        .as_str()
        .unwrap()
        .ends_with("src/main.rs"));

    let assert = Command::cargo_bin("by-rs")
        .unwrap()
        .args([
            "reference",
            "read-file",
            "--path",
            "../secret.txt",
            "--base-dir",
        ])
        .arg(project.path())
        .assert()
        .success()
        .stderr(predicate::str::is_empty());
    let stdout = String::from_utf8(assert.get_output().stdout.clone()).unwrap();
    let denied: serde_json::Value = serde_json::from_str(&stdout).unwrap();
    assert!(denied["error"].as_str().unwrap().contains("Access denied"));
}

#[test]
fn update_helpers_are_live_free_json_projections() {
    let assert = Command::cargo_bin("by-rs")
        .unwrap()
        .args([
            "update",
            "slug",
            "--request",
            "Can we update the Rust port safely?",
            "--max-chars",
            "40",
        ])
        .assert()
        .success()
        .stderr(predicate::str::is_empty());
    let stdout = String::from_utf8(assert.get_output().stdout.clone()).unwrap();
    let slug: serde_json::Value = serde_json::from_str(&stdout).unwrap();
    assert_eq!(slug["projection"], "common.update/update$slug");
    assert_eq!(slug["slug"], "update-rust-port-safely");

    let assert = Command::cargo_bin("by-rs")
        .unwrap()
        .args([
            "update",
            "frontmatter",
            "--request",
            "Replace timeout",
            "--slug",
            "replace-timeout",
            "--mode",
            "pattern",
            "--target",
            "src/main.rs",
            "--rollback",
            "git checkout -- src/main.rs",
            "--pre",
            r#"{:head_rev "abc" :match_count 1}"#,
            "--apply",
            r#"{:pattern "old" :replacement "new" :replaced 1}"#,
            "--verify",
            r#"{:diff_match true :tests ["cargo test"]}"#,
            "--ok",
            "--summary",
            "Updated\n timeout",
            "--created",
            "2026-01-03T04:05:06Z",
            "--turn-id",
            "turn-2",
            "--session-id",
            "session-2",
        ])
        .assert()
        .success()
        .stderr(predicate::str::is_empty());
    let stdout = String::from_utf8(assert.get_output().stdout.clone()).unwrap();
    let frontmatter: serde_json::Value = serde_json::from_str(&stdout).unwrap();
    assert_eq!(
        frontmatter["projection"],
        "common.update/update$frontmatter"
    );
    let frontmatter = frontmatter["frontmatter"].as_str().unwrap();
    assert!(frontmatter.contains("agent: update-agent"));
    assert!(frontmatter.contains("mode: pattern"));
    assert!(frontmatter.contains("target: src/main.rs"));
    assert!(frontmatter.contains("pre:\n  head_rev: abc\n  match_count: 1"));
    assert!(frontmatter.contains("apply:\n  pattern: old\n  replacement: new\n  replaced: 1"));
    assert!(frontmatter.contains("verify:\n  diff_match: true\n  tests: [\"cargo test\"]"));
    assert!(frontmatter.contains("summary: >\n  Updated timeout"));
    assert!(frontmatter.contains("turn_id: \"turn-2\""));

    let project = tempfile::tempdir().unwrap();
    let edits_dir = project.path().join(".brainyard/agents/update-agent/edits");
    std::fs::create_dir_all(&edits_dir).unwrap();
    let edit_path = edits_dir.join("20260103-replace-timeout.md");
    std::fs::write(&edit_path, format!("{frontmatter}\n# Body\n")).unwrap();
    std::fs::write(
        project
            .path()
            .join(".brainyard/agents/update-agent/INDEX.md"),
        "",
    )
    .unwrap();

    let assert = Command::cargo_bin("by-rs")
        .unwrap()
        .args(["update", "index-append-preview", "--path"])
        .arg(&edit_path)
        .args([
            "--slug",
            "replace-timeout",
            "--mode",
            ":pattern",
            "--target",
            "src/main.rs",
            "--summary",
            "Updated\n timeout",
            "--ok",
            "false",
            "--created",
            "2026-01-03 04:05",
            "--base-dir",
        ])
        .arg(project.path())
        .assert()
        .success()
        .stderr(predicate::str::is_empty());
    let stdout = String::from_utf8(assert.get_output().stdout.clone()).unwrap();
    let preview: serde_json::Value = serde_json::from_str(&stdout).unwrap();
    assert_eq!(
        preview["projection"],
        "common.update/update$index-append-preview"
    );
    assert_eq!(preview["write-skipped?"], true);
    assert_eq!(preview["existing?"], true);
    assert_eq!(preview["mode"], "pattern");
    assert_eq!(preview["target"], "main.rs");
    assert_eq!(preview["ok?"], false);
    assert_eq!(
        preview["line"],
        "- 2026-01-03 04:05 [replace-timeout](edits/20260103-replace-timeout.md) — pattern · `main.rs` · Updated timeout · ❌\n"
    );

    let assert = Command::cargo_bin("by-rs")
        .unwrap()
        .args(["update", "read-record", "--path"])
        .arg(&edit_path)
        .assert()
        .success()
        .stderr(predicate::str::is_empty());
    let stdout = String::from_utf8(assert.get_output().stdout.clone()).unwrap();
    let parsed: serde_json::Value = serde_json::from_str(&stdout).unwrap();
    assert_eq!(parsed["projection"], "common.update/update$read-record");
    assert_eq!(parsed["slug"], "replace-timeout");
    assert_eq!(parsed["request"], "Replace timeout");
    assert_eq!(parsed["pre"]["match_count"], 1);
    assert_eq!(parsed["apply"]["replaced"], 1);
    assert_eq!(parsed["verify"]["diff_match"], true);
    assert_eq!(parsed["summary"], "Updated timeout");
    assert_eq!(parsed["ok"], true);

    let assert = Command::cargo_bin("by-rs")
        .unwrap()
        .args(["update", "find", "--query", "timeout", "--base-dir"])
        .arg(project.path())
        .assert()
        .success()
        .stderr(predicate::str::is_empty());
    let stdout = String::from_utf8(assert.get_output().stdout.clone()).unwrap();
    let found: serde_json::Value = serde_json::from_str(&stdout).unwrap();
    assert_eq!(found["projection"], "common.update/update$find");
    assert_eq!(found["n-matches"], 1);
    assert_eq!(
        found["matches"][0]["path"],
        ".brainyard/agents/update-agent/edits/20260103-replace-timeout.md"
    );
    assert_eq!(found["matches"][0]["ok?"], true);
}

#[test]
fn workflow_and_research_helpers_are_live_free_json_projections() {
    let assert = Command::cargo_bin("by-rs")
        .unwrap()
        .args([
            "workflow",
            "id",
            "--template",
            "feature-launch",
            "--question",
            "Can we ship the Rust port?",
            "--max-chars",
            "40",
        ])
        .assert()
        .success()
        .stderr(predicate::str::is_empty());
    let stdout = String::from_utf8(assert.get_output().stdout.clone()).unwrap();
    let workflow_id: serde_json::Value = serde_json::from_str(&stdout).unwrap();
    assert_eq!(workflow_id["slug"], "feature-launch--rust-port");

    let assert = Command::cargo_bin("by-rs")
        .unwrap()
        .args([
            "research",
            "id",
            "--question",
            "How can we port Clojure to Rust?",
            "--max-chars",
            "40",
        ])
        .assert()
        .success()
        .stderr(predicate::str::is_empty());
    let stdout = String::from_utf8(assert.get_output().stdout.clone()).unwrap();
    let research_id: serde_json::Value = serde_json::from_str(&stdout).unwrap();
    assert_eq!(research_id["slug"], "port-clojure-rust");

    let project = tempfile::tempdir().unwrap();
    let user = tempfile::tempdir().unwrap();
    let workflows_dir = project.path().join(".brainyard/workflows");
    std::fs::create_dir_all(&workflows_dir).unwrap();
    std::fs::write(
        workflows_dir.join("project-flow.edn"),
        r#"{:workflow/id :project-flow
 :workflow/name "Project Flow"
 :workflow/description "Project local"
 :acceptance [{:id :a1 :text "Project accepted"}]
 :stages [{:id :scope :recommended-agent :research-agent}]}"#,
    )
    .unwrap();

    let assert = Command::cargo_bin("by-rs")
        .unwrap()
        .env("HOME", user.path())
        .args(["workflow", "list-templates", "--base-dir"])
        .arg(project.path())
        .assert()
        .success()
        .stderr(predicate::str::is_empty());
    let stdout = String::from_utf8(assert.get_output().stdout.clone()).unwrap();
    let listed: serde_json::Value = serde_json::from_str(&stdout).unwrap();
    let ids = listed["templates"]
        .as_array()
        .unwrap()
        .iter()
        .map(|item| item["id"].as_str().unwrap())
        .collect::<Vec<_>>();
    assert!(ids.contains(&"project-flow"));
    assert!(ids.contains(&"feature-launch"));

    let assert = Command::cargo_bin("by-rs")
        .unwrap()
        .env("HOME", user.path())
        .args([
            "workflow",
            "load-template",
            "--id",
            "project-flow",
            "--base-dir",
        ])
        .arg(project.path())
        .assert()
        .success()
        .stderr(predicate::str::is_empty());
    let stdout = String::from_utf8(assert.get_output().stdout.clone()).unwrap();
    let project_template: serde_json::Value = serde_json::from_str(&stdout).unwrap();
    assert_eq!(project_template["source"], "project");
    assert_eq!(project_template["template"]["workflow/id"], "project-flow");
    assert_eq!(project_template["template"]["stages"][0]["id"], "scope");

    let assert = Command::cargo_bin("by-rs")
        .unwrap()
        .env("HOME", user.path())
        .args([
            "workflow",
            "load-template",
            "--id",
            "doc-update",
            "--base-dir",
        ])
        .arg(project.path())
        .assert()
        .success()
        .stderr(predicate::str::is_empty());
    let stdout = String::from_utf8(assert.get_output().stdout.clone()).unwrap();
    let builtin_template: serde_json::Value = serde_json::from_str(&stdout).unwrap();
    assert_eq!(builtin_template["source"], "built-in");
    assert_eq!(
        builtin_template["template"]["workflow/name"],
        "Documentation Update Workflow"
    );

    std::fs::write(workflows_dir.join("feature-launch.edn"), "{}").unwrap();
    let assert = Command::cargo_bin("by-rs")
        .unwrap()
        .args(["workflow", "install-starters-preview", "--base-dir"])
        .arg(project.path())
        .assert()
        .success()
        .stderr(predicate::str::is_empty());
    let stdout = String::from_utf8(assert.get_output().stdout.clone()).unwrap();
    let starters: serde_json::Value = serde_json::from_str(&stdout).unwrap();
    assert_eq!(
        starters["projection"],
        "common.workflow/workflow$install-starters-preview"
    );
    assert_eq!(starters["write-skipped?"], true);
    assert_eq!(starters["installed"][0], "doc-update");
    assert_eq!(starters["skipped"][0], "feature-launch");
    let feature_launch = starters["templates"]
        .as_array()
        .unwrap()
        .iter()
        .find(|template| template["id"] == "feature-launch")
        .unwrap();
    assert_eq!(feature_launch["exists?"], true);
    assert_eq!(feature_launch["action"], "skip");

    let assert = Command::cargo_bin("by-rs")
        .unwrap()
        .args([
            "workflow",
            "bootstrap-preview",
            "--id",
            "feature-launch--bootstrap-preview",
            "--purpose",
            "Preview workflow\nwith folded purpose",
            "--acceptance",
            r#"[{"id":"a1","text":"Ship preview","status":"open"}]"#,
            "--stages",
            r#"[{"id":"scope","recommended-agent":"research-agent"},{"id":"implement","agent":"exec-agent","status":"in-progress"}]"#,
            "--template-id",
            "feature-launch",
            "--hitl-mode",
            "gates",
            "--created",
            "2026-01-05T00:00:00Z",
            "--base-dir",
        ])
        .arg(project.path())
        .assert()
        .success()
        .stderr(predicate::str::is_empty());
    let stdout = String::from_utf8(assert.get_output().stdout.clone()).unwrap();
    let workflow_bootstrap: serde_json::Value = serde_json::from_str(&stdout).unwrap();
    assert_eq!(
        workflow_bootstrap["projection"],
        "common.workflow/workflow$bootstrap-preview"
    );
    assert_eq!(workflow_bootstrap["write-skipped?"], true);
    assert_eq!(workflow_bootstrap["exists?"], false);
    assert_eq!(workflow_bootstrap["template-id"], "feature-launch");
    assert_eq!(workflow_bootstrap["hitl-mode"], "gates");
    assert_eq!(workflow_bootstrap["stages"][0]["agent"], "research-agent");
    assert_eq!(workflow_bootstrap["stages"][0]["status"], "pending");
    assert_eq!(workflow_bootstrap["stages"][0]["attempts"], 0);
    assert_eq!(
        workflow_bootstrap["files"]["purpose.md"],
        "Preview workflow\nwith folded purpose\n"
    );
    assert_eq!(workflow_bootstrap["files"]["findings.log"], "");
    let workflow_bootstrap_acceptance = workflow_bootstrap["files"]["acceptance.md"]
        .as_str()
        .unwrap();
    assert!(workflow_bootstrap_acceptance.contains("# Acceptance criteria (workflow-level)\n\n"));
    assert!(workflow_bootstrap_acceptance.contains("- **a1** [open]: Ship preview\n"));
    let workflow_bootstrap_dossier = workflow_bootstrap["files"]["dossier.md"].as_str().unwrap();
    assert!(workflow_bootstrap_dossier.contains("workflow_id: feature-launch--bootstrap-preview\n"));
    assert!(workflow_bootstrap_dossier.contains("workflow_template: feature-launch\n"));
    assert!(workflow_bootstrap_dossier.contains("created: 2026-01-05T00:00:00Z\n"));
    assert!(workflow_bootstrap_dossier.contains("hitl_mode: gates\n"));
    assert!(
        workflow_bootstrap_dossier.contains("purpose: >\n  Preview workflow with folded purpose\n")
    );
    assert!(workflow_bootstrap_dossier.contains("stages_roster: stages.edn\n"));
    let workflow_bootstrap_stages = workflow_bootstrap["files"]["stages.edn"].as_str().unwrap();
    assert!(
        workflow_bootstrap_stages.contains(":workflow/id \"feature-launch--bootstrap-preview\"")
    );
    assert!(workflow_bootstrap_stages.contains(":template-id :feature-launch"));
    assert!(workflow_bootstrap_stages.contains(":status :pending"));
    assert!(workflow_bootstrap_stages.contains(":attempts 0"));
    assert!(workflow_bootstrap_stages.contains(":agent :research-agent"));
    assert!(workflow_bootstrap["files"]["template.edn"]
        .as_str()
        .unwrap()
        .contains(":template :ad-hoc"));

    let workflow_dir = project
        .path()
        .join(".brainyard/agents/workflow-agent/feature-launch--rust-port");
    std::fs::create_dir_all(&workflow_dir).unwrap();
    std::fs::write(
        workflow_dir.join("dossier.md"),
        r#"---
workflow_id: feature-launch--rust-port
workflow_template: feature-launch
created: 2026-01-01T00:00:00Z
last_iteration: 3
status: in-progress
hitl_mode: gates
purpose: >
  Port Rust
acceptance:
  - id: a1
    text: "One"
    status: satisfied
  - id: a2
    text: "Two"
    status: open
stages:
  - id: implement
    agent: exec-agent
    status: in-progress
---
"#,
    )
    .unwrap();
    std::fs::write(
        workflow_dir.join("stages.edn"),
        r#"{:workflow/id "feature-launch--rust-port"
 :stages [{:id :research-feasibility :status :satisfied}
          {:id :implement :status :in-progress}
          {:id "announce" :status :skipped}
          {:id :verify}]}"#,
    )
    .unwrap();

    let assert = Command::cargo_bin("by-rs")
        .unwrap()
        .args([
            "workflow",
            "resume",
            "--id",
            "feature-launch--rust-port",
            "--base-dir",
        ])
        .arg(project.path())
        .assert()
        .success()
        .stderr(predicate::str::is_empty());
    let stdout = String::from_utf8(assert.get_output().stdout.clone()).unwrap();
    let workflow_resume: serde_json::Value = serde_json::from_str(&stdout).unwrap();
    assert_eq!(workflow_resume["exists?"], true);
    assert_eq!(workflow_resume["last-iteration"], 3);
    assert_eq!(workflow_resume["acceptance-state"]["a1"], "satisfied");
    assert_eq!(workflow_resume["acceptance-state"]["a2"], "open");
    assert_eq!(workflow_resume["pending-stages"][0], "implement");
    assert_eq!(workflow_resume["pending-stages"][1], "verify");
    assert_eq!(workflow_resume["stage-count"], 4);
    assert_eq!(workflow_resume["n-pending"], 2);

    let assert = Command::cargo_bin("by-rs")
        .unwrap()
        .args([
            "workflow",
            "update-stage-preview",
            "--id",
            "feature-launch--rust-port",
            "--stage-id",
            "implement",
            "--status",
            "satisfied",
            "--artifact",
            "artifacts/implement.md",
            "--plan-slug",
            "p1",
            "--todo-slug",
            "t1",
            "--item-progress",
            "3/5",
            "--gate",
            r#"{:status :approved :at "2026-01-03T00:00:00Z"}"#,
            "--completed-at",
            "2026-01-03T00:01:00Z",
            "--base-dir",
        ])
        .arg(project.path())
        .assert()
        .success()
        .stderr(predicate::str::is_empty());
    let stdout = String::from_utf8(assert.get_output().stdout.clone()).unwrap();
    let workflow_stage: serde_json::Value = serde_json::from_str(&stdout).unwrap();
    assert_eq!(
        workflow_stage["projection"],
        "common.workflow/workflow$update-stage-preview"
    );
    assert_eq!(workflow_stage["write-skipped?"], true);
    assert_eq!(workflow_stage["updated?"], true);
    assert_eq!(workflow_stage["stage-id"], "implement");
    assert_eq!(workflow_stage["from"], "in-progress");
    assert_eq!(workflow_stage["to"], "satisfied");
    assert_eq!(workflow_stage["attempts"], 1);
    assert_eq!(workflow_stage["stage"]["status"], "satisfied");
    assert_eq!(workflow_stage["stage"]["attempts"], 1);
    assert_eq!(
        workflow_stage["stage"]["artifact"],
        "artifacts/implement.md"
    );
    assert_eq!(workflow_stage["stage"]["plan-slug"], "p1");
    assert_eq!(workflow_stage["stage"]["todo-slug"], "t1");
    assert_eq!(workflow_stage["stage"]["item-progress"], "3/5");
    assert_eq!(workflow_stage["stage"]["gate"]["status"], "approved");
    assert_eq!(
        workflow_stage["stage"]["completed-at"],
        "2026-01-03T00:01:00Z"
    );
    let workflow_stage_content = workflow_stage["content"].as_str().unwrap();
    assert!(workflow_stage_content.contains(":status :satisfied"));
    assert!(workflow_stage_content.contains(":attempts 1"));
    assert!(workflow_stage_content.contains(":artifact \"artifacts/implement.md\""));
    assert!(workflow_stage_content.contains(":completed-at \"2026-01-03T00:01:00Z\""));

    let assert = Command::cargo_bin("by-rs")
        .unwrap()
        .args([
            "workflow",
            "update-acceptance-preview",
            "--id",
            "feature-launch--rust-port",
            "--criterion-id",
            "a2",
            "--status",
            "satisfied",
            "--base-dir",
        ])
        .arg(project.path())
        .assert()
        .success()
        .stderr(predicate::str::is_empty());
    let stdout = String::from_utf8(assert.get_output().stdout.clone()).unwrap();
    let workflow_acceptance: serde_json::Value = serde_json::from_str(&stdout).unwrap();
    assert_eq!(
        workflow_acceptance["projection"],
        "common.workflow/workflow$update-acceptance-preview"
    );
    assert_eq!(workflow_acceptance["write-skipped?"], true);
    assert_eq!(workflow_acceptance["criterion-id"], "a2");
    assert_eq!(workflow_acceptance["from"], "open");
    assert_eq!(workflow_acceptance["to"], "satisfied");
    assert!(workflow_acceptance["content"]
        .as_str()
        .unwrap()
        .contains("status: satisfied"));

    std::fs::write(workflow_dir.join("findings.log"), "").unwrap();
    let assert = Command::cargo_bin("by-rs")
        .unwrap()
        .args([
            "workflow",
            "append-log-preview",
            "--id",
            "feature-launch--rust-port",
            "--iter",
            "4",
            "--stage",
            ":implement",
            "--agent",
            "exec-agent",
            "--action",
            "gate",
            "--summary",
            "Built step",
            "--pointers",
            r#"{:plan_slug "p1" :items_done ["a"]}"#,
            "--base-dir",
        ])
        .arg(project.path())
        .assert()
        .success()
        .stderr(predicate::str::is_empty());
    let stdout = String::from_utf8(assert.get_output().stdout.clone()).unwrap();
    let workflow_log: serde_json::Value = serde_json::from_str(&stdout).unwrap();
    assert_eq!(
        workflow_log["projection"],
        "common.workflow/workflow$append-log-preview"
    );
    assert_eq!(workflow_log["write-skipped?"], true);
    assert_eq!(workflow_log["existing?"], true);
    assert_eq!(workflow_log["entry"]["iter"], 4);
    assert_eq!(workflow_log["entry"]["stage"], "implement");
    assert_eq!(workflow_log["entry"]["agent"], "exec-agent");
    assert_eq!(workflow_log["entry"]["action"], "gate");
    assert_eq!(workflow_log["entry"]["plan_slug"], "p1");
    assert_eq!(workflow_log["entry"]["items_done"][0], "a");
    let workflow_log_line: serde_json::Value =
        serde_json::from_str(workflow_log["line"].as_str().unwrap()).unwrap();
    assert_eq!(workflow_log_line["summary"], "Built step");
    assert_eq!(workflow_log_line["stage"], "implement");
    assert_eq!(workflow_log_line["plan_slug"], "p1");

    let assert = Command::cargo_bin("by-rs")
        .unwrap()
        .args([
            "workflow",
            "write-verdict-preview",
            "--id",
            "feature-launch--rust-port",
            "--status",
            "partial",
            "--terminated",
            "2026-01-03T00:00:00Z",
            "--narrative",
            "## Workflow Verdict\nPartial ship",
            "--base-dir",
        ])
        .arg(project.path())
        .assert()
        .success()
        .stderr(predicate::str::is_empty());
    let stdout = String::from_utf8(assert.get_output().stdout.clone()).unwrap();
    let workflow_verdict: serde_json::Value = serde_json::from_str(&stdout).unwrap();
    assert_eq!(
        workflow_verdict["projection"],
        "common.workflow/workflow$write-verdict-preview"
    );
    assert_eq!(workflow_verdict["write-skipped?"], true);
    assert_eq!(workflow_verdict["status"], "partial");
    assert_eq!(workflow_verdict["acceptance-outcome"]["a1"], "satisfied");
    assert_eq!(workflow_verdict["acceptance-outcome"]["a2"], "open");
    assert_eq!(workflow_verdict["stage-outcomes"][1][0], "implement");
    assert_eq!(workflow_verdict["stage-outcomes"][1][1], "in-progress");
    let workflow_verdict_content = workflow_verdict["content"].as_str().unwrap();
    assert!(workflow_verdict_content.contains("workflow_id: feature-launch--rust-port\n"));
    assert!(workflow_verdict_content.contains("workflow_template: feature-launch\n"));
    assert!(workflow_verdict_content.contains("iterations: 3\n"));
    assert!(workflow_verdict_content.contains("acceptance_outcome:\n"));
    assert!(workflow_verdict_content.contains("  a2: open\n"));
    assert!(workflow_verdict_content.contains("stage_outcomes:\n"));
    assert!(workflow_verdict_content.contains("  implement: in-progress\n"));
    assert!(workflow_verdict_content.contains("## Verdict\nPartial ship\n"));

    let assert = Command::cargo_bin("by-rs")
        .unwrap()
        .args([
            "workflow",
            "write-verdict-preview",
            "--id",
            "feature-launch--rust-port",
            "--status",
            "in-progress",
            "--terminated",
            "",
            "--narrative",
            "Still running",
            "--base-dir",
        ])
        .arg(project.path())
        .assert()
        .success()
        .stderr(predicate::str::is_empty());
    let stdout = String::from_utf8(assert.get_output().stdout.clone()).unwrap();
    let workflow_in_progress_verdict: serde_json::Value = serde_json::from_str(&stdout).unwrap();
    assert_eq!(workflow_in_progress_verdict["status"], "in-progress");
    assert!(workflow_in_progress_verdict["content"]
        .as_str()
        .unwrap()
        .contains("status: in-progress\n"));

    let assert = Command::cargo_bin("by-rs")
        .unwrap()
        .args([
            "workflow",
            "index-append-preview",
            "--id",
            "feature-launch--rust-port",
            "--status",
            "partial",
            "--one-line",
            "  Rust   port   partially ready  ",
            "--created",
            "2026-01-03 12:34",
            "--base-dir",
        ])
        .arg(project.path())
        .assert()
        .success()
        .stderr(predicate::str::is_empty());
    let stdout = String::from_utf8(assert.get_output().stdout.clone()).unwrap();
    let workflow_index: serde_json::Value = serde_json::from_str(&stdout).unwrap();
    assert_eq!(
        workflow_index["projection"],
        "common.workflow/workflow$index-append-preview"
    );
    assert_eq!(workflow_index["write-skipped?"], true);
    assert_eq!(
        workflow_index["line"],
        "- 2026-01-03 12:34 [feature-launch--rust-port](feature-launch--rust-port/) — partial · Rust port partially ready\n"
    );

    let assert = Command::cargo_bin("by-rs")
        .unwrap()
        .args([
            "workflow",
            "index-append-preview",
            "--id",
            "feature-launch--rust-port",
            "--status",
            "in-progress",
            "--one-line",
            "Rust port still running",
            "--created",
            "2026-01-03 12:35",
            "--base-dir",
        ])
        .arg(project.path())
        .assert()
        .success()
        .stderr(predicate::str::is_empty());
    let stdout = String::from_utf8(assert.get_output().stdout.clone()).unwrap();
    let workflow_in_progress_index: serde_json::Value = serde_json::from_str(&stdout).unwrap();
    assert_eq!(
        workflow_in_progress_index["line"],
        "- 2026-01-03 12:35 [feature-launch--rust-port](feature-launch--rust-port/) — in-progress · Rust port still running\n"
    );

    let assert = Command::cargo_bin("by-rs")
        .unwrap()
        .args([
            "research",
            "bootstrap-preview",
            "--id",
            "research-bootstrap-preview",
            "--purpose",
            "Preview research\nwith folded purpose",
            "--acceptance",
            r#"[{"id":"a1","text":"Check parity","status":"open"}]"#,
            "--direction",
            r#"["Check parity","Capture risks"]"#,
            "--created",
            "2026-01-05T00:00:00Z",
            "--base-dir",
        ])
        .arg(project.path())
        .assert()
        .success()
        .stderr(predicate::str::is_empty());
    let stdout = String::from_utf8(assert.get_output().stdout.clone()).unwrap();
    let research_bootstrap: serde_json::Value = serde_json::from_str(&stdout).unwrap();
    assert_eq!(
        research_bootstrap["projection"],
        "common.research/research$bootstrap-preview"
    );
    assert_eq!(research_bootstrap["write-skipped?"], true);
    assert_eq!(research_bootstrap["exists?"], false);
    assert_eq!(research_bootstrap["direction"][0], "Check parity");
    assert_eq!(
        research_bootstrap["files"]["purpose.md"],
        "Preview research\nwith folded purpose\n"
    );
    assert_eq!(research_bootstrap["files"]["findings.log"], "");
    assert!(research_bootstrap["files"]["acceptance.md"]
        .as_str()
        .unwrap()
        .contains("- **a1** [open]: Check parity\n"));
    assert!(research_bootstrap["files"]["direction.md"]
        .as_str()
        .unwrap()
        .contains("- Capture risks\n"));
    let research_bootstrap_dossier = research_bootstrap["files"]["dossier.md"].as_str().unwrap();
    assert!(research_bootstrap_dossier.contains("research_id: research-bootstrap-preview\n"));
    assert!(research_bootstrap_dossier.contains("created: 2026-01-05T00:00:00Z\n"));
    assert!(
        research_bootstrap_dossier.contains("purpose: >\n  Preview research with folded purpose\n")
    );
    assert!(research_bootstrap_dossier.contains("direction:\n"));
    assert!(research_bootstrap_dossier.contains("  - \"Capture risks\"\n"));
    assert!(research_bootstrap_dossier.contains("calls_log: findings.log\n"));

    let research_dir = project
        .path()
        .join(".brainyard/agents/research-agent/port-clojure-rust");
    std::fs::create_dir_all(&research_dir).unwrap();
    std::fs::write(
        research_dir.join("dossier.md"),
        r#"---
research_id: port-clojure-rust
created: 2026-01-02T00:00:00Z
last_iteration: 2
status: partial
purpose: >
  Port Clojure to Rust
acceptance:
  - id: a1
    text: "Compatibility assessed"
    status: partial
direction:
  - "Check parity"
artifacts:
  exploration: []
  plan_slug: null
  todo_slug: null
  evals: []
calls_log: findings.log
---
"#,
    )
    .unwrap();
    std::fs::write(research_dir.join("findings.log"), "").unwrap();
    let assert = Command::cargo_bin("by-rs")
        .unwrap()
        .args([
            "research",
            "resume",
            "--id",
            "port-clojure-rust",
            "--base-dir",
        ])
        .arg(project.path())
        .assert()
        .success()
        .stderr(predicate::str::is_empty());
    let stdout = String::from_utf8(assert.get_output().stdout.clone()).unwrap();
    let research_resume: serde_json::Value = serde_json::from_str(&stdout).unwrap();
    assert_eq!(research_resume["exists?"], true);
    assert_eq!(research_resume["status"], "partial");
    assert_eq!(research_resume["last-iteration"], 2);
    assert_eq!(research_resume["acceptance-state"]["a1"], "partial");

    let assert = Command::cargo_bin("by-rs")
        .unwrap()
        .args([
            "research",
            "append-log-preview",
            "--id",
            "port-clojure-rust",
            "--iter",
            "3",
            "--agent",
            "research-agent",
            "--summary",
            "Checked parity",
            "--pointers",
            r#"{:exploration_slug "exp1" :items_done ["a"]}"#,
            "--base-dir",
        ])
        .arg(project.path())
        .assert()
        .success()
        .stderr(predicate::str::is_empty());
    let stdout = String::from_utf8(assert.get_output().stdout.clone()).unwrap();
    let research_log: serde_json::Value = serde_json::from_str(&stdout).unwrap();
    assert_eq!(
        research_log["projection"],
        "common.research/research$append-log-preview"
    );
    assert_eq!(research_log["write-skipped?"], true);
    assert_eq!(research_log["entry"]["iter"], 3);
    assert_eq!(research_log["entry"]["agent"], "research-agent");
    assert_eq!(research_log["entry"]["exploration_slug"], "exp1");
    let research_log_line: serde_json::Value =
        serde_json::from_str(research_log["line"].as_str().unwrap()).unwrap();
    assert_eq!(research_log_line["summary"], "Checked parity");
    assert_eq!(research_log_line["exploration_slug"], "exp1");

    let assert = Command::cargo_bin("by-rs")
        .unwrap()
        .args([
            "research",
            "update-status-preview",
            "--id",
            "port-clojure-rust",
            "--criterion-id",
            "a1",
            "--status",
            "satisfied",
            "--base-dir",
        ])
        .arg(project.path())
        .assert()
        .success()
        .stderr(predicate::str::is_empty());
    let stdout = String::from_utf8(assert.get_output().stdout.clone()).unwrap();
    let research_status: serde_json::Value = serde_json::from_str(&stdout).unwrap();
    assert_eq!(
        research_status["projection"],
        "common.research/research$update-status-preview"
    );
    assert_eq!(research_status["write-skipped?"], true);
    assert_eq!(research_status["criterion-id"], "a1");
    assert_eq!(research_status["from"], "partial");
    assert_eq!(research_status["to"], "satisfied");
    assert!(research_status["content"]
        .as_str()
        .unwrap()
        .contains("status: satisfied"));

    let assert = Command::cargo_bin("by-rs")
        .unwrap()
        .args([
            "research",
            "write-verdict-preview",
            "--id",
            "port-clojure-rust",
            "--status",
            "partial",
            "--terminated",
            "2026-01-04T00:00:00Z",
            "--narrative",
            "# Verdict\nPartial",
            "--base-dir",
        ])
        .arg(project.path())
        .assert()
        .success()
        .stderr(predicate::str::is_empty());
    let stdout = String::from_utf8(assert.get_output().stdout.clone()).unwrap();
    let research_verdict: serde_json::Value = serde_json::from_str(&stdout).unwrap();
    assert_eq!(
        research_verdict["projection"],
        "common.research/research$write-verdict-preview"
    );
    assert_eq!(research_verdict["write-skipped?"], true);
    assert_eq!(research_verdict["status"], "partial");
    assert_eq!(research_verdict["acceptance-outcome"]["a1"], "partial");
    let research_verdict_content = research_verdict["content"].as_str().unwrap();
    assert!(research_verdict_content.contains("research_id: port-clojure-rust\n"));
    assert!(research_verdict_content.contains("iterations: 2\n"));
    assert!(research_verdict_content.contains("acceptance_outcome:\n"));
    assert!(research_verdict_content.contains("  a1: partial\n"));
    assert!(research_verdict_content.contains("## Verdict\nPartial\n"));

    let assert = Command::cargo_bin("by-rs")
        .unwrap()
        .args([
            "research",
            "index-append-preview",
            "--id",
            "port-clojure-rust",
            "--status",
            "partial",
            "--one-line",
            "  Rust   port   partially ready  ",
            "--created",
            "2026-01-04 12:34",
            "--base-dir",
        ])
        .arg(project.path())
        .assert()
        .success()
        .stderr(predicate::str::is_empty());
    let stdout = String::from_utf8(assert.get_output().stdout.clone()).unwrap();
    let research_index: serde_json::Value = serde_json::from_str(&stdout).unwrap();
    assert_eq!(
        research_index["projection"],
        "common.research/research$index-append-preview"
    );
    assert_eq!(research_index["write-skipped?"], true);
    assert_eq!(
        research_index["line"],
        "- 2026-01-04 12:34 [port-clojure-rust](port-clojure-rust/) — partial · Rust port partially ready\n"
    );
}

#[test]
fn chart_guard_and_main_helpers_are_live_free_json_projections() {
    let assert = Command::cargo_bin("by-rs")
        .unwrap()
        .args([
            "chart",
            "recommend-type",
            "--description",
            "Revenue trend over time by month",
        ])
        .assert()
        .success()
        .stderr(predicate::str::is_empty());
    let stdout = String::from_utf8(assert.get_output().stdout.clone()).unwrap();
    let chart: serde_json::Value = serde_json::from_str(&stdout).unwrap();
    assert_eq!(chart["recommended"], "line");
    assert_eq!(
        chart["reason"],
        "Time-series data is best shown as a line chart"
    );

    let assert = Command::cargo_bin("by-rs")
        .unwrap()
        .args([
            "chart",
            "recommend-type",
            "--description",
            "Share breakdown by plan",
        ])
        .assert()
        .success()
        .stderr(predicate::str::is_empty());
    let stdout = String::from_utf8(assert.get_output().stdout.clone()).unwrap();
    let chart: serde_json::Value = serde_json::from_str(&stdout).unwrap();
    assert_eq!(chart["recommended"], "pie");

    let assert = Command::cargo_bin("by-rs")
        .unwrap()
        .args([
            "chart",
            "create-plotly",
            "--data",
            r#"[{"type":"bar","x":["A"],"y":[3]}]"#,
            "--layout",
            r#"{"showlegend":false}"#,
            "--title",
            "Revenue",
        ])
        .assert()
        .success()
        .stderr(predicate::str::is_empty());
    let stdout = String::from_utf8(assert.get_output().stdout.clone()).unwrap();
    let plotly: serde_json::Value = serde_json::from_str(&stdout).unwrap();
    assert_eq!(plotly["projection"], "chart-command/create-plotly");
    assert_eq!(plotly["artifact-type"], "chart");
    assert_eq!(plotly["artifact-data"]["format"], "plotly");
    assert_eq!(
        plotly["artifact-data"]["chart-spec"]["data"][0]["type"],
        "bar"
    );
    assert_eq!(
        plotly["artifact-data"]["chart-spec"]["layout"]["template"],
        "plotly_dark"
    );
    assert_eq!(
        plotly["artifact-data"]["chart-spec"]["layout"]["title"],
        "Revenue"
    );
    assert_eq!(plotly["summary"], "Created Revenue with 1 trace(s)");
    assert_eq!(plotly["render-skipped?"], true);
    assert_eq!(plotly["write-skipped?"], true);

    let assert = Command::cargo_bin("by-rs")
        .unwrap()
        .args([
            "chart",
            "export-html-preview",
            "--chart-spec",
            r#"{:data [{:type "bar" :x ["A"] :y [3]}] :layout {:title "Revenue"}}"#,
            "--filepath",
            "/tmp/revenue.html",
        ])
        .assert()
        .success()
        .stderr(predicate::str::is_empty());
    let stdout = String::from_utf8(assert.get_output().stdout.clone()).unwrap();
    let html: serde_json::Value = serde_json::from_str(&stdout).unwrap();
    assert_eq!(html["projection"], "chart-command/export-html-preview");
    assert_eq!(html["success"], true);
    assert_eq!(html["filepath"], "/tmp/revenue.html");
    assert_eq!(html["write-skipped?"], true);
    assert!(html["html"].as_str().unwrap().contains("Plotly.newPlot"));
    assert!(html["html"].as_str().unwrap().contains("plotly-2.32.0"));
    assert_eq!(html["size"], html["html"].as_str().unwrap().len());

    let assert = Command::cargo_bin("by-rs")
        .unwrap()
        .args([
            "guard",
            "scan-secrets",
            "--text",
            "inline sk-abcdefghijklmnopqrstuvwxyz value",
        ])
        .assert()
        .success()
        .stderr(predicate::str::is_empty());
    let stdout = String::from_utf8(assert.get_output().stdout.clone()).unwrap();
    let secrets: serde_json::Value = serde_json::from_str(&stdout).unwrap();
    assert_eq!(secrets["n-matches"], 1);
    assert_eq!(secrets["matches"][0], "sk-abcde…");

    let assert = Command::cargo_bin("by-rs")
        .unwrap()
        .args(["guard", "content-violation", "--content", "clean"])
        .assert()
        .success()
        .stderr(predicate::str::is_empty());
    let stdout = String::from_utf8(assert.get_output().stdout.clone()).unwrap();
    let ok: serde_json::Value = serde_json::from_str(&stdout).unwrap();
    assert_eq!(ok["ok?"], true);
    assert!(ok["violation"].is_null());

    let assert = Command::cargo_bin("by-rs")
        .unwrap()
        .args([
            "guard",
            "content-violation",
            "--content",
            "clean",
            "--cap",
            "3",
        ])
        .assert()
        .success()
        .stderr(predicate::str::is_empty());
    let stdout = String::from_utf8(assert.get_output().stdout.clone()).unwrap();
    let budget: serde_json::Value = serde_json::from_str(&stdout).unwrap();
    assert_eq!(budget["ok?"], false);
    assert_eq!(budget["violation"]["stage"], "budget-exceeded");
    assert_eq!(budget["violation"]["size"], 5);
    assert_eq!(budget["violation"]["cap"], 3);

    let assert = Command::cargo_bin("by-rs")
        .unwrap()
        .args([
            "guard",
            "content-violation",
            "--content",
            "inline sk-abcdefghijklmnopqrstuvwxyz value",
        ])
        .assert()
        .success()
        .stderr(predicate::str::is_empty());
    let stdout = String::from_utf8(assert.get_output().stdout.clone()).unwrap();
    let secret_violation: serde_json::Value = serde_json::from_str(&stdout).unwrap();
    assert_eq!(secret_violation["ok?"], false);
    assert_eq!(secret_violation["violation"]["stage"], "secret-detected");
    assert_eq!(secret_violation["violation"]["matches"][0], "sk-abcde…");

    let project = tempfile::tempdir().unwrap();
    let routing_dir = project
        .path()
        .join(".brainyard/agents/main-agent/session-1");
    std::fs::create_dir_all(&routing_dir).unwrap();
    std::fs::write(
        routing_dir.join("routing.log"),
        format!(
            "{}\n{}\n",
            serde_json::json!({
                "turn": 1,
                "iter": 1,
                "question": "Q1",
                "shape": "explore",
                "routed-to": "explore-agent",
                "artifact": "results/a.md",
                "reason": "Need repo facts."
            }),
            serde_json::json!({
                "turn": 2,
                "iter": 1,
                "question": "Q2",
                "shape": "direct-answer",
                "routed-to": serde_json::Value::Null,
                "artifact": serde_json::Value::Null,
                "reason": "Answer from context."
            })
        ),
    )
    .unwrap();

    let assert = Command::cargo_bin("by-rs")
        .unwrap()
        .args(["main", "resume", "--session-id", "session-1", "--base-dir"])
        .arg(project.path())
        .assert()
        .success()
        .stderr(predicate::str::is_empty());
    let stdout = String::from_utf8(assert.get_output().stdout.clone()).unwrap();
    let resume: serde_json::Value = serde_json::from_str(&stdout).unwrap();
    assert_eq!(resume["exists?"], true);
    assert_eq!(resume["line-count"], 2);
    assert_eq!(resume["turn-count"], 2);
    assert_eq!(resume["last-shape"], "direct-answer");
    assert!(resume["last-artifact"].is_null());

    let assert = Command::cargo_bin("by-rs")
        .unwrap()
        .args([
            "main",
            "last-shape",
            "--session-id",
            "session-1",
            "--base-dir",
        ])
        .arg(project.path())
        .assert()
        .success()
        .stderr(predicate::str::is_empty());
    let stdout = String::from_utf8(assert.get_output().stdout.clone()).unwrap();
    let last_shape: serde_json::Value = serde_json::from_str(&stdout).unwrap();
    assert_eq!(last_shape["exists?"], true);
    assert_eq!(last_shape["shape"], "direct-answer");
    assert_eq!(last_shape["question"], "Q2");
    assert!(last_shape["artifact"].is_null());

    let assert = Command::cargo_bin("by-rs")
        .unwrap()
        .args([
            "main",
            "read-routing-log",
            "--session-id",
            "session-1",
            "--base-dir",
        ])
        .arg(project.path())
        .assert()
        .success()
        .stderr(predicate::str::is_empty());
    let stdout = String::from_utf8(assert.get_output().stdout.clone()).unwrap();
    let log: serde_json::Value = serde_json::from_str(&stdout).unwrap();
    assert_eq!(log["count"], 2);
    assert_eq!(log["entries"][0]["shape"], "explore");
    assert_eq!(log["entries"][0]["artifact"], "results/a.md");

    let assert = Command::cargo_bin("by-rs")
        .unwrap()
        .args([
            "main",
            "parse-saved-lines",
            "--text",
            "Saved plan: .brainyard/plans/a.md\nignored\nSaved todo: todos/one.md",
        ])
        .assert()
        .success()
        .stderr(predicate::str::is_empty());
    let stdout = String::from_utf8(assert.get_output().stdout.clone()).unwrap();
    let saved: serde_json::Value = serde_json::from_str(&stdout).unwrap();
    assert_eq!(saved["count"], 2);
    assert_eq!(saved["saved-lines"][0]["kind"], "plan");
    assert_eq!(saved["saved-lines"][1]["path"], "todos/one.md");

    let assert = Command::cargo_bin("by-rs")
        .unwrap()
        .args([
            "main",
            "bootstrap-preview",
            "--session-id",
            "session-1",
            "--base-dir",
        ])
        .arg(project.path())
        .assert()
        .success()
        .stderr(predicate::str::is_empty());
    let stdout = String::from_utf8(assert.get_output().stdout.clone()).unwrap();
    let bootstrap: serde_json::Value = serde_json::from_str(&stdout).unwrap();
    assert_eq!(
        bootstrap["projection"],
        "common.main/main$bootstrap-preview"
    );
    assert_eq!(bootstrap["exists?"], true);
    assert_eq!(bootstrap["write-skipped?"], true);
    assert_eq!(
        bootstrap["log-path"],
        ".brainyard/agents/main-agent/session-1/routing.log"
    );

    let assert = Command::cargo_bin("by-rs")
        .unwrap()
        .args([
            "main",
            "append-log-preview",
            "--session-id",
            "session-1",
            "--turn",
            "3",
            "--iter",
            "1",
            "--question",
            "Q3",
            "--shape",
            ":plan-author",
            "--routed-to",
            "plan-agent",
            "--artifact",
            "plans/a.md",
            "--reason",
            "Need a durable plan.",
            "--base-dir",
        ])
        .arg(project.path())
        .assert()
        .success()
        .stderr(predicate::str::is_empty());
    let stdout = String::from_utf8(assert.get_output().stdout.clone()).unwrap();
    let append_log: serde_json::Value = serde_json::from_str(&stdout).unwrap();
    assert_eq!(
        append_log["projection"],
        "common.main/main$append-log-preview"
    );
    assert_eq!(append_log["write-skipped?"], true);
    let line: serde_json::Value =
        serde_json::from_str(append_log["line"].as_str().unwrap()).unwrap();
    assert_eq!(line["turn"], 3);
    assert_eq!(line["shape"], "plan-author");
    assert_eq!(line["routed-to"], "plan-agent");

    let assert = Command::cargo_bin("by-rs")
        .unwrap()
        .args([
            "main",
            "append-log-preview",
            "--session-id",
            "session-1",
            "--turn",
            "4",
            "--iter",
            "1",
            "--question",
            "Q4",
            "--shape",
            ":tool-lifecycle",
            "--routed-to",
            "tool-agent",
            "--artifact",
            "tools/a.md",
            "--reason",
            "Need tool lifecycle handling.",
            "--base-dir",
        ])
        .arg(project.path())
        .assert()
        .success()
        .stderr(predicate::str::is_empty());
    let stdout = String::from_utf8(assert.get_output().stdout.clone()).unwrap();
    let append_tool_log: serde_json::Value = serde_json::from_str(&stdout).unwrap();
    let tool_line: serde_json::Value =
        serde_json::from_str(append_tool_log["line"].as_str().unwrap()).unwrap();
    assert_eq!(tool_line["shape"], "tool-lifecycle");
    assert_eq!(tool_line["routed-to"], "tool-agent");

    let assert = Command::cargo_bin("by-rs")
        .unwrap()
        .args([
            "main",
            "append-pointer-preview",
            "--session-id",
            "session-1",
            "--path",
            "plans/a.md",
            "--caption",
            "plan body\nfor porting",
            "--created",
            "2026-06-03 12:35",
            "--base-dir",
        ])
        .arg(project.path())
        .assert()
        .success()
        .stderr(predicate::str::is_empty());
    let stdout = String::from_utf8(assert.get_output().stdout.clone()).unwrap();
    let pointer: serde_json::Value = serde_json::from_str(&stdout).unwrap();
    assert_eq!(
        pointer["projection"],
        "common.main/main$append-pointer-preview"
    );
    assert_eq!(pointer["write-skipped?"], true);
    assert_eq!(
        pointer["line"],
        "- 2026-06-03 12:35 [plans/a.md](plans/a.md) — plan body for porting"
    );
    assert_eq!(pointer["would-create-pointers?"], true);

    let assert = Command::cargo_bin("by-rs")
        .unwrap()
        .args([
            "main",
            "index-append-preview",
            "--session-id",
            "session-1",
            "--turn-count",
            "4",
            "--shapes",
            r#"[:explore "direct-answer" :tool-lifecycle :explore]"#,
            "--created",
            "2026-06-03 12:34",
            "--base-dir",
        ])
        .arg(project.path())
        .assert()
        .success()
        .stderr(predicate::str::is_empty());
    let stdout = String::from_utf8(assert.get_output().stdout.clone()).unwrap();
    let index: serde_json::Value = serde_json::from_str(&stdout).unwrap();
    assert_eq!(index["projection"], "common.main/main$index-append-preview");
    assert_eq!(index["appended"], true);
    assert_eq!(index["write-skipped?"], true);
    assert_eq!(
        index["line"],
        "- 2026-06-03 12:34 [session session-1](session-1/) — turns: 4 · shapes: explore, direct-answer, tool-lifecycle"
    );
    assert_eq!(index["shapes"][0], "explore");
    assert_eq!(index["shapes"][1], "direct-answer");
    assert_eq!(index["shapes"][2], "tool-lifecycle");
    assert!(index["path"]
        .as_str()
        .unwrap()
        .ends_with(".brainyard/agents/main-agent/INDEX.md"));
}

#[test]
fn email_helpers_are_live_free_json_projections() {
    let assert = Command::cargo_bin("by-rs")
        .unwrap()
        .args(["email", "validate-address", "--email", "alice@example.com"])
        .assert()
        .success()
        .stderr(predicate::str::is_empty());
    let stdout = String::from_utf8(assert.get_output().stdout.clone()).unwrap();
    let valid: serde_json::Value = serde_json::from_str(&stdout).unwrap();
    assert_eq!(valid["projection"], "email-command/valid-email");
    assert_eq!(valid["valid?"], true);

    let assert = Command::cargo_bin("by-rs")
        .unwrap()
        .args(["email", "validate-address", "--email", "alice@example"])
        .assert()
        .success()
        .stderr(predicate::str::is_empty());
    let stdout = String::from_utf8(assert.get_output().stdout.clone()).unwrap();
    let invalid: serde_json::Value = serde_json::from_str(&stdout).unwrap();
    assert_eq!(invalid["valid?"], false);

    let assert = Command::cargo_bin("by-rs")
        .unwrap()
        .args([
            "email",
            "send-preflight",
            "--to",
            "not-an-email",
            "--subject",
            "Hello",
        ])
        .assert()
        .success()
        .stderr(predicate::str::is_empty());
    let stdout = String::from_utf8(assert.get_output().stdout.clone()).unwrap();
    let invalid_send: serde_json::Value = serde_json::from_str(&stdout).unwrap();
    assert_eq!(invalid_send["error"], "Invalid email address: not-an-email");
    assert_eq!(invalid_send["ses-skipped?"], true);

    let assert = Command::cargo_bin("by-rs")
        .unwrap()
        .args([
            "email",
            "send-preflight",
            "--to",
            "alice@example.com",
            "--subject",
            "   ",
        ])
        .assert()
        .success()
        .stderr(predicate::str::is_empty());
    let stdout = String::from_utf8(assert.get_output().stdout.clone()).unwrap();
    let blank_subject: serde_json::Value = serde_json::from_str(&stdout).unwrap();
    assert_eq!(blank_subject["error"], "Subject is required");

    let assert = Command::cargo_bin("by-rs")
        .unwrap()
        .args([
            "email",
            "send-preflight",
            "--to",
            "alice@example.com",
            "--subject",
            "Hello",
            "--body",
            "Plain text",
            "--html",
            "<b>Hello</b>",
            "--cc",
            "bob@example.com",
            "--bcc",
            "ops@example.com",
            "--sender",
            "bot@example.com",
            "--sender-name",
            "Brainyard",
            "--reply-to",
            "reply@example.com",
        ])
        .assert()
        .success()
        .stderr(predicate::str::is_empty());
    let stdout = String::from_utf8(assert.get_output().stdout.clone()).unwrap();
    let preflight: serde_json::Value = serde_json::from_str(&stdout).unwrap();
    assert_eq!(
        preflight["projection"],
        "email-command/send-email-preflight"
    );
    assert_eq!(preflight["success"], true);
    assert_eq!(preflight["sender"], "Brainyard <bot@example.com>");
    assert_eq!(preflight["body-content"]["html"], "<b>Hello</b>");
    assert_eq!(preflight["body-content"]["text"], "Plain text");
    assert_eq!(preflight["cc"][0], "bob@example.com");
    assert_eq!(preflight["bcc"][0], "ops@example.com");
    assert_eq!(preflight["reply-to"][0], "reply@example.com");
    assert_eq!(preflight["ses-skipped?"], true);

    let assert = Command::cargo_bin("by-rs")
        .unwrap()
        .args([
            "email",
            "notification-preview",
            "--to",
            "alice@example.com",
            "--subject",
            "Alert",
            "--message",
            "Needs attention",
            "--severity",
            "warning",
        ])
        .assert()
        .success()
        .stderr(predicate::str::is_empty());
    let stdout = String::from_utf8(assert.get_output().stdout.clone()).unwrap();
    let notification: serde_json::Value = serde_json::from_str(&stdout).unwrap();
    assert_eq!(
        notification["projection"],
        "email-command/send-notification-preview"
    );
    assert_eq!(notification["title"], "Alert");
    assert_eq!(notification["severity"], "warning");
    assert_eq!(notification["body-content"]["text"], "Needs attention");
    assert!(notification["body-content"]["html"]
        .as_str()
        .unwrap()
        .contains("background:#fef3c7"));
    assert_eq!(notification["ses-skipped?"], true);

    let assert = Command::cargo_bin("by-rs")
        .unwrap()
        .env_remove("MAIL_SENDER_NAME")
        .env_remove("AWS_REGION")
        .args([
            "email",
            "verify-config-preview",
            "--sender",
            "bot@example.com",
            "--region",
            "us-west-2",
        ])
        .assert()
        .success()
        .stderr(predicate::str::is_empty());
    let stdout = String::from_utf8(assert.get_output().stdout.clone()).unwrap();
    let verify_missing_client: serde_json::Value = serde_json::from_str(&stdout).unwrap();
    assert_eq!(
        verify_missing_client["projection"],
        "email-command/verify-config-preview"
    );
    assert_eq!(verify_missing_client["configured"], false);
    assert_eq!(
        verify_missing_client["error"],
        "AWS SES client not available"
    );
    assert_eq!(verify_missing_client["sender"], "bot@example.com");
    assert_eq!(verify_missing_client["region"], "us-west-2");
    assert_eq!(verify_missing_client["ses-skipped?"], true);

    let identities = r#"{"email-identities":[{"identity-name":"bot@example.com"},{"IdentityName":"ops@example.com"}]}"#;
    let assert = Command::cargo_bin("by-rs")
        .unwrap()
        .env_remove("MAIL_SENDER_NAME")
        .args([
            "email",
            "verify-config-preview",
            "--sender",
            "bot@example.com",
            "--identities",
            identities,
        ])
        .assert()
        .success()
        .stderr(predicate::str::is_empty());
    let stdout = String::from_utf8(assert.get_output().stdout.clone()).unwrap();
    let verify_fixture: serde_json::Value = serde_json::from_str(&stdout).unwrap();
    assert_eq!(verify_fixture["configured"], true);
    assert_eq!(verify_fixture["sender"], "bot@example.com");
    assert_eq!(verify_fixture["sender-verified"], true);
    assert_eq!(verify_fixture["identity-count"], 2);
    assert_eq!(verify_fixture["identities"][0], "bot@example.com");
    assert_eq!(verify_fixture["identities"][1], "ops@example.com");
    assert_eq!(verify_fixture["ses-skipped?"], true);

    let assert = Command::cargo_bin("by-rs")
        .unwrap()
        .args(["email", "list-templates-preview"])
        .assert()
        .success()
        .stderr(predicate::str::is_empty());
    let stdout = String::from_utf8(assert.get_output().stdout.clone()).unwrap();
    let templates_missing_client: serde_json::Value = serde_json::from_str(&stdout).unwrap();
    assert_eq!(
        templates_missing_client["projection"],
        "email-command/list-templates-preview"
    );
    assert_eq!(
        templates_missing_client["error"],
        "AWS SES client not available"
    );
    assert!(templates_missing_client["templates"].is_null());
    assert!(templates_missing_client["count"].is_null());
    assert_eq!(templates_missing_client["ses-skipped?"], true);

    let templates =
        r#"{"templates-metadata":[{"template-name":"welcome"},{"TemplateName":"digest"}]}"#;
    let assert = Command::cargo_bin("by-rs")
        .unwrap()
        .args(["email", "list-templates-preview", "--templates", templates])
        .assert()
        .success()
        .stderr(predicate::str::is_empty());
    let stdout = String::from_utf8(assert.get_output().stdout.clone()).unwrap();
    let templates_fixture: serde_json::Value = serde_json::from_str(&stdout).unwrap();
    assert_eq!(templates_fixture["templates"][0], "welcome");
    assert_eq!(templates_fixture["templates"][1], "digest");
    assert_eq!(templates_fixture["count"], 2);
    assert_eq!(templates_fixture["ses-skipped?"], true);
}

#[test]
fn slack_helpers_are_live_free_json_projections() {
    let channels = r#"{:channels [{:id "C111" :name "general" :topic {:value "General chat"} :num_members 3 :is_private false}]}"#;
    let users = r#"[{:id "U222" :real_name "Alice Smith" :profile {:display_name "alice" :email "alice@example.com"}}]"#;

    let assert = Command::cargo_bin("by-rs")
        .unwrap()
        .args(["slack", "resolve-recipient", "--recipient", "C111"])
        .assert()
        .success()
        .stderr(predicate::str::is_empty());
    let stdout = String::from_utf8(assert.get_output().stdout.clone()).unwrap();
    let channel_id: serde_json::Value = serde_json::from_str(&stdout).unwrap();
    assert_eq!(channel_id["projection"], "slack-command/resolve-recipient");
    assert_eq!(channel_id["type"], "channel");
    assert_eq!(channel_id["id"], "C111");

    let assert = Command::cargo_bin("by-rs")
        .unwrap()
        .args([
            "slack",
            "resolve-recipient",
            "--recipient",
            "#general",
            "--channels",
            channels,
        ])
        .assert()
        .success()
        .stderr(predicate::str::is_empty());
    let stdout = String::from_utf8(assert.get_output().stdout.clone()).unwrap();
    let channel_name: serde_json::Value = serde_json::from_str(&stdout).unwrap();
    assert_eq!(channel_name["type"], "channel");
    assert_eq!(channel_name["id"], "C111");
    assert_eq!(channel_name["display-name"], "general");

    let assert = Command::cargo_bin("by-rs")
        .unwrap()
        .args([
            "slack",
            "resolve-recipient",
            "--recipient",
            "alice@example.com",
            "--users",
            users,
        ])
        .assert()
        .success()
        .stderr(predicate::str::is_empty());
    let stdout = String::from_utf8(assert.get_output().stdout.clone()).unwrap();
    let email: serde_json::Value = serde_json::from_str(&stdout).unwrap();
    assert_eq!(email["type"], "user");
    assert_eq!(email["id"], "U222");
    assert_eq!(email["display-name"], "Alice Smith");

    let assert = Command::cargo_bin("by-rs")
        .unwrap()
        .args([
            "slack",
            "send-preflight",
            "--recipient",
            "#general",
            "--message",
            "Deploy finished",
            "--thread-ts",
            "1710000000.123",
            "--channels",
            channels,
            "--token",
            "xoxb-1234567890",
        ])
        .assert()
        .success()
        .stderr(predicate::str::is_empty());
    let stdout = String::from_utf8(assert.get_output().stdout.clone()).unwrap();
    let send: serde_json::Value = serde_json::from_str(&stdout).unwrap();
    assert_eq!(send["projection"], "slack-command/send-message-preflight");
    assert_eq!(send["success"], true);
    assert_eq!(send["channel"], "C111");
    assert_eq!(send["recipient-type"], "channel");
    assert_eq!(send["masked-token"], "xoxb-123…");
    assert_eq!(send["post-skipped?"], true);

    let assert = Command::cargo_bin("by-rs")
        .unwrap()
        .args([
            "slack",
            "lookup-user-preview",
            "--email",
            "alice@example.com",
            "--users",
            users,
            "--token",
            "xoxb-1234567890",
        ])
        .assert()
        .success()
        .stderr(predicate::str::is_empty());
    let stdout = String::from_utf8(assert.get_output().stdout.clone()).unwrap();
    let lookup: serde_json::Value = serde_json::from_str(&stdout).unwrap();
    assert_eq!(lookup["projection"], "slack-command/lookup-user-preview");
    assert_eq!(lookup["user-id"], "U222");
    assert_eq!(lookup["name"], "Alice Smith");
    assert_eq!(lookup["display-name"], "alice");
    assert_eq!(lookup["lookup-skipped?"], true);

    let assert = Command::cargo_bin("by-rs")
        .unwrap()
        .args(["slack", "list-channels-preview", "--channels", channels])
        .assert()
        .success()
        .stderr(predicate::str::is_empty());
    let stdout = String::from_utf8(assert.get_output().stdout.clone()).unwrap();
    let list: serde_json::Value = serde_json::from_str(&stdout).unwrap();
    assert_eq!(list["projection"], "slack-command/list-channels-preview");
    assert_eq!(list["count"], 1);
    assert_eq!(list["channels"][0]["topic"], "General chat");
    assert_eq!(list["channels"][0]["member-count"], 3);
    assert_eq!(list["channels"][0]["is-private"], false);

    let assert = Command::cargo_bin("by-rs")
        .unwrap()
        .args([
            "slack",
            "token-preflight",
            "--agent-config",
            r#"{:slack {:bot-token "xoxb-configured-token"}}"#,
        ])
        .assert()
        .success()
        .stderr(predicate::str::is_empty());
    let stdout = String::from_utf8(assert.get_output().stdout.clone()).unwrap();
    let token: serde_json::Value = serde_json::from_str(&stdout).unwrap();
    assert_eq!(token["projection"], "slack-command/token-preflight");
    assert_eq!(token["configured"], true);
    assert_eq!(token["token-source"], "agent-config");
    assert_eq!(token["auth-test-skipped?"], true);

    let assert = Command::cargo_bin("by-rs")
        .unwrap()
        .env_remove("SLACK_BOT_TOKEN")
        .args(["slack", "verify-token-preview"])
        .assert()
        .success()
        .stderr(predicate::str::is_empty());
    let stdout = String::from_utf8(assert.get_output().stdout.clone()).unwrap();
    let verify_missing_token: serde_json::Value = serde_json::from_str(&stdout).unwrap();
    assert_eq!(
        verify_missing_token["projection"],
        "slack-command/verify-token-preview"
    );
    assert_eq!(
        verify_missing_token["error"],
        "SLACK_BOT_TOKEN not configured"
    );
    assert_eq!(verify_missing_token["api-skipped?"], true);

    let assert = Command::cargo_bin("by-rs")
        .unwrap()
        .args([
            "slack",
            "verify-token-preview",
            "--token",
            "xoxb-1234567890",
            "--auth-test",
            r#"{:ok true :bot_id "B111" :team "Brainyard" :user "brainyard-bot"}"#,
        ])
        .assert()
        .success()
        .stderr(predicate::str::is_empty());
    let stdout = String::from_utf8(assert.get_output().stdout.clone()).unwrap();
    let verify_fixture: serde_json::Value = serde_json::from_str(&stdout).unwrap();
    assert_eq!(
        verify_fixture["projection"],
        "slack-command/verify-token-preview"
    );
    assert_eq!(verify_fixture["ok"], true);
    assert_eq!(verify_fixture["bot-id"], "B111");
    assert_eq!(verify_fixture["team"], "Brainyard");
    assert_eq!(verify_fixture["user"], "brainyard-bot");
    assert_eq!(verify_fixture["masked-token"], "xoxb-123…");
    assert_eq!(verify_fixture["auth-test-skipped?"], true);
}

#[test]
fn query_helpers_are_live_free_json_projections() {
    let datasources = r#"{:default {:schema-info {:tables [{:name "orders" :columns [{:name "id" :type "int"}]}]}} :analytics {:schema-info {}}}"#;

    let assert = Command::cargo_bin("by-rs")
        .unwrap()
        .args(["query", "list-datasources", "--datasources", datasources])
        .assert()
        .success()
        .stderr(predicate::str::is_empty());
    let stdout = String::from_utf8(assert.get_output().stdout.clone()).unwrap();
    let list: serde_json::Value = serde_json::from_str(&stdout).unwrap();
    assert_eq!(list["projection"], "query-command/list-datasources");
    assert_eq!(list["datasources"][0]["name"], "analytics");
    assert_eq!(list["datasources"][0]["tables"], 0);
    assert_eq!(list["datasources"][0]["has-schema"], false);
    assert_eq!(list["datasources"][1]["name"], "default");
    assert_eq!(list["datasources"][1]["tables"], 1);
    assert_eq!(list["datasources"][1]["has-schema"], true);

    let assert = Command::cargo_bin("by-rs")
        .unwrap()
        .args(["query", "get-schema", "--datasources", datasources])
        .assert()
        .success()
        .stderr(predicate::str::is_empty());
    let stdout = String::from_utf8(assert.get_output().stdout.clone()).unwrap();
    let schema: serde_json::Value = serde_json::from_str(&stdout).unwrap();
    assert_eq!(schema["projection"], "query-command/get-schema");
    assert_eq!(schema["lookup-name"], "default");
    assert!(schema["datasource"].is_null());
    assert_eq!(schema["schema"]["tables"][0]["name"], "orders");

    let assert = Command::cargo_bin("by-rs")
        .unwrap()
        .args([
            "query",
            "execute-preflight",
            "--datasources",
            datasources,
            "--datasource-name",
            "missing",
            "--sql",
            "select 1",
        ])
        .assert()
        .success()
        .stderr(predicate::str::is_empty());
    let stdout = String::from_utf8(assert.get_output().stdout.clone()).unwrap();
    let missing: serde_json::Value = serde_json::from_str(&stdout).unwrap();
    assert_eq!(
        missing["error"],
        "Datasource not found: missing. Available: analytics, default"
    );
    assert_eq!(missing["jdbc-skipped?"], true);

    let assert = Command::cargo_bin("by-rs")
        .unwrap()
        .args([
            "query",
            "execute-preflight",
            "--datasources",
            datasources,
            "--sql",
            "select * from orders",
            "--max-rows",
            "25",
        ])
        .assert()
        .success()
        .stderr(predicate::str::is_empty());
    let stdout = String::from_utf8(assert.get_output().stdout.clone()).unwrap();
    let execute: serde_json::Value = serde_json::from_str(&stdout).unwrap();
    assert_eq!(execute["projection"], "query-command/execute-preflight");
    assert_eq!(execute["success"], true);
    assert_eq!(execute["datasource-name"], "default");
    assert_eq!(execute["max-rows"], 25);
    assert_eq!(execute["execute-skipped?"], true);

    let assert = Command::cargo_bin("by-rs")
        .unwrap()
        .args([
            "query",
            "validate-sql-preflight",
            "--datasources",
            datasources,
            "--sql",
            "   ",
        ])
        .assert()
        .success()
        .stderr(predicate::str::is_empty());
    let stdout = String::from_utf8(assert.get_output().stdout.clone()).unwrap();
    let blank: serde_json::Value = serde_json::from_str(&stdout).unwrap();
    assert_eq!(blank["error"], "SQL is required");

    let assert = Command::cargo_bin("by-rs")
        .unwrap()
        .args([
            "query",
            "validate-sql-preflight",
            "--datasources",
            datasources,
            "--sql",
            "select * from orders",
        ])
        .assert()
        .success()
        .stderr(predicate::str::is_empty());
    let stdout = String::from_utf8(assert.get_output().stdout.clone()).unwrap();
    let validate: serde_json::Value = serde_json::from_str(&stdout).unwrap();
    assert_eq!(
        validate["projection"],
        "query-command/validate-sql-preflight"
    );
    assert!(validate["valid"].is_null());
    assert_eq!(validate["explain-sql"], "EXPLAIN select * from orders");
    assert_eq!(validate["explain-skipped?"], true);
}

#[test]
fn query_llm_and_clone_project_live_skip_contracts() {
    let assert = Command::cargo_bin("by-rs")
        .unwrap()
        .args([
            "query",
            "llm",
            "--prompt",
            "Summarize the release risk.",
            "--sub-context",
            "Use only local evidence.",
        ])
        .assert()
        .success()
        .stderr(predicate::str::is_empty());
    let single: serde_json::Value =
        serde_json::from_slice(&assert.get_output().stdout).expect("query llm single json");
    assert_eq!(single["projection"], "common.commands/query$llm");
    assert_eq!(single["source"], "live-sub-lm-skipped");
    assert_eq!(single["live-skipped?"], true);
    assert_eq!(single["sub-lm-live-skipped?"], true);
    assert_eq!(single["input-contract-only?"], true);
    assert_eq!(single["mode"], "single");
    assert!(single["result"].is_null());
    assert!(single["prompt-bytes"].as_u64().unwrap() > 0);
    assert!(single["sub-context-bytes"].as_u64().unwrap() > 0);
    assert!(single["error"]
        .as_str()
        .unwrap()
        .contains("requires a live sub-LLM"));

    let assert = Command::cargo_bin("by-rs")
        .unwrap()
        .args(["query", "llm", "--prompts", "[\"one\" \"two\"]"])
        .assert()
        .success()
        .stderr(predicate::str::is_empty());
    let batch: serde_json::Value =
        serde_json::from_slice(&assert.get_output().stdout).expect("query llm batch json");
    assert_eq!(batch["projection"], "common.commands/query$llm");
    assert_eq!(batch["mode"], "batched");
    assert_eq!(batch["prompt-count"], 2);
    assert_eq!(batch["prompts"][0], "one");
    assert_eq!(batch["prompts"][1], "two");
    assert_eq!(batch["results"].as_array().unwrap().len(), 0);

    let assert = Command::cargo_bin("by-rs")
        .unwrap()
        .args(["query", "llm", "--prompt", "one", "--prompts", "[\"two\"]"])
        .assert()
        .success()
        .stderr(predicate::str::is_empty());
    let conflict: serde_json::Value =
        serde_json::from_slice(&assert.get_output().stdout).expect("query llm conflict json");
    assert_eq!(conflict["projection"], "common.commands/query$llm");
    assert_eq!(conflict["error"], "supply :prompt OR :prompts, not both");
    assert_eq!(conflict["live-skipped?"], true);

    let assert = Command::cargo_bin("by-rs")
        .unwrap()
        .args([
            "query",
            "clone",
            "--query",
            "Inspect the plan.",
            "--agent-context",
            "local context",
            "--instruction",
            "be concise",
            "--tool-context",
            "no tools",
        ])
        .assert()
        .success()
        .stderr(predicate::str::is_empty());
    let clone: serde_json::Value =
        serde_json::from_slice(&assert.get_output().stdout).expect("query clone json");
    assert_eq!(clone["projection"], "common.commands/query$clone");
    assert_eq!(clone["source"], "live-agent-skipped");
    assert_eq!(clone["live-skipped?"], true);
    assert_eq!(clone["current-agent-skipped?"], true);
    assert_eq!(clone["input-contract-only?"], true);
    assert!(clone["result"].is_null());
    assert!(clone["query-bytes"].as_u64().unwrap() > 0);
    assert!(clone["agent-context-bytes"].as_u64().unwrap() > 0);
    assert!(clone["instruction-bytes"].as_u64().unwrap() > 0);
    assert!(clone["tool-context-bytes"].as_u64().unwrap() > 0);
    assert!(clone["error"]
        .as_str()
        .unwrap()
        .contains("requires a running current agent"));

    let assert = Command::cargo_bin("by-rs")
        .unwrap()
        .args(["query", "clone", "--query", "   "])
        .assert()
        .success()
        .stderr(predicate::str::is_empty());
    let blank: serde_json::Value =
        serde_json::from_slice(&assert.get_output().stdout).expect("blank clone json");
    assert_eq!(blank["projection"], "common.commands/query$clone");
    assert_eq!(blank["error"], "query is required");
}

#[test]
fn query_llm_bedrock_dry_run_prepares_sub_lm_request_without_network() {
    let assert = Command::cargo_bin("by-rs")
        .unwrap()
        .args([
            "query",
            "llm",
            "--dry-run",
            "--provider",
            "bedrock",
            "--model",
            "global.anthropic.claude-haiku-4-5-20251001-v1:0",
            "--region",
            "ap-northeast-2",
            "--aws-profile",
            "grumatic",
            "--max-tokens",
            "24",
            "--prompt",
            "Reply with exactly: BY_RS_QUERY_LLM_DRY_OK",
            "--sub-context",
            "Return no extra text.",
        ])
        .assert()
        .success()
        .stderr(predicate::str::is_empty());

    let report: serde_json::Value = serde_json::from_slice(&assert.get_output().stdout)
        .expect("query llm bedrock dry-run json");
    assert_eq!(report["projection"], "common.commands/query$llm");
    assert_eq!(report["source"], "bedrock-dry-run");
    assert_eq!(report["network"], false);
    assert_eq!(report["live-skipped?"], false);
    assert_eq!(report["sub-lm-live-skipped?"], false);
    assert_eq!(report["input-contract-only?"], false);
    assert_eq!(report["provider"], "bedrock");
    assert_eq!(
        report["model"],
        "global.anthropic.claude-haiku-4-5-20251001-v1:0"
    );
    assert_eq!(report["region"], "ap-northeast-2");
    assert_eq!(report["aws_profile"], "grumatic");
    assert_eq!(report["mode"], "single");
    assert_eq!(
        report["prompt-bytes"].as_u64().unwrap(),
        "Reply with exactly: BY_RS_QUERY_LLM_DRY_OK".len() as u64
    );
    assert!(report["sub-context-bytes"].as_u64().unwrap() > 0);
    assert!(report["result"].is_null());
    assert_eq!(
        report["request"]["modelId"],
        "global.anthropic.claude-haiku-4-5-20251001-v1:0"
    );
    assert_eq!(report["request"]["inferenceConfig"]["maxTokens"], 24);
    assert_eq!(report["request"]["messages"].as_array().unwrap().len(), 1);
    assert!(!report["request"]["system"].as_array().unwrap().is_empty());
}

#[test]
fn query_llm_claude_code_live_invokes_cli_and_prints_result() {
    let home = tempfile::tempdir().unwrap();
    let path_dir = tempfile::tempdir().unwrap();
    let capture_path = home.path().join("query-claude-stdin.txt");
    write_fake_executable(
        &path_dir.path().join("claude"),
        r#"#!/bin/sh
cat > "$CLAUDE_CAPTURE_STDIN"
printf '%s\n' '{"type":"result","subtype":"success","is_error":false,"result":"Claude query answer","usage":{"input_tokens":11,"output_tokens":12}}'
"#,
    );

    let assert = Command::cargo_bin("by-rs")
        .unwrap()
        .env("HOME", home.path())
        .env("BY_NO_DOTENV", "1")
        .env("PATH", prepend_path(path_dir.path()))
        .env("CLAUDE_CAPTURE_STDIN", &capture_path)
        .args([
            "query",
            "llm",
            "--live",
            "--provider",
            "claude-code",
            "--model",
            "haiku",
            "--prompt",
            "Summarize the release risk.",
            "--sub-context",
            "Use only local evidence.",
        ])
        .assert()
        .success()
        .stderr(predicate::str::is_empty());

    let report: serde_json::Value = serde_json::from_slice(&assert.get_output().stdout)
        .expect("query llm claude-code live json");
    assert_eq!(report["projection"], "common.commands/query$llm");
    assert_eq!(report["source"], "claude-code-live");
    assert_eq!(report["network"], true);
    assert_eq!(report["provider"], "claude-code");
    assert_eq!(report["model"], "haiku");
    assert_eq!(report["mode"], "single");
    assert_eq!(report["result"], "Claude query answer");
    assert_eq!(report["stop_reason"], "end_turn");
    assert!(report["sub-context-bytes"].as_u64().unwrap() > 0);
    assert!(report["request"]["argv"]
        .as_array()
        .unwrap()
        .iter()
        .any(|arg| arg.as_str() == Some("--system-prompt")));
    assert!(report["request"]["argv"]
        .as_array()
        .unwrap()
        .iter()
        .any(|arg| arg.as_str() == Some("Use only local evidence.")));
    assert_eq!(
        std::fs::read_to_string(capture_path).unwrap(),
        "Summarize the release risk."
    );
}

#[test]
fn sandbox_metadata_helpers_are_live_free_json_projections() {
    let assert = Command::cargo_bin("by-rs")
        .unwrap()
        .args(["sandbox", "list", "--category", "doc"])
        .assert()
        .success()
        .stderr(predicate::str::is_empty());
    let stdout = String::from_utf8(assert.get_output().stdout.clone()).unwrap();
    let listed: serde_json::Value = serde_json::from_str(&stdout).unwrap();
    assert_eq!(listed["count"], 5);
    let names = listed["functions"]
        .as_array()
        .unwrap()
        .iter()
        .map(|item| item["name"].as_str().unwrap())
        .collect::<Vec<_>>();
    assert!(names.contains(&"doc$list"));
    assert!(names.contains(&"doc$read"));
    assert!(names.contains(&"doc$delete"));

    let assert = Command::cargo_bin("by-rs")
        .unwrap()
        .args(["sandbox", "read", "--name", "mcp$server"])
        .assert()
        .success()
        .stderr(predicate::str::is_empty());
    let stdout = String::from_utf8(assert.get_output().stdout.clone()).unwrap();
    let entry: serde_json::Value = serde_json::from_str(&stdout).unwrap();
    assert_eq!(entry["category"], "mcp");
    assert_eq!(entry["args"], "<op> [<server-name>]");
    assert_eq!(entry["code-template"], "(mcp$server :op \"%s\")");

    let assert = Command::cargo_bin("by-rs")
        .unwrap()
        .args(["sandbox", "menu"])
        .assert()
        .success()
        .stderr(predicate::str::is_empty());
    let stdout = String::from_utf8(assert.get_output().stdout.clone()).unwrap();
    let menu: serde_json::Value = serde_json::from_str(&stdout).unwrap();
    assert_eq!(menu["menu-items"][0][0], "/sandbox context-index");
    assert!(menu["menu-items"][0][1]
        .as_str()
        .unwrap()
        .contains("(context) Overview"));

    let assert = Command::cargo_bin("by-rs")
        .unwrap()
        .args(["sandbox", "format-help"])
        .assert()
        .success()
        .stderr(predicate::str::is_empty());
    let stdout = String::from_utf8(assert.get_output().stdout.clone()).unwrap();
    let help: serde_json::Value = serde_json::from_str(&stdout).unwrap();
    let help = help["help"].as_str().unwrap();
    assert!(help.contains("Sandbox Functions:\n  [context]"));
    assert!(help.contains("doc$read <kind> <slug>"));
    assert!(help.contains("fetch-url <url>"));
}

#[test]
fn code_eval_helpers_are_live_free_json_projections() {
    let assert = Command::cargo_bin("by-rs")
        .unwrap()
        .args([
            "code-eval",
            "sandbox-arm-error",
            "--code",
            "(println :hello)",
        ])
        .assert()
        .success()
        .stderr(predicate::str::is_empty());
    let stdout = String::from_utf8(assert.get_output().stdout.clone()).unwrap();
    let projection: serde_json::Value = serde_json::from_str(&stdout).unwrap();
    assert_eq!(projection["projection"], "code-eval/sandbox-arm-error");
    assert_eq!(projection["result"]["backend"], "sandbox");
    assert_eq!(projection["result"]["code"], "(println :hello)");
    assert_eq!(projection["result"]["output"], "");
    assert!(projection["result"]["error"]
        .as_str()
        .unwrap()
        .contains("CoAct ```clojure fence"));
}

#[test]
fn delegation_use_tree_is_live_free_json_projection() {
    let assert = Command::cargo_bin("by-rs")
        .unwrap()
        .args([
            "delegation-use",
            "tree",
            "--id-prefix",
            ":research",
            "--question-key",
            ":delegated-question",
            "--result-key",
            ":agent-result",
            "--rewrite-signature",
            "RewriteQuestion",
            "--describe-signature",
            "DescribeResult",
        ])
        .assert()
        .success()
        .stderr(predicate::str::is_empty());
    let stdout = String::from_utf8(assert.get_output().stdout.clone()).unwrap();
    let projection: serde_json::Value = serde_json::from_str(&stdout).unwrap();
    assert_eq!(
        projection["projection"],
        "common.delegation-use/delegation-use"
    );
    assert_eq!(projection["question-key"], "delegated-question");
    assert_eq!(projection["result-key"], "agent-result");
    assert_eq!(projection["rewrite-skipped?"], false);
    assert_eq!(projection["calls-skipped?"], true);
    assert_eq!(
        projection["node-ids"]["sequence"],
        "research.sequence/agent-result"
    );
    assert_eq!(
        projection["node-ids"]["rewrite"],
        "research.action/rewrite-question"
    );
    assert_eq!(projection["node-ids"]["ask"], "research.action/ask-agent");
    assert_eq!(
        projection["node-ids"]["describe"],
        "research.action/describe-result"
    );
    assert_eq!(projection["tree"][0], "sequence");
    assert_eq!(
        projection["tree"][1]["id"],
        "research.sequence/agent-result"
    );
    assert_eq!(projection["tree"][2][1]["signature"], "RewriteQuestion");
    assert_eq!(projection["tree"][2][1]["operation"], "chain-of-thought");
    assert_eq!(projection["tree"][3][2], "ask-agent-fn");
    assert_eq!(projection["tree"][4][1]["signature"], "DescribeResult");

    let assert = Command::cargo_bin("by-rs")
        .unwrap()
        .args([
            "delegation-use",
            "tree",
            "--id-prefix",
            "research",
            "--question-key",
            "q",
            "--result-key",
            "answer",
            "--describe-signature",
            "DescribeResult",
        ])
        .assert()
        .success()
        .stderr(predicate::str::is_empty());
    let stdout = String::from_utf8(assert.get_output().stdout.clone()).unwrap();
    let projection: serde_json::Value = serde_json::from_str(&stdout).unwrap();
    assert_eq!(projection["rewrite-skipped?"], true);
    assert_eq!(projection["node-ids"]["rewrite"], serde_json::Value::Null);
    assert_eq!(projection["tree"].as_array().unwrap().len(), 4);
    assert_eq!(projection["tree"][2][1]["id"], "research.action/ask-agent");
    assert_eq!(
        projection["tree"][3][1]["id"],
        "research.action/describe-result"
    );
}

#[test]
fn rlm_trajectory_helpers_are_live_free_json_projections() {
    let assert = Command::cargo_bin("by-rs")
        .unwrap()
        .args([
            "rlm",
            "trajectory-entry",
            "--iteration",
            "2",
            "--eval-results",
            r#"[{:code "(+ 1 2)" :output "=> 3"}]"#,
            "--error",
            "boom",
            "--response-text",
            "Reasoning\n```clojure\n(+ 1 2)\n```\nMore",
        ])
        .assert()
        .success()
        .stderr(predicate::str::is_empty());
    let stdout = String::from_utf8(assert.get_output().stdout.clone()).unwrap();
    let entry: serde_json::Value = serde_json::from_str(&stdout).unwrap();
    assert_eq!(entry["iteration"], 2);
    assert_eq!(entry["code"][0], "(+ 1 2)");
    assert_eq!(entry["output"][0], "=> 3");
    assert_eq!(entry["output"][1], "ERROR: boom");
    assert_eq!(entry["reasoning"], "Reasoning\nMore");

    let assert = Command::cargo_bin("by-rs")
        .unwrap()
        .args([
            "rlm",
            "trajectory-build",
            "--question",
            "How do we calculate a small sum?",
            "--result",
            r#"{:terminated-by :final
                :answer "Done"
                :iterations [{:iteration 1
                              :eval-results [{:code "(+ 1 2)" :output "=> 3"}]
                              :error "boom"}]}"#,
            "--usage-summary",
            r#"{:totals {:total-cost 0.25}}"#,
            "--model",
            "model-a",
            "--agent-id",
            "rlm-agent",
            "--session-id",
            "session-1",
            "--depth",
            "1",
            "--context-key",
            "memory",
            "--context-key",
            "tools",
            "--started-at",
            "100",
            "--ended-at",
            "150",
        ])
        .assert()
        .success()
        .stderr(predicate::str::is_empty());
    let stdout = String::from_utf8(assert.get_output().stdout.clone()).unwrap();
    let mut trajectory: serde_json::Value = serde_json::from_str(&stdout).unwrap();
    assert_eq!(trajectory["success"], true);
    assert_eq!(trajectory["terminated-by"], "final");
    assert_eq!(trajectory["answer"], "Done");
    assert_eq!(trajectory["total-iterations"], 1);
    assert_eq!(trajectory["cost"].as_f64().unwrap(), 0.25);
    assert_eq!(trajectory["model"], "model-a");
    assert_eq!(trajectory["context-summary"], "Context keys: memory, tools");
    assert_eq!(trajectory["timing"]["duration-ms"], 50);
    assert_eq!(trajectory["metadata"]["agent-id"], "rlm-agent");
    assert_eq!(trajectory["trajectory"][0]["code"][0], "(+ 1 2)");
    assert_eq!(trajectory["trajectory"][0]["output"][1], "ERROR: boom");

    trajectory
        .as_object_mut()
        .unwrap()
        .insert("id".to_string(), serde_json::json!("trajectory-1"));
    let trajectory_arg = serde_json::to_string(&trajectory).unwrap();

    let assert = Command::cargo_bin("by-rs")
        .unwrap()
        .args([
            "rlm",
            "trajectory-summary",
            "--trajectory",
            trajectory_arg.as_str(),
        ])
        .assert()
        .success()
        .stderr(predicate::str::is_empty());
    let stdout = String::from_utf8(assert.get_output().stdout.clone()).unwrap();
    let summary: serde_json::Value = serde_json::from_str(&stdout).unwrap();
    assert_eq!(summary["id"], "trajectory-1");
    assert_eq!(summary["success"], true);
    assert_eq!(summary["duration-ms"], 50);
    assert_eq!(summary["iterations"][0]["iteration"], 1);
    assert_eq!(summary["iterations"][0]["has-code"], true);
    assert_eq!(summary["iterations"][0]["has-error"], true);
    assert!(summary["iterations"][0]["output-length"].as_u64().unwrap() > 0);

    let assert = Command::cargo_bin("by-rs")
        .unwrap()
        .args([
            "rlm",
            "trajectory-format-export",
            "--trajectory",
            trajectory_arg.as_str(),
        ])
        .assert()
        .success()
        .stderr(predicate::str::is_empty());
    let stdout = String::from_utf8(assert.get_output().stdout.clone()).unwrap();
    let export: serde_json::Value = serde_json::from_str(&stdout).unwrap();
    let export = export.as_object().unwrap();
    assert!(export.contains_key("query"));
    assert!(export.contains_key("trajectory"));
    assert!(export.contains_key("model"));
    assert!(!export.contains_key("metadata"));
    assert!(!export.contains_key("timing"));
}

#[test]
fn evaluation_helpers_are_live_free_json_projections() {
    let temp = tempfile::tempdir().unwrap();
    let recovered_path = temp.path().join("full-output.txt");
    std::fs::write(&recovered_path, "full recovered output").unwrap();
    let marker = format!(
        "head\n--- Full content saved to: {} ---\ntail",
        recovered_path.display()
    );

    let assert = Command::cargo_bin("by-rs")
        .unwrap()
        .args(["evaluation", "recover-truncated", "--text", marker.as_str()])
        .assert()
        .success()
        .stderr(predicate::str::is_empty());
    let stdout = String::from_utf8(assert.get_output().stdout.clone()).unwrap();
    let recovered: serde_json::Value = serde_json::from_str(&stdout).unwrap();
    assert_eq!(recovered["recovered?"], true);
    assert_eq!(recovered["text"], "full recovered output");

    let iterations = serde_json::json!([
        {
            "iteration": 1,
            "eval-results": [
                {
                    "code": "(+ 1 2)",
                    "output": marker,
                    "script-content": "println(3)"
                }
            ],
            "error": "boom"
        }
    ])
    .to_string();
    let assert = Command::cargo_bin("by-rs")
        .unwrap()
        .args([
            "evaluation",
            "iteration-evidence",
            "--iterations",
            iterations.as_str(),
        ])
        .assert()
        .success()
        .stderr(predicate::str::is_empty());
    let stdout = String::from_utf8(assert.get_output().stdout.clone()).unwrap();
    let evidence: serde_json::Value = serde_json::from_str(&stdout).unwrap();
    let evidence_text = evidence["evidence"].as_str().unwrap();
    assert!(evidence_text.contains("--- Iteration 1 ---"));
    assert!(evidence_text.contains("Code: (+ 1 2)"));
    assert!(evidence_text.contains("Script content:\nprintln(3)"));
    assert!(evidence_text.contains("Error: boom"));
    assert!(evidence_text.contains("Output:\nfull recovered output"));

    let react_iterations = r#"[{:iteration 2
                               :thought "Need a tool"
                               :actions [{:tool-name :read-file
                                          :tool-args {:path "README.md"}
                                          :tool-result "ok"}]
                               :observation "done"}]"#;
    let assert = Command::cargo_bin("by-rs")
        .unwrap()
        .args(["evaluation", "evidence", "--iterations", react_iterations])
        .assert()
        .success()
        .stderr(predicate::str::is_empty());
    let stdout = String::from_utf8(assert.get_output().stdout.clone()).unwrap();
    let react: serde_json::Value = serde_json::from_str(&stdout).unwrap();
    let react_text = react["evidence"].as_str().unwrap();
    assert!(react_text.contains("Thought: Need a tool"));
    assert!(react_text.contains("Tool: read-file"));
    assert!(react_text.contains("Result: ok"));
    assert!(react_text.contains("Observation: done"));

    let long_thought = "x".repeat(201);
    let thoughts = serde_json::json!(["short", long_thought]).to_string();
    let assert = Command::cargo_bin("by-rs")
        .unwrap()
        .args([
            "evaluation",
            "react-block",
            "--kind",
            "thoughts",
            "--items",
            thoughts.as_str(),
        ])
        .assert()
        .success()
        .stderr(predicate::str::is_empty());
    let stdout = String::from_utf8(assert.get_output().stdout.clone()).unwrap();
    let block: serde_json::Value = serde_json::from_str(&stdout).unwrap();
    assert_eq!(block["projection"], "react-agent/format-thoughts-block");
    let rendered = block["rendered"].as_str().unwrap();
    assert!(rendered.contains("(1) short"));
    assert!(rendered.contains(&format!("(2) {}…", "x".repeat(200))));

    let iterations = r#"[{:iteration 9
                         :thought "Think"
                         :actions [{:tool-name :read-file} {:tool-name "write-file"}]
                         :observation "observed"}]"#;
    let assert = Command::cargo_bin("by-rs")
        .unwrap()
        .args([
            "evaluation",
            "react-block",
            "--kind",
            "iterations",
            "--items",
            iterations,
        ])
        .assert()
        .success()
        .stderr(predicate::str::is_empty());
    let stdout = String::from_utf8(assert.get_output().stdout.clone()).unwrap();
    let iterations_block: serde_json::Value = serde_json::from_str(&stdout).unwrap();
    assert_eq!(
        iterations_block["projection"],
        "react-agent/format-iterations-block"
    );
    assert_eq!(
        iterations_block["rendered"],
        "(9) Think => read-file; write-file | observed"
    );

    let assert = Command::cargo_bin("by-rs")
        .unwrap()
        .args([
            "evaluation",
            "react-block",
            "--kind",
            "summarize-iterations-deterministic",
            "--items",
            "[]",
        ])
        .assert()
        .success()
        .stderr(predicate::str::is_empty());
    let stdout = String::from_utf8(assert.get_output().stdout.clone()).unwrap();
    let summary: serde_json::Value = serde_json::from_str(&stdout).unwrap();
    assert_eq!(
        summary["projection"],
        "react-agent/summarize-iterations-deterministic"
    );
    assert_eq!(summary["rendered"], "");

    let snapshot = serde_json::json!({
        "instruction": "x".repeat(505),
        "agent-context": "agent facts",
        "tool-context": "tool facts"
    })
    .to_string();
    let assert = Command::cargo_bin("by-rs")
        .unwrap()
        .args(["evaluation", "context", "--snapshot", snapshot.as_str()])
        .assert()
        .success()
        .stderr(predicate::str::is_empty());
    let stdout = String::from_utf8(assert.get_output().stdout.clone()).unwrap();
    let context: serde_json::Value = serde_json::from_str(&stdout).unwrap();
    let context = context["context"].as_str().unwrap();
    assert!(context.contains("Agent instruction:\n"));
    assert!(context.contains(&format!("{}...", "x".repeat(500))));
    assert!(context.contains("Agent context:\nagent facts"));
    assert!(context.contains("Tool context:\ntool facts"));
}

#[test]
fn coact_format_helpers_are_live_free_json_projections() {
    let artifacts = r#"[{:name "Skill"
                        :content "Use the tool."}
                       {:content "anonymous"}]"#;
    let assert = Command::cargo_bin("by-rs")
        .unwrap()
        .args(["coact-format", "live-artifacts", "--artifacts", artifacts])
        .assert()
        .success()
        .stderr(predicate::str::is_empty());
    let stdout = String::from_utf8(assert.get_output().stdout.clone()).unwrap();
    let artifacts_out: serde_json::Value = serde_json::from_str(&stdout).unwrap();
    assert_eq!(artifacts_out["projection"], "coact/format-live-artifacts");
    assert_eq!(artifacts_out["artifact-count"], 2);
    let rendered = artifacts_out["rendered"].as_str().unwrap();
    assert!(rendered.contains("### Skill\nUse the tool."));
    assert!(rendered.contains("### artifact\nanonymous"));

    let previous_turns = r#"[{:question "How?"
                             :answer "Done"
                             :depth :full
                             :iterations [{:iteration 1
                                           :code ["(+ 1 2)" "(+ 3 4)"]
                                           :result ["3"]}
                                          {:iteration 2
                                           :code ["(println :ok)"]
                                           :result ["ok"]}]}
                            {:question "Next"
                             :answer ""
                             :depth :summary}]"#;
    let assert = Command::cargo_bin("by-rs")
        .unwrap()
        .args(["coact-format", "previous-turns", "--turns", previous_turns])
        .assert()
        .success()
        .stderr(predicate::str::is_empty());
    let stdout = String::from_utf8(assert.get_output().stdout.clone()).unwrap();
    let turns_out: serde_json::Value = serde_json::from_str(&stdout).unwrap();
    assert_eq!(turns_out["projection"], "coact/format-previous-turns");
    assert_eq!(turns_out["turn-count"], 2);
    let rendered = turns_out["rendered"].as_str().unwrap();
    assert!(rendered.contains("[Turn 1 · full]\n  Q: How?\n  A: Done"));
    assert!(rendered.contains("  Iterations:\n    1. (+ 1 2); (+ 3 4)\n       → 3"));
    assert!(rendered.contains("[Turn 2 · summary]\n  Q: Next"));

    let iterations = r#"[{:iteration 1
                         :channel "tool"
                         :thought "Need data"
                         :tool-results [{:tool-name :read-file
                                         :tool-result "content"}]}
                        {:iteration 2
                         :async-completion? true
                         :eval-results [{:task-id "task-7"
                                         :status :resolved
                                         :from-iteration 1
                                         :result "async result"}]}
                        {:iteration 3
                         :in-flight-roster? true
                         :tasks [{:task-id "task-8"
                                  :lang "clojure"
                                  :submitted-iter 2
                                  :age-s 42}]}]"#;
    let assert = Command::cargo_bin("by-rs")
        .unwrap()
        .args([
            "coact-format",
            "iterations-block",
            "--iterations",
            iterations,
        ])
        .assert()
        .success()
        .stderr(predicate::str::is_empty());
    let stdout = String::from_utf8(assert.get_output().stdout.clone()).unwrap();
    let iterations_out: serde_json::Value = serde_json::from_str(&stdout).unwrap();
    assert_eq!(
        iterations_out["projection"],
        "coact/format-iterations-block"
    );
    let rendered = iterations_out["rendered"].as_str().unwrap();
    assert!(rendered.contains("(1) [tool] Need data => read-file→content"));
    assert!(rendered
        .contains("(2) [↺ async from iter 1] task-id=task-7 status=resolved => async result"));
    assert!(rendered.contains("(3) [🚦 ACTIVE BG] task-8(clj, iter 2, 42s)"));

    let assert = Command::cargo_bin("by-rs")
        .unwrap()
        .args([
            "coact-format",
            "summarize-iterations-deterministic",
            "--iterations",
            "[]",
        ])
        .assert()
        .success()
        .stderr(predicate::str::is_empty());
    let stdout = String::from_utf8(assert.get_output().stdout.clone()).unwrap();
    let summary: serde_json::Value = serde_json::from_str(&stdout).unwrap();
    assert_eq!(
        summary["projection"],
        "coact/summarize-iterations-deterministic"
    );
    assert_eq!(summary["rendered"], "");
}

#[test]
fn debug_agent_promotion_formatters_are_live_free_json_projections() {
    let assert = Command::cargo_bin("by-rs")
        .unwrap()
        .args(["debug-agent", "drift-markers"])
        .assert()
        .success()
        .stderr(predicate::str::is_empty());
    let stdout = String::from_utf8(assert.get_output().stdout.clone()).unwrap();
    let drift: serde_json::Value = serde_json::from_str(&stdout).unwrap();
    assert_eq!(drift["projection"], "debug-agent/clj-nrepl$drift-markers");
    assert_eq!(drift["count"], 0);
    assert_eq!(drift["drifted?"], false);
    assert_eq!(drift["markers"].as_array().unwrap().len(), 0);
    assert_eq!(drift["live-skipped?"], true);

    let assert = Command::cargo_bin("by-rs")
        .unwrap()
        .args([
            "debug-agent",
            "promote-hot-patch",
            "--target-file",
            "src/app.clj",
            "--target-symbol",
            "app/foo",
            "--pattern",
            "(def x 1)",
            "--replacement",
            "(def x 2)",
            "--rationale",
            "Fix stale var.",
            "--validation-evidence",
            "REPL check passed.",
        ])
        .assert()
        .success()
        .stderr(predicate::str::is_empty());
    let stdout = String::from_utf8(assert.get_output().stdout.clone()).unwrap();
    let promotion_skip: serde_json::Value = serde_json::from_str(&stdout).unwrap();
    assert_eq!(
        promotion_skip["projection"],
        "debug-agent/debug$promote-hot-patch"
    );
    assert_eq!(promotion_skip["live-skipped?"], true);
    assert_eq!(promotion_skip["write-skipped?"], true);
    assert_eq!(promotion_skip["drift-count"], 0);
    assert_eq!(promotion_skip["target-file"], "src/app.clj");
    assert_eq!(promotion_skip["target-symbol"], "app/foo");
    assert!(promotion_skip["error"]
        .as_str()
        .unwrap()
        .contains("no drift markers recorded yet"));

    let assert = Command::cargo_bin("by-rs")
        .unwrap()
        .args([
            "debug-agent",
            "promote-hot-patch",
            "--drift-index",
            "7",
            "--target-file",
            "src/app.clj",
            "--target-symbol",
            "app/foo",
            "--pattern",
            "(def x 1)",
            "--replacement",
            "(def x 2)",
            "--rationale",
            "Fix stale var.",
            "--validation-evidence",
            "REPL check passed.",
        ])
        .assert()
        .success()
        .stderr(predicate::str::is_empty());
    let stdout = String::from_utf8(assert.get_output().stdout.clone()).unwrap();
    let promotion_out_of_range: serde_json::Value = serde_json::from_str(&stdout).unwrap();
    assert_eq!(
        promotion_out_of_range["projection"],
        "debug-agent/debug$promote-hot-patch"
    );
    assert_eq!(promotion_out_of_range["drift-index"], 7);
    assert!(promotion_out_of_range["error"]
        .as_str()
        .unwrap()
        .contains("drift-index 7 out of range"));

    let promotion = r#"{:created-at "2026-06-03T00:00:00Z"
                       :debug-session "debug-1"
                       :nrepl-session "nrepl-1"
                       :drift-index 2
                       :target-file "src/app.clj"
                       :target-symbol "app/foo"
                       :edit-mode :pattern
                       :status :proposed
                       :drift-marker {:timestamp "2026-06-03T00:01:00Z"
                                      :code-preview "(alter-var-root #'x inc)"}
                       :pattern "(def x 1)"
                       :replacement "(def x 2)"
                       :rationale "Fix stale var."
                       :validation-evidence "REPL check passed."
                       :artifact-path ".brainyard/agents/debug-agent/promotions/p.md"}"#;

    let assert = Command::cargo_bin("by-rs")
        .unwrap()
        .args(["debug-agent", "frontmatter", "--promotion-json", promotion])
        .assert()
        .success()
        .stderr(predicate::str::is_empty());
    let stdout = String::from_utf8(assert.get_output().stdout.clone()).unwrap();
    let frontmatter: serde_json::Value = serde_json::from_str(&stdout).unwrap();
    assert_eq!(frontmatter["projection"], "debug-agent/format-frontmatter");
    let rendered = frontmatter["rendered"].as_str().unwrap();
    assert!(rendered.starts_with("---\n"));
    assert!(rendered.ends_with("---\n"));
    assert!(rendered.contains("created-at: 2026-06-03T00:00:00Z\n"));
    assert!(rendered.contains("debug-session: debug-1\n"));
    assert!(rendered.contains("nrepl-session: nrepl-1\n"));
    assert!(rendered.contains("drift-marker-index: 2\n"));
    assert!(rendered.contains("target-file: src/app.clj\n"));
    assert!(rendered.contains("target-symbol: app/foo\n"));
    assert!(rendered.contains("edit-mode: pattern\n"));
    assert!(rendered.contains("status: proposed\n"));

    let assert = Command::cargo_bin("by-rs")
        .unwrap()
        .args(["debug-agent", "body", "--promotion-json", promotion])
        .assert()
        .success()
        .stderr(predicate::str::is_empty());
    let stdout = String::from_utf8(assert.get_output().stdout.clone()).unwrap();
    let body: serde_json::Value = serde_json::from_str(&stdout).unwrap();
    assert_eq!(body["projection"], "debug-agent/format-body");
    let rendered = body["rendered"].as_str().unwrap();
    assert!(rendered.contains("## What changed in the live runtime"));
    assert!(rendered.contains("Drift marker recorded at 2026-06-03T00:01:00Z:"));
    assert!(rendered.contains("(alter-var-root #'x inc)"));
    assert!(rendered.contains("## Validation evidence\n\nREPL check passed."));
    assert!(rendered.contains("```clojure\n(def x 1)\n```"));
    assert!(rendered.contains("```clojure\n(def x 2)\n```"));
    assert!(rendered.contains("Fix stale var."));
    assert!(rendered.contains("Saved hot-patch: .brainyard/agents/debug-agent/promotions/p.md"));
    assert!(rendered.contains(
        "Promotion request: bb tui ask \"@.brainyard/agents/debug-agent/promotions/p.md\" -a update-agent"
    ));

    let sparse = r#"{:drift-marker {}
                    :artifact-path "promotion.md"}"#;
    let assert = Command::cargo_bin("by-rs")
        .unwrap()
        .args(["debug-agent", "body", "--promotion-json", sparse])
        .assert()
        .success()
        .stderr(predicate::str::is_empty());
    let stdout = String::from_utf8(assert.get_output().stdout.clone()).unwrap();
    let sparse_body: serde_json::Value = serde_json::from_str(&stdout).unwrap();
    let rendered = sparse_body["rendered"].as_str().unwrap();
    assert!(rendered.contains("<no drift marker — promotion requested without prior mutation>"));
    assert!(rendered.contains("<no validation evidence supplied>"));
    assert!(rendered.contains("<no rationale supplied>"));
}

#[test]
fn memory_entry_id_helper_is_live_free_json_projection() {
    let assert = Command::cargo_bin("by-rs")
        .unwrap()
        .args([
            "memory",
            "entry-id",
            "--layer",
            ":l3",
            "--content",
            "User prefers polylith",
        ])
        .assert()
        .success()
        .stderr(predicate::str::is_empty());
    let stdout = String::from_utf8(assert.get_output().stdout.clone()).unwrap();
    let entry_id: serde_json::Value = serde_json::from_str(&stdout).unwrap();
    assert_eq!(entry_id["projection"], "memory-agent/entry-id-for");
    assert_eq!(entry_id["layer"], "l3");
    assert_eq!(entry_id["normalized"], "user prefers polylith");
    assert_eq!(entry_id["entry-id"], "l3/768f6b2db81a6f38");

    let assert = Command::cargo_bin("by-rs")
        .unwrap()
        .args([
            "memory",
            "entry-id",
            "--layer",
            "l3",
            "--content",
            "User; prefers, polylith!",
        ])
        .assert()
        .success()
        .stderr(predicate::str::is_empty());
    let stdout = String::from_utf8(assert.get_output().stdout.clone()).unwrap();
    let punctuated: serde_json::Value = serde_json::from_str(&stdout).unwrap();
    assert_eq!(punctuated["normalized"], "user prefers polylith ");

    let assert = Command::cargo_bin("by-rs")
        .unwrap()
        .args([
            "memory",
            "remember-preflight",
            "--layer",
            "l2",
            "--content",
            "Remember blue deploy plan",
            "--tag",
            "deploy",
            "--tag",
            "blue",
            "--role",
            "assistant",
            "--session-id",
            "s1",
        ])
        .assert()
        .success()
        .stderr(predicate::str::is_empty());
    let stdout = String::from_utf8(assert.get_output().stdout.clone()).unwrap();
    let preflight: serde_json::Value = serde_json::from_str(&stdout).unwrap();
    assert_eq!(
        preflight["projection"],
        "common.commands/memory$remember-preflight"
    );
    assert_eq!(preflight["write-skipped?"], true);
    assert_eq!(preflight["layer"], "l2");
    assert!(preflight["entry-id"].as_str().unwrap().starts_with("l2/"));
    assert_eq!(preflight["entry"]["kind"], "observation");
    assert_eq!(preflight["entry"]["role"], "assistant");
    assert_eq!(preflight["entry"]["session-id"], "s1");
    assert_eq!(
        preflight["entry"]["tags"],
        serde_json::json!(["blue", "deploy"])
    );

    let assert = Command::cargo_bin("by-rs")
        .unwrap()
        .args([
            "memory",
            "remember-preflight",
            "--layer",
            ":l3",
            "--kind",
            "preference",
            "--content",
            "User prefers Rust",
            "--confidence",
            "0.8",
        ])
        .assert()
        .success()
        .stderr(predicate::str::is_empty());
    let stdout = String::from_utf8(assert.get_output().stdout.clone()).unwrap();
    let l3_preflight: serde_json::Value = serde_json::from_str(&stdout).unwrap();
    assert_eq!(l3_preflight["layer"], "l3");
    assert_eq!(l3_preflight["entry"]["kind"], "preference");
    assert_eq!(l3_preflight["entry"]["confidence"], 0.8);
    assert!(l3_preflight["entry"].get("session-id").is_none());
}

#[test]
fn cache_helpers_are_live_free_json_projections() {
    let assert = Command::cargo_bin("by-rs")
        .unwrap()
        .args(["cache", "threshold", "--query", "short query"])
        .assert()
        .success()
        .stderr(predicate::str::is_empty());
    let stdout = String::from_utf8(assert.get_output().stdout.clone()).unwrap();
    let threshold: serde_json::Value = serde_json::from_str(&stdout).unwrap();
    assert_eq!(threshold["word-count"], 2);
    assert_eq!(threshold["threshold"].as_f64().unwrap(), 0.98);

    let assert = Command::cargo_bin("by-rs")
        .unwrap()
        .args(["cache", "cosine", "--a", "[1 0]", "--b", "[1 1]"])
        .assert()
        .success()
        .stderr(predicate::str::is_empty());
    let stdout = String::from_utf8(assert.get_output().stdout.clone()).unwrap();
    let cosine: serde_json::Value = serde_json::from_str(&stdout).unwrap();
    assert!(
        (cosine["similarity"].as_f64().unwrap() - std::f64::consts::FRAC_1_SQRT_2).abs() < 1e-9
    );
    assert_eq!(cosine["a-dim"], 2);
    assert_eq!(cosine["b-dim"], 2);

    let entries = r#"[
        {:query "cached exact" :response "cached answer" :embedding [1 0] :attributes {:model "m1"}}
        {:query "other" :response "other answer" :embedding [0 1] :attributes {:model "m2"}}
    ]"#;
    let assert = Command::cargo_bin("by-rs")
        .unwrap()
        .args([
            "cache",
            "lookup",
            "--query",
            "cached exact",
            "--embedding",
            "[1 0]",
            "--entries",
            entries,
        ])
        .assert()
        .success()
        .stderr(predicate::str::is_empty());
    let stdout = String::from_utf8(assert.get_output().stdout.clone()).unwrap();
    let lookup: serde_json::Value = serde_json::from_str(&stdout).unwrap();
    assert_eq!(lookup["hit?"], true);
    assert_eq!(lookup["threshold"].as_f64().unwrap(), 0.98);
    assert_eq!(lookup["result"]["response"], "cached answer");
    assert_eq!(lookup["result"]["cached-query"], "cached exact");
    assert_eq!(lookup["result"]["attributes"]["model"], "m1");

    let assert = Command::cargo_bin("by-rs")
        .unwrap()
        .args([
            "cache",
            "stats",
            "--entries",
            entries,
            "--hits",
            "3",
            "--misses",
            "1",
        ])
        .assert()
        .success()
        .stderr(predicate::str::is_empty());
    let stdout = String::from_utf8(assert.get_output().stdout.clone()).unwrap();
    let stats: serde_json::Value = serde_json::from_str(&stdout).unwrap();
    assert_eq!(stats["entries"], 2);
    assert_eq!(stats["hits"], 3);
    assert_eq!(stats["misses"], 1);
    assert_eq!(stats["hit-rate"].as_f64().unwrap(), 0.75);
}

#[test]
fn loop_guard_decision_is_live_free_json_projection() {
    let iterations = r#"[
        {:iteration 1
         :actions [{:tool-name "config$read"
                    :tool-args [{:name :scope :value "project"}]
                    :tool-result "project config"}]}
        {:iteration 2
         :actions [{:tool-name "config$read"
                    :tool-args [{:scope "project"}]
                    :tool-result "latest config"}]}
    ]"#;
    let assert = Command::cargo_bin("by-rs")
        .unwrap()
        .args([
            "loop-guard",
            "decision",
            "--iterations",
            iterations,
            "--tool-name",
            "config$read",
            "--args",
            r#"{:scope "project"}"#,
        ])
        .assert()
        .success()
        .stderr(predicate::str::is_empty());
    let stdout = String::from_utf8(assert.get_output().stdout.clone()).unwrap();
    let decision: serde_json::Value = serde_json::from_str(&stdout).unwrap();
    assert_eq!(decision["blocked?"], true);
    assert_eq!(decision["decision"]["result"], "block");
    assert_eq!(
        decision["decision"]["reason"],
        "Same tool call issued in each of the last 2 iterations."
    );
    assert!(decision["decision"]["answer"]
        .as_str()
        .unwrap()
        .contains("latest config"));

    let varying_args = r#"[
        {:iteration 1
         :actions [{:tool-name "config$read"
                    :tool-args [{:scope "project"}]
                    :tool-result "project config"}]}
        {:iteration 2
         :actions [{:tool-name "config$read"
                    :tool-args [{:scope "user"}]
                    :tool-result "user config"}]}
    ]"#;
    let assert = Command::cargo_bin("by-rs")
        .unwrap()
        .args([
            "loop-guard",
            "decision",
            "--iterations",
            varying_args,
            "--tool-name",
            "config$read",
            "--args",
            r#"{:scope "system"}"#,
        ])
        .assert()
        .success()
        .stderr(predicate::str::is_empty());
    let stdout = String::from_utf8(assert.get_output().stdout.clone()).unwrap();
    let decision: serde_json::Value = serde_json::from_str(&stdout).unwrap();
    assert_eq!(decision["blocked?"], true);
    assert!(decision["decision"]["reason"]
        .as_str()
        .unwrap()
        .contains("Idempotent tool 'config$read' already called 2 time(s)"));

    let assert = Command::cargo_bin("by-rs")
        .unwrap()
        .args([
            "loop-guard",
            "decision",
            "--iterations",
            varying_args,
            "--tool-name",
            "config$read",
            "--args",
            r#"{:scope "system"}"#,
            "--depth",
            "1",
        ])
        .assert()
        .success()
        .stderr(predicate::str::is_empty());
    let stdout = String::from_utf8(assert.get_output().stdout.clone()).unwrap();
    let allowed: serde_json::Value = serde_json::from_str(&stdout).unwrap();
    assert_eq!(allowed["blocked?"], false);
    assert!(allowed["decision"].is_null());
}

#[test]
fn schema_artifact_and_rag_helpers_are_live_free_json_projections() {
    let assert = Command::cargo_bin("by-rs")
        .unwrap()
        .args(["schema", "list", "--query", "tool"])
        .assert()
        .success()
        .stderr(predicate::str::is_empty());
    let stdout = String::from_utf8(assert.get_output().stdout.clone()).unwrap();
    let schemas: serde_json::Value = serde_json::from_str(&stdout).unwrap();
    assert!(schemas["count"].as_u64().unwrap() >= 6);
    let names = schemas["schemas"]
        .as_array()
        .unwrap()
        .iter()
        .map(|item| item["name"].as_str().unwrap())
        .collect::<Vec<_>>();
    assert!(names.contains(&"tool-spec"));
    assert!(names.contains(&"tool-results"));

    let assert = Command::cargo_bin("by-rs")
        .unwrap()
        .args(["schema", "read", "--name", "::conversation"])
        .assert()
        .success()
        .stderr(predicate::str::is_empty());
    let stdout = String::from_utf8(assert.get_output().stdout.clone()).unwrap();
    let schema: serde_json::Value = serde_json::from_str(&stdout).unwrap();
    assert_eq!(schema["name"], "conversation");
    assert_eq!(
        schema["qualified-name"],
        "ai.brainyard.agent.common.schema/conversation"
    );
    assert!(schema["form"]
        .as_str()
        .unwrap()
        .contains("History of conversation"));

    let artifacts = r#"[
        {:artifact-type :chart :artifact-data {:chart-spec {:type "bar" :x [1]}}}
        {:artifact-type :image :artifact-data {:url "file://old.png"}}
    ]"#;
    let replacement =
        r#"{:artifact-type :chart :artifact-data {:chart-spec {:type "bar" :x [1]} :title "new"}}"#;
    let assert = Command::cargo_bin("by-rs")
        .unwrap()
        .args([
            "artifact",
            "add",
            "--artifacts",
            artifacts,
            "--artifact",
            replacement,
        ])
        .assert()
        .success()
        .stderr(predicate::str::is_empty());
    let stdout = String::from_utf8(assert.get_output().stdout.clone()).unwrap();
    let artifact_result: serde_json::Value = serde_json::from_str(&stdout).unwrap();
    assert_eq!(artifact_result["count"], 2);
    assert_eq!(artifact_result["removed-count"], 1);
    assert_eq!(artifact_result["replaced?"], true);
    assert_eq!(
        artifact_result["artifacts"][1]["artifact-data"]["title"],
        "new"
    );

    let assert = Command::cargo_bin("by-rs")
        .unwrap()
        .args(["rag", "cosine", "--a", "[1 0]", "--b", "[1 1]"])
        .assert()
        .success()
        .stderr(predicate::str::is_empty());
    let stdout = String::from_utf8(assert.get_output().stdout.clone()).unwrap();
    let cosine: serde_json::Value = serde_json::from_str(&stdout).unwrap();
    assert!(
        (cosine["similarity"].as_f64().unwrap() - std::f64::consts::FRAC_1_SQRT_2).abs() < 1e-9
    );

    let documents = r#"[
        {:id "doc-a" :text "alpha document" :embedding [1 0] :metadata {:kind "alpha"}}
        {:id "doc-b" :text "beta document" :embedding [0 1] :metadata {:kind "beta"}}
    ]"#;
    let assert = Command::cargo_bin("by-rs")
        .unwrap()
        .args([
            "rag",
            "search",
            "--query",
            "alpha",
            "--embedding",
            "[1 0]",
            "--documents",
            documents,
            "--threshold",
            "0.7",
            "--top-k",
            "1",
        ])
        .assert()
        .success()
        .stderr(predicate::str::is_empty());
    let stdout = String::from_utf8(assert.get_output().stdout.clone()).unwrap();
    let search: serde_json::Value = serde_json::from_str(&stdout).unwrap();
    assert_eq!(
        search["projection"],
        "common.rag-commands/rag-command$search"
    );
    assert_eq!(search["count"], 1);
    assert_eq!(search["query"], "alpha");
    assert_eq!(search["results"][0]["id"], "doc-a");
    assert_eq!(search["results"][0]["metadata"]["kind"], "alpha");

    let assert = Command::cargo_bin("by-rs")
        .unwrap()
        .args([
            "rag",
            "add-document-preview",
            "--text",
            "alpha document",
            "--metadata",
            r#"{:kind "alpha"}"#,
            "--id",
            "doc-a",
            "--embedding",
            "[1 0]",
            "--timestamp",
            "1710000000000",
        ])
        .assert()
        .success()
        .stderr(predicate::str::is_empty());
    let stdout = String::from_utf8(assert.get_output().stdout.clone()).unwrap();
    let add_doc: serde_json::Value = serde_json::from_str(&stdout).unwrap();
    assert_eq!(
        add_doc["projection"],
        "common.rag-commands/rag-command$add-document-preview"
    );
    assert_eq!(add_doc["success"], true);
    assert_eq!(add_doc["id"], "doc-a");
    assert_eq!(add_doc["text-length"], 14);
    assert_eq!(add_doc["document"]["metadata"]["kind"], "alpha");
    assert_eq!(add_doc["document"]["embedding"][0], 1.0);
    assert_eq!(add_doc["document"]["timestamp"], 1710000000000_i64);
    assert_eq!(add_doc["store-skipped?"], true);

    let assert = Command::cargo_bin("by-rs")
        .unwrap()
        .args(["rag", "stats", "--documents", documents, "--configured"])
        .assert()
        .success()
        .stderr(predicate::str::is_empty());
    let stdout = String::from_utf8(assert.get_output().stdout.clone()).unwrap();
    let stats: serde_json::Value = serde_json::from_str(&stdout).unwrap();
    assert_eq!(stats["projection"], "common.rag-commands/rag-command$stats");
    assert_eq!(stats["configured"], true);
    assert_eq!(stats["document-count"], 2);
    assert_eq!(stats["total-chars"], 27);

    let assert = Command::cargo_bin("by-rs")
        .unwrap()
        .args(["rag", "clear", "--documents", documents])
        .assert()
        .success()
        .stderr(predicate::str::is_empty());
    let stdout = String::from_utf8(assert.get_output().stdout.clone()).unwrap();
    let cleared: serde_json::Value = serde_json::from_str(&stdout).unwrap();
    assert_eq!(
        cleared["projection"],
        "common.rag-commands/rag-command$clear"
    );
    assert_eq!(cleared["success"], true);
    assert_eq!(cleared["cleared"], 2);
    assert_eq!(cleared["documents"].as_array().unwrap().len(), 0);
    assert_eq!(cleared["store-skipped?"], true);
}

#[test]
fn previous_turns_helpers_are_live_free_json_projections() {
    let existing = r#"[
        {:question "q1" :answer "a1" :iterations [{:code "old"}]}
        {:question "q2" :answer "a2" :iterations [{:code "mid"}]}
        {:question "q3" :answer "a3" :iterations [{:code "recent"}]}
    ]"#;
    let assert = Command::cargo_bin("by-rs")
        .unwrap()
        .args([
            "previous-turns",
            "append",
            "--existing",
            existing,
            "--turn",
            r#"{:question "q4" :answer "a4" :iterations [{:code "new"}]}"#,
            "--max-turns",
            "3",
            "--full-depth",
            "1",
            "--summary-depth",
            "1",
        ])
        .assert()
        .success()
        .stderr(predicate::str::is_empty());
    let stdout = String::from_utf8(assert.get_output().stdout.clone()).unwrap();
    let appended: serde_json::Value = serde_json::from_str(&stdout).unwrap();
    assert_eq!(appended["count"], 3);
    assert_eq!(appended["truncation-mode"], "inline-no-file");
    assert_eq!(appended["depth-counts"]["minimal"], 1);
    assert_eq!(appended["depth-counts"]["summary"], 1);
    assert_eq!(appended["depth-counts"]["full"], 1);
    assert_eq!(appended["turns"][0]["question"], "q2");
    assert_eq!(appended["turns"][0]["depth"], "minimal");
    assert!(appended["turns"][0]["iterations"].is_null());
    assert_eq!(appended["turns"][1]["depth"], "summary");
    assert!(appended["turns"][1]["iterations"].is_null());
    assert_eq!(appended["turns"][2]["depth"], "full");
    assert_eq!(appended["turns"][2]["iterations"][0]["code"], "new");

    let appended_turns = appended["turns"].to_string();
    let assert = Command::cargo_bin("by-rs")
        .unwrap()
        .args(["previous-turns", "estimate", "--turns"])
        .arg(&appended_turns)
        .assert()
        .success()
        .stderr(predicate::str::is_empty());
    let stdout = String::from_utf8(assert.get_output().stdout.clone()).unwrap();
    let estimate: serde_json::Value = serde_json::from_str(&stdout).unwrap();
    assert_eq!(estimate["count"], 3);
    assert!(estimate["chars"].as_u64().unwrap() > 0);
    assert!(estimate["tokens"].as_u64().unwrap() > 0);
    assert_eq!(estimate["truncation-mode"], "inline-no-file");

    let long_turns = (0..12)
        .map(|i| {
            serde_json::json!({
                "question": format!("q{i}"),
                "answer": "x".repeat(1200),
                "iterations": [{"code": format!("step-{i}")}],
            })
        })
        .collect::<Vec<_>>();
    let long_turns = serde_json::to_string(&long_turns).unwrap();
    let assert = Command::cargo_bin("by-rs")
        .unwrap()
        .args([
            "previous-turns",
            "compact",
            "--turns",
            long_turns.as_str(),
            "--target-tokens",
            "1",
        ])
        .assert()
        .success()
        .stderr(predicate::str::is_empty());
    let stdout = String::from_utf8(assert.get_output().stdout.clone()).unwrap();
    let compacted: serde_json::Value = serde_json::from_str(&stdout).unwrap();
    assert_eq!(compacted["count"], 10);
    assert_eq!(compacted["turns"][0]["question"], "q2");
    assert_eq!(compacted["turns"][0]["depth"], "minimal");
    assert_eq!(compacted["depth-counts"]["minimal"], 10);
    assert!(compacted["after-tokens"].as_u64().unwrap() > 0);
    assert!(
        compacted["before-tokens"].as_u64().unwrap() >= compacted["after-tokens"].as_u64().unwrap()
    );
    assert!(compacted["turns"][0]["answer"]
        .as_str()
        .unwrap()
        .contains("TRUNCATED inline"));
}

#[test]
fn context_budget_helpers_are_live_free_json_projections() {
    let assert = Command::cargo_bin("by-rs")
        .unwrap()
        .args(["context-budget", "estimate", "--text", "abcd"])
        .assert()
        .success()
        .stderr(predicate::str::is_empty());
    let stdout = String::from_utf8(assert.get_output().stdout.clone()).unwrap();
    let estimate: serde_json::Value = serde_json::from_str(&stdout).unwrap();
    assert_eq!(estimate["chars"], 4);
    assert_eq!(estimate["tokens"], 1);

    let sections = r#"{
        :role "abcd"
        :previous-turns "xxxxxxxx"
        :tools "tool text"
        :footer ""
    }"#;
    let order = "[:role :footer :previous-turns :tools]";
    let assert = Command::cargo_bin("by-rs")
        .unwrap()
        .args([
            "context-budget",
            "section-tokens",
            "--sections",
            sections,
            "--order",
            order,
        ])
        .assert()
        .success()
        .stderr(predicate::str::is_empty());
    let stdout = String::from_utf8(assert.get_output().stdout.clone()).unwrap();
    let tokens: serde_json::Value = serde_json::from_str(&stdout).unwrap();
    assert_eq!(tokens["section-tokens"]["role"], 1);
    assert_eq!(tokens["section-tokens"]["footer"], 0);
    assert_eq!(tokens["section-tokens"]["previous-turns"], 2);
    assert_eq!(tokens["section-tokens"]["tools"], 3);
    assert_eq!(tokens["total-tokens"], 6);

    let assert = Command::cargo_bin("by-rs")
        .unwrap()
        .args([
            "context-budget",
            "compose",
            "--sections",
            sections,
            "--order",
            order,
        ])
        .assert()
        .success()
        .stderr(predicate::str::is_empty());
    let stdout = String::from_utf8(assert.get_output().stdout.clone()).unwrap();
    let composed: serde_json::Value = serde_json::from_str(&stdout).unwrap();
    assert_eq!(composed["text"], "abcd\n\nxxxxxxxx\n\ntool text");
    assert_eq!(composed["tokens"], 7);

    let assert = Command::cargo_bin("by-rs")
        .unwrap()
        .args([
            "context-budget",
            "model-budget",
            "--max-context-tokens",
            "100",
            "--max-output-tokens",
            "10",
            "--safety-ratio",
            "0.1",
        ])
        .assert()
        .success()
        .stderr(predicate::str::is_empty());
    let stdout = String::from_utf8(assert.get_output().stdout.clone()).unwrap();
    let model_budget: serde_json::Value = serde_json::from_str(&stdout).unwrap();
    assert_eq!(model_budget["budget"], 81);

    let assert = Command::cargo_bin("by-rs")
        .unwrap()
        .args(["context-budget", "policies"])
        .assert()
        .success()
        .stderr(predicate::str::is_empty());
    let stdout = String::from_utf8(assert.get_output().stdout.clone()).unwrap();
    let policies: serde_json::Value = serde_json::from_str(&stdout).unwrap();
    let previous_turns = policies["policies"]
        .as_array()
        .unwrap()
        .iter()
        .find(|policy| policy["section"] == "previous-turns")
        .unwrap();
    assert_eq!(previous_turns["priority"], 50);
    assert_eq!(previous_turns["compact"], "bump-previous-turns");

    let assert = Command::cargo_bin("by-rs")
        .unwrap()
        .args([
            "context-budget",
            "compactable",
            "--sections",
            sections,
            "--order",
            order,
            "--strategies",
            "[:tools-tier :bump-previous-turns]",
        ])
        .assert()
        .success()
        .stderr(predicate::str::is_empty());
    let stdout = String::from_utf8(assert.get_output().stdout.clone()).unwrap();
    let compactable: serde_json::Value = serde_json::from_str(&stdout).unwrap();
    assert_eq!(compactable["count"], 2);
    assert_eq!(compactable["compactable"][0]["section"], "previous-turns");
    assert_eq!(
        compactable["compactable"][0]["strategy"],
        "bump-previous-turns"
    );
    assert_eq!(compactable["compactable"][1]["section"], "tools");
    assert_eq!(compactable["compactable"][1]["strategy"], "tools-tier");
}

#[test]
fn tool_helpers_are_live_free_json_projections() {
    let input_schema = r#"[:map
        [:count {:optional true} [:int {:desc "Count" :default 1}]]
        [:debug [:boolean {:desc "Debug flag"}]]
        [:mode [:keyword {:desc "Mode"}]]
        [:items [:vector :string]]
    ]"#;
    let assert = Command::cargo_bin("by-rs")
        .unwrap()
        .args(["tool", "schema-type", "--input-schema", input_schema])
        .assert()
        .success()
        .stderr(predicate::str::is_empty());
    let stdout = String::from_utf8(assert.get_output().stdout.clone()).unwrap();
    let schema_type: serde_json::Value = serde_json::from_str(&stdout).unwrap();
    assert_eq!(schema_type["count"], 4);
    assert_eq!(schema_type["projection"], "malli-json-schema-subset");
    assert_eq!(schema_type["properties"]["count"]["type"], "integer");
    assert_eq!(schema_type["properties"]["count"]["desc"], "Count");
    assert_eq!(schema_type["properties"]["count"]["default"], 1);
    assert_eq!(schema_type["properties"]["count"]["optional"], true);
    assert_eq!(schema_type["properties"]["debug"]["type"], "boolean");
    assert_eq!(schema_type["properties"]["mode"]["type"], "string");
    assert_eq!(schema_type["properties"]["items"]["type"], "array");
    assert_eq!(
        schema_type["properties"]["items"]["items"]["type"],
        "string"
    );

    let assert = Command::cargo_bin("by-rs")
        .unwrap()
        .args(["tool", "inputs-schema", "--input-schema", input_schema])
        .assert()
        .success()
        .stderr(predicate::str::is_empty());
    let stdout = String::from_utf8(assert.get_output().stdout.clone()).unwrap();
    let inputs_schema: serde_json::Value = serde_json::from_str(&stdout).unwrap();
    assert_eq!(inputs_schema["entries"], 4);
    assert_eq!(inputs_schema["input-schema"][1][2][0], "maybe");
    assert_eq!(inputs_schema["input-schema"][1][2][1][0], "int");

    let standard_args = r#"[
        {:name "count" :value "3"}
        {:name "debug" :value "true"}
        {:name "mode" :value ":nrepl"}
        {:name "items" :value "[1,2]"}
    ]"#;
    let assert = Command::cargo_bin("by-rs")
        .unwrap()
        .args(["tool", "normalize-args", "--args", standard_args])
        .assert()
        .success()
        .stderr(predicate::str::is_empty());
    let stdout = String::from_utf8(assert.get_output().stdout.clone()).unwrap();
    let normalized: serde_json::Value = serde_json::from_str(&stdout).unwrap();
    assert_eq!(normalized["count"], 4);
    assert_eq!(normalized["args"]["count"], "3");
    assert_eq!(normalized["args"]["debug"], "true");
    assert_eq!(normalized["args"]["mode"], ":nrepl");

    let compact_args = r#"[{:count "7"} {:debug "false"}]"#;
    let assert = Command::cargo_bin("by-rs")
        .unwrap()
        .args(["tool", "normalize-args", "--args", compact_args])
        .assert()
        .success()
        .stderr(predicate::str::is_empty());
    let stdout = String::from_utf8(assert.get_output().stdout.clone()).unwrap();
    let compact_normalized: serde_json::Value = serde_json::from_str(&stdout).unwrap();
    assert_eq!(compact_normalized["args"]["count"], "7");
    assert_eq!(compact_normalized["args"]["debug"], "false");

    let properties = r#"{
        :count {:type "integer"}
        :debug {:type "boolean"}
        :mode {:type "keyword"}
        :items {:type "array"}
        :config {:type "object"}
    }"#;
    let args = r#"[
        {:name "count" :value "3"}
        {:name "debug" :value "true"}
        {:name "mode" :value ":nrepl"}
        {:name "items" :value "[1,2]"}
        {:name "config" :value "{\"x\":1}"}
    ]"#;
    let assert = Command::cargo_bin("by-rs")
        .unwrap()
        .args([
            "tool",
            "coerce-args",
            "--args",
            args,
            "--properties",
            properties,
        ])
        .assert()
        .success()
        .stderr(predicate::str::is_empty());
    let stdout = String::from_utf8(assert.get_output().stdout.clone()).unwrap();
    let coerced: serde_json::Value = serde_json::from_str(&stdout).unwrap();
    assert_eq!(coerced["count"], 5);
    assert_eq!(coerced["args"]["count"], 3);
    assert_eq!(coerced["args"]["debug"], true);
    assert_eq!(coerced["args"]["mode"], "nrepl");
    assert_eq!(coerced["args"]["items"][0], 1);
    assert_eq!(coerced["args"]["config"]["x"], 1);

    let assert = Command::cargo_bin("by-rs")
        .unwrap()
        .args([
            "tool",
            "coerce-value",
            "--value",
            ":nrepl",
            "--type",
            "keyword",
        ])
        .assert()
        .success()
        .stderr(predicate::str::is_empty());
    let stdout = String::from_utf8(assert.get_output().stdout.clone()).unwrap();
    let value: serde_json::Value = serde_json::from_str(&stdout).unwrap();
    assert_eq!(value["value"], "nrepl");
    assert_eq!(value["keyword-projection"], true);

    let hidden = r#"{:meta {:tool-use-control {:visibility :hidden}}}"#;
    let assert = Command::cargo_bin("by-rs")
        .unwrap()
        .args([
            "tool",
            "visible",
            "--tool-def",
            hidden,
            "--agent-id",
            "react-agent",
        ])
        .assert()
        .success()
        .stderr(predicate::str::is_empty());
    let stdout = String::from_utf8(assert.get_output().stdout.clone()).unwrap();
    let visible: serde_json::Value = serde_json::from_str(&stdout).unwrap();
    assert_eq!(visible["visible?"], false);

    let allow = r#"{:meta {:tool-use-control {:allow ["react-*"]}}}"#;
    let assert = Command::cargo_bin("by-rs")
        .unwrap()
        .args([
            "tool",
            "visible",
            "--tool-def",
            allow,
            "--agent-id",
            ":react-agent",
        ])
        .assert()
        .success()
        .stderr(predicate::str::is_empty());
    let stdout = String::from_utf8(assert.get_output().stdout.clone()).unwrap();
    let visible: serde_json::Value = serde_json::from_str(&stdout).unwrap();
    assert_eq!(visible["visible?"], true);

    let assert = Command::cargo_bin("by-rs")
        .unwrap()
        .args([
            "tool",
            "visible",
            "--tool-def",
            allow,
            "--agent-id",
            "coact-agent",
        ])
        .assert()
        .success()
        .stderr(predicate::str::is_empty());
    let stdout = String::from_utf8(assert.get_output().stdout.clone()).unwrap();
    let visible: serde_json::Value = serde_json::from_str(&stdout).unwrap();
    assert_eq!(visible["visible?"], false);

    let deny = r#"{:tool-use-control {:deny ["coact-*"]}}"#;
    let assert = Command::cargo_bin("by-rs")
        .unwrap()
        .args([
            "tool",
            "visible",
            "--tool-def",
            deny,
            "--agent-id",
            "coact-agent",
        ])
        .assert()
        .success()
        .stderr(predicate::str::is_empty());
    let stdout = String::from_utf8(assert.get_output().stdout.clone()).unwrap();
    let visible: serde_json::Value = serde_json::from_str(&stdout).unwrap();
    assert_eq!(visible["visible?"], false);
}

#[test]
fn tool_edit_helpers_are_live_free_json_projections() {
    let assert = Command::cargo_bin("by-rs")
        .unwrap()
        .args([
            "tool-edit",
            "replace",
            "--content",
            "alpha beta beta",
            "--pattern",
            "beta",
            "--replacement",
            "$1",
        ])
        .assert()
        .success()
        .stderr(predicate::str::is_empty());
    let stdout = String::from_utf8(assert.get_output().stdout.clone()).unwrap();
    let literal: serde_json::Value = serde_json::from_str(&stdout).unwrap();
    assert_eq!(literal["projection"], "common.tools/apply-replacement");
    assert_eq!(literal["replaced"], 1);
    assert_eq!(literal["new-content"], "alpha $1 beta");

    let assert = Command::cargo_bin("by-rs")
        .unwrap()
        .args([
            "tool-edit",
            "replace",
            "--content",
            "abc 123 def 456",
            "--pattern",
            r"(\d+)",
            "--replacement",
            "NUM-$1",
            "--regex",
            "--all",
        ])
        .assert()
        .success()
        .stderr(predicate::str::is_empty());
    let stdout = String::from_utf8(assert.get_output().stdout.clone()).unwrap();
    let regex_replace: serde_json::Value = serde_json::from_str(&stdout).unwrap();
    assert_eq!(regex_replace["replaced"], 2);
    assert_eq!(regex_replace["new-content"], "abc NUM-123 def NUM-456");

    let assert = Command::cargo_bin("by-rs")
        .unwrap()
        .args([
            "tool-edit",
            "fallback-diff",
            "--old",
            "a\nb\nc",
            "--new",
            "a\nB\nc\nd",
            "--path",
            "sample.txt",
        ])
        .assert()
        .success()
        .stderr(predicate::str::is_empty());
    let stdout = String::from_utf8(assert.get_output().stdout.clone()).unwrap();
    let diff: serde_json::Value = serde_json::from_str(&stdout).unwrap();
    assert_eq!(diff["diff-source"], "fallback");
    assert!(diff["diff"]
        .as_str()
        .unwrap()
        .contains("--- sample.txt (old)"));
    assert!(diff["diff"].as_str().unwrap().contains("- b\n+ B"));
    assert!(diff["diff"].as_str().unwrap().contains("+ d"));

    let assert = Command::cargo_bin("by-rs")
        .unwrap()
        .args([
            "tool-edit",
            "match-query",
            "--query",
            "ai brainyard rs",
            "--text",
            "Brainyard native AI migration",
        ])
        .assert()
        .success()
        .stderr(predicate::str::is_empty());
    let stdout = String::from_utf8(assert.get_output().stdout.clone()).unwrap();
    let matched: serde_json::Value = serde_json::from_str(&stdout).unwrap();
    assert_eq!(matched["projection"], "common.tools/search-token-match");
    assert_eq!(matched["tokens"], serde_json::json!(["brainyard"]));
    assert_eq!(matched["matches?"], true);
}

#[test]
fn init_doc_helpers_are_live_free_json_projections() {
    let body = "Intro\n## build   & run\nbb test\n## Notes\nkeep";
    let assert = Command::cargo_bin("by-rs")
        .unwrap()
        .args(["init-doc", "parse-sections", "--body", body])
        .assert()
        .success()
        .stderr(predicate::str::is_empty());
    let stdout = String::from_utf8(assert.get_output().stdout.clone()).unwrap();
    let parsed: serde_json::Value = serde_json::from_str(&stdout).unwrap();
    assert_eq!(parsed["projection"], "common.init/parse-sections");
    assert_eq!(parsed["section-count"], 3);
    assert_eq!(parsed["sections"][0]["heading"], serde_json::Value::Null);
    assert_eq!(parsed["sections"][1]["heading"], "Build & Run");
    assert_eq!(parsed["sections"][2]["heading"], "Notes");

    let assert = Command::cargo_bin("by-rs")
        .unwrap()
        .args([
            "init-doc",
            "section-delta",
            "--before",
            "## Overview\nold\n## Notes\nkeep",
            "--after",
            "## Overview\nnew\n## Build & Run\nbb test\n## Notes\nkeep",
        ])
        .assert()
        .success()
        .stderr(predicate::str::is_empty());
    let stdout = String::from_utf8(assert.get_output().stdout.clone()).unwrap();
    let delta: serde_json::Value = serde_json::from_str(&stdout).unwrap();
    assert_eq!(
        delta["structural"]["added"],
        serde_json::json!(["build & run"])
    );
    assert_eq!(delta["structural"]["removed"], serde_json::json!([]));
    assert_eq!(
        delta["structural"]["changed"],
        serde_json::json!(["overview"])
    );

    let assert = Command::cargo_bin("by-rs")
        .unwrap()
        .args([
            "init-doc",
            "secret-scan",
            "--body",
            "never paste sk-ABCDEFGHIJKLMNOPQRST here",
        ])
        .assert()
        .success()
        .stderr(predicate::str::is_empty());
    let stdout = String::from_utf8(assert.get_output().stdout.clone()).unwrap();
    let secrets: serde_json::Value = serde_json::from_str(&stdout).unwrap();
    assert_eq!(secrets["secret-detected?"], true);
    assert_eq!(secrets["matches"][0], "sk-ABCDE…");

    let assert = Command::cargo_bin("by-rs")
        .unwrap()
        .args([
            "init-doc",
            "validate-apply",
            "--op",
            "append",
            "--current",
            "## Notes\nkeep",
            "--body",
            "## Notes\nkeep\nextra",
            "--reason",
            "append note",
            "--auto",
        ])
        .assert()
        .success()
        .stderr(predicate::str::is_empty());
    let stdout = String::from_utf8(assert.get_output().stdout.clone()).unwrap();
    let valid: serde_json::Value = serde_json::from_str(&stdout).unwrap();
    assert_eq!(valid["ok?"], true);
    assert_eq!(valid["auto-ok?"], true);
    assert_eq!(
        valid["projection"],
        "common.init/init$apply-validate-prefix"
    );

    let assert = Command::cargo_bin("by-rs")
        .unwrap()
        .args([
            "init-doc",
            "validate-apply",
            "--op",
            "curate",
            "--current",
            "## Notes\nkeep",
            "--body",
            "## Notes\nchanged",
            "--reason",
            "curate",
            "--confirm",
        ])
        .assert()
        .success()
        .stderr(predicate::str::is_empty());
    let stdout = String::from_utf8(assert.get_output().stdout.clone()).unwrap();
    let invalid: serde_json::Value = serde_json::from_str(&stdout).unwrap();
    assert_eq!(invalid["ok?"], false);
    assert_eq!(invalid["stage"], "notes-modified");

    let assert = Command::cargo_bin("by-rs")
        .unwrap()
        .args([
            "init-doc",
            "frontmatter",
            "--slug",
            "init-doc-test",
            "--question",
            "Port init helpers",
            "--session-id",
            "s1",
            "--scope",
            "user",
            "--brainyard-path",
            "/tmp/BRAINYARD.md",
            "--snapshot",
            "/tmp/snap.md",
            "--writes",
            "1",
            "--next-step",
            "review",
            "--started",
            "2026-06-03T00:00:00Z",
        ])
        .assert()
        .success()
        .stderr(predicate::str::is_empty());
    let stdout = String::from_utf8(assert.get_output().stdout.clone()).unwrap();
    let frontmatter: serde_json::Value = serde_json::from_str(&stdout).unwrap();
    assert_eq!(frontmatter["projection"], "common.init/init$frontmatter");
    let frontmatter_text = frontmatter["frontmatter"].as_str().unwrap();
    assert!(frontmatter_text.contains("agent: init-agent"));
    assert!(frontmatter_text.contains("scope: user"));
    assert!(frontmatter_text.contains("snapshots: [/tmp/snap.md]"));

    let assert = Command::cargo_bin("by-rs")
        .unwrap()
        .args([
            "init-doc",
            "default-sources",
            "--project-dir",
            "/repo",
            "--user-dir",
            "/home/me",
        ])
        .assert()
        .success()
        .stderr(predicate::str::is_empty());
    let stdout = String::from_utf8(assert.get_output().stdout.clone()).unwrap();
    let sources: serde_json::Value = serde_json::from_str(&stdout).unwrap();
    assert_eq!(sources["count"], 6);
    assert_eq!(sources["sources"][0]["resolved-path"], "/repo/CLAUDE.md");
    assert_eq!(
        sources["sources"][4]["resolved-path"],
        "/home/me/.codex/AGENTS.md"
    );

    let project = tempfile::tempdir().unwrap();
    let user = tempfile::tempdir().unwrap();
    std::fs::write(project.path().join("CLAUDE.md"), "# Project rules\n").unwrap();
    std::fs::create_dir_all(user.path().join(".codex")).unwrap();
    std::fs::write(
        user.path().join(".codex").join("AGENTS.md"),
        "# User rules\n",
    )
    .unwrap();
    let assert = Command::cargo_bin("by-rs")
        .unwrap()
        .args([
            "init-doc",
            "detect-sources",
            "--scope",
            "both",
            "--project-dir",
            project.path().to_str().unwrap(),
            "--user-dir",
            user.path().to_str().unwrap(),
        ])
        .assert()
        .success()
        .stderr(predicate::str::is_empty());
    let stdout = String::from_utf8(assert.get_output().stdout.clone()).unwrap();
    let detected: serde_json::Value = serde_json::from_str(&stdout).unwrap();
    assert_eq!(detected["projection"], "common.init/init$detect-sources");
    assert_eq!(detected["read-skipped?"], true);
    assert_eq!(detected["scope"], "both");
    assert_eq!(detected["found"].as_array().unwrap().len(), 2);
    assert_eq!(detected["missing"].as_array().unwrap().len(), 4);
    assert!(detected["found"].as_array().unwrap().iter().any(|source| {
        source["source"] == "claude-md"
            && source["scope"] == "project"
            && source["path"] == project.path().join("CLAUDE.md").display().to_string()
            && source["exists?"] == true
            && source["size"] == 16
            && source.get("mtime").is_some()
    }));
    assert!(detected["found"].as_array().unwrap().iter().any(|source| {
        source["source"] == "agents-md"
            && source["scope"] == "user"
            && source["path"]
                == user
                    .path()
                    .join(".codex")
                    .join("AGENTS.md")
                    .display()
                    .to_string()
            && source["exists?"] == true
    }));

    let assert = Command::cargo_bin("by-rs")
        .unwrap()
        .args([
            "init-doc",
            "parse-snapshot-name",
            "--filename",
            "20260603-120000-project-port-rust.md",
        ])
        .assert()
        .success()
        .stderr(predicate::str::is_empty());
    let stdout = String::from_utf8(assert.get_output().stdout.clone()).unwrap();
    let snapshot: serde_json::Value = serde_json::from_str(&stdout).unwrap();
    assert_eq!(snapshot["valid?"], true);
    assert_eq!(snapshot["parsed"]["scope"], "project");
    assert_eq!(snapshot["parsed"]["reason"], "port-rust");
}

#[test]
fn init_doc_state_previews_are_live_free_json_projections() {
    let project = tempfile::tempdir().unwrap();
    let user = tempfile::tempdir().unwrap();
    let brainyard_dir = project.path().join(".brainyard");
    std::fs::create_dir_all(&brainyard_dir).unwrap();
    let brainyard_file = brainyard_dir.join("BRAINYARD.md");
    let current = "## Overview\nold\n## Notes\nkeep";
    std::fs::write(&brainyard_file, format!("{current}\n")).unwrap();
    let snapshot_dir = brainyard_dir
        .join("agents")
        .join("init-agent")
        .join("snapshots");
    std::fs::create_dir_all(&snapshot_dir).unwrap();
    let snapshot_file = snapshot_dir.join("20260603-120000-project-before.md");
    std::fs::write(&snapshot_file, "## Overview\nsnapshot\n").unwrap();

    let assert = Command::cargo_bin("by-rs")
        .unwrap()
        .args([
            "init-doc",
            "read",
            "--scope",
            "project",
            "--project-dir",
            project.path().to_str().unwrap(),
            "--user-dir",
            user.path().to_str().unwrap(),
        ])
        .assert()
        .success()
        .stderr(predicate::str::is_empty());
    let stdout = String::from_utf8(assert.get_output().stdout.clone()).unwrap();
    let read: serde_json::Value = serde_json::from_str(&stdout).unwrap();
    assert_eq!(read["projection"], "common.init/init$read");
    assert_eq!(read["project"]["scope"], "project");
    assert_eq!(
        read["project"]["path"],
        brainyard_file.display().to_string()
    );
    assert_eq!(read["project"]["exists?"], true);
    assert_eq!(read["project"]["content"], current);
    assert_eq!(read["project"]["sections"].as_array().unwrap().len(), 2);

    let proposed = "## Overview\nnew\n## Notes\nkeep";
    let assert = Command::cargo_bin("by-rs")
        .unwrap()
        .args([
            "init-doc",
            "diff",
            "--scope",
            "project",
            "--proposed",
            proposed,
            "--project-dir",
            project.path().to_str().unwrap(),
            "--user-dir",
            user.path().to_str().unwrap(),
        ])
        .assert()
        .success()
        .stderr(predicate::str::is_empty());
    let stdout = String::from_utf8(assert.get_output().stdout.clone()).unwrap();
    let diff: serde_json::Value = serde_json::from_str(&stdout).unwrap();
    assert_eq!(diff["projection"], "common.init/init$diff");
    assert_eq!(diff["before"], current);
    assert_eq!(diff["after"], proposed);
    assert_eq!(
        diff["structural"]["changed"],
        serde_json::json!(["overview"])
    );

    let assert = Command::cargo_bin("by-rs")
        .unwrap()
        .args([
            "init-doc",
            "list-snapshots",
            "--scope",
            "project",
            "--project-dir",
            project.path().to_str().unwrap(),
            "--user-dir",
            user.path().to_str().unwrap(),
        ])
        .assert()
        .success()
        .stderr(predicate::str::is_empty());
    let stdout = String::from_utf8(assert.get_output().stdout.clone()).unwrap();
    let snapshots: serde_json::Value = serde_json::from_str(&stdout).unwrap();
    assert_eq!(snapshots["projection"], "common.init/init$list-snapshots");
    assert_eq!(snapshots["scope"], "project");
    assert_eq!(
        snapshots["base-dirs"],
        serde_json::json!([brainyard_dir.display().to_string()])
    );
    assert_eq!(
        snapshots["snapshots"][0]["filename"],
        "20260603-120000-project-before.md"
    );
    assert_eq!(snapshots["snapshots"][0]["reason"], "before");

    let assert = Command::cargo_bin("by-rs")
        .unwrap()
        .args([
            "init-doc",
            "smoke-test",
            "--scope",
            "project",
            "--project-dir",
            project.path().to_str().unwrap(),
            "--user-dir",
            user.path().to_str().unwrap(),
        ])
        .assert()
        .success()
        .stderr(predicate::str::is_empty());
    let stdout = String::from_utf8(assert.get_output().stdout.clone()).unwrap();
    let smoke: serde_json::Value = serde_json::from_str(&stdout).unwrap();
    assert_eq!(smoke["projection"], "common.init/init$smoke-test");
    assert_eq!(smoke["reload-skipped?"], true);
    assert_eq!(smoke["project"]["ok?"], true);
    assert_eq!(smoke["project"]["section-count"], 2);

    let assert = Command::cargo_bin("by-rs")
        .unwrap()
        .args([
            "init-doc",
            "snapshot-preview",
            "--scope",
            "project",
            "--reason",
            "manual edit",
            "--timestamp",
            "20260603-130000",
            "--project-dir",
            project.path().to_str().unwrap(),
            "--user-dir",
            user.path().to_str().unwrap(),
        ])
        .assert()
        .success()
        .stderr(predicate::str::is_empty());
    let stdout = String::from_utf8(assert.get_output().stdout.clone()).unwrap();
    let snapshot: serde_json::Value = serde_json::from_str(&stdout).unwrap();
    assert_eq!(snapshot["projection"], "common.init/init$snapshot-preview");
    assert_eq!(snapshot["write-skipped?"], true);
    assert_eq!(snapshot["content"], current);
    assert!(snapshot["path"]
        .as_str()
        .unwrap()
        .ends_with("20260603-130000-project-manual-edit.md"));
    assert!(!Path::new(snapshot["path"].as_str().unwrap()).exists());

    let assert = Command::cargo_bin("by-rs")
        .unwrap()
        .args([
            "init-doc",
            "revert-preview",
            "--snapshot-path",
            snapshot_file.to_str().unwrap(),
            "--timestamp",
            "20260603-140000",
            "--project-dir",
            project.path().to_str().unwrap(),
            "--user-dir",
            user.path().to_str().unwrap(),
        ])
        .assert()
        .success()
        .stderr(predicate::str::is_empty());
    let stdout = String::from_utf8(assert.get_output().stdout.clone()).unwrap();
    let revert: serde_json::Value = serde_json::from_str(&stdout).unwrap();
    assert_eq!(revert["projection"], "common.init/init$revert-preview");
    assert_eq!(revert["write-skipped?"], true);
    assert_eq!(revert["content"], "## Overview\nsnapshot");
    assert_eq!(revert["dest"], brainyard_file.display().to_string());
    assert!(revert["pre-revert-snapshot"]
        .as_str()
        .unwrap()
        .ends_with("20260603-140000-project-revert-before.md"));
    assert_eq!(
        std::fs::read_to_string(&brainyard_file).unwrap(),
        format!("{current}\n")
    );

    let proposed_append = format!("{current}\nextra");
    let assert = Command::cargo_bin("by-rs")
        .unwrap()
        .args([
            "init-doc",
            "apply-preview",
            "--scope",
            "project",
            "--op",
            "append",
            "--body",
            &proposed_append,
            "--reason",
            "append note",
            "--auto",
            "--timestamp",
            "20260603-150000",
            "--project-dir",
            project.path().to_str().unwrap(),
            "--user-dir",
            user.path().to_str().unwrap(),
        ])
        .assert()
        .success()
        .stderr(predicate::str::is_empty());
    let stdout = String::from_utf8(assert.get_output().stdout.clone()).unwrap();
    let apply: serde_json::Value = serde_json::from_str(&stdout).unwrap();
    assert_eq!(apply["projection"], "common.init/init$apply-preview");
    assert_eq!(apply["ok?"], true);
    assert_eq!(apply["write-skipped?"], true);
    assert_eq!(apply["snapshot-skipped?"], true);
    assert_eq!(apply["dossier-skipped?"], true);
    assert_eq!(apply["validated-prefix"]["ok?"], true);
    assert_eq!(apply["validated-prefix"]["auto-ok?"], true);
    assert!(apply["snapshot-path"]
        .as_str()
        .unwrap()
        .ends_with("20260603-150000-project-append-note.md"));
    assert!(apply["dossier-path"]
        .as_str()
        .unwrap()
        .ends_with("20260603-150000-append-note.md"));
    assert_eq!(
        std::fs::read_to_string(&brainyard_file).unwrap(),
        format!("{current}\n")
    );
}

#[test]
fn task_format_helpers_are_live_free_json_projections() {
    let tasks = r#"
      [
        {"id":"task-2","status":"running","job-type":"bash","name":"long running command","created-at":2000,"started-at":2000},
        {"id":"task-1","status":"completed","job-type":"tool","name":"short task","created-at":1000,"started-at":1000,"completed-at":2500}
      ]
    "#;
    let assert = Command::cargo_bin("by-rs")
        .unwrap()
        .args([
            "task-format",
            "list",
            "--tasks-json",
            tasks,
            "--now-ms",
            "3200",
        ])
        .assert()
        .success()
        .stderr(predicate::str::is_empty());
    let stdout = String::from_utf8(assert.get_output().stdout.clone()).unwrap();
    let list: serde_json::Value = serde_json::from_str(&stdout).unwrap();
    assert_eq!(list["projection"], "task.format/format-task-list");
    assert_eq!(list["rows"][0]["id"], "task-1");
    assert_eq!(list["rows"][0]["elapsed"], "1.5s");
    assert_eq!(list["rows"][1]["elapsed"], "1.2s");
    assert!(list["rendered"].as_str().unwrap().contains("Tasks\n"));

    let task = r#"{"id":"task-9","name":"run tests","status":"completed","job-type":"bash","created-at":1000,"started-at":2000,"completed-at":62000,"result":{"exit-code":0},"output-lines":["ok"]}"#;
    let assert = Command::cargo_bin("by-rs")
        .unwrap()
        .args(["task-format", "detail", "--task-json", task])
        .assert()
        .success()
        .stderr(predicate::str::is_empty());
    let stdout = String::from_utf8(assert.get_output().stdout.clone()).unwrap();
    let detail: serde_json::Value = serde_json::from_str(&stdout).unwrap();
    assert_eq!(detail["projection"], "task.format/format-task-detail");
    assert!(detail["rendered"]
        .as_str()
        .unwrap()
        .contains("Elapsed:  1.0m"));

    let assert = Command::cargo_bin("by-rs")
        .unwrap()
        .args([
            "task-format",
            "output",
            "--task-id",
            "task-9",
            "--lines-json",
            r#"["one","two","three"]"#,
            "--last-n",
            "2",
        ])
        .assert()
        .success()
        .stderr(predicate::str::is_empty());
    let stdout = String::from_utf8(assert.get_output().stdout.clone()).unwrap();
    let output: serde_json::Value = serde_json::from_str(&stdout).unwrap();
    assert_eq!(output["showing"], 2);
    assert_eq!(output["lines"], serde_json::json!(["two", "three"]));
    assert!(output["rendered"]
        .as_str()
        .unwrap()
        .contains("last 2 of 3 lines"));

    let assert = Command::cargo_bin("by-rs")
        .unwrap()
        .args([
            "task-format",
            "notification",
            "--task-id",
            "task-9",
            "--name",
            "run tests",
            "--status",
            "completed",
            "--prev-status",
            "running",
            "--started-at",
            "2000",
            "--completed-at",
            "62000",
        ])
        .assert()
        .success()
        .stderr(predicate::str::is_empty());
    let stdout = String::from_utf8(assert.get_output().stdout.clone()).unwrap();
    let notification: serde_json::Value = serde_json::from_str(&stdout).unwrap();
    assert_eq!(
        notification["notification"],
        "  ✓ [task] task-9 completed: run tests (1.0m)"
    );

    let assert = Command::cargo_bin("by-rs")
        .unwrap()
        .args([
            "task-format",
            "activity-line",
            "--bullet",
            "●",
            "--task-id",
            "task-9",
            "--task-name",
            "abcdefghi",
            "--cols",
            "16",
        ])
        .assert()
        .success()
        .stderr(predicate::str::is_empty());
    let stdout = String::from_utf8(assert.get_output().stdout.clone()).unwrap();
    let activity: serde_json::Value = serde_json::from_str(&stdout).unwrap();
    assert_eq!(activity["line"], "● task-9: abc...");

    let assert = Command::cargo_bin("by-rs")
        .unwrap()
        .args([
            "task-format",
            "status-bar",
            "--task-id",
            "task-9",
            "--name",
            "run tests",
            "--status",
            "failed",
            "--prev-status",
            "running",
        ])
        .assert()
        .success()
        .stderr(predicate::str::is_empty());
    let stdout = String::from_utf8(assert.get_output().stdout.clone()).unwrap();
    let status_bar: serde_json::Value = serde_json::from_str(&stdout).unwrap();
    assert_eq!(status_bar["status-bar"], "✗ task-9 failed");
}

#[test]
fn context_actions_helpers_are_live_free_json_projections() {
    let hits = r#"
      [
        {"_layer":"l2","role":"user","created-at":"2026-06-03T00:00:00Z","content":"episode content"},
        {"_layer":"l3","kind":"preference","tags":["rust","port"],"content":"fact content"},
        {"_layer":"l1","kind":"policy","content":"overlay content"},
        {"_layer":"lx","content":"other content"}
      ]
    "#;
    let assert = Command::cargo_bin("by-rs")
        .unwrap()
        .args(["context-actions", "recalled-memory", "--hits-json", hits])
        .assert()
        .success()
        .stderr(predicate::str::is_empty());
    let stdout = String::from_utf8(assert.get_output().stdout.clone()).unwrap();
    let recalled: serde_json::Value = serde_json::from_str(&stdout).unwrap();
    assert_eq!(
        recalled["projection"],
        "context-actions/format-recalled-memory"
    );
    let rendered = recalled["rendered"].as_str().unwrap();
    assert!(rendered.contains("### Episodes (L2 — recent activity)"));
    assert!(rendered.contains("- [2026-06-03 user] episode content"));
    assert!(rendered.contains("- (preference) [rust, port] fact content"));
    assert!(rendered.contains("- (policy overlay) overlay content"));
    assert!(rendered.contains("### Other"));

    let messages = r#"
      [
        {"role":"user","content":"first"},
        {"role":"assistant","content":"answer"},
        {"role":"user","content":"What next?"}
      ]
    "#;
    let assert = Command::cargo_bin("by-rs")
        .unwrap()
        .args([
            "context-actions",
            "conversation",
            "--messages-json",
            messages,
            "--question",
            " What next? ",
            "--limit",
            "2",
        ])
        .assert()
        .success()
        .stderr(predicate::str::is_empty());
    let stdout = String::from_utf8(assert.get_output().stdout.clone()).unwrap();
    let conversation: serde_json::Value = serde_json::from_str(&stdout).unwrap();
    assert_eq!(conversation["dropped-question-tail?"], true);
    assert_eq!(conversation["count"], 1);
    assert_eq!(conversation["conversation"][0]["content"], "answer");

    let assert = Command::cargo_bin("by-rs")
        .unwrap()
        .args([
            "context-actions",
            "entity-terms",
            "--result",
            "AlphaBeta mentioned ExtraTerm and `foo/bar:baz` twice AlphaBeta",
        ])
        .assert()
        .success()
        .stderr(predicate::str::is_empty());
    let stdout = String::from_utf8(assert.get_output().stdout.clone()).unwrap();
    let terms: serde_json::Value = serde_json::from_str(&stdout).unwrap();
    assert_eq!(
        terms["terms"],
        serde_json::json!(["AlphaBeta", "ExtraTerm", "foo/bar:baz"])
    );

    let assert = Command::cargo_bin("by-rs")
        .unwrap()
        .args([
            "context-actions",
            "novel-terms",
            "--terms-json",
            r#"["AlphaBeta","ExtraTerm","foo/bar:baz"]"#,
            "--recalled-text",
            "AlphaBeta already present",
        ])
        .assert()
        .success()
        .stderr(predicate::str::is_empty());
    let stdout = String::from_utf8(assert.get_output().stdout.clone()).unwrap();
    let novel: serde_json::Value = serde_json::from_str(&stdout).unwrap();
    assert_eq!(
        novel["novel-terms"],
        serde_json::json!(["ExtraTerm", "foo/bar:baz"])
    );

    let assert = Command::cargo_bin("by-rs")
        .unwrap()
        .args([
            "context-actions",
            "merge-hits",
            "--existing-json",
            r#"[{"content":"same","_layer":"l2"}]"#,
            "--new-json",
            r#"[{"content":"same","_layer":"l3"},{"content":"new","_layer":"l3"}]"#,
        ])
        .assert()
        .success()
        .stderr(predicate::str::is_empty());
    let stdout = String::from_utf8(assert.get_output().stdout.clone()).unwrap();
    let merged: serde_json::Value = serde_json::from_str(&stdout).unwrap();
    assert_eq!(merged["merged-count"], 2);
    assert_eq!(merged["merged"][1]["content"], "new");

    let assert = Command::cargo_bin("by-rs")
        .unwrap()
        .args([
            "context-actions",
            "mid-turn-query",
            "--result",
            "AlphaBeta mentioned ExtraTerm and `foo/bar:baz` in a long result",
            "--recalled-text",
            "AlphaBeta already present",
            "--min-result-chars",
            "10",
        ])
        .assert()
        .success()
        .stderr(predicate::str::is_empty());
    let stdout = String::from_utf8(assert.get_output().stdout.clone()).unwrap();
    let mid_turn: serde_json::Value = serde_json::from_str(&stdout).unwrap();
    assert_eq!(mid_turn["should-recall?"], true);
    assert_eq!(mid_turn["query"], "ExtraTerm foo/bar:baz");

    let assert = Command::cargo_bin("by-rs")
        .unwrap()
        .args([
            "context-actions",
            "tool-cache-decision",
            "--tool-name",
            "read",
            "--args",
            r#"{"path":"README.md"}"#,
            "--hits-json",
            r#"[{"data":{"args":{"path":"README.md"},"result":"cached file"}}]"#,
            "--ttl-s",
            "60",
        ])
        .assert()
        .success()
        .stderr(predicate::str::is_empty());
    let stdout = String::from_utf8(assert.get_output().stdout.clone()).unwrap();
    let cache: serde_json::Value = serde_json::from_str(&stdout).unwrap();
    assert_eq!(cache["replace?"], true);
    assert_eq!(cache["decision"]["replacement"], "cached file");
}

#[test]
fn context_format_helpers_are_live_free_json_projections() {
    let long_content = "x".repeat(501);
    let messages = serde_json::json!([
        {"role": null, "content": long_content},
        {"role": "assistant", "content": "ok"}
    ])
    .to_string();
    let assert = Command::cargo_bin("by-rs")
        .unwrap()
        .args([
            "context-format",
            "conversation",
            "--messages-json",
            messages.as_str(),
        ])
        .assert()
        .success()
        .stderr(predicate::str::is_empty());
    let stdout = String::from_utf8(assert.get_output().stdout.clone()).unwrap();
    let conversation: serde_json::Value = serde_json::from_str(&stdout).unwrap();
    assert_eq!(
        conversation["projection"],
        "context.formatters/format-conversation"
    );
    let rendered = conversation["rendered"].as_str().unwrap();
    assert!(rendered.contains(&format!("- **unknown**: {}…", "x".repeat(500))));
    assert!(rendered.contains("- **assistant**: ok"));

    let tools = r#"[
      {
        "name": "read-file",
        "description": "Read files",
        "tool-fn-type": "tool",
        "parameters": {
          "properties": {
            "path": {"type": "string", "description": "Path to read"},
            "offset": {"type": "integer", "desc": "Start byte"}
          },
          "required": ["path"]
        },
        "outputSchema": ["map", ["content", ["string", {"desc": "File content"}]]]
      },
      {
        "name": "bash",
        "toolFnType": "command",
        "inputSchema": [
          "map",
          ["cmd", ["string", {"desc": "Command to run"}]],
          ["cwd", {"optional": true}, ["string", {"desc": "Working dir"}]]
        ],
        "outputs": {"exit-code": {"type": "integer", "description": "Process status"}}
      }
    ]"#;
    let assert = Command::cargo_bin("by-rs")
        .unwrap()
        .args(["context-format", "agent-tools", "--tools-json", tools])
        .assert()
        .success()
        .stderr(predicate::str::is_empty());
    let stdout = String::from_utf8(assert.get_output().stdout.clone()).unwrap();
    let detailed: serde_json::Value = serde_json::from_str(&stdout).unwrap();
    assert_eq!(
        detailed["projection"],
        "context.formatters/format-agent-tools"
    );
    assert_eq!(detailed["registry-live?"], false);
    let rendered = detailed["rendered"].as_str().unwrap();
    assert!(rendered.contains("- `read-file` [tool] — Read files"));
    assert!(rendered.contains("    - `path` : string — Path to read"));
    assert!(rendered.contains("    - `offset` : integer (optional) — Start byte"));
    assert!(rendered.contains("  Outputs:\n    - `content` : string — File content"));
    assert!(rendered.contains("- `bash` [command]"));
    assert!(rendered.contains("    - `cmd` : string — Command to run"));
    assert!(rendered.contains("    - `cwd` : string (optional) — Working dir"));
    assert!(rendered.contains("    - `exit-code` : integer — Process status"));

    let assert = Command::cargo_bin("by-rs")
        .unwrap()
        .args([
            "context-format",
            "agent-tools-compact",
            "--tools-json",
            tools,
        ])
        .assert()
        .success()
        .stderr(predicate::str::is_empty());
    let stdout = String::from_utf8(assert.get_output().stdout.clone()).unwrap();
    let compact: serde_json::Value = serde_json::from_str(&stdout).unwrap();
    assert_eq!(
        compact["rendered"],
        "- `read-file` (tool)\n- `bash` (command)"
    );
}

#[test]
fn tui_format_and_ansi_helpers_are_live_free_json_projections() {
    let assert = Command::cargo_bin("by-rs")
        .unwrap()
        .args([
            "tui-format",
            "display-width",
            "--text",
            "\u{1b}[31m한a⚠\u{fe0f}\u{1b}[0m",
        ])
        .assert()
        .success()
        .stderr(predicate::str::is_empty());
    let stdout = String::from_utf8(assert.get_output().stdout.clone()).unwrap();
    let width: serde_json::Value = serde_json::from_str(&stdout).unwrap();
    assert_eq!(width["projection"], "tui.format/display-width");
    assert_eq!(width["width"], 5);

    let assert = Command::cargo_bin("by-rs")
        .unwrap()
        .args([
            "tui-format",
            "word-wrap",
            "--line",
            "alpha beta gamma",
            "--max-width",
            "10",
        ])
        .assert()
        .success()
        .stderr(predicate::str::is_empty());
    let stdout = String::from_utf8(assert.get_output().stdout.clone()).unwrap();
    let wrapped: serde_json::Value = serde_json::from_str(&stdout).unwrap();
    assert_eq!(wrapped["lines"], serde_json::json!(["alpha beta", "gamma"]));

    let assert = Command::cargo_bin("by-rs")
        .unwrap()
        .args(["tui-format", "format-number", "--n", "-1234567"])
        .assert()
        .success()
        .stderr(predicate::str::is_empty());
    let stdout = String::from_utf8(assert.get_output().stdout.clone()).unwrap();
    let number: serde_json::Value = serde_json::from_str(&stdout).unwrap();
    assert_eq!(number["formatted"], "-1,234,567");

    let assert = Command::cargo_bin("by-rs")
        .unwrap()
        .args([
            "tui-format",
            "strip-ansi",
            "--text",
            "\u{1b}[1;32mok\u{1b}[0m",
        ])
        .assert()
        .success()
        .stderr(predicate::str::is_empty());
    let stdout = String::from_utf8(assert.get_output().stdout.clone()).unwrap();
    let stripped: serde_json::Value = serde_json::from_str(&stdout).unwrap();
    assert_eq!(stripped["text"], "ok");

    let assert = Command::cargo_bin("by-rs")
        .unwrap()
        .args([
            "ansi",
            "style",
            "--text",
            "ok",
            "--codes-json",
            r#"["bold","green"]"#,
        ])
        .assert()
        .success()
        .stderr(predicate::str::is_empty());
    let stdout = String::from_utf8(assert.get_output().stdout.clone()).unwrap();
    let styled: serde_json::Value = serde_json::from_str(&stdout).unwrap();
    assert_eq!(styled["projection"], "tui.ansi/style");
    assert_eq!(styled["rendered"], "\u{1b}[1m\u{1b}[32mok\u{1b}[0m");

    let assert = Command::cargo_bin("by-rs")
        .unwrap()
        .args([
            "ansi",
            "rule",
            "--label",
            "Ready",
            "--width",
            "16",
            "--no-color",
        ])
        .assert()
        .success()
        .stderr(predicate::str::is_empty());
    let stdout = String::from_utf8(assert.get_output().stdout.clone()).unwrap();
    let rule: serde_json::Value = serde_json::from_str(&stdout).unwrap();
    assert_eq!(rule["rendered"], "──── Ready ─────");

    let assert = Command::cargo_bin("by-rs")
        .unwrap()
        .args(["ansi", "cursor-to", "--row", "2", "--col", "5"])
        .assert()
        .success()
        .stderr(predicate::str::is_empty());
    let stdout = String::from_utf8(assert.get_output().stdout.clone()).unwrap();
    let cursor: serde_json::Value = serde_json::from_str(&stdout).unwrap();
    assert_eq!(cursor["sequence"], "\u{1b}[2;5H");
}

#[test]
fn tui_high_level_format_helpers_are_live_free_json_projections() {
    let assert = Command::cargo_bin("by-rs")
        .unwrap()
        .args([
            "tui-format",
            "render-markdown",
            "--text",
            "# Title\n- **bold** item",
            "--max-width",
            "40",
            "--no-color",
        ])
        .assert()
        .success()
        .stderr(predicate::str::is_empty());
    let stdout = String::from_utf8(assert.get_output().stdout.clone()).unwrap();
    let markdown: serde_json::Value = serde_json::from_str(&stdout).unwrap();
    assert_eq!(markdown["projection"], "tui.format/render-markdown");
    assert_eq!(
        markdown["lines"],
        serde_json::json!(["Title", "  • bold item"])
    );

    let todos = r#"[
      {"description":"Implement parser","done":true,"result":"covered","independent":true},
      {"description":"Live smoke","done":false}
    ]"#;
    let assert = Command::cargo_bin("by-rs")
        .unwrap()
        .args([
            "tui-format",
            "todo-list",
            "--items-json",
            todos,
            "--no-color",
        ])
        .assert()
        .success()
        .stderr(predicate::str::is_empty());
    let stdout = String::from_utf8(assert.get_output().stdout.clone()).unwrap();
    let todo_list: serde_json::Value = serde_json::from_str(&stdout).unwrap();
    let rendered = todo_list["rendered"].as_str().unwrap();
    assert!(rendered.contains("TODO [1/2]"));
    assert!(rendered.contains("[✓] Implement parser (parallel)"));
    assert!(rendered.contains("covered"));

    let usage = r#"{"totals":{"call-count":3,"total-tokens":12345,"total-cost":0.25}}"#;
    let assert = Command::cargo_bin("by-rs")
        .unwrap()
        .args([
            "tui-format",
            "usage-summary",
            "--usage-json",
            usage,
            "--no-color",
        ])
        .assert()
        .success()
        .stderr(predicate::str::is_empty());
    let stdout = String::from_utf8(assert.get_output().stdout.clone()).unwrap();
    let usage_summary: serde_json::Value = serde_json::from_str(&stdout).unwrap();
    assert_eq!(
        usage_summary["rendered"],
        "3 calls │ 12,345 tokens │ $0.2500"
    );

    let usage_history = r#"[
      {:latency-ms 1000
       :input-tokens 120
       :output-tokens 15
       :cache {:read-tokens 20 :write-tokens 10}
       :cost {:total-cost 0.1}
       :turn-id "t1"
       :iteration 1
       :provider :bedrock
       :model "m1"
       :agent-instance-id :main
       :input-token-breakdown
       {:system-prompt {:estimated-tokens 50 :parts {:policy {:estimated-tokens 50}}}
        :dspy-signature {:estimated-tokens 5}
        :user-message {:estimated-tokens 30 :parts {:query {:estimated-tokens 30}}}
        :system-context {:estimated-tokens 10}}}
      {:latency-ms 2000
       :input-tokens 80
       :output-tokens 20
       :cost {:total-cost 0.2}
       :turn-id "t2"
       :iteration 2
       :provider :openai
       :model "m2"
       :agent-instance-id :worker
       :input-token-breakdown
       {:system-prompt {:estimated-tokens 40
                        :parts {:policy {:estimated-tokens 25}
                                :tools {:estimated-tokens 15}}}
        :dspy-signature {:estimated-tokens 4}
        :user-message {:estimated-tokens 60
                       :parts {:query {:estimated-tokens 45}
                               :history {:estimated-tokens 15}}}
        :system-context {:estimated-tokens 8}}}]"#;
    let assert = Command::cargo_bin("by-rs")
        .unwrap()
        .args([
            "tui-format",
            "usage-table",
            "--history-json",
            usage_history,
            "--breakdown",
            "--no-color",
        ])
        .assert()
        .success()
        .stderr(predicate::str::is_empty());
    let stdout = String::from_utf8(assert.get_output().stdout.clone()).unwrap();
    let usage_table: serde_json::Value = serde_json::from_str(&stdout).unwrap();
    assert_eq!(usage_table["projection"], "tui.format/format-usage-table");
    assert_eq!(usage_table["call-count"], 2);
    assert_eq!(usage_table["breakdown?"], true);
    let rendered = usage_table["rendered"].as_str().unwrap();
    assert!(rendered.contains("Usage (2 calls, 3.0s total, avg 1.5s/call)"));
    assert!(rendered.contains("── main · 1 calls · 135 tok · $0.1000 ──"));
    assert!(rendered.contains("System Prompt Parts (latest):"));
    assert!(rendered.contains("Growth: User message 30 → 60 tokens (+100%) over 2 calls"));

    let messages = r#"[
      {"role":"user","content":"hello"},
      {"role":"assistant","agent-id":":worker/main","content":"done"}
    ]"#;
    let assert = Command::cargo_bin("by-rs")
        .unwrap()
        .args([
            "tui-format",
            "conversation-history",
            "--messages-json",
            messages,
            "--last-n",
            "1",
            "--no-color",
        ])
        .assert()
        .success()
        .stderr(predicate::str::is_empty());
    let stdout = String::from_utf8(assert.get_output().stdout.clone()).unwrap();
    let history: serde_json::Value = serde_json::from_str(&stdout).unwrap();
    assert_eq!(history["rendered"], "Agent (main): done");

    let assert = Command::cargo_bin("by-rs")
        .unwrap()
        .args([
            "tui-format",
            "answer",
            "--text",
            "**ok**",
            "--cols",
            "50",
            "--no-color",
        ])
        .assert()
        .success()
        .stderr(predicate::str::is_empty());
    let stdout = String::from_utf8(assert.get_output().stdout.clone()).unwrap();
    let answer: serde_json::Value = serde_json::from_str(&stdout).unwrap();
    let rendered = answer["rendered"].as_str().unwrap();
    assert!(rendered.starts_with("\n┌"));
    assert!(rendered.contains(" ok "));

    let status = r#"{
      "agent-id":"main",
      "status":"running",
      "iteration":2,
      "max-iterations":5,
      "todo-progress":"1/2",
      "goal-achieved":false
    }"#;
    let assert = Command::cargo_bin("by-rs")
        .unwrap()
        .args([
            "tui-format",
            "status-summary",
            "--status-json",
            status,
            "--no-color",
        ])
        .assert()
        .success()
        .stderr(predicate::str::is_empty());
    let stdout = String::from_utf8(assert.get_output().stdout.clone()).unwrap();
    let status: serde_json::Value = serde_json::from_str(&stdout).unwrap();
    let rendered = status["rendered"].as_str().unwrap();
    assert!(rendered.contains("Agent Status"));
    assert!(rendered.contains("Agent:      main"));
    assert!(rendered.contains("Status:     running"));
    assert!(rendered.contains("Goal:       in progress"));
}

#[test]
fn tui_event_format_helpers_are_live_free_json_projections() {
    let assert = Command::cargo_bin("by-rs")
        .unwrap()
        .args([
            "tui-format",
            "thought",
            "--text",
            "Need **plan**\n```clojure\n(+ 1 2)\n```",
            "--cols",
            "80",
            "--no-color",
        ])
        .assert()
        .success()
        .stderr(predicate::str::is_empty());
    let stdout = String::from_utf8(assert.get_output().stdout.clone()).unwrap();
    let thought: serde_json::Value = serde_json::from_str(&stdout).unwrap();
    assert_eq!(thought["projection"], "tui.format/format-thought");
    let rendered = thought["rendered"].as_str().unwrap();
    assert!(rendered.contains("• Thinking:"));
    assert!(rendered.contains("Need plan"));
    assert!(rendered.contains("│ (+ 1 2)"));

    let calls = r#"[{"tool-name":"read-file","tool-args":[{"name":"path","value":"README.md"}]}]"#;
    let assert = Command::cargo_bin("by-rs")
        .unwrap()
        .args([
            "tui-format",
            "tool-calls",
            "--calls-json",
            calls,
            "--cols",
            "80",
            "--no-color",
        ])
        .assert()
        .success()
        .stderr(predicate::str::is_empty());
    let stdout = String::from_utf8(assert.get_output().stdout.clone()).unwrap();
    let tool_calls: serde_json::Value = serde_json::from_str(&stdout).unwrap();
    assert_eq!(tool_calls["projection"], "tui.format/format-tool-calls");
    assert_eq!(tool_calls["rendered"], "  → read-file(path=\"README.md\")");

    let results = r#"[{"tool-name":"read-file","tool-result":"content"}]"#;
    let assert = Command::cargo_bin("by-rs")
        .unwrap()
        .args([
            "tui-format",
            "tool-results",
            "--results-json",
            results,
            "--cols",
            "80",
            "--no-color",
        ])
        .assert()
        .success()
        .stderr(predicate::str::is_empty());
    let stdout = String::from_utf8(assert.get_output().stdout.clone()).unwrap();
    let tool_results: serde_json::Value = serde_json::from_str(&stdout).unwrap();
    assert_eq!(tool_results["projection"], "tui.format/format-tool-results");
    assert_eq!(tool_results["rendered"], "  ← read-file: content");

    let assert = Command::cargo_bin("by-rs")
        .unwrap()
        .args([
            "tui-format",
            "observation",
            "--text",
            "**ok**",
            "--cols",
            "80",
            "--no-color",
        ])
        .assert()
        .success()
        .stderr(predicate::str::is_empty());
    let stdout = String::from_utf8(assert.get_output().stdout.clone()).unwrap();
    let observation: serde_json::Value = serde_json::from_str(&stdout).unwrap();
    assert_eq!(observation["projection"], "tui.format/format-observation");
    assert_eq!(
        observation["rendered"],
        "  • Observation:\n    ┌─\n    │ ok\n    └─"
    );

    let assert = Command::cargo_bin("by-rs")
        .unwrap()
        .args([
            "tui-format",
            "welcome-banner",
            "--agent-id",
            ":coact-agent",
            "--session-id",
            "s1",
            "--lm-provider",
            ":bedrock",
            "--lm-model",
            "amazon.nova-lite-v1:0",
            "--no-color",
        ])
        .assert()
        .success()
        .stderr(predicate::str::is_empty());
    let stdout = String::from_utf8(assert.get_output().stdout.clone()).unwrap();
    let banner: serde_json::Value = serde_json::from_str(&stdout).unwrap();
    assert_eq!(banner["projection"], "tui.format/format-welcome-banner");
    let rendered = banner["rendered"].as_str().unwrap();
    assert!(rendered
        .contains("Brainyard TUI — coact-agent · bedrock/amazon.nova-lite-v1:0 · session s1"));
    assert!(rendered.contains("Type /help for commands. AI output may be inaccurate."));

    let analytics = r#"{
      "pqs":{
        "overall-score":82,
        "dimensions":{
          "specificity":20,
          "task-atomicity":22,
          "context-completeness":17,
          "acceptance-criteria":18,
          "clarity":9
        },
        "recommendations":["Add explicit live validation gate"]
      },
      "waste":{
        "total-waste-tokens":1234,
        "total-waste-cost":0.12,
        "waste-percentage":4.5,
        "patterns":[{"pattern-id":":duplicate-context","severity":":high","detail":"Repeated prompt context"}]
      },
      "cost":{
        "actual":{"total-cost":0.42,"total-tokens":12345,"call-count":3},
        "savings-potential":0.05,
        "throughput":{"output-tokens-per-sec":12.5,"avg-latency-ms":800}
      }
    }"#;
    let assert = Command::cargo_bin("by-rs")
        .unwrap()
        .args([
            "tui-format",
            "analytics-display",
            "--analytics-json",
            analytics,
            "--cols",
            "80",
            "--no-color",
        ])
        .assert()
        .success()
        .stderr(predicate::str::is_empty());
    let stdout = String::from_utf8(assert.get_output().stdout.clone()).unwrap();
    let analytics: serde_json::Value = serde_json::from_str(&stdout).unwrap();
    assert_eq!(
        analytics["projection"],
        "tui.format/format-analytics-display"
    );
    let rendered = analytics["rendered"].as_str().unwrap();
    assert!(rendered.contains("Session Analytics"));
    assert!(rendered.contains("Overall: 82/100"));
    assert!(rendered.contains("Waste: 1,234 tokens ($0.1200, 4.5%)"));
    assert!(rendered.contains("Actual cost: $0.4200 (12,345 tokens, 3 calls)"));

    let trace = r#"{"agent-id":":main","depth":2,"content":"entered node"}"#;
    let assert = Command::cargo_bin("by-rs")
        .unwrap()
        .args(["tui-format", "trace", "--trace-json", trace, "--no-color"])
        .assert()
        .success()
        .stderr(predicate::str::is_empty());
    let stdout = String::from_utf8(assert.get_output().stdout.clone()).unwrap();
    let trace: serde_json::Value = serde_json::from_str(&stdout).unwrap();
    assert_eq!(trace["projection"], "tui.format/format-trace");
    assert_eq!(trace["rendered"], "  [trace]     [:main] entered node");

    let mulog = r#"{"mulog/event-name":":ai.brainyard.agent/chat-completion","model":"claude-haiku","total-tokens":1234,"cost":0.42}"#;
    let assert = Command::cargo_bin("by-rs")
        .unwrap()
        .args([
            "tui-format",
            "mulog-event",
            "--event-json",
            mulog,
            "--no-color",
        ])
        .assert()
        .success()
        .stderr(predicate::str::is_empty());
    let stdout = String::from_utf8(assert.get_output().stdout.clone()).unwrap();
    let mulog: serde_json::Value = serde_json::from_str(&stdout).unwrap();
    assert_eq!(mulog["projection"], "tui.format/format-mulog-event");
    assert_eq!(
        mulog["rendered"],
        "  [mulog] chat model=claude-haiku tokens=1234 cost=$0.4200"
    );
}

#[test]
fn aws_profiles_read_local_config_without_sts_or_writes() {
    let home = tempfile::tempdir().unwrap();
    let aws_dir = home.path().join(".aws");
    std::fs::create_dir_all(&aws_dir).unwrap();
    std::fs::write(
        aws_dir.join("credentials"),
        r#"
[default]
aws_access_key_id = AKIA1234567890ABCD
aws_secret_access_key = secret1234567890WXYZ
region = us-west-2

[dev]
aws_access_key_id = DEVKEY1234567890
"#,
    )
    .unwrap();
    std::fs::write(
        aws_dir.join("config"),
        r#"
[default]
region = us-east-1

[profile sso]
sso_start_url = https://example.awsapps.com/start
sso_account_id = 123456789012
sso_role_name = Admin
sso_region = us-east-1
region = us-east-2

[profile role]
role_arn = arn:aws:iam::123:role/demo
source_profile = default
region = us-west-1
"#,
    )
    .unwrap();

    let assert = Command::cargo_bin("by-rs")
        .unwrap()
        .args(["aws", "list-profiles", "--user-dir"])
        .arg(home.path())
        .assert()
        .success()
        .stderr(predicate::str::is_empty());
    let stdout = String::from_utf8(assert.get_output().stdout.clone()).unwrap();
    let list: serde_json::Value = serde_json::from_str(&stdout).unwrap();
    assert_eq!(list["result"]["total"], 4);
    assert_eq!(list["result"]["profiles"][0]["name"], "default");
    assert_eq!(list["result"]["profiles"][0]["region"], "us-west-2");
    assert_eq!(list["result"]["profiles"][0]["source"], "static-keys");
    assert_eq!(list["result"]["profiles"][2]["name"], "role");
    assert_eq!(list["result"]["profiles"][2]["source"], "assume-role");

    let assert = Command::cargo_bin("by-rs")
        .unwrap()
        .args([
            "aws",
            "get-profile",
            "--profile-name",
            "default",
            "--user-dir",
        ])
        .arg(home.path())
        .assert()
        .success()
        .stderr(predicate::str::is_empty());
    let stdout = String::from_utf8(assert.get_output().stdout.clone()).unwrap();
    let default_profile: serde_json::Value = serde_json::from_str(&stdout).unwrap();
    assert_eq!(default_profile["result"]["region"], "us-west-2");
    assert_eq!(default_profile["result"]["source"], "static-keys");
    let masked_access_key = default_profile["result"]["access-key-id"].as_str().unwrap();
    assert!(masked_access_key.starts_with('*'));
    assert!(masked_access_key.ends_with("ABCD"));
    assert!(!masked_access_key.contains("AKIA"));
    let masked_secret = default_profile["result"]["secret-access-key"]
        .as_str()
        .unwrap();
    assert!(masked_secret.starts_with('*'));
    assert!(masked_secret.ends_with("WXYZ"));
    assert!(!masked_secret.contains("secret"));

    let assert = Command::cargo_bin("by-rs")
        .unwrap()
        .args(["aws", "get-profile", "--profile-name", "sso", "--user-dir"])
        .arg(home.path())
        .assert()
        .success()
        .stderr(predicate::str::is_empty());
    let stdout = String::from_utf8(assert.get_output().stdout.clone()).unwrap();
    let sso_profile: serde_json::Value = serde_json::from_str(&stdout).unwrap();
    assert_eq!(sso_profile["result"]["source"], "sso");
    assert_eq!(
        sso_profile["result"]["sso-start-url"],
        "https://example.awsapps.com/start"
    );

    let assert = Command::cargo_bin("by-rs")
        .unwrap()
        .args([
            "aws",
            "get-profile",
            "--profile-name",
            "missing",
            "--user-dir",
        ])
        .arg(home.path())
        .assert()
        .success()
        .stderr(predicate::str::is_empty());
    let stdout = String::from_utf8(assert.get_output().stdout.clone()).unwrap();
    let missing: serde_json::Value = serde_json::from_str(&stdout).unwrap();
    assert_eq!(missing["error"], "Profile 'missing' not found");

    let assert = Command::cargo_bin("by-rs")
        .unwrap()
        .args([
            "aws",
            "whoami-preview",
            "--profile",
            "dev",
            "--identity",
            r#"{"Account":"123456789012","Arn":"arn:aws:sts::123456789012:assumed-role/demo/session","UserId":"AROAI:user"}"#,
        ])
        .assert()
        .success()
        .stderr(predicate::str::is_empty());
    let stdout = String::from_utf8(assert.get_output().stdout.clone()).unwrap();
    let whoami: serde_json::Value = serde_json::from_str(&stdout).unwrap();
    assert_eq!(whoami["projection"], "aws/whoami-preview");
    assert_eq!(whoami["result"]["account"], "123456789012");
    assert_eq!(
        whoami["result"]["arn"],
        "arn:aws:sts::123456789012:assumed-role/demo/session"
    );
    assert_eq!(whoami["result"]["user-id"], "AROAI:user");
    assert_eq!(whoami["result"]["profile"], "dev");
    assert_eq!(whoami["sts-skipped?"], true);

    let assert = Command::cargo_bin("by-rs")
        .unwrap()
        .args([
            "aws",
            "set-profile-preflight",
            "--profile",
            "dev",
            "--region",
            "us-west-2",
            "--restart",
            "true",
            "--updated-servers",
            r#"["aws-knowledge","aws-docs"]"#,
        ])
        .assert()
        .success()
        .stderr(predicate::str::is_empty());
    let stdout = String::from_utf8(assert.get_output().stdout.clone()).unwrap();
    let set_profile: serde_json::Value = serde_json::from_str(&stdout).unwrap();
    assert_eq!(set_profile["projection"], "aws/set-profile-preflight");
    assert_eq!(set_profile["result"]["profile"], "dev");
    assert_eq!(set_profile["result"]["region"], "us-west-2");
    assert_eq!(set_profile["result"]["updated-servers"][0], "aws-knowledge");
    assert_eq!(set_profile["result"]["updated-servers"][1], "aws-docs");
    assert_eq!(set_profile["result"]["restarted?"], true);
    assert_eq!(set_profile["write-skipped?"], true);
    assert_eq!(set_profile["restart-skipped?"], true);
}

#[test]
fn skills_list_find_and_read_local_filesystem_without_npx() {
    let project = tempfile::tempdir().unwrap();
    let home = tempfile::tempdir().unwrap();
    let skill_dir = project.path().join(".brainyard/skills/plan-helper");
    std::fs::create_dir_all(skill_dir.join("scripts")).unwrap();
    std::fs::write(
        skill_dir.join("SKILL.md"),
        r#"---
title: Plan Helper
description: >
  Plans migration work
  without live calls
tags: rust, migration
version: 1.2.3
---
# Plan Helper

Fallback description.
"#,
    )
    .unwrap();
    std::fs::write(skill_dir.join("scripts/run.sh"), "#!/bin/sh\n").unwrap();
    let claude_dir = home.path().join(".claude/skills/review-helper");
    std::fs::create_dir_all(&claude_dir).unwrap();
    std::fs::write(
        claude_dir.join("SKILL.md"),
        "# Review Helper\n\nReviews local diffs.\n",
    )
    .unwrap();

    let assert = Command::cargo_bin("by-rs")
        .unwrap()
        .args(["skills", "list", "--project-dir"])
        .arg(project.path())
        .args(["--user-dir"])
        .arg(home.path())
        .assert()
        .success();
    let stdout = String::from_utf8(assert.get_output().stdout.clone()).unwrap();
    let list: serde_json::Value = serde_json::from_str(&stdout).unwrap();
    assert_eq!(list["total"], 2);
    let skills = list["skills"].as_array().unwrap();
    let plan = skills
        .iter()
        .find(|skill| skill["name"] == "plan-helper")
        .unwrap();
    assert_eq!(plan["type"], "brainyard");
    assert_eq!(plan["scope"], "project");
    assert_eq!(plan["title"], "Plan Helper");
    assert_eq!(
        plan["description"],
        "Plans migration work without live calls"
    );
    assert_eq!(plan["tags"][0], "rust");
    assert_eq!(plan["version"], "1.2.3");
    assert_eq!(plan["file-count"], 2);
    let review = skills
        .iter()
        .find(|skill| skill["name"] == "review-helper")
        .unwrap();
    assert_eq!(review["type"], "claude");

    let assert = Command::cargo_bin("by-rs")
        .unwrap()
        .args(["skills", "find", "--query", "migration", "--project-dir"])
        .arg(project.path())
        .args(["--user-dir"])
        .arg(home.path())
        .assert()
        .success();
    let stdout = String::from_utf8(assert.get_output().stdout.clone()).unwrap();
    let find: serde_json::Value = serde_json::from_str(&stdout).unwrap();
    assert_eq!(find["total"], 1);
    assert_eq!(find["matches"][0]["name"], "plan-helper");

    let assert = Command::cargo_bin("by-rs")
        .unwrap()
        .args([
            "skills",
            "read",
            "plan-helper",
            "--type",
            "brainyard",
            "--scope",
            "project",
            "--project-dir",
        ])
        .arg(project.path())
        .args(["--user-dir"])
        .arg(home.path())
        .assert()
        .success();
    let stdout = String::from_utf8(assert.get_output().stdout.clone()).unwrap();
    let detail: serde_json::Value = serde_json::from_str(&stdout).unwrap();
    assert_eq!(detail["name"], "plan-helper");
    assert!(detail["content"]
        .as_str()
        .unwrap()
        .contains("# Plan Helper"));
    let files = detail["files"].as_array().unwrap();
    assert!(files.iter().any(|file| file["path"] == "SKILL.md"));
    assert!(files.iter().any(|file| file["path"] == "scripts/run.sh"));

    let assert = Command::cargo_bin("by-rs")
        .unwrap()
        .args(["skills", "reload-preview", "--project-dir"])
        .arg(project.path())
        .args(["--user-dir"])
        .arg(home.path())
        .args([
            "--registered-json",
            r#"["skill$plan-helper","skill$stale"]"#,
        ])
        .assert()
        .success()
        .stderr(predicate::str::is_empty());
    let stdout = String::from_utf8(assert.get_output().stdout.clone()).unwrap();
    let preview: serde_json::Value = serde_json::from_str(&stdout).unwrap();
    assert_eq!(preview["projection"], "common.skills/skills$reload-preview");
    assert_eq!(preview["registry-skipped?"], true);
    assert_eq!(preview["source"], "local-filesystem");
    assert_eq!(preview["total"], 2);
    assert_eq!(preview["registered"][0], "skill$plan-helper");
    assert_eq!(preview["registered"][1], "skill$review-helper");
    assert_eq!(preview["unregistered"][0], "skill$stale");
    assert_eq!(preview["plan"]["register"][0], "skill$review-helper");
    assert_eq!(preview["plan"]["unchanged"][0], "skill$plan-helper");
    assert_eq!(preview["plan"]["drop-stale"][0], "skill$stale");
    assert_eq!(preview["skills"][0]["id"], "skill$plan-helper");
    assert_eq!(preview["skills"][1]["id"], "skill$review-helper");
}

#[test]
fn skills_missing_roots_are_empty_and_read_only() {
    let dir = tempfile::tempdir().unwrap();
    let project = dir.path().join("missing-project");
    let home = dir.path().join("missing-home");

    let assert = Command::cargo_bin("by-rs")
        .unwrap()
        .args(["skills", "list", "--project-dir"])
        .arg(&project)
        .args(["--user-dir"])
        .arg(&home)
        .assert()
        .success();
    let stdout = String::from_utf8(assert.get_output().stdout.clone()).unwrap();
    let value: serde_json::Value = serde_json::from_str(&stdout).unwrap();
    assert_eq!(value["total"], 0);
    assert!(value["skills"].as_array().unwrap().is_empty());

    let assert = Command::cargo_bin("by-rs")
        .unwrap()
        .args(["skills", "read", "missing", "--project-dir"])
        .arg(&project)
        .args(["--user-dir"])
        .arg(&home)
        .assert()
        .success();
    let stdout = String::from_utf8(assert.get_output().stdout.clone()).unwrap();
    let read: serde_json::Value = serde_json::from_str(&stdout).unwrap();
    assert_eq!(read["error"], "Skill not found: missing");
    assert!(!project.exists());
    assert!(!home.exists());
}

#[test]
fn skills_write_preview_projects_mutations_without_writes_or_npx() {
    let project = tempfile::tempdir().unwrap();
    let home = tempfile::tempdir().unwrap();

    let create_content = r#"---
title: Plan Helper
description: Build local migration plans
---
# Plan Helper
"#;
    let assert = Command::cargo_bin("by-rs")
        .unwrap()
        .args([
            "skills",
            "write-preview",
            "--op",
            "create",
            "--skill-name",
            "Plan Helper!",
            "--content",
            create_content,
            "--scripts",
            r##"{"run.sh":"#!/bin/sh\n"}"##,
            "--resources",
            r#"{:note "keep"}"#,
            "--project-dir",
        ])
        .arg(project.path())
        .args(["--user-dir"])
        .arg(home.path())
        .args(["--ts", "2026-06-03T00:00:00Z"])
        .assert()
        .success()
        .stderr(predicate::str::is_empty());
    let stdout = String::from_utf8(assert.get_output().stdout.clone()).unwrap();
    let create: serde_json::Value = serde_json::from_str(&stdout).unwrap();
    assert_eq!(create["projection"], "common.skills/skills$write-preview");
    assert_eq!(create["write-skipped?"], true);
    assert_eq!(create["op"], "create");
    assert_eq!(create["name"], "plan-helper");
    assert_eq!(create["requested-name"], "Plan Helper!");
    assert_eq!(create["scope"], "project");
    assert_eq!(create["created"], "2026-06-03T00:00:00Z");
    let write_paths = create["would-write"]
        .as_array()
        .unwrap()
        .iter()
        .map(|entry| entry["path"].as_str().unwrap())
        .collect::<Vec<_>>();
    assert_eq!(
        write_paths,
        vec!["SKILL.md", "scripts/run.sh", "resources/note"]
    );
    assert!(!project.path().join(".brainyard").exists());

    let existing_skill = home.path().join(".brainyard/skills/plan-helper");
    std::fs::create_dir_all(&existing_skill).unwrap();
    std::fs::write(
        existing_skill.join("SKILL.md"),
        "# Old Helper\n\nOld body.\n",
    )
    .unwrap();
    let update_content = "# Updated Helper\n\nNew body.\n";
    let assert = Command::cargo_bin("by-rs")
        .unwrap()
        .args([
            "skills",
            "write-preview",
            "--op",
            "update",
            "--skill-name",
            "plan-helper",
            "--content",
            update_content,
            "--resources",
            r#"{"guide.md":"new"}"#,
            "--project-dir",
        ])
        .arg(project.path())
        .args(["--user-dir"])
        .arg(home.path())
        .args(["--ts", "2026-06-03T00:01:00Z"])
        .assert()
        .success()
        .stderr(predicate::str::is_empty());
    let stdout = String::from_utf8(assert.get_output().stdout.clone()).unwrap();
    let update: serde_json::Value = serde_json::from_str(&stdout).unwrap();
    assert_eq!(update["op"], "update");
    assert_eq!(update["scope"], "user");
    assert_eq!(update["title"], "Updated Helper");
    assert_eq!(update["updated"], "2026-06-03T00:01:00Z");
    let update_paths = update["would-write"]
        .as_array()
        .unwrap()
        .iter()
        .map(|entry| entry["path"].as_str().unwrap())
        .collect::<Vec<_>>();
    assert_eq!(update_paths, vec!["SKILL.md", "resources/guide.md"]);
    let unchanged = std::fs::read_to_string(existing_skill.join("SKILL.md")).unwrap();
    assert!(unchanged.contains("# Old Helper"));

    let assert = Command::cargo_bin("by-rs")
        .unwrap()
        .args([
            "skills",
            "write-preview",
            "--op",
            "remove",
            "--skill-name",
            "plan-helper",
            "--type",
            "brainyard",
            "--scope",
            "user",
            "--project-dir",
        ])
        .arg(project.path())
        .args(["--user-dir"])
        .arg(home.path())
        .assert()
        .success()
        .stderr(predicate::str::is_empty());
    let stdout = String::from_utf8(assert.get_output().stdout.clone()).unwrap();
    let remove: serde_json::Value = serde_json::from_str(&stdout).unwrap();
    assert_eq!(remove["op"], "remove");
    assert_eq!(remove["deleted"], "plan-helper");
    assert_eq!(remove["scope"], "user");
    assert!(remove["would-delete"]
        .as_array()
        .unwrap()
        .iter()
        .any(|entry| entry["path"] == "SKILL.md"));
    assert!(existing_skill.exists());

    let assert = Command::cargo_bin("by-rs")
        .unwrap()
        .args([
            "skills",
            "write-preview",
            "--op",
            "remove",
            "--skill-name",
            "market-skill",
            "--type",
            "claude",
            "--project-dir",
        ])
        .arg(project.path())
        .args(["--user-dir"])
        .arg(home.path())
        .assert()
        .success()
        .stderr(predicate::str::is_empty());
    let stdout = String::from_utf8(assert.get_output().stdout.clone()).unwrap();
    let cli_remove: serde_json::Value = serde_json::from_str(&stdout).unwrap();
    assert_eq!(cli_remove["op"], "remove");
    assert_eq!(cli_remove["npx-skipped?"], true);
    assert!(cli_remove["commands"][0]["command"]
        .as_str()
        .unwrap()
        .contains("--target claude"));

    let assert = Command::cargo_bin("by-rs")
        .unwrap()
        .args([
            "skills",
            "install-preview",
            "--package",
            "owner/repo@plan-helper",
            "--type",
            "agents",
        ])
        .assert()
        .success()
        .stderr(predicate::str::is_empty());
    let stdout = String::from_utf8(assert.get_output().stdout.clone()).unwrap();
    let install: serde_json::Value = serde_json::from_str(&stdout).unwrap();
    assert_eq!(
        install["projection"],
        "common.skills/skills$install-preview"
    );
    assert_eq!(install["package"], "owner/repo@plan-helper");
    assert_eq!(install["type"], "agents");
    assert_eq!(install["npx-skipped?"], true);
    assert_eq!(
        install["command"],
        "npx skills add owner/repo@plan-helper -g -y --target agents"
    );

    let assert = Command::cargo_bin("by-rs")
        .unwrap()
        .args(["skills", "sync-preview"])
        .assert()
        .success()
        .stderr(predicate::str::is_empty());
    let stdout = String::from_utf8(assert.get_output().stdout.clone()).unwrap();
    let sync: serde_json::Value = serde_json::from_str(&stdout).unwrap();
    assert_eq!(sync["projection"], "common.skills/skills$sync-preview");
    assert_eq!(sync["type"], "both");
    assert_eq!(sync["npx-skipped?"], true);
    assert_eq!(sync["commands"][0]["type"], "claude");
    assert_eq!(sync["commands"][1]["type"], "agents");
}

#[test]
fn explore_and_update_write_persist_records_without_live_agent() {
    let temp = tempfile::tempdir().unwrap();
    let content =
        "---\nslug: rust-port\nagent: explore-agent\n---\n\n# Result\n\nNo live model needed.\n";

    let assert = Command::cargo_bin("by-rs")
        .unwrap()
        .args([
            "explore",
            "write",
            "--slug",
            "rust-port",
            "--content",
            content,
            "--base-dir",
        ])
        .arg(temp.path())
        .args(["--ts", "20260102-030405"])
        .assert()
        .success()
        .stderr(predicate::str::is_empty());
    let stdout = String::from_utf8(assert.get_output().stdout.clone()).unwrap();
    let written: serde_json::Value = serde_json::from_str(&stdout).unwrap();
    assert_eq!(written["projection"], "common.explore/explore$write");
    assert_eq!(written["written?"], true);
    assert_eq!(written["slug"], "rust-port");
    assert_eq!(
        written["rel-path"],
        ".brainyard/agents/explore-agent/results/20260102-030405-rust-port.md"
    );
    assert!(temp
        .path()
        .join(".brainyard/agents/explore-agent/results/20260102-030405-rust-port.md")
        .is_file());

    let assert = Command::cargo_bin("by-rs")
        .unwrap()
        .args([
            "explore",
            "write",
            "--slug",
            "rust-port",
            "--content",
            content,
            "--base-dir",
        ])
        .arg(temp.path())
        .args(["--ts", "20260102-030406"])
        .assert()
        .success()
        .stderr(predicate::str::is_empty());
    let stdout = String::from_utf8(assert.get_output().stdout.clone()).unwrap();
    let collision: serde_json::Value = serde_json::from_str(&stdout).unwrap();
    assert_eq!(collision["slug"], "rust-port-2");
    assert_eq!(collision["collision?"], true);

    let update_content =
        "---\nslug: replace-timeout\nagent: update-agent\nok: true\n---\n\n# Edit\n\nReplace timeout.\n";
    let assert = Command::cargo_bin("by-rs")
        .unwrap()
        .args([
            "update",
            "write",
            "--slug",
            "replace-timeout",
            "--content",
            update_content,
            "--base-dir",
        ])
        .arg(temp.path())
        .args(["--ts", "20260102-040506"])
        .assert()
        .success()
        .stderr(predicate::str::is_empty());
    let stdout = String::from_utf8(assert.get_output().stdout.clone()).unwrap();
    let update_written: serde_json::Value = serde_json::from_str(&stdout).unwrap();
    assert_eq!(update_written["projection"], "common.update/update$write");
    assert_eq!(update_written["written?"], true);
    assert!(temp
        .path()
        .join(".brainyard/agents/update-agent/edits/20260102-040506-replace-timeout.md")
        .is_file());
}

#[test]
fn update_apply_pattern_persists_edit_record_without_live_agent() {
    let temp = tempfile::tempdir().unwrap();
    let src_dir = temp.path().join("src");
    std::fs::create_dir_all(&src_dir).unwrap();
    let target = src_dir.join("main.rs");
    std::fs::write(&target, "fn main() {\n    let timeout = 30000;\n}\n").unwrap();

    let assert = Command::cargo_bin("by-rs")
        .unwrap()
        .args([
            "update",
            "apply",
            "--request",
            "Replace timeout",
            "--target",
            "src/main.rs",
            "--mode",
            "pattern",
            "--pattern",
            "30000",
            "--replacement",
            "120000",
            "--base-dir",
        ])
        .arg(temp.path())
        .args(["--ts", "20260103-040506"])
        .assert()
        .success()
        .stderr(predicate::str::is_empty());
    let stdout = String::from_utf8(assert.get_output().stdout.clone()).unwrap();
    let applied: serde_json::Value = serde_json::from_str(&stdout).unwrap();
    assert_eq!(applied["projection"], "common.update/update$apply");
    assert_eq!(applied["ok?"], true);
    assert_eq!(applied["mode"], "pattern");
    assert_eq!(applied["target"], "src/main.rs");
    assert_eq!(applied["replaced"], 1);
    assert_eq!(applied["write"]["projection"], "common.update/update$write");
    assert_eq!(applied["write"]["written?"], true);
    assert!(std::fs::read_to_string(&target).unwrap().contains("120000"));
    let rel_path = applied["write"]["rel-path"].as_str().unwrap();
    assert!(temp.path().join(rel_path).is_file());
    assert!(std::fs::read_to_string(temp.path().join(rel_path))
        .unwrap()
        .contains("timeout = 120000"));
}

#[test]
fn main_session_id_projects_explicit_or_env_session_without_live_agent() {
    let explicit = Command::cargo_bin("by-rs")
        .unwrap()
        .env_remove("BRAINYARD_SESSION_ID")
        .args(["main", "session-id", "--session-id", "main-session-1"])
        .assert()
        .success()
        .stderr(predicate::str::is_empty());
    let explicit_stdout = String::from_utf8(explicit.get_output().stdout.clone()).unwrap();
    let explicit_json: serde_json::Value = serde_json::from_str(&explicit_stdout).unwrap();
    assert_eq!(explicit_json["projection"], "common.main/main$session-id");
    assert_eq!(explicit_json["session-id"], "main-session-1");

    let from_env = Command::cargo_bin("by-rs")
        .unwrap()
        .env("BRAINYARD_SESSION_ID", "main-session-env")
        .args(["main", "session-id"])
        .assert()
        .success()
        .stderr(predicate::str::is_empty());
    let from_env_stdout = String::from_utf8(from_env.get_output().stdout.clone()).unwrap();
    let from_env_json: serde_json::Value = serde_json::from_str(&from_env_stdout).unwrap();
    assert_eq!(from_env_json["projection"], "common.main/main$session-id");
    assert_eq!(from_env_json["session-id"], "main-session-env");

    let missing = Command::cargo_bin("by-rs")
        .unwrap()
        .env_remove("BRAINYARD_SESSION_ID")
        .args(["main", "session-id"])
        .assert()
        .success()
        .stderr(predicate::str::is_empty());
    let missing_stdout = String::from_utf8(missing.get_output().stdout.clone()).unwrap();
    let missing_json: serde_json::Value = serde_json::from_str(&missing_stdout).unwrap();
    assert_eq!(missing_json["projection"], "common.main/main$session-id");
    assert!(missing_json["error"]
        .as_str()
        .unwrap()
        .contains("No current main agent session"));
}

#[test]
fn user_tools_list_and_read_persisted_records_without_registry() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("tools");
    std::fs::create_dir_all(&root).unwrap();
    std::fs::write(
        root.join("shout.edn"),
        r#"{:name "shout"
 :description "Uppercase text"
 :input-schema [:map [:text :string]]
 :body "(fn [args] (:text args))"}"#,
    )
    .unwrap();

    let assert = Command::cargo_bin("by-rs")
        .unwrap()
        .args(["user-tools", "list", "--root"])
        .arg(&root)
        .assert()
        .success();
    let stdout = String::from_utf8(assert.get_output().stdout.clone()).unwrap();
    let list: serde_json::Value = serde_json::from_str(&stdout).unwrap();
    assert_eq!(list["total"], 1);
    assert_eq!(list["tools"][0]["id"], "user$shout");
    assert_eq!(list["tools"][0]["name"], "shout");
    assert_eq!(list["tools"][0]["description"], "Uppercase text");
    assert_eq!(list["tools"][0]["input-schema"][0], "map");
    assert_eq!(list["tools"][0]["body-present"], true);

    let assert = Command::cargo_bin("by-rs")
        .unwrap()
        .args(["user-tools", "read", "--root"])
        .arg(&root)
        .arg("shout")
        .assert()
        .success();
    let stdout = String::from_utf8(assert.get_output().stdout.clone()).unwrap();
    let detail: serde_json::Value = serde_json::from_str(&stdout).unwrap();
    assert_eq!(detail["projection"], "common.user-tools/tools$read");
    assert_eq!(detail["id"], "user$shout");
    assert_eq!(detail["body"], "(fn [args] (:text args))");
    assert!(detail["file"].as_str().unwrap().ends_with("shout.edn"));
}

#[test]
fn user_tools_validate_definition_is_live_free_projection() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("tools");

    let assert = Command::cargo_bin("by-rs")
        .unwrap()
        .args([
            "user-tools",
            "validate-definition",
            "--name",
            "shout",
            "--description",
            "Uppercase text",
            "--input-schema-edn",
            "[:map [:text :string]]",
            "--body",
            "(fn [args] (:text args))",
        ])
        .assert()
        .success();
    let stdout = String::from_utf8(assert.get_output().stdout.clone()).unwrap();
    let valid: serde_json::Value = serde_json::from_str(&stdout).unwrap();
    assert_eq!(
        valid["projection"],
        "common.user-tools/define-tool-validate"
    );
    assert_eq!(valid["ok?"], true);
    assert!(valid["errors"].as_array().unwrap().is_empty());
    assert_eq!(valid["normalized"]["name"], "shout");
    assert_eq!(valid["normalized"]["id"], "user$shout");
    assert_eq!(valid["normalized"]["description-present"], true);
    assert_eq!(valid["normalized"]["input-schema"][0], "map");
    assert_eq!(valid["normalized"]["body-present"], true);
    assert_eq!(valid["eval-skipped?"], true);
    assert_eq!(valid["write-skipped?"], true);

    let assert = Command::cargo_bin("by-rs")
        .unwrap()
        .args([
            "user-tools",
            "validate-definition",
            "--name",
            "User$bad",
            "--input-schema-edn",
            "[:vector]",
        ])
        .assert()
        .success();
    let stdout = String::from_utf8(assert.get_output().stdout.clone()).unwrap();
    let invalid: serde_json::Value = serde_json::from_str(&stdout).unwrap();
    assert_eq!(invalid["ok?"], false);
    let fields = invalid["errors"]
        .as_array()
        .unwrap()
        .iter()
        .map(|error| error["field"].as_str().unwrap())
        .collect::<Vec<_>>();
    assert!(fields.contains(&"name"));
    assert!(fields.contains(&"body"));
    assert!(fields.contains(&"input-schema"));
    assert_eq!(invalid["normalized"]["input-schema"][0], "vector");

    let assert = Command::cargo_bin("by-rs")
        .unwrap()
        .args(["user-tools", "create-preview", "--root"])
        .arg(&root)
        .args([
            "--name",
            "shout",
            "--description",
            "Uppercase text",
            "--input-schema-edn",
            "[:map [:text :string]]",
            "--body",
            "(fn [args] (:text args))",
        ])
        .assert()
        .success();
    let stdout = String::from_utf8(assert.get_output().stdout.clone()).unwrap();
    let create: serde_json::Value = serde_json::from_str(&stdout).unwrap();
    assert_eq!(
        create["projection"],
        "common.user-tools/tools$create-preview"
    );
    assert_eq!(create["ok?"], true);
    assert_eq!(create["id"], "user$shout");
    assert_eq!(
        create["persisted"],
        root.join("shout.edn").display().to_string()
    );
    assert_eq!(create["would-write"][0]["kind"], "user-tool-edn");
    assert_eq!(create["would-write"][0]["create-parent?"], true);
    assert_eq!(create["eval-skipped?"], true);
    assert_eq!(create["registry-skipped?"], true);
    assert_eq!(create["write-skipped?"], true);
    assert!(!root.exists());

    std::fs::create_dir_all(&root).unwrap();
    std::fs::write(
        root.join("shout.edn"),
        r#"{:name "shout" :input-schema [:map] :body "(fn [args] args)"}"#,
    )
    .unwrap();
    let assert = Command::cargo_bin("by-rs")
        .unwrap()
        .args(["user-tools", "delete-preview", "--root"])
        .arg(&root)
        .arg("shout")
        .assert()
        .success();
    let stdout = String::from_utf8(assert.get_output().stdout.clone()).unwrap();
    let delete: serde_json::Value = serde_json::from_str(&stdout).unwrap();
    assert_eq!(
        delete["projection"],
        "common.user-tools/tools$delete-preview"
    );
    assert_eq!(delete["ok?"], true);
    assert_eq!(delete["id"], "user$shout");
    assert_eq!(delete["exists?"], true);
    assert_eq!(
        delete["would-delete"][0],
        root.join("shout.edn").display().to_string()
    );
    assert_eq!(delete["registry-skipped?"], true);
    assert_eq!(delete["write-skipped?"], true);
    assert!(root.join("shout.edn").is_file());
}

#[test]
fn user_tools_validate_projects_tools_validate_without_live_registry() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("tools");
    std::fs::create_dir_all(&root).unwrap();
    std::fs::write(
        root.join("shout.edn"),
        r#"{:name "shout" :input-schema [:map] :body "(fn [args] args)"}"#,
    )
    .unwrap();

    let assert = Command::cargo_bin("by-rs")
        .unwrap()
        .args(["user-tools", "validate", "--root"])
        .arg(&root)
        .args([
            "--name",
            "shout",
            "--input-schema-edn",
            "[:map [:text :string]]",
            "--body",
            "(fn [args] (:text args))",
        ])
        .assert()
        .success();
    let stdout = String::from_utf8(assert.get_output().stdout.clone()).unwrap();
    let valid: serde_json::Value = serde_json::from_str(&stdout).unwrap();
    assert_eq!(valid["projection"], "common.user-tools/tools$validate");
    assert_eq!(valid["valid"], true);
    assert_eq!(valid["name-ok"], true);
    assert_eq!(valid["collision"], true);
    assert_eq!(valid["schema-ok"], true);
    assert_eq!(valid["body-ok"], true);
    assert!(valid["errors"].as_array().unwrap().is_empty());
    assert_eq!(valid["eval-skipped?"], true);
    assert_eq!(valid["registry-skipped?"], true);
    assert_eq!(valid["write-skipped?"], true);

    let assert = Command::cargo_bin("by-rs")
        .unwrap()
        .args([
            "user-tools",
            "validate",
            "--input-schema-edn",
            "[:map [:text :string]]",
            "--body",
            "(fn [args] (:text args))",
        ])
        .assert()
        .success();
    let stdout = String::from_utf8(assert.get_output().stdout.clone()).unwrap();
    let unnamed: serde_json::Value = serde_json::from_str(&stdout).unwrap();
    assert_eq!(unnamed["projection"], "common.user-tools/tools$validate");
    assert_eq!(unnamed["valid"], true);
    assert!(unnamed.get("name-ok").is_none());
    assert_eq!(unnamed["collision"], false);
    assert_eq!(unnamed["schema-ok"], true);
    assert_eq!(unnamed["body-ok"], true);

    let assert = Command::cargo_bin("by-rs")
        .unwrap()
        .args(["user-tools", "validate", "--input-schema-edn", "[:vector]"])
        .assert()
        .success();
    let stdout = String::from_utf8(assert.get_output().stdout.clone()).unwrap();
    let invalid: serde_json::Value = serde_json::from_str(&stdout).unwrap();
    assert_eq!(invalid["valid"], false);
    assert_eq!(invalid["collision"], false);
    assert_eq!(invalid["schema-ok"], false);
    assert_eq!(invalid["body-ok"], false);
    let errors = invalid["errors"]
        .as_array()
        .unwrap()
        .iter()
        .map(|error| error.as_str().unwrap())
        .collect::<Vec<_>>();
    assert!(errors.iter().any(|error| error.contains(":input-schema")));
    assert!(errors.iter().any(|error| error.contains(":body")));
}

#[test]
fn user_tools_missing_root_is_empty_and_read_only() {
    let dir = tempfile::tempdir().unwrap();
    let missing = dir.path().join("missing-tools");

    let assert = Command::cargo_bin("by-rs")
        .unwrap()
        .args(["user-tools", "list", "--root"])
        .arg(&missing)
        .assert()
        .success();
    let stdout = String::from_utf8(assert.get_output().stdout.clone()).unwrap();
    let list: serde_json::Value = serde_json::from_str(&stdout).unwrap();
    assert_eq!(list["total"], 0);
    assert!(list["tools"].as_array().unwrap().is_empty());

    let assert = Command::cargo_bin("by-rs")
        .unwrap()
        .args(["user-tools", "read", "--root"])
        .arg(&missing)
        .arg("shout")
        .assert()
        .success();
    let stdout = String::from_utf8(assert.get_output().stdout.clone()).unwrap();
    let detail: serde_json::Value = serde_json::from_str(&stdout).unwrap();
    assert_eq!(detail["error"], "no user tool named \"shout\"");
    assert!(!missing.exists());
}

#[test]
fn memory_default_db_missing_is_read_only_and_does_not_create_dirs() {
    let project = tempfile::tempdir().unwrap();
    let home = tempfile::tempdir().unwrap();

    Command::cargo_bin("by-rs")
        .unwrap()
        .current_dir(project.path())
        .env("HOME", home.path())
        .env("BY_USER_ID", "missing-user")
        .env_remove("BY_ENV_FILE")
        .env_remove("BY_NO_DOTENV")
        .args(["memory", "inspect"])
        .assert()
        .failure()
        .stderr(predicate::str::contains(
            "opening memory sqlite database read-only",
        ));

    assert!(!home.path().join(".brainyard").exists());
    assert!(!project.path().join(".brainyard").exists());
}

#[test]
fn ask_dry_run_accepts_user_id_flag_from_main_contract() {
    Command::cargo_bin("by-rs")
        .unwrap()
        .env("BY_USER_ID", "env-user")
        .args([
            "ask",
            "--provider",
            "bedrock",
            "--model",
            "amazon.nova-lite-v1:0",
            "--user-id",
            "alice",
            "--dry-run",
            "What is 2+2?",
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains(
            "\"modelId\": \"amazon.nova-lite-v1:0\"",
        ))
        .stdout(predicate::str::contains("\"user_id\": \"alice\""))
        .stdout(predicate::str::contains("env-user").not());
}

#[test]
fn ask_dry_run_resolves_user_id_from_environment_when_flag_blank() {
    Command::cargo_bin("by-rs")
        .unwrap()
        .env("BY_USER_ID", "env-user")
        .args([
            "ask",
            "--provider",
            "bedrock",
            "--model",
            "amazon.nova-lite-v1:0",
            "--user-id",
            "   ",
            "--dry-run",
            "What is 2+2?",
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("\"user_id\": \"env-user\""));
}

#[test]
fn ask_dry_run_reads_user_id_and_bedrock_runtime_from_project_dotenv() {
    let project = tempfile::tempdir().unwrap();
    let home = tempfile::tempdir().unwrap();
    let nested = project.path().join("nested/work");
    std::fs::create_dir_all(&nested).unwrap();
    std::fs::write(
        project.path().join(".env"),
        r#"
        BY_USER_ID=dotenv-user
        AWS_REGION=ap-northeast-2
        AWS_PROFILE=dotenv-profile
        "#,
    )
    .unwrap();

    Command::cargo_bin("by-rs")
        .unwrap()
        .current_dir(&nested)
        .env("HOME", home.path())
        .env_remove("BY_ENV_FILE")
        .env_remove("BY_NO_DOTENV")
        .env_remove("BY_USER_ID")
        .env_remove("AWS_REGION")
        .env_remove("AWS_DEFAULT_REGION")
        .env_remove("AWS_PROFILE")
        .env_remove("AWS_DEFAULT_PROFILE")
        .args([
            "ask",
            "--provider",
            "bedrock",
            "--model",
            "amazon.nova-lite-v1:0",
            "--dry-run",
            "What is 2+2?",
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("\"user_id\": \"dotenv-user\""))
        .stdout(predicate::str::contains("\"region\": \"ap-northeast-2\""))
        .stdout(predicate::str::contains(
            "\"aws_profile\": \"dotenv-profile\"",
        ));
}

#[test]
fn ask_dry_run_prefers_environment_over_project_dotenv() {
    let project = tempfile::tempdir().unwrap();
    let home = tempfile::tempdir().unwrap();
    std::fs::write(
        project.path().join(".env"),
        r#"
        BY_USER_ID=dotenv-user
        AWS_REGION=ap-northeast-2
        AWS_PROFILE=dotenv-profile
        "#,
    )
    .unwrap();

    Command::cargo_bin("by-rs")
        .unwrap()
        .current_dir(project.path())
        .env("HOME", home.path())
        .env_remove("BY_ENV_FILE")
        .env_remove("BY_NO_DOTENV")
        .env("BY_USER_ID", "env-user")
        .env("AWS_REGION", "eu-west-1")
        .env("AWS_PROFILE", "env-profile")
        .args([
            "ask",
            "--provider",
            "bedrock",
            "--model",
            "amazon.nova-lite-v1:0",
            "--dry-run",
            "What is 2+2?",
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("\"user_id\": \"env-user\""))
        .stdout(predicate::str::contains("\"region\": \"eu-west-1\""))
        .stdout(predicate::str::contains("\"aws_profile\": \"env-profile\""))
        .stdout(predicate::str::contains("dotenv-user").not())
        .stdout(predicate::str::contains("ap-northeast-2").not())
        .stdout(predicate::str::contains("dotenv-profile").not());
}

#[test]
fn ask_dry_run_renders_bedrock_converse_request_without_network() {
    Command::cargo_bin("by-rs")
        .unwrap()
        .args([
            "ask",
            "--provider",
            "bedrock",
            "--model",
            "amazon.nova-lite-v1:0",
            "--dry-run",
            "What is 2+2?",
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("\"provider\": \"bedrock\""))
        .stdout(predicate::str::contains("\"operation\": \"Converse\""))
        .stdout(predicate::str::contains(
            "\"modelId\": \"amazon.nova-lite-v1:0\"",
        ))
        .stdout(predicate::str::contains("\"role\": \"user\""))
        .stdout(predicate::str::contains("\"text\": \"What is 2+2?\""));
}

#[test]
fn ask_dry_run_accepts_short_provider_and_model_flags_like_clojure() {
    Command::cargo_bin("by-rs")
        .unwrap()
        .args([
            "ask",
            "-p",
            "bedrock",
            "-m",
            "amazon.nova-lite-v1:0",
            "--dry-run",
            "What is 2+2?",
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("\"provider\": \"bedrock\""))
        .stdout(predicate::str::contains("\"operation\": \"Converse\""))
        .stdout(predicate::str::contains(
            "\"modelId\": \"amazon.nova-lite-v1:0\"",
        ))
        .stdout(predicate::str::contains("\"text\": \"What is 2+2?\""));
}

#[test]
fn ask_dry_run_renders_openai_chat_completions_request_without_network() {
    Command::cargo_bin("by-rs")
        .unwrap()
        .env("BY_NO_DOTENV", "1")
        .args([
            "ask",
            "--provider",
            "openai",
            "--model",
            "gpt-4o",
            "--temperature",
            "0.2",
            "--max-tokens",
            "64",
            "--dry-run",
            "What is 2+2?",
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("\"provider\": \"openai\""))
        .stdout(predicate::str::contains(
            "\"operation\": \"chat/completions\"",
        ))
        .stdout(predicate::str::contains("\"network\": false"))
        .stdout(predicate::str::contains("\"model\": \"gpt-4o\""))
        .stdout(predicate::str::contains("\"temperature\": 0.2"))
        .stdout(predicate::str::contains("\"max_tokens\": 64"))
        .stdout(predicate::str::contains("\"role\": \"user\""))
        .stdout(predicate::str::contains("\"content\": \"What is 2+2?\""));
}

#[test]
fn ask_dry_run_drops_openai_temperature_for_gpt5_family() {
    Command::cargo_bin("by-rs")
        .unwrap()
        .env("BY_NO_DOTENV", "1")
        .args([
            "ask",
            "--provider",
            "openai",
            "--model",
            "gpt-5",
            "--temperature",
            "0.2",
            "--dry-run",
            "What is 2+2?",
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("\"provider\": \"openai\""))
        .stdout(predicate::str::contains("\"model\": \"gpt-5\""))
        .stdout(predicate::str::contains("\"temperature\"").not());
}

#[test]
fn ask_dry_run_renders_anthropic_messages_request_without_network() {
    Command::cargo_bin("by-rs")
        .unwrap()
        .env("BY_NO_DOTENV", "1")
        .args([
            "ask",
            "--provider",
            "anthropic",
            "--model",
            "claude-sonnet-4-5",
            "--dry-run",
            "What is 2+2?",
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("\"provider\": \"anthropic\""))
        .stdout(predicate::str::contains("\"operation\": \"messages\""))
        .stdout(predicate::str::contains("\"network\": false"))
        .stdout(predicate::str::contains("\"model\": \"claude-sonnet-4-5\""))
        .stdout(predicate::str::contains("\"max_tokens\": 4096"))
        .stdout(predicate::str::contains("\"role\": \"user\""))
        .stdout(predicate::str::contains("\"content\": \"What is 2+2?\""));
}

#[test]
fn ask_dry_run_renders_claude_code_subprocess_request_without_network() {
    Command::cargo_bin("by-rs")
        .unwrap()
        .env("BY_NO_DOTENV", "1")
        .args([
            "ask",
            "--provider",
            "claude-code",
            "--model",
            "claude-sonnet-4-6",
            "--max-tokens",
            "32",
            "--dry-run",
            "What is 2+2?",
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("\"provider\": \"claude-code\""))
        .stdout(predicate::str::contains(
            "\"operation\": \"claude-code/subprocess\"",
        ))
        .stdout(predicate::str::contains("\"network\": false"))
        .stdout(predicate::str::contains("\"argv\""))
        .stdout(predicate::str::contains("\"--strict-mcp-config\""))
        .stdout(predicate::str::contains("\"stdin\": \"What is 2+2?\""));
}

#[test]
fn ask_claude_code_live_invokes_cli_and_prints_result() {
    let home = tempfile::tempdir().unwrap();
    let path_dir = tempfile::tempdir().unwrap();
    let capture_path = home.path().join("claude-ask-stdin.txt");
    write_fake_executable(
        &path_dir.path().join("claude"),
        r#"#!/usr/bin/env bash
cat > "$CLAUDE_CAPTURE_STDIN"
printf '%s\n' '{"type":"result","subtype":"success","is_error":false,"result":"Claude ask fixture answer","usage":{"input_tokens":5,"output_tokens":6}}'
"#,
    );

    Command::cargo_bin("by-rs")
        .unwrap()
        .env("HOME", home.path())
        .env("BY_NO_DOTENV", "1")
        .env("PATH", prepend_path(path_dir.path()))
        .env("CLAUDE_CAPTURE_STDIN", &capture_path)
        .args(["ask", "-p", "claude-code", "-m", "haiku", "Hello Claude"])
        .assert()
        .success()
        .stdout(predicate::str::contains(
            "LM configured: claude-code / haiku",
        ))
        .stdout(predicate::str::contains("Claude ask fixture answer"))
        .stderr(predicate::str::is_empty());

    assert_eq!(
        std::fs::read_to_string(capture_path).unwrap(),
        "Hello Claude"
    );
}

#[test]
fn memory_essence_extract_claude_code_live_invokes_cli_and_parses_result() {
    let home = tempfile::tempdir().unwrap();
    let path_dir = tempfile::tempdir().unwrap();
    let capture_path = home.path().join("claude-memory-stdin.txt");
    write_fake_executable(
        &path_dir.path().join("claude"),
        r#"#!/usr/bin/env bash
cat > "$CLAUDE_CAPTURE_STDIN"
printf '%s\n' '{"type":"result","subtype":"success","is_error":false,"result":"{\"essences\":[\"carry provider parity\"],\"reasoning\":\"parsed\"}","usage":{"input_tokens":7,"output_tokens":8}}'
"#,
    );

    Command::cargo_bin("by-rs")
        .unwrap()
        .env("HOME", home.path())
        .env("BY_NO_DOTENV", "1")
        .env("PATH", prepend_path(path_dir.path()))
        .env("CLAUDE_CAPTURE_STDIN", &capture_path)
        .args([
            "memory",
            "essence-extract",
            "--turn-summary",
            "summary",
            "--turn-messages",
            "user: hi",
            "--recent-episodes",
            "[]",
            "--live",
            "--provider",
            "claude-code",
            "--model",
            "haiku",
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("\"provider\": \"claude-code\""))
        .stdout(predicate::str::contains("\"source\": \"claude-code-live\""))
        .stdout(predicate::str::contains("\"network\": true"))
        .stdout(predicate::str::contains("carry provider parity"))
        .stdout(predicate::str::contains("\"reasoning\": \"parsed\""))
        .stderr(predicate::str::is_empty());

    let captured = std::fs::read_to_string(capture_path).unwrap();
    assert!(captured.contains("summary"));
    assert!(captured.contains("user: hi"));
}

#[test]
fn ask_dry_run_renders_acp_prompt_request_without_network() {
    Command::cargo_bin("by-rs")
        .unwrap()
        .env("BY_NO_DOTENV", "1")
        .args([
            "ask",
            "--provider",
            "acp",
            "--model",
            "stub",
            "--dry-run",
            "What is 2+2?",
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("\"provider\": \"acp\""))
        .stdout(predicate::str::contains(
            "\"operation\": \"acp/session-prompt\"",
        ))
        .stdout(predicate::str::contains("\"network\": false"))
        .stdout(predicate::str::contains("\"backend\": \"stub\""))
        .stdout(predicate::str::contains("\"text\": \"What is 2+2?\""));
}

#[test]
fn ask_help_matches_clojure_command_surface() {
    Command::cargo_bin("by-rs")
        .unwrap()
        .args(["ask", "--help"])
        .assert()
        .success()
        .stderr(predicate::str::contains(
            "NAME:\n by ask - Ask a one-shot question (non-interactive)",
        ))
        .stderr(predicate::str::contains("coact-agent  Agent ID"))
        .stderr(predicate::str::contains("claude-code  LM provider"))
        .stderr(predicate::str::contains("-u, --user-id S"))
        .stderr(predicate::str::contains("--dry-run").not())
        .stderr(predicate::str::contains("--live").not())
        .stderr(predicate::str::contains("--fixture-response").not());
}

#[test]
fn ask_fixture_response_replays_bedrock_output_without_network() {
    let fixture = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../fixtures/bedrock/converse-response.json");

    Command::cargo_bin("by-rs")
        .unwrap()
        .env_remove("AWS_REGION")
        .env_remove("AWS_DEFAULT_REGION")
        .env_remove("AWS_PROFILE")
        .env_remove("AWS_DEFAULT_PROFILE")
        .args([
            "ask",
            "--provider",
            "bedrock",
            "--model",
            "amazon.nova-lite-v1:0",
            "--fixture-response",
        ])
        .arg(fixture)
        .arg("What is 2+2?")
        .assert()
        .success()
        .stdout(predicate::str::contains("Hello world"));
}

#[test]
fn ask_fixture_response_replays_openai_output_without_network() {
    let fixture = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../fixtures/openai/chat-completion-response.json");

    Command::cargo_bin("by-rs")
        .unwrap()
        .env("BY_NO_DOTENV", "1")
        .env_remove("AWS_REGION")
        .env_remove("AWS_DEFAULT_REGION")
        .env_remove("AWS_PROFILE")
        .env_remove("AWS_DEFAULT_PROFILE")
        .args([
            "ask",
            "--provider",
            "openai",
            "--model",
            "gpt-5",
            "--fixture-response",
        ])
        .arg(fixture)
        .arg("What is 2+2?")
        .assert()
        .success()
        .stdout(predicate::str::contains("Hello from OpenAI fixture"));
}

#[test]
fn ask_fixture_response_replays_anthropic_output_without_network() {
    let fixture = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../fixtures/anthropic/messages-response.json");

    Command::cargo_bin("by-rs")
        .unwrap()
        .env("BY_NO_DOTENV", "1")
        .env_remove("AWS_REGION")
        .env_remove("AWS_DEFAULT_REGION")
        .env_remove("AWS_PROFILE")
        .env_remove("AWS_DEFAULT_PROFILE")
        .args([
            "ask",
            "--provider",
            "anthropic",
            "--model",
            "claude-sonnet-4-5",
            "--fixture-response",
        ])
        .arg(fixture)
        .arg("What is 2+2?")
        .assert()
        .success()
        .stdout(predicate::str::contains("Hello from Anthropic fixture"));
}

#[test]
fn ask_fixture_response_replays_claude_code_output_without_network() {
    let fixture = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../fixtures/claude-code/result-events.json");

    Command::cargo_bin("by-rs")
        .unwrap()
        .env("BY_NO_DOTENV", "1")
        .args([
            "ask",
            "--provider",
            "claude-code",
            "--model",
            "claude-sonnet-4-6",
            "--fixture-response",
        ])
        .arg(fixture)
        .arg("What is 2+2?")
        .assert()
        .success()
        .stdout(predicate::str::contains("Hello from Claude Code fixture"));
}

#[test]
fn ask_fixture_response_replays_acp_output_without_network() {
    let fixture = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../fixtures/acp/session-prompt-response.json");

    Command::cargo_bin("by-rs")
        .unwrap()
        .env("BY_NO_DOTENV", "1")
        .args([
            "ask",
            "--provider",
            "acp",
            "--model",
            "stub",
            "--fixture-response",
        ])
        .arg(fixture)
        .arg("What is 2+2?")
        .assert()
        .success()
        .stdout(predicate::str::contains("Hello from ACP fixture"));
}

#[test]
fn ask_fixture_response_rejects_other_execution_modes() {
    let fixture = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../fixtures/bedrock/converse-response.json");

    Command::cargo_bin("by-rs")
        .unwrap()
        .args([
            "ask",
            "--provider",
            "bedrock",
            "--model",
            "amazon.nova-lite-v1:0",
            "--dry-run",
            "--fixture-response",
        ])
        .arg(fixture)
        .arg("What is 2+2?")
        .assert()
        .failure()
        .stderr(predicate::str::contains(
            "choose only one of --dry-run or --fixture-response",
        ));
}

#[test]
fn ask_default_bedrock_without_model_matches_clojure_setup_error() {
    Command::cargo_bin("by-rs")
        .unwrap()
        .env("BY_NO_DOTENV", "1")
        .args(["ask", "--provider", "bedrock", "What is 2+2?"])
        .assert()
        .code(255)
        .stderr(predicate::str::contains("** ERROR: **"))
        .stderr(predicate::str::contains(
            "Cannot invoke \"String.contains(java.lang.CharSequence)\" because \"model\" is null",
        ));
}

#[test]
fn ask_missing_question_matches_clojure_usage_error() {
    Command::cargo_bin("by-rs")
        .unwrap()
        .args([
            "ask",
            "--provider",
            "bedrock",
            "--model",
            "amazon.nova-lite-v1:0",
            "--dry-run",
        ])
        .assert()
        .failure()
        .stdout(predicate::str::contains(
            "Error: question argument is required.",
        ))
        .stdout(predicate::str::contains("Usage: by ask [options] QUESTION"))
        .stderr(predicate::str::is_empty());
}

#[test]
fn ask_missing_question_precedes_execution_mode_requirement() {
    Command::cargo_bin("by-rs")
        .unwrap()
        .args([
            "ask",
            "--provider",
            "bedrock",
            "--model",
            "amazon.nova-lite-v1:0",
        ])
        .assert()
        .failure()
        .stdout(predicate::str::contains(
            "Error: question argument is required.",
        ))
        .stdout(predicate::str::contains("Usage: by ask [options] QUESTION"))
        .stderr(predicate::str::is_empty());
}

#[test]
fn ask_missing_question_with_short_provider_and_model_flags_matches_clojure() {
    Command::cargo_bin("by-rs")
        .unwrap()
        .args(["ask", "-p", "bedrock", "-m", "amazon.nova-lite-v1:0"])
        .assert()
        .failure()
        .stdout(predicate::str::contains(
            "Error: question argument is required.",
        ))
        .stdout(predicate::str::contains("Usage: by ask [options] QUESTION"))
        .stderr(predicate::str::is_empty());
}

#[test]
fn ask_test_only_options_are_unknown_without_gate() {
    Command::cargo_bin("by-rs")
        .unwrap()
        .env_remove("BY_RS_ALLOW_ASK_TEST_OPTIONS")
        .args(["ask", "--dry-run", "hello"])
        .assert()
        .code(255)
        .stdout(predicate::str::is_empty())
        .stderr(predicate::str::contains(
            "Option error: Unknown option: \"--dry-run\"",
        ))
        .stderr(predicate::str::contains("by ask - Ask a one-shot question"));
}

#[test]
fn ask_blank_question_matches_clojure_usage_error() {
    Command::cargo_bin("by-rs")
        .unwrap()
        .args([
            "ask",
            "--provider",
            "bedrock",
            "--model",
            "amazon.nova-lite-v1:0",
            "--dry-run",
            "   ",
        ])
        .assert()
        .failure()
        .stdout(predicate::str::contains(
            "Error: question argument is required.",
        ))
        .stdout(predicate::str::contains("Usage: by ask [options] QUESTION"))
        .stderr(predicate::str::is_empty());
}

#[test]
fn ask_live_flag_is_not_part_of_clojure_cli_surface() {
    Command::cargo_bin("by-rs")
        .unwrap()
        .args([
            "ask",
            "--provider",
            "bedrock",
            "--model",
            "amazon.nova-lite-v1:0",
            "--dry-run",
            "--live",
            "What is 2+2?",
        ])
        .assert()
        .failure()
        .stderr(predicate::str::contains("unexpected argument '--live'"));
}

#[test]
fn ask_live_flag_is_rejected_before_provider_handling() {
    Command::cargo_bin("by-rs")
        .unwrap()
        .env("BY_NO_DOTENV", "1")
        .args([
            "ask",
            "--provider",
            "openai",
            "--model",
            "gpt-5",
            "--live",
            "What is 2+2?",
        ])
        .assert()
        .failure()
        .stderr(predicate::str::contains("unexpected argument '--live'"));
}

#[test]
fn ask_dry_run_uses_config_defaults_when_provider_and_model_are_omitted() {
    let home = tempfile::tempdir().unwrap();
    let brainyard = home.path().join(".brainyard");
    std::fs::create_dir_all(&brainyard).unwrap();
    std::fs::write(
        brainyard.join("config.edn"),
        r#"{:llm {:default-provider :bedrock
                 :default-model "amazon.nova-lite-v1:0"}}"#,
    )
    .unwrap();

    Command::cargo_bin("by-rs")
        .unwrap()
        .current_dir(home.path())
        .env("HOME", home.path())
        .env_remove("BRAINYARD_PROJECT_DIR")
        .args(["ask", "--dry-run", "What is 2+2?"])
        .assert()
        .success()
        .stdout(predicate::str::contains("\"provider\": \"bedrock\""))
        .stdout(predicate::str::contains(
            "\"modelId\": \"amazon.nova-lite-v1:0\"",
        ));
}

#[test]
fn ask_dry_run_parses_legacy_provider_model_positional() {
    Command::cargo_bin("by-rs")
        .unwrap()
        .args([
            "ask",
            "--dry-run",
            "bedrock:amazon.nova-lite-v1",
            "What is 2+2?",
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("\"provider\": \"bedrock\""))
        .stdout(predicate::str::contains(
            "\"modelId\": \"amazon.nova-lite-v1\"",
        ))
        .stdout(predicate::str::contains("\"text\": \"What is 2+2?\""));
}

#[test]
fn ask_dry_run_removes_legacy_provider_model_from_any_positional() {
    Command::cargo_bin("by-rs")
        .unwrap()
        .args([
            "ask",
            "--dry-run",
            "What is 2+2?",
            "bedrock:amazon.nova-lite-v1",
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("\"provider\": \"bedrock\""))
        .stdout(predicate::str::contains(
            "\"modelId\": \"amazon.nova-lite-v1\"",
        ))
        .stdout(predicate::str::contains("\"text\": \"What is 2+2?\""));
}

#[test]
fn ask_dry_run_keeps_bedrock_model_id_with_second_colon_as_question_text() {
    Command::cargo_bin("by-rs")
        .unwrap()
        .args([
            "ask",
            "--provider",
            "bedrock",
            "--model",
            "amazon.nova-lite-v1:0",
            "--dry-run",
            "bedrock:amazon.nova-lite-v1:0",
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains(
            "\"modelId\": \"amazon.nova-lite-v1:0\"",
        ))
        .stdout(predicate::str::contains(
            "\"text\": \"bedrock:amazon.nova-lite-v1:0\"",
        ));
}

#[test]
fn ask_dry_run_resolves_bedrock_region_and_profile_from_environment() {
    Command::cargo_bin("by-rs")
        .unwrap()
        .env("AWS_REGION", "eu-west-1")
        .env("AWS_DEFAULT_REGION", "us-east-1")
        .env("AWS_PROFILE", "dev")
        .env("AWS_DEFAULT_PROFILE", "fallback")
        .args([
            "ask",
            "--provider",
            "bedrock",
            "--model",
            "amazon.nova-lite-v1:0",
            "--dry-run",
            "What is 2+2?",
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("\"region\": \"eu-west-1\""))
        .stdout(predicate::str::contains("\"aws_profile\": \"dev\""));
}

#[test]
fn ask_dry_run_uses_aws_default_profile_when_aws_profile_is_absent() {
    Command::cargo_bin("by-rs")
        .unwrap()
        .env("BY_NO_DOTENV", "1")
        .env_remove("AWS_PROFILE")
        .env("AWS_DEFAULT_PROFILE", "fallback")
        .args([
            "ask",
            "--provider",
            "bedrock",
            "--model",
            "amazon.nova-lite-v1:0",
            "--dry-run",
            "What is 2+2?",
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("\"aws_profile\": \"fallback\""));
}

#[test]
fn ask_dry_run_prefers_bedrock_catalog_region_pin_over_environment() {
    let assert = Command::cargo_bin("by-rs")
        .unwrap()
        .env("BY_NO_DOTENV", "1")
        .env("AWS_REGION", "ap-northeast-2")
        .env("AWS_DEFAULT_REGION", "eu-west-1")
        .env_remove("AWS_PROFILE")
        .env_remove("AWS_DEFAULT_PROFILE")
        .args([
            "ask",
            "--provider",
            "bedrock",
            "--model",
            "openai.gpt-oss-120b-1:0",
            "--dry-run",
            "What is 2+2?",
        ])
        .assert()
        .success();
    let stdout = String::from_utf8(assert.get_output().stdout.clone()).unwrap();
    let value: serde_json::Value = serde_json::from_str(&stdout).unwrap();

    assert_eq!(value["region"], "us-east-1");
    assert_eq!(value["request"]["modelId"], "openai.gpt-oss-120b-1:0");
}

#[test]
fn ask_dry_run_explicit_region_and_profile_win_over_environment() {
    Command::cargo_bin("by-rs")
        .unwrap()
        .env("AWS_REGION", "eu-west-1")
        .env("AWS_PROFILE", "dev")
        .args([
            "ask",
            "--provider",
            "bedrock",
            "--model",
            "amazon.nova-lite-v1:0",
            "--region",
            "ap-northeast-2",
            "--aws-profile",
            "sandbox",
            "--dry-run",
            "What is 2+2?",
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("\"region\": \"ap-northeast-2\""))
        .stdout(predicate::str::contains("\"aws_profile\": \"sandbox\""));
}

#[test]
fn ask_dry_run_exposes_bedrock_inference_overrides() {
    Command::cargo_bin("by-rs")
        .unwrap()
        .env("BY_NO_DOTENV", "1")
        .env_remove("AWS_REGION")
        .env_remove("AWS_DEFAULT_REGION")
        .env_remove("AWS_PROFILE")
        .env_remove("AWS_DEFAULT_PROFILE")
        .args([
            "ask",
            "--provider",
            "bedrock",
            "--model",
            "amazon.nova-lite-v1:0",
            "--temperature",
            "0.2",
            "--max-tokens",
            "64",
            "--no-prompt-cache",
            "--dry-run",
            "What is 2+2?",
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("\"region\": \"us-east-1\""))
        .stdout(predicate::str::contains("\"temperature\": 0.2"))
        .stdout(predicate::str::contains("\"maxTokens\": 64"))
        .stdout(predicate::str::contains("cachePoint").not());
}

#[test]
fn tui_snapshot_renders_static_chrome() {
    Command::cargo_bin("by-rs")
        .unwrap()
        .args([
            "tui",
            "snapshot",
            "--agent",
            "coact-agent",
            "--model",
            "bedrock:amazon.nova-lite-v1:0",
            "--rows",
            "12",
            "--cols",
            "56",
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("Brainyard by-rs"))
        .stdout(predicate::str::contains("agent coact-agent"))
        .stdout(predicate::str::contains(" 0:main0*"))
        .stdout(predicate::str::contains(
            "idle │ 0 calls │ 0 tokens │ $0.0000",
        ));
}

fn create_memory_schema(conn: &Connection) {
    conn.execute_batch(
        r#"
        CREATE TABLE memory_metadata (
          key TEXT PRIMARY KEY,
          value TEXT NOT NULL,
          updated_at DATETIME DEFAULT CURRENT_TIMESTAMP
        );

        CREATE TABLE episodes (
          id INTEGER PRIMARY KEY AUTOINCREMENT,
          session_id TEXT NOT NULL,
          user_id TEXT NOT NULL,
          timestamp DATETIME DEFAULT CURRENT_TIMESTAMP,
          episode_type TEXT NOT NULL,
          role TEXT,
          content TEXT NOT NULL,
          metadata TEXT,
          tags TEXT,
          sources TEXT,
          entry_id TEXT,
          keep_flag INTEGER NOT NULL DEFAULT 0,
          archived_flag INTEGER NOT NULL DEFAULT 0,
          tombstoned_flag INTEGER NOT NULL DEFAULT 0
        );

        CREATE VIRTUAL TABLE episodes_fts USING fts5(
          content,
          episode_type,
          role,
          content='episodes',
          content_rowid='id',
          tokenize='porter unicode61'
        );

        CREATE TRIGGER episodes_ai AFTER INSERT ON episodes BEGIN
          INSERT INTO episodes_fts(rowid, content, episode_type, role)
          VALUES (new.id, new.content, new.episode_type, new.role);
        END;

        CREATE TABLE semantic_facts (
          id INTEGER PRIMARY KEY AUTOINCREMENT,
          user_id TEXT NOT NULL,
          fact_type TEXT NOT NULL,
          content TEXT NOT NULL,
          source TEXT,
          confidence REAL DEFAULT 1.0,
          created_at DATETIME DEFAULT CURRENT_TIMESTAMP,
          updated_at DATETIME DEFAULT CURRENT_TIMESTAMP,
          access_count INTEGER DEFAULT 0,
          last_accessed DATETIME,
          metadata TEXT,
          tags TEXT,
          sources TEXT,
          entry_id TEXT,
          keep_flag INTEGER NOT NULL DEFAULT 0,
          archived_flag INTEGER NOT NULL DEFAULT 0,
          tombstoned_flag INTEGER NOT NULL DEFAULT 0
        );

        CREATE VIRTUAL TABLE semantic_fts USING fts5(
          content,
          fact_type,
          content='semantic_facts',
          content_rowid='id',
          tokenize='porter unicode61'
        );

        CREATE TRIGGER semantic_facts_ai AFTER INSERT ON semantic_facts BEGIN
          INSERT INTO semantic_fts(rowid, content, fact_type)
          VALUES (new.id, new.content, new.fact_type);
        END;

        CREATE TABLE memory_audit (
          id INTEGER PRIMARY KEY AUTOINCREMENT,
          user_id TEXT NOT NULL,
          session_id TEXT NOT NULL,
          agent_id TEXT,
          turn_id INTEGER NOT NULL,
          total_turns INTEGER,
          entry_id TEXT NOT NULL,
          layer TEXT NOT NULL,
          byte_cost INTEGER,
          created_at DATETIME DEFAULT CURRENT_TIMESTAMP
        );
        "#,
    )
    .unwrap();
}

fn seed_memory_explain_rows(conn: &Connection) {
    conn.execute(
        "INSERT INTO episodes
         (session_id, user_id, episode_type, role, content, metadata, tags, sources, entry_id,
          keep_flag, archived_flag)
         VALUES
         ('s1', 'u1', 'conversation', 'assistant', 'explain blue episode',
          '{\"ttl\":\"session\",\"data\":{\"topic\":\"blue\"},\"metadata\":{\"origin\":\"fixture\"}}',
          '[\"blue\",\"audit\"]', '[{\"kind\":\"manual\"}]', 'ep-explain', 1, 1)",
        [],
    )
    .unwrap();
    conn.execute(
        "INSERT INTO semantic_facts
         (user_id, fact_type, content, source, confidence, access_count, metadata, tags, sources,
          entry_id, tombstoned_flag)
         VALUES
         ('u1', 'preference', 'explain green fact', 'manual', 0.8, 7,
          '{\"data\":{\"topic\":\"green\"}}',
          '[\"green\"]', '[\"manual\"]', 'fact-explain', 1)",
        [],
    )
    .unwrap();
    conn.execute(
        "INSERT INTO memory_audit
         (user_id, session_id, agent_id, turn_id, total_turns, entry_id, layer, byte_cost)
         VALUES ('u1', 's1', 'coact-agent', 3, 2, 'ep-explain', 'l2', 20)",
        [],
    )
    .unwrap();
    conn.execute(
        "INSERT INTO memory_audit
         (user_id, session_id, agent_id, turn_id, total_turns, entry_id, layer, byte_cost)
         VALUES ('u1', 's1', 'coact-agent', 3, 2, 'fact-explain', 'l3', 31)",
        [],
    )
    .unwrap();
}

#[test]
fn workflow_resume_projects_existing_dossier_without_live_agent() {
    let project = tempfile::tempdir().unwrap();
    let workflow_dir = project
        .path()
        .join(".brainyard/agents/workflow-agent/rust-port");
    std::fs::create_dir_all(&workflow_dir).unwrap();
    std::fs::write(
        workflow_dir.join("dossier.md"),
        r#"---
status: in-progress
last_iteration: 3
hitl_mode: gates
acceptance:
  - id: scope
    status: satisfied
    text: Scope locked
  - id: tests
    status: open
    text: Tests passing
---
Body
"#,
    )
    .unwrap();
    std::fs::write(
        workflow_dir.join("stages.edn"),
        r#"{:stages [{:id :plan :status :satisfied}
           {:id :exec :status :in-progress}
           {:id "qa" :status :pending}
           {:id :skip :status :skipped}]}"#,
    )
    .unwrap();

    let assert = Command::cargo_bin("by-rs")
        .unwrap()
        .args(["workflow", "resume", "--id", "rust-port", "--base-dir"])
        .arg(project.path())
        .assert()
        .success()
        .stderr(predicate::str::is_empty());
    let stdout = String::from_utf8(assert.get_output().stdout.clone()).unwrap();
    let projected: serde_json::Value = serde_json::from_str(&stdout).unwrap();

    assert_eq!(projected["projection"], "common.workflow/workflow$resume?");
    assert_eq!(projected["exists?"], true);
    assert_eq!(projected["status"], "in-progress");
    assert_eq!(projected["last-iteration"], 3);
    assert_eq!(projected["hitl-mode"], "gates");
    assert_eq!(projected["acceptance-state"]["scope"], "satisfied");
    assert_eq!(projected["acceptance-state"]["tests"], "open");
    assert_eq!(
        projected["pending-stages"],
        serde_json::json!(["exec", "qa"])
    );
    assert_eq!(projected["stage-count"], 4);
    assert_eq!(projected["n-pending"], 2);
}

#[test]
fn research_resume_projects_existing_dossier_without_live_agent() {
    let project = tempfile::tempdir().unwrap();
    let research_dir = project
        .path()
        .join(".brainyard/agents/research-agent/rust-port");
    std::fs::create_dir_all(&research_dir).unwrap();
    std::fs::write(
        research_dir.join("dossier.md"),
        r#"---
status: in-progress
last_iteration: 4
acceptance:
  - id: scope
    status: satisfied
    text: Scope locked
  - id: tests
    status: open
    text: Tests passing
---
Body
"#,
    )
    .unwrap();

    let assert = Command::cargo_bin("by-rs")
        .unwrap()
        .args(["research", "resume", "--id", "rust-port", "--base-dir"])
        .arg(project.path())
        .assert()
        .success()
        .stderr(predicate::str::is_empty());
    let stdout = String::from_utf8(assert.get_output().stdout.clone()).unwrap();
    let projected: serde_json::Value = serde_json::from_str(&stdout).unwrap();

    assert_eq!(projected["projection"], "common.research/research$resume?");
    assert_eq!(projected["exists?"], true);
    assert_eq!(projected["status"], "in-progress");
    assert_eq!(projected["last-iteration"], 4);
    assert_eq!(projected["acceptance-state"]["scope"], "satisfied");
    assert_eq!(projected["acceptance-state"]["tests"], "open");
}

#[test]
fn main_agent_resume_projects_existing_routing_log_without_live_agent() {
    let project = tempfile::tempdir().unwrap();
    let session_dir = project
        .path()
        .join(".brainyard/agents/main-agent/agt-rust-port");
    std::fs::create_dir_all(&session_dir).unwrap();
    std::fs::write(
        session_dir.join("routing.log"),
        "{\"turn\":1,\"shape\":\"analysis\",\"artifact\":\"plan.md\"}\n{\"turn\":2,\"shape\":\"tool-lifecycle\",\"artifact\":\"verdict.md\"}\n",
    )
    .unwrap();

    let assert = Command::cargo_bin("by-rs")
        .unwrap()
        .args([
            "main",
            "resume",
            "--session-id",
            "agt-rust-port",
            "--base-dir",
        ])
        .arg(project.path())
        .assert()
        .success()
        .stderr(predicate::str::is_empty());
    let stdout = String::from_utf8(assert.get_output().stdout.clone()).unwrap();
    let projected: serde_json::Value = serde_json::from_str(&stdout).unwrap();

    assert_eq!(projected["projection"], "common.main/main$resume?");
    assert_eq!(projected["exists?"], true);
    assert_eq!(projected["line-count"], 2);
    assert_eq!(projected["turn-count"], 2);
    assert_eq!(projected["last-shape"], "tool-lifecycle");
    assert_eq!(projected["last-artifact"], "verdict.md");
}

#[test]
fn workflow_and_research_identity_template_probes_include_oracle_projection() {
    let project = tempfile::tempdir().unwrap();
    let workflows_dir = project.path().join(".brainyard/workflows");
    std::fs::create_dir_all(&workflows_dir).unwrap();
    std::fs::write(
        workflows_dir.join("project-flow.edn"),
        r#"{:workflow/id :project-flow
 :workflow/name "Project Flow"
 :workflow/description "Project local"}"#,
    )
    .unwrap();

    let assert = Command::cargo_bin("by-rs")
        .unwrap()
        .args([
            "workflow",
            "id",
            "--template",
            "feature-launch",
            "--question",
            "Can we ship the Rust port?",
            "--max-chars",
            "40",
        ])
        .assert()
        .success()
        .stderr(predicate::str::is_empty());
    let stdout = String::from_utf8(assert.get_output().stdout.clone()).unwrap();
    let workflow_id: serde_json::Value = serde_json::from_str(&stdout).unwrap();
    assert_eq!(workflow_id["projection"], "common.workflow/workflow$id");
    assert_eq!(workflow_id["slug"], "feature-launch--rust-port");

    let assert = Command::cargo_bin("by-rs")
        .unwrap()
        .args(["workflow", "list-templates", "--base-dir"])
        .arg(project.path())
        .assert()
        .success()
        .stderr(predicate::str::is_empty());
    let stdout = String::from_utf8(assert.get_output().stdout.clone()).unwrap();
    let templates: serde_json::Value = serde_json::from_str(&stdout).unwrap();
    assert_eq!(
        templates["projection"],
        "common.workflow/workflow$list-templates"
    );
    assert!(templates["templates"]
        .as_array()
        .unwrap()
        .iter()
        .any(|template| template["id"] == "project-flow"));

    let assert = Command::cargo_bin("by-rs")
        .unwrap()
        .args([
            "workflow",
            "load-template",
            "--id",
            "doc-update",
            "--base-dir",
        ])
        .arg(project.path())
        .assert()
        .success()
        .stderr(predicate::str::is_empty());
    let stdout = String::from_utf8(assert.get_output().stdout.clone()).unwrap();
    let template: serde_json::Value = serde_json::from_str(&stdout).unwrap();
    assert_eq!(
        template["projection"],
        "common.workflow/workflow$load-template"
    );
    assert_eq!(template["source"], "built-in");

    let assert = Command::cargo_bin("by-rs")
        .unwrap()
        .args([
            "research",
            "id",
            "--question",
            "Port Clojure Rust?",
            "--max-chars",
            "40",
        ])
        .assert()
        .success()
        .stderr(predicate::str::is_empty());
    let stdout = String::from_utf8(assert.get_output().stdout.clone()).unwrap();
    let research_id: serde_json::Value = serde_json::from_str(&stdout).unwrap();
    assert_eq!(research_id["projection"], "common.research/research$id");
    assert_eq!(research_id["slug"], "port-clojure-rust");
}

#[test]
fn main_and_memory_read_only_probes_include_oracle_projection() {
    let project = tempfile::tempdir().unwrap();
    let routing_dir = project
        .path()
        .join(".brainyard/agents/main-agent/session-1");
    std::fs::create_dir_all(&routing_dir).unwrap();
    std::fs::write(
        routing_dir.join("routing.log"),
        "{\"turn\":1,\"shape\":\"explore\",\"artifact\":\"a.md\"}\n{\"turn\":2,\"shape\":\"direct-answer\",\"artifact\":\"b.md\"}\n",
    )
    .unwrap();

    let assert = Command::cargo_bin("by-rs")
        .unwrap()
        .args([
            "main",
            "last-shape",
            "--session-id",
            "session-1",
            "--base-dir",
        ])
        .arg(project.path())
        .assert()
        .success()
        .stderr(predicate::str::is_empty());
    let stdout = String::from_utf8(assert.get_output().stdout.clone()).unwrap();
    let last_shape: serde_json::Value = serde_json::from_str(&stdout).unwrap();
    assert_eq!(last_shape["projection"], "common.main/main$last-shape");
    assert_eq!(last_shape["shape"], "direct-answer");

    let db_dir = tempfile::tempdir().unwrap();
    let db_path = db_dir.path().join("memory.db");
    let conn = rusqlite::Connection::open(&db_path).unwrap();
    create_memory_schema(&conn);
    conn.execute(
        "INSERT OR REPLACE INTO memory_metadata (key, value) VALUES ('schema_version', '2.0.0')",
        [],
    )
    .unwrap();
    conn.execute(
        "INSERT INTO episodes (session_id, user_id, episode_type, role, content, keep_flag) VALUES ('s1', 'u1', 'conversation', 'assistant', 'blue deploy note', 1)",
        [],
    )
    .unwrap();
    conn.execute(
        "INSERT INTO semantic_facts (user_id, fact_type, content, confidence) VALUES ('u1', 'preference', 'green release preference', 0.9)",
        [],
    )
    .unwrap();
    conn.execute(
        "INSERT INTO memory_audit (user_id, session_id, agent_id, turn_id, total_turns, entry_id, layer, byte_cost) VALUES ('u1', 's1', 'coact-agent', 1, 1, 'entry-1', 'l2', 42)",
        [],
    )
    .unwrap();
    drop(conn);

    let assert = Command::cargo_bin("by-rs")
        .unwrap()
        .args(["memory", "stats", "--db"])
        .arg(&db_path)
        .args(["--user-id", "u1", "--session-id", "s1"])
        .assert()
        .success()
        .stderr(predicate::str::is_empty());
    let stdout = String::from_utf8(assert.get_output().stdout.clone()).unwrap();
    let stats: serde_json::Value = serde_json::from_str(&stdout).unwrap();
    assert_eq!(stats["projection"], "common.commands/memory$stats");
    assert_eq!(stats["stats"]["l2"]["total"], 1);

    let assert = Command::cargo_bin("by-rs")
        .unwrap()
        .args([
            "memory",
            "keywords",
            "--text",
            "AWS EC2 costs are high, need to optimize EC2 spending",
        ])
        .assert()
        .success()
        .stderr(predicate::str::is_empty());
    let stdout = String::from_utf8(assert.get_output().stdout.clone()).unwrap();
    let keywords: serde_json::Value = serde_json::from_str(&stdout).unwrap();
    assert_eq!(keywords["projection"], "common.commands/memory$keywords");
    assert!(keywords["keywords"]
        .as_array()
        .unwrap()
        .iter()
        .any(|keyword| keyword == "ec2"));
}

#[test]
fn aws_config_and_task_read_only_probes_include_oracle_projection() {
    let aws_home = tempfile::tempdir().unwrap();
    let aws_dir = aws_home.path().join(".aws");
    std::fs::create_dir_all(&aws_dir).unwrap();
    std::fs::write(
        aws_dir.join("credentials"),
        "[default]\naws_access_key_id = AKIADEFAULT\naws_secret_access_key = SECRETDEFAULT\n[dev]\naws_access_key_id = AKIADEV\naws_secret_access_key = SECRETDEV\n",
    )
    .unwrap();
    std::fs::write(
        aws_dir.join("config"),
        "[default]\nregion = us-east-1\n[profile dev]\nregion = ap-northeast-2\noutput = json\n",
    )
    .unwrap();

    let assert = Command::cargo_bin("by-rs")
        .unwrap()
        .args(["aws", "list-profiles", "--aws-dir"])
        .arg(&aws_dir)
        .assert()
        .success();
    let list: serde_json::Value = serde_json::from_slice(&assert.get_output().stdout).unwrap();
    assert_eq!(list["projection"], "common.aws-commands/aws$list-profiles");
    assert_eq!(list["result"]["total"], 2);

    let assert = Command::cargo_bin("by-rs")
        .unwrap()
        .args(["aws", "get-profile", "--profile-name", "dev", "--aws-dir"])
        .arg(&aws_dir)
        .assert()
        .success();
    let profile: serde_json::Value = serde_json::from_slice(&assert.get_output().stdout).unwrap();
    assert_eq!(profile["projection"], "common.aws-commands/aws$get-profile");
    assert_eq!(profile["result"]["name"], "dev");

    let assert = Command::cargo_bin("by-rs")
        .unwrap()
        .args([
            "config",
            "slug",
            "--reason",
            "Need Bedrock parity now",
            "--max-chars",
            "20",
        ])
        .assert()
        .success();
    let slug: serde_json::Value = serde_json::from_slice(&assert.get_output().stdout).unwrap();
    assert_eq!(slug["projection"], "common.config/config$slug");
    assert_eq!(slug["slug"], "need-bedrock-parity-");

    let project = tempfile::tempdir().unwrap();
    let home = tempfile::tempdir().unwrap();
    let assert = Command::cargo_bin("by-rs")
        .unwrap()
        .env("HOME", home.path())
        .env("BY_NO_DOTENV", "1")
        .args(["config", "diff", "--scope", "project", "--project-dir"])
        .arg(project.path())
        .args(["--proposed-edn", "{:llm {:default-provider :bedrock}}"])
        .assert()
        .success();
    let diff: serde_json::Value = serde_json::from_slice(&assert.get_output().stdout).unwrap();
    assert_eq!(diff["projection"], "common.config/config$diff");
    assert_eq!(
        diff["structural"]["adds"]["llm"]["default-provider"],
        "bedrock"
    );

    let assert = Command::cargo_bin("by-rs")
        .unwrap()
        .args([
            "config",
            "frontmatter",
            "--slug",
            "cfg-demo",
            "--session-id",
            "s1",
            "--config-path",
            "/tmp/config.edn",
            "--snapshot",
            "/tmp/snap.edn",
            "--writes",
            "1",
            "--reverts",
            "0",
            "--started",
            "2026-01-01T00:00:00Z",
            "--ended",
            "2026-01-01T00:01:00Z",
            "--next-step",
            "done",
        ])
        .assert()
        .success();
    let frontmatter: serde_json::Value =
        serde_json::from_slice(&assert.get_output().stdout).unwrap();
    assert_eq!(
        frontmatter["projection"],
        "common.config/config$frontmatter"
    );
    assert!(frontmatter["frontmatter"]
        .as_str()
        .unwrap()
        .contains("agent: config-agent"));

    let tasks_dir = tempfile::tempdir().unwrap();
    let root = tasks_dir.path().join("tasks");
    let task_dir = root.join("task-1");
    std::fs::create_dir_all(&task_dir).unwrap();
    std::fs::write(
        task_dir.join("meta.edn"),
        r#"{:id :task-1
            :name "build fixture"
            :job-type :bash
            :status :completed
            :created-at 100
            :started-at 120
            :completed-at 150
            :result {:exit-code 0}}"#,
    )
    .unwrap();
    std::fs::write(task_dir.join("output.log"), "first\nsecond\nthird\n").unwrap();

    let assert = Command::cargo_bin("by-rs")
        .unwrap()
        .args(["tasks", "list", "--root"])
        .arg(&root)
        .assert()
        .success();
    let tasks: serde_json::Value = serde_json::from_slice(&assert.get_output().stdout).unwrap();
    assert_eq!(tasks["projection"], "task.commands/task$list");
    assert_eq!(tasks["total"], 1);

    let assert = Command::cargo_bin("by-rs")
        .unwrap()
        .args(["tasks", "detail", "--root"])
        .arg(&root)
        .args(["task-1", "--last-n", "2"])
        .assert()
        .success();
    let detail: serde_json::Value = serde_json::from_slice(&assert.get_output().stdout).unwrap();
    assert_eq!(detail["projection"], "task.commands/task$detail");
    assert_eq!(detail["id"], "task-1");

    let assert = Command::cargo_bin("by-rs")
        .unwrap()
        .args(["tasks", "sweep", "--root"])
        .arg(&root)
        .args(["--retention-count", "0", "--retention-days", "0"])
        .assert()
        .success();
    let sweep: serde_json::Value = serde_json::from_slice(&assert.get_output().stdout).unwrap();
    assert_eq!(sweep["projection"], "task.commands/task$sweep");
    assert_eq!(sweep["results"][0]["candidates"][0]["id"], "task-1");
}

#[test]
fn rlm_read_only_helpers_include_oracle_projection() {
    let assert = Command::cargo_bin("by-rs")
        .unwrap()
        .args([
            "rlm",
            "chunk-text",
            "--text",
            "abcdef",
            "--size",
            "4",
            "--overlap",
            "1",
        ])
        .assert()
        .success();
    let chunk_text: serde_json::Value =
        serde_json::from_slice(&assert.get_output().stdout).unwrap();
    assert_eq!(chunk_text["projection"], "common.rlm/rlm$chunk-text");
    assert_eq!(chunk_text["n-chunks"], 2);

    let dir = tempfile::tempdir().unwrap();
    let one = dir.path().join("one.txt");
    let two = dir.path().join("two.txt");
    std::fs::write(&one, "alpha").unwrap();
    std::fs::write(&two, "beta").unwrap();
    let assert = Command::cargo_bin("by-rs")
        .unwrap()
        .args(["rlm", "chunk-files", "--path"])
        .arg(&one)
        .args(["--path"])
        .arg(&two)
        .args(["--group-size", "1", "--max-bytes", "100"])
        .assert()
        .success();
    let chunk_files: serde_json::Value =
        serde_json::from_slice(&assert.get_output().stdout).unwrap();
    assert_eq!(chunk_files["projection"], "common.rlm/rlm$chunk-files");
    assert_eq!(chunk_files["n-chunks"], 2);

    let assert = Command::cargo_bin("by-rs")
        .unwrap()
        .args([
            "rlm",
            "parse-map-results",
            "--result",
            "{:category :bug}",
            "--result",
            "{\"category\":\"feature\"}",
            "--shape",
            "edn",
        ])
        .assert()
        .success();
    let parsed: serde_json::Value = serde_json::from_slice(&assert.get_output().stdout).unwrap();
    assert_eq!(parsed["projection"], "common.rlm/rlm$parse-map-results");
    assert_eq!(parsed["n-parsed"], 2);

    let assert = Command::cargo_bin("by-rs")
        .unwrap()
        .args([
            "rlm",
            "reduce-counts",
            "--parsed-results",
            "[{:category :bug} {:category :bug} {:category :feature}]",
            "--key",
            "category",
        ])
        .assert()
        .success();
    let reduced: serde_json::Value = serde_json::from_slice(&assert.get_output().stdout).unwrap();
    assert_eq!(reduced["projection"], "common.rlm/rlm$reduce-counts");
    assert_eq!(reduced["total"], 3);

    let assert = Command::cargo_bin("by-rs")
        .unwrap()
        .args([
            "rlm",
            "conservative-verdict",
            "--parsed-results",
            "[{:malicious? false} {:parse-failed true :raw \"x\"} {:malicious? true}]",
            "--positive-key",
            "malicious?",
        ])
        .assert()
        .success();
    let verdict: serde_json::Value = serde_json::from_slice(&assert.get_output().stdout).unwrap();
    assert_eq!(verdict["projection"], "common.rlm/rlm$conservative-verdict");
    assert_eq!(verdict["verdict"], true);
}
