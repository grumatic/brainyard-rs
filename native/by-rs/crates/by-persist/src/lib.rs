#![forbid(unsafe_code)]

use anyhow::{Context, Result};
use by_contracts::{parse_map, EdnMap, EdnValue};
use std::path::{Path, PathBuf};

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SessionSummary {
    pub id: String,
    pub label: Option<String>,
    pub agent: Option<String>,
    pub bytes: u64,
    pub created_at: Option<String>,
    pub started_at: Option<String>,
    pub started_at_millis: Option<i64>,
    pub last_active: Option<String>,
    pub last_attached_at_millis: Option<i64>,
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

pub fn delete_session_dir(root: impl AsRef<Path>, session_id: &str) -> Result<bool> {
    let root = root.as_ref();
    let Some(target_name) = safe_session_dir_name(session_id) else {
        anyhow::bail!("invalid session id for deletion: {session_id}");
    };
    let target = root.join(target_name);
    if !target.exists() {
        return Ok(false);
    }

    let metadata = std::fs::symlink_metadata(&target)
        .with_context(|| format!("failed to stat session path {}", target.display()))?;
    if !metadata.is_dir() {
        return Ok(false);
    }

    std::fs::remove_dir_all(&target)
        .with_context(|| format!("failed to delete session path {}", target.display()))?;
    Ok(!target.exists())
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
            agent: None,
            bytes: dir_size(&path)?,
            created_at: None,
            started_at: None,
            started_at_millis: None,
            last_active: None,
            last_attached_at_millis: None,
            path,
        },
    })
}

fn safe_session_dir_name(session_id: &str) -> Option<&str> {
    if session_id.is_empty() {
        return None;
    }

    let mut components = Path::new(session_id).components();
    match (components.next(), components.next()) {
        (Some(std::path::Component::Normal(_)), None) => Some(session_id),
        _ => None,
    }
}

fn summary_from_meta(dir_id: String, path: PathBuf, meta: &EdnMap) -> SessionSummary {
    SessionSummary {
        id: meta.string("id").unwrap_or(&dir_id).to_string(),
        label: meta.string("label").map(ToOwned::to_owned),
        agent: text_value(meta.get("defagent-id")).or_else(|| text_value(meta.get("agent-id"))),
        bytes: dir_size(&path).unwrap_or(0),
        created_at: meta.instant("created-at").map(ToOwned::to_owned),
        started_at: meta
            .instant("started-at")
            .map(ToOwned::to_owned)
            .or_else(|| meta.i64("started-at").map(|value| value.to_string())),
        started_at_millis: meta.i64("started-at"),
        last_active: meta.instant("last-active").map(ToOwned::to_owned),
        last_attached_at_millis: meta.i64("last-attached-at"),
        path,
    }
}

fn text_value(value: Option<&EdnValue>) -> Option<String> {
    match value {
        Some(EdnValue::String(value))
        | Some(EdnValue::Keyword(value))
        | Some(EdnValue::Symbol(value)) => Some(value.clone()),
        _ => None,
    }
}

fn dir_size(path: &Path) -> Result<u64> {
    let mut total = 0_u64;
    accumulate_file_size(path, &mut total)?;
    Ok(total)
}

fn accumulate_file_size(path: &Path, total: &mut u64) -> Result<()> {
    let metadata = std::fs::symlink_metadata(path)
        .with_context(|| format!("failed to stat session path {}", path.display()))?;

    if metadata.is_file() {
        *total = total.saturating_add(metadata.len());
        return Ok(());
    }

    if !metadata.is_dir() {
        return Ok(());
    }

    for entry in std::fs::read_dir(path)
        .with_context(|| format!("failed to read session path {}", path.display()))?
    {
        let entry = entry?;
        accumulate_file_size(&entry.path(), total)?;
    }

    Ok(())
}
