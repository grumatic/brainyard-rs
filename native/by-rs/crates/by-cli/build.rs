#![forbid(unsafe_code)]

use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

fn main() {
    let repo_root = repo_root();
    emit_git_rerun_hints(&repo_root);
    println!("cargo:rerun-if-env-changed=BY_BUILD_VERSION_OVERRIDE");

    let version = env::var("BY_BUILD_VERSION_OVERRIDE")
        .ok()
        .filter(|value| !value.trim().is_empty())
        .or_else(|| git_describe(&repo_root))
        .unwrap_or_else(|| env::var("CARGO_PKG_VERSION").unwrap_or_else(|_| "dev".to_string()));

    println!("cargo:rustc-env=BY_BUILD_VERSION={version}");
}

fn repo_root() -> PathBuf {
    let manifest_dir = PathBuf::from(env::var("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR"));
    manifest_dir
        .join("../../../..")
        .canonicalize()
        .unwrap_or(manifest_dir)
}

fn emit_git_rerun_hints(repo_root: &Path) {
    let git_dir = git_dir(repo_root);
    for path in ["HEAD", "index", "packed-refs"] {
        let path = git_dir.join(path);
        if path.exists() {
            println!("cargo:rerun-if-changed={}", path.display());
        }
    }

    if let Ok(head) = fs::read_to_string(git_dir.join("HEAD")) {
        if let Some(ref_name) = head.trim().strip_prefix("ref: ") {
            let path = git_dir.join(ref_name.trim());
            if path.exists() {
                println!("cargo:rerun-if-changed={}", path.display());
            }
        }
    }
}

fn git_dir(repo_root: &Path) -> PathBuf {
    let dot_git = repo_root.join(".git");
    if dot_git.is_dir() {
        return dot_git;
    }

    if let Ok(raw) = fs::read_to_string(&dot_git) {
        if let Some(path) = raw.trim().strip_prefix("gitdir:") {
            let path = PathBuf::from(path.trim());
            return if path.is_absolute() {
                path
            } else {
                repo_root.join(path)
            };
        }
    }

    dot_git
}

fn git_describe(repo_root: &Path) -> Option<String> {
    let output = Command::new("git")
        .arg("-C")
        .arg(repo_root)
        .args(["describe", "--tags", "--always", "--dirty"])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let version = String::from_utf8(output.stdout).ok()?;
    let version = version.trim();
    (!version.is_empty()).then(|| version.to_string())
}
