#![forbid(unsafe_code)]

use anyhow::{Context, Result};
use by_contracts::{parse_map, EdnMap};
use std::path::{Path, PathBuf};

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SessionSummary {
    pub id: String,
    pub label: Option<String>,
    pub created_at: Option<String>,
    pub started_at: Option<String>,
    pub last_active: Option<String>,
    pub path: PathBuf,
}

pub fn list_sessions(root: impl AsRef<Path>) -> Result<Vec<SessionSummary>> {
    let root = root.as_ref();
    if !root.exists() {
        return Ok(Vec::new());
    }

    let mut entries = Vec::new();
    for entry in std::fs::read_dir(root)
        .with_context(|| format!("failed to read session root {}", root.display()))?
    {
        let entry = entry?;
        let file_type = entry.file_type()?;
        if !file_type.is_dir() {
            continue;
        }
        let dir_name = entry.file_name().to_string_lossy().to_string();
        if dir_name.starts_with('.') {
            continue;
        }
        entries.push((dir_name, entry.path()));
    }
    entries.sort_by(|left, right| left.0.cmp(&right.0));

    entries
        .into_iter()
        .map(|(dir_id, path)| load_session_summary(dir_id, path))
        .collect()
}

fn load_session_summary(dir_id: String, path: PathBuf) -> Result<SessionSummary> {
    let meta_path = path.join("meta.edn");
    let meta = std::fs::read_to_string(&meta_path)
        .ok()
        .and_then(|raw| parse_map(&raw).ok());

    Ok(match meta.as_ref() {
        Some(meta) => summary_from_meta(dir_id, path, meta),
        None => SessionSummary {
            id: dir_id,
            label: None,
            created_at: None,
            started_at: None,
            last_active: None,
            path,
        },
    })
}

fn summary_from_meta(dir_id: String, path: PathBuf, meta: &EdnMap) -> SessionSummary {
    SessionSummary {
        id: meta.string("id").unwrap_or(&dir_id).to_string(),
        label: meta.string("label").map(ToOwned::to_owned),
        created_at: meta.instant("created-at").map(ToOwned::to_owned),
        started_at: meta.instant("started-at").map(ToOwned::to_owned),
        last_active: meta.instant("last-active").map(ToOwned::to_owned),
        path,
    }
}
