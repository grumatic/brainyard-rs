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

        let stderr = fs::read_to_string(out_dir.join("top_help.stderr.txt"))
            .expect("read top_help stderr snapshot");
        assert!(
            !stderr.contains("No such file or directory"),
            "{script} failed to resolve relative --bin path: {stderr:?}"
        );
    }
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
        "#!/usr/bin/env bash\nprintf 'fake'\nfor arg in \"$@\"; do printf ' %s' \"$arg\"; done\nprintf '\\nBY_NO_DOTENV=%s\\n' \"${BY_NO_DOTENV:-}\"\nprintf 'BY_RS_ORACLE_USER_HOME=%s\\n' \"${BY_RS_ORACLE_USER_HOME:-}\"\nprintf 'HOME=%s\\n' \"${HOME:-}\"\n",
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
