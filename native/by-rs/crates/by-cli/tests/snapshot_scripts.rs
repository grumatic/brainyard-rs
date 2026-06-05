use std::{
    fs,
    path::{Path, PathBuf},
    process::Command,
};

#[test]
fn capture_scripts_accept_relative_bin_paths() {
    let native_root = native_root();
    let workspace = tempfile::Builder::new()
        .prefix("snapshot-scripts-")
        .tempdir_in(native_root.join("target"))
        .expect("snapshot script tempdir");
    let fake_bin = workspace.path().join("bin/fake-by");
    write_fake_binary(&fake_bin);
    let relative_bin = fake_bin
        .strip_prefix(&native_root)
        .expect("fake binary under native root");

    for script in [
        "capture-clojure-cli-snapshots.sh",
        "capture-rust-cli-snapshots.sh",
    ] {
        let stem = script.trim_end_matches(".sh");
        let out_dir = workspace.path().join(stem).join("out");
        let home_dir = workspace.path().join(stem).join("home");
        let output = Command::new(native_root.join("scripts").join(script))
            .current_dir(&native_root)
            .arg("--bin")
            .arg(relative_bin)
            .arg("--out")
            .arg(&out_dir)
            .arg("--home")
            .arg(&home_dir)
            .output()
            .expect("run snapshot capture script");
        assert!(
            output.status.success(),
            "{script} failed: stdout={} stderr={}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );

        let top_help = fs::read_to_string(out_dir.join("top_help.stdout.txt"))
            .expect("read top_help stdout snapshot");
        assert!(
            top_help.contains("fake --help"),
            "{script} did not execute the relative --bin path: {top_help:?}"
        );
        assert!(
            top_help.contains("BY_NO_DOTENV=1"),
            "{script} should disable host dotenv loading: {top_help:?}"
        );
        if script == "capture-clojure-cli-snapshots.sh" {
            assert!(
                top_help.contains(&format!("BY_RS_ORACLE_USER_HOME={}", home_dir.display())),
                "{script} should force the Clojure oracle Java user.home to the isolated HOME: {top_help:?}"
            );
        }

        let manifest =
            fs::read_to_string(out_dir.join("manifest.json")).expect("read snapshot manifest");
        for inline_case in ["run_inline_quit", "run_inline_help", "run_inline_config"] {
            assert!(
                manifest.contains(inline_case),
                "{script} manifest should include {inline_case}: {manifest:?}"
            );
            let inline_stdout =
                fs::read_to_string(out_dir.join(format!("{inline_case}.stdout.txt")))
                    .expect("read inline run stdout snapshot");
            assert!(
                inline_stdout.contains("fake run --inline"),
                "{script} should execute {inline_case} with run --inline: {inline_stdout:?}"
            );
        }
        let sessions_list_pos = manifest
            .find("sessions_list")
            .expect("manifest should include sessions_list");
        let run_inline_pos = manifest
            .find("run_inline_quit")
            .expect("manifest should include run_inline_quit");
        assert!(
            sessions_list_pos < run_inline_pos,
            "{script} should capture sessions_list before inline run cases to keep list fixtures stable: {manifest:?}"
        );
        let quit_stdout = fs::read_to_string(out_dir.join("run_inline_quit.stdout.txt"))
            .expect("read run_inline_quit stdout snapshot");
        assert!(
            quit_stdout.contains("BRAINYARD_SESSION_ID=agt-snapshot-run-inline-quit"),
            "{script} should make run_inline_quit session id deterministic: {quit_stdout:?}"
        );
        assert!(
            quit_stdout.contains(
                "STDIN-BEGIN
/quit
STDIN-END"
            ),
            "{script} should feed /quit to run_inline_quit stdin: {quit_stdout:?}"
        );
        let help_stdout = fs::read_to_string(out_dir.join("run_inline_help.stdout.txt"))
            .expect("read run_inline_help stdout snapshot");
        assert!(
            help_stdout.contains(
                "STDIN-BEGIN
/help
/quit
STDIN-END"
            ),
            "{script} should feed /help then /quit to run_inline_help stdin: {help_stdout:?}"
        );
        let config_stdout = fs::read_to_string(out_dir.join("run_inline_config.stdout.txt"))
            .expect("read run_inline_config stdout snapshot");
        assert!(
            config_stdout.contains(
                "STDIN-BEGIN
/config
/quit
STDIN-END"
            ),
            "{script} should feed /config then /quit to run_inline_config stdin: {config_stdout:?}"
        );

        let stderr = fs::read_to_string(out_dir.join("top_help.stderr.txt"))
            .expect("read top_help stderr snapshot");
        assert!(
            !stderr.contains("No such file or directory"),
            "{script} failed to resolve relative --bin path: {stderr:?}"
        );
    }
}

#[test]
fn compare_script_normalizes_manifest_roots() {
    let native_root = native_root();
    let workspace = tempfile::Builder::new()
        .prefix("snapshot-compare-")
        .tempdir_in(native_root.join("target"))
        .expect("snapshot compare tempdir");
    let clj_root = workspace.path().join("clj");
    let rust_root = workspace.path().join("rust");
    let clj_home = workspace.path().join("clj-home");
    let rust_home = workspace.path().join("rust-home");
    let clj_workdir = workspace.path().join("clj-workdir");
    let rust_workdir = workspace.path().join("rust-workdir");

    write_minimal_snapshot(
        &clj_root,
        "clojure",
        &clj_home,
        &clj_workdir,
        &format!(
            "dirs {{:user-dir \"{}\", :project-dir \"{}/project\", :working-dir \"{}\"}}\n",
            clj_home.display(),
            clj_home.display(),
            clj_workdir.display()
        ),
    );
    write_minimal_snapshot(
        &rust_root,
        "rust",
        &rust_home,
        &rust_workdir,
        &format!(
            "dirs {{:user-dir \"{}\", :project-dir \"{}/project\", :working-dir \"{}\"}}\n",
            rust_home.display(),
            rust_home.display(),
            rust_workdir.display()
        ),
    );

    let output = Command::new("python3")
        .arg(native_root.join("scripts/compare-cli-snapshots.py"))
        .arg("--clojure")
        .arg(&clj_root)
        .arg("--rust")
        .arg(&rust_root)
        .arg("--strict")
        .output()
        .expect("run snapshot compare script");

    assert!(
        output.status.success(),
        "compare script should normalize manifest roots: stdout={} stderr={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn clj_oracle_runs_without_user_home_override() {
    if Command::new("clojure").arg("-Sdescribe").output().is_err() {
        eprintln!("skipping by-clj-oracle wrapper test because clojure is unavailable");
        return;
    }

    let native_root = native_root();
    let home = tempfile::Builder::new()
        .prefix("by-clj-oracle-home-")
        .tempdir_in(native_root.join("target"))
        .expect("oracle HOME tempdir");
    let output = Command::new("bash")
        .current_dir(&native_root)
        .arg("-lc")
        .arg("unset TMUX; exec \"$BY_CLJ_ORACLE\" --help")
        .env("BY_CLJ_ORACLE", native_root.join("scripts/by-clj-oracle"))
        .env("HOME", home.path())
        .env_remove("BY_RS_ORACLE_USER_HOME")
        .output()
        .expect("run by-clj-oracle wrapper through bash -lc");

    assert!(
        output.status.success(),
        "by-clj-oracle failed without BY_RS_ORACLE_USER_HOME: stdout={} stderr={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

fn write_minimal_snapshot(
    root: &Path,
    source: &str,
    isolated_home: &Path,
    working_directory: &Path,
    stdout: &str,
) {
    fs::create_dir_all(root).expect("create minimal snapshot root");
    fs::write(root.join("case.stdout.txt"), stdout).expect("write minimal stdout");
    fs::write(root.join("case.stderr.txt"), "").expect("write minimal stderr");
    fs::write(root.join("case.exitcode"), "0\n").expect("write minimal exitcode");
    fs::write(
        root.join("manifest.json"),
        format!(
            r#"{{
  "schemaVersion": 2,
  "source": "{source}",
  "binary": "fake",
  "workingDirectory": "{}",
  "isolatedHome": "{}",
  "cases": [
    {{"name": "case", "command": "run --inline", "stdout": "case.stdout.txt", "stderr": "case.stderr.txt", "exitCode": 0}}
  ]
}}
"#,
            working_directory.display(),
            isolated_home.display()
        ),
    )
    .expect("write minimal manifest");
}

fn native_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(2)
        .expect("native root ancestor")
        .to_path_buf()
}

#[cfg(unix)]
fn write_fake_binary(path: &Path) {
    use std::os::unix::fs::PermissionsExt;

    fs::create_dir_all(path.parent().expect("fake binary parent")).expect("create fake bin dir");
    fs::write(
        path,
        r#"#!/usr/bin/env bash
printf 'fake'
for arg in "$@"; do printf ' %s' "$arg"; done
printf '
BY_NO_DOTENV=%s
' "${BY_NO_DOTENV:-}"
printf 'BY_RS_ORACLE_USER_HOME=%s
' "${BY_RS_ORACLE_USER_HOME:-}"
printf 'HOME=%s
' "${HOME:-}"
printf 'BRAINYARD_SESSION_ID=%s
' "${BRAINYARD_SESSION_ID:-}"
printf 'STDIN-BEGIN
'
cat
printf 'STDIN-END
'
"#,
    )
    .expect("write fake snapshot binary");

    let mut perms = fs::metadata(path)
        .expect("fake snapshot binary metadata")
        .permissions();
    perms.set_mode(0o755);
    fs::set_permissions(path, perms).expect("chmod fake snapshot binary");
}

#[cfg(not(unix))]
fn write_fake_binary(_path: &Path) {
    panic!("snapshot script tests require a Unix shell");
}
