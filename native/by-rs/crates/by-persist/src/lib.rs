#![forbid(unsafe_code)]

use anyhow::{Context, Result};
use by_contracts::{parse_map, parse_value, EdnMap, EdnValue};
use std::collections::BTreeMap;
use std::fs::OpenOptions;
use std::io::{BufRead, ErrorKind, Write};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

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

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SessionReadWarning {
    pub session_id: String,
    pub path: PathBuf,
    pub message: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SessionList {
    pub sessions: Vec<SessionSummary>,
    pub warnings: Vec<SessionReadWarning>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SessionMessage {
    pub role: String,
    pub content: String,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct SessionMetaUpdate {
    pub user_id: Option<String>,
    pub label: Option<String>,
    pub agent_id: Option<String>,
    pub defagent_id: Option<String>,
    pub started_at_millis: Option<i64>,
    pub last_attached_at_millis: Option<i64>,
    pub working_dir: Option<String>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SessionSnapshotKind {
    Meta,
    Session,
    Layout,
    PendingDialogs,
    Permissions,
    Queue,
    Todo,
    Status,
    InputHistory,
    UsageTracker,
}

impl SessionSnapshotKind {
    pub fn file_name(self) -> &'static str {
        match self {
            Self::Meta => "meta.edn",
            Self::Session => "session.edn",
            Self::Layout => "layout.edn",
            Self::PendingDialogs => "pending-dialogs.edn",
            Self::Permissions => "permissions.edn",
            Self::Queue => "queue.edn",
            Self::Todo => "todo.edn",
            Self::Status => "status.edn",
            Self::InputHistory => "input-history.edn",
            Self::UsageTracker => "usage-tracker.edn",
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct RestoredSession {
    pub id: String,
    pub user_id: Option<String>,
    pub agent: Option<String>,
    pub total_turns: Option<i64>,
    pub agent_activity_seq: Option<i64>,
    pub messages: Vec<SessionMessage>,
    pub usage_tracker: Option<EdnValue>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SessionLock {
    path: PathBuf,
    pid: u32,
}

impl SessionLock {
    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn pid(&self) -> u32 {
        self.pid
    }
}

pub fn list_sessions(root: impl AsRef<Path>) -> Result<Vec<SessionSummary>> {
    Ok(list_sessions_with_warnings(root)?.sessions)
}

pub fn list_sessions_with_warnings(root: impl AsRef<Path>) -> Result<SessionList> {
    let root = root.as_ref();
    if !root.exists() {
        return Ok(SessionList {
            sessions: Vec::new(),
            warnings: Vec::new(),
        });
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

    let mut sessions = Vec::new();
    let mut warnings = Vec::new();
    for (dir_id, path) in entries {
        let (summary, warning) = load_session_summary(dir_id, path)?;
        sessions.push(summary);
        if let Some(warning) = warning {
            warnings.push(warning);
        }
    }

    Ok(SessionList { sessions, warnings })
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

pub fn try_acquire_session_lock(
    root: impl AsRef<Path>,
    session_id: &str,
) -> Result<Option<SessionLock>> {
    let root = root.as_ref();
    let Some(target_name) = safe_session_dir_name(session_id) else {
        anyhow::bail!("invalid session id for lock acquire: {session_id}");
    };

    let target = root.join(target_name);
    std::fs::create_dir_all(&target)
        .with_context(|| format!("failed to create session path {}", target.display()))?;
    let lock_path = target.join("by-host.lock");
    let pid = std::process::id();

    loop {
        match OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&lock_path)
        {
            Ok(mut file) => {
                writeln!(file, "{pid}")
                    .with_context(|| format!("failed to write lock {}", lock_path.display()))?;
                return Ok(Some(SessionLock {
                    path: lock_path,
                    pid,
                }));
            }
            Err(error) if error.kind() == ErrorKind::AlreadyExists => {
                let owner = read_lock_pid(&lock_path)?;
                if owner == Some(pid) || owner.is_some_and(process_alive) {
                    return Ok(None);
                }
                match std::fs::remove_file(&lock_path) {
                    Ok(()) => continue,
                    Err(error) if error.kind() == ErrorKind::NotFound => continue,
                    Err(error) => {
                        return Err(error).with_context(|| {
                            format!("failed to remove stale lock {}", lock_path.display())
                        });
                    }
                }
            }
            Err(error) => {
                return Err(error)
                    .with_context(|| format!("failed to create lock {}", lock_path.display()));
            }
        }
    }
}

pub fn release_session_lock(lock: SessionLock) -> Result<()> {
    match read_lock_pid(&lock.path) {
        Ok(Some(pid)) if pid != lock.pid => return Ok(()),
        Ok(_) => {}
        Err(error)
            if error
                .downcast_ref::<std::io::Error>()
                .is_some_and(|io_error| io_error.kind() == ErrorKind::NotFound) =>
        {
            return Ok(())
        }
        Err(error) => return Err(error),
    }

    match std::fs::remove_file(&lock.path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == ErrorKind::NotFound => Ok(()),
        Err(error) => {
            Err(error).with_context(|| format!("failed to remove lock {}", lock.path.display()))
        }
    }
}

pub fn save_session_meta(
    root: impl AsRef<Path>,
    session_id: &str,
    update: &SessionMetaUpdate,
) -> Result<PathBuf> {
    let root = root.as_ref();
    let Some(target_name) = safe_session_dir_name(session_id) else {
        anyhow::bail!("invalid session id for meta write: {session_id}");
    };
    let target = root.join(target_name);
    std::fs::create_dir_all(&target)
        .with_context(|| format!("failed to create session path {}", target.display()))?;

    let meta_path = target.join("meta.edn");
    let mut entries = read_meta_entries(&meta_path)?;
    insert_string(&mut entries, "user-id", update.user_id.as_deref());
    insert_string(&mut entries, "label", update.label.as_deref());
    insert_keyword(&mut entries, "agent-id", update.agent_id.as_deref());
    insert_keyword(&mut entries, "defagent-id", update.defagent_id.as_deref());
    insert_string(&mut entries, "working-dir", update.working_dir.as_deref());
    insert_i64(
        &mut entries,
        "last-attached-at",
        update.last_attached_at_millis,
    );

    if let Some(started_at) = update.started_at_millis {
        entries.insert("started-at".to_string(), EdnValue::Integer(started_at));
    }
    if !entries.contains_key("started-at") {
        entries.insert(
            "started-at".to_string(),
            EdnValue::Integer(current_epoch_millis().unwrap_or(0)),
        );
    }

    let raw = format_meta_entries(&entries);
    let temp_path = meta_path.with_extension(format!("edn.tmp-{}", std::process::id()));
    std::fs::write(&temp_path, raw)
        .with_context(|| format!("failed to write session meta {}", temp_path.display()))?;
    std::fs::rename(&temp_path, &meta_path).with_context(|| {
        format!(
            "failed to replace session meta {} from {}",
            meta_path.display(),
            temp_path.display()
        )
    })?;

    Ok(meta_path)
}

pub fn append_session_message(
    root: impl AsRef<Path>,
    session_id: &str,
    role: &str,
    content: &str,
) -> Result<PathBuf> {
    let root = root.as_ref();
    let Some(target_name) = safe_session_dir_name(session_id) else {
        anyhow::bail!("invalid session id for message append: {session_id}");
    };
    let target = root.join(target_name);
    std::fs::create_dir_all(&target)
        .with_context(|| format!("failed to create session path {}", target.display()))?;

    let log_path = target.join("messages.log");
    let millis = current_epoch_millis().unwrap_or(0);
    let line = format!(
        "{{:t {millis} :kind :message :payload {{:role \"{}\" :content \"{}\"}}}}\n",
        escape_edn_string(role),
        escape_edn_string(content)
    );
    let mut file = OpenOptions::new()
        .create(true)
        .append(true)
        .open(&log_path)
        .with_context(|| format!("failed to open session messages {}", log_path.display()))?;
    file.write_all(line.as_bytes())
        .with_context(|| format!("failed to append session message {}", log_path.display()))?;

    Ok(log_path)
}

pub fn read_session_messages(
    root: impl AsRef<Path>,
    session_id: &str,
) -> Result<Vec<SessionMessage>> {
    let root = root.as_ref();
    let Some(target_name) = safe_session_dir_name(session_id) else {
        anyhow::bail!("invalid session id for message read: {session_id}");
    };
    let log_path = root.join(target_name).join("messages.log");
    let file = match std::fs::File::open(&log_path) {
        Ok(file) => file,
        Err(error) if error.kind() == ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => {
            return Err(error).with_context(|| {
                format!("failed to read session messages {}", log_path.display())
            });
        }
    };

    let mut messages = Vec::new();
    for line in std::io::BufReader::new(file).lines() {
        let line = line.with_context(|| {
            format!(
                "failed to read session messages line from {}",
                log_path.display()
            )
        })?;
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        let Ok(event) = parse_map(trimmed) else {
            continue;
        };
        if !matches!(event.get("kind"), Some(EdnValue::Keyword(kind)) if kind == "message") {
            continue;
        }
        let Some(EdnValue::Map(payload)) = event.get("payload") else {
            continue;
        };
        let (Some(role), Some(content)) = (payload.string("role"), payload.string("content"))
        else {
            continue;
        };
        messages.push(SessionMessage {
            role: role.to_string(),
            content: content.to_string(),
        });
    }

    Ok(messages)
}

pub fn read_session_snapshot(
    root: impl AsRef<Path>,
    session_id: &str,
    kind: SessionSnapshotKind,
) -> Result<Option<EdnValue>> {
    let root = root.as_ref();
    let Some(target_name) = safe_session_dir_name(session_id) else {
        anyhow::bail!("invalid session id for snapshot read: {session_id}");
    };
    let snapshot_path = root.join(target_name).join(kind.file_name());
    let raw = match std::fs::read_to_string(&snapshot_path) {
        Ok(raw) => raw,
        Err(error) if error.kind() == ErrorKind::NotFound => return Ok(None),
        Err(error) => {
            return Err(error).with_context(|| {
                format!(
                    "failed to read session snapshot {}",
                    snapshot_path.display()
                )
            });
        }
    };
    if raw.trim().is_empty() {
        return Ok(None);
    }

    parse_value(&raw)
        .with_context(|| {
            format!(
                "failed to parse session snapshot {}",
                snapshot_path.display()
            )
        })
        .map(Some)
}

pub fn write_session_snapshot(
    root: impl AsRef<Path>,
    session_id: &str,
    kind: SessionSnapshotKind,
    value: &EdnValue,
) -> Result<PathBuf> {
    let root = root.as_ref();
    let Some(target_name) = safe_session_dir_name(session_id) else {
        anyhow::bail!("invalid session id for snapshot write: {session_id}");
    };
    let target = root.join(target_name);
    std::fs::create_dir_all(&target)
        .with_context(|| format!("failed to create session path {}", target.display()))?;

    let snapshot_path = target.join(kind.file_name());
    let temp_path = snapshot_path.with_extension(format!("edn.tmp-{}", std::process::id()));
    std::fs::write(&temp_path, format!("{}\n", format_edn_value(value)))
        .with_context(|| format!("failed to write session snapshot {}", temp_path.display()))?;
    std::fs::rename(&temp_path, &snapshot_path).with_context(|| {
        format!(
            "failed to replace session snapshot {} from {}",
            snapshot_path.display(),
            temp_path.display()
        )
    })?;

    Ok(snapshot_path)
}

pub fn restore_session(root: impl AsRef<Path>, session_id: &str) -> Result<RestoredSession> {
    let root = root.as_ref();
    let Some(target_name) = safe_session_dir_name(session_id) else {
        anyhow::bail!("invalid session id for restore: {session_id}");
    };
    let session_dir = root.join(target_name);
    if !session_dir.is_dir() {
        anyhow::bail!("session path does not exist: {}", session_dir.display());
    }

    let meta = EdnMap::new(read_meta_entries(
        &session_dir.join(SessionSnapshotKind::Meta.file_name()),
    )?);
    let session_snapshot = read_session_snapshot(root, target_name, SessionSnapshotKind::Session)?;
    let session_map = match session_snapshot {
        Some(EdnValue::Map(map)) => Some(map),
        Some(_) => {
            anyhow::bail!(
                "session snapshot {} must be an EDN map",
                session_dir
                    .join(SessionSnapshotKind::Session.file_name())
                    .display()
            );
        }
        None => None,
    };
    let usage_tracker =
        read_session_snapshot(root, target_name, SessionSnapshotKind::UsageTracker)?;
    let messages = read_session_messages(root, target_name)?;
    let session_ref = session_map.as_ref();

    Ok(RestoredSession {
        id: target_name.to_string(),
        user_id: session_ref
            .and_then(|session| text_value(session.get("user-id")))
            .or_else(|| text_value(meta.get("user-id"))),
        agent: session_ref
            .and_then(agent_from_map)
            .or_else(|| agent_from_map(&meta)),
        total_turns: session_ref.and_then(|session| session.i64("total-turns")),
        agent_activity_seq: session_ref.and_then(|session| session.i64("agent-activity-seq")),
        messages,
        usage_tracker,
    })
}

fn load_session_summary(
    dir_id: String,
    path: PathBuf,
) -> Result<(SessionSummary, Option<SessionReadWarning>)> {
    let meta_path = path.join("meta.edn");
    let warning_message = match std::fs::read_to_string(&meta_path) {
        Ok(raw) => match parse_map(&raw) {
            Ok(meta) => return Ok((summary_from_meta(dir_id, path, &meta), None)),
            Err(error) => Some(error.to_string()),
        },
        Err(error) if error.kind() == ErrorKind::NotFound => None,
        Err(error) => Some(error.to_string()),
    };

    let warning = warning_message.map(|message| SessionReadWarning {
        session_id: dir_id.clone(),
        path: meta_path,
        message,
    });
    Ok((
        SessionSummary {
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
        warning,
    ))
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
        id: dir_id,
        label: meta.string("label").map(ToOwned::to_owned),
        agent: agent_from_map(meta),
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

fn agent_from_map(map: &EdnMap) -> Option<String> {
    text_value(map.get("defagent-id")).or_else(|| text_value(map.get("agent-id")))
}

fn text_value(value: Option<&EdnValue>) -> Option<String> {
    match value {
        Some(EdnValue::String(value))
        | Some(EdnValue::Keyword(value))
        | Some(EdnValue::Symbol(value)) => Some(value.clone()),
        _ => None,
    }
}

fn read_meta_entries(path: &Path) -> Result<BTreeMap<String, EdnValue>> {
    let raw = match std::fs::read_to_string(path) {
        Ok(raw) => raw,
        Err(error) if error.kind() == ErrorKind::NotFound => return Ok(BTreeMap::new()),
        Err(error) => {
            return Err(error)
                .with_context(|| format!("failed to read session meta {}", path.display()));
        }
    };
    if raw.trim().is_empty() {
        return Ok(BTreeMap::new());
    }

    let meta = parse_map(&raw)
        .with_context(|| format!("failed to parse session meta {}", path.display()))?;
    Ok(meta
        .iter()
        .map(|(key, value)| (key.clone(), value.clone()))
        .collect())
}

fn read_lock_pid(path: &Path) -> Result<Option<u32>> {
    let raw = match std::fs::read_to_string(path) {
        Ok(raw) => raw,
        Err(error) if error.kind() == ErrorKind::NotFound => return Ok(None),
        Err(error) => {
            return Err(error).with_context(|| format!("failed to read lock {}", path.display()));
        }
    };
    Ok(raw.trim().parse::<u32>().ok())
}

fn process_alive(pid: u32) -> bool {
    if pid == 0 {
        return false;
    }
    if pid == std::process::id() {
        return true;
    }
    process_alive_platform(pid)
}

#[cfg(unix)]
fn process_alive_platform(pid: u32) -> bool {
    std::process::Command::new("kill")
        .arg("-0")
        .arg(pid.to_string())
        .status()
        .map(|status| status.success())
        .unwrap_or(true)
}

#[cfg(not(unix))]
fn process_alive_platform(_pid: u32) -> bool {
    true
}

fn insert_string(entries: &mut BTreeMap<String, EdnValue>, key: &str, value: Option<&str>) {
    if let Some(value) = value {
        entries.insert(key.to_string(), EdnValue::String(value.to_string()));
    }
}

fn insert_keyword(entries: &mut BTreeMap<String, EdnValue>, key: &str, value: Option<&str>) {
    if let Some(value) = value {
        entries.insert(key.to_string(), EdnValue::Keyword(value.to_string()));
    }
}

fn insert_i64(entries: &mut BTreeMap<String, EdnValue>, key: &str, value: Option<i64>) {
    if let Some(value) = value {
        entries.insert(key.to_string(), EdnValue::Integer(value));
    }
}

fn format_meta_entries(entries: &BTreeMap<String, EdnValue>) -> String {
    let mut output = String::from("{");
    let mut first = true;
    for key in [
        "user-id",
        "label",
        "agent-id",
        "defagent-id",
        "started-at",
        "last-attached-at",
        "working-dir",
    ] {
        if let Some(value) = entries.get(key) {
            append_edn_entry(&mut output, &mut first, key, value);
        }
    }
    for (key, value) in entries {
        if matches!(
            key.as_str(),
            "user-id"
                | "label"
                | "agent-id"
                | "defagent-id"
                | "started-at"
                | "last-attached-at"
                | "working-dir"
        ) {
            continue;
        }
        append_edn_entry(&mut output, &mut first, key, value);
    }
    if !first {
        output.push('\n');
    }
    output.push_str("}\n");
    output
}

fn append_edn_entry(output: &mut String, first: &mut bool, key: &str, value: &EdnValue) {
    if *first {
        output.push('\n');
        *first = false;
    }
    output.push_str(" :");
    output.push_str(key);
    output.push(' ');
    output.push_str(&format_edn_value(value));
    output.push('\n');
}

fn format_edn_value(value: &EdnValue) -> String {
    match value {
        EdnValue::Nil => "nil".to_string(),
        EdnValue::Bool(value) => value.to_string(),
        EdnValue::Integer(value) => value.to_string(),
        EdnValue::Float(value) => format_edn_float(*value),
        EdnValue::String(value) => format!("\"{}\"", escape_edn_string(value)),
        EdnValue::Keyword(value) => format!(":{value}"),
        EdnValue::Symbol(value) => value.clone(),
        EdnValue::Instant(value) => format!("#inst \"{}\"", escape_edn_string(value)),
        EdnValue::Uuid(value) => format!("#uuid \"{}\"", escape_edn_string(value)),
        EdnValue::Vector(values) => {
            let values = values
                .iter()
                .map(format_edn_value)
                .collect::<Vec<_>>()
                .join(" ");
            format!("[{values}]")
        }
        EdnValue::List(values) => {
            let values = values
                .iter()
                .map(format_edn_value)
                .collect::<Vec<_>>()
                .join(" ");
            format!("({values})")
        }
        EdnValue::Set(values) => {
            let values = values
                .iter()
                .map(format_edn_value)
                .collect::<Vec<_>>()
                .join(" ");
            format!("#{{{values}}}")
        }
        EdnValue::Map(map) => {
            let values = map
                .iter()
                .map(|(key, value)| format!(":{key} {}", format_edn_value(value)))
                .collect::<Vec<_>>()
                .join(" ");
            format!("{{{values}}}")
        }
        EdnValue::MapEntries(entries) => {
            let values = entries
                .iter()
                .map(|(key, value)| {
                    format!("{} {}", format_edn_map_key(key), format_edn_value(value))
                })
                .collect::<Vec<_>>()
                .join(" ");
            format!("{{{values}}}")
        }
    }
}

fn format_edn_map_key(key: &EdnValue) -> String {
    match key {
        EdnValue::Keyword(value) => format!(":{value}"),
        EdnValue::String(value) => format!("\"{}\"", escape_edn_string(value)),
        EdnValue::Symbol(value) => value.clone(),
        EdnValue::Integer(value) => value.to_string(),
        EdnValue::Float(value) => format_edn_float(*value),
        _ => format_edn_value(key),
    }
}

fn format_edn_float(value: f64) -> String {
    let raw = value.to_string();
    if raw.contains('.') || raw.contains('e') || raw.contains('E') {
        raw
    } else {
        format!("{raw}.0")
    }
}

fn escape_edn_string(value: &str) -> String {
    let mut escaped = String::new();
    for ch in value.chars() {
        match ch {
            '"' => escaped.push_str("\\\""),
            '\\' => escaped.push_str("\\\\"),
            '\n' => escaped.push_str("\\n"),
            '\r' => escaped.push_str("\\r"),
            '\t' => escaped.push_str("\\t"),
            other => escaped.push(other),
        }
    }
    escaped
}

fn current_epoch_millis() -> Option<i64> {
    let duration = SystemTime::now().duration_since(UNIX_EPOCH).ok()?;
    i64::try_from(duration.as_millis()).ok()
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
