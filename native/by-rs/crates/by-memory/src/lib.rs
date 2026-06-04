#![forbid(unsafe_code)]

use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    path::{Path, PathBuf},
};

use anyhow::{bail, Context, Result};
use rusqlite::{params, params_from_iter, types::Value as SqlValue, Connection, OpenFlags};
use serde_json::{json, Map, Value};
use sha2::{Digest, Sha256};

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct MemorySearchRequest {
    pub query: String,
    pub limit: usize,
}

impl MemorySearchRequest {
    pub fn new(query: impl Into<String>) -> Self {
        Self {
            query: query.into(),
            limit: 20,
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct MemoryRecallRequest {
    pub user_id: String,
    pub session_id: Option<String>,
    pub query: String,
    pub layer: Option<String>,
    pub limit: usize,
    pub match_mode: String,
    pub kind: Option<String>,
    pub min_confidence: Option<f64>,
}

impl MemoryRecallRequest {
    pub fn new(user_id: impl Into<String>, query: impl Into<String>) -> Self {
        Self {
            user_id: user_id.into(),
            session_id: None,
            query: query.into(),
            layer: None,
            limit: 10,
            match_mode: "or".to_string(),
            kind: None,
            min_confidence: None,
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct MemoryReadRequest {
    pub user_id: String,
    pub layer: String,
    pub query: Value,
    pub limit: usize,
    pub include_archived: bool,
}

impl MemoryReadRequest {
    pub fn new(user_id: impl Into<String>, layer: impl Into<String>) -> Self {
        Self {
            user_id: user_id.into(),
            layer: layer.into(),
            query: Value::Object(Map::new()),
            limit: 20,
            include_archived: false,
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct MemoryWriteRequest {
    pub user_id: String,
    pub layer: String,
    pub entry: Value,
}

impl MemoryWriteRequest {
    pub fn new(user_id: impl Into<String>, layer: impl Into<String>, entry: Value) -> Self {
        Self {
            user_id: user_id.into(),
            layer: layer.into(),
            entry,
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct MemoryPromoteRequest {
    pub user_id: String,
    pub from_layer: String,
    pub to_layer: String,
    pub entry: Value,
    pub new_entry_id: Option<String>,
}

impl MemoryPromoteRequest {
    pub fn new(
        user_id: impl Into<String>,
        from_layer: impl Into<String>,
        to_layer: impl Into<String>,
        entry: Value,
    ) -> Self {
        Self {
            user_id: user_id.into(),
            from_layer: from_layer.into(),
            to_layer: to_layer.into(),
            entry,
            new_entry_id: None,
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct MemoryPurgePlanRequest {
    pub user_id: String,
    pub sessions_root: Option<PathBuf>,
    pub cap: usize,
    pub stale_days: i64,
}

impl MemoryPurgePlanRequest {
    pub fn new(user_id: impl Into<String>) -> Self {
        Self {
            user_id: user_id.into(),
            sessions_root: None,
            cap: 500,
            stale_days: 60,
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct MemorySweepL2Request {
    pub user_id: String,
    pub retention_days: i64,
}

impl MemorySweepL2Request {
    pub fn new(user_id: impl Into<String>) -> Self {
        Self {
            user_id: user_id.into(),
            retention_days: 30,
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct MemoryConsolidateRequest {
    pub user_id: String,
    pub session_id: Option<String>,
    pub window_ms: i64,
    pub min_batch: usize,
    pub reducer: String,
}

impl MemoryConsolidateRequest {
    pub fn new(user_id: impl Into<String>) -> Self {
        Self {
            user_id: user_id.into(),
            session_id: None,
            window_ms: 600_000,
            min_batch: 3,
            reducer: "heuristic".to_string(),
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct MemoryEntryMutationRequest {
    pub user_id: String,
    pub layer: String,
    pub entry_id: String,
    pub value: bool,
}

impl MemoryEntryMutationRequest {
    pub fn new(
        user_id: impl Into<String>,
        layer: impl Into<String>,
        entry_id: impl Into<String>,
    ) -> Self {
        Self {
            user_id: user_id.into(),
            layer: layer.into(),
            entry_id: entry_id.into(),
            value: true,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum MemoryLayer {
    L2,
    L3,
}

impl MemoryLayer {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::L2 => "l2",
            Self::L3 => "l3",
        }
    }
}

pub fn normalize_for_hash(value: impl ToString) -> String {
    let lower = value.to_string().to_lowercase();
    let punctuation_spaced = lower
        .trim()
        .chars()
        .map(|ch| if ch.is_ascii_punctuation() { ' ' } else { ch })
        .collect::<String>();

    let mut normalized = String::new();
    let mut in_whitespace = false;
    for ch in punctuation_spaced.chars() {
        if ch.is_whitespace() {
            if !in_whitespace {
                normalized.push(' ');
                in_whitespace = true;
            }
        } else {
            normalized.push(ch);
            in_whitespace = false;
        }
    }
    normalized
}

pub fn entry_id_for(layer: &str, content: impl ToString) -> Result<String> {
    let layer = normalize_entry_id_layer(layer)?;
    let normalized = normalize_for_hash(content);
    let digest = Sha256::digest(normalized.as_bytes());
    let hex = digest
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect::<String>();
    Ok(format!("{}/{}", layer, &hex[..16]))
}

fn normalize_entry_id_layer(layer: &str) -> Result<&'static str> {
    let normalized = layer
        .trim()
        .trim_start_matches(':')
        .trim_matches('"')
        .to_lowercase();
    match normalized.as_str() {
        "l1" => Ok("l1"),
        "l2" => Ok("l2"),
        "l3" => Ok("l3"),
        _ => bail!("unsupported memory layer: {layer}"),
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct MemorySearchHit {
    pub layer: MemoryLayer,
    pub db_id: i64,
    pub entry_id: Option<String>,
    pub kind: String,
    pub content: String,
    pub rank: f64,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct MemoryInspectReport {
    pub schema_version: Option<String>,
    pub sqlite_user_version: i64,
    pub journal_mode: String,
    pub tables: Vec<MemoryTableStats>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct MemoryTableStats {
    pub name: String,
    pub present: bool,
    pub rows: Option<i64>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct MemoryStatsRequest {
    pub user_id: String,
    pub session_id: Option<String>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct MemoryAuditRow {
    id: i64,
    user_id: String,
    session_id: String,
    agent_id: Option<String>,
    turn_id: i64,
    total_turns: Option<i64>,
    entry_id: String,
    layer: String,
    byte_cost: Option<i64>,
    created_at: Option<String>,
}

type MemoryAuditTurnSlot = (Option<String>, i64, Option<i64>, Vec<MemoryAuditRow>);

pub fn search_memory(
    db_path: impl AsRef<Path>,
    request: MemorySearchRequest,
) -> Result<Vec<MemorySearchHit>> {
    let Some(query) = normalize_fts_query(&request.query) else {
        return Ok(Vec::new());
    };
    if request.limit == 0 {
        return Ok(Vec::new());
    }

    let conn = Connection::open_with_flags(db_path, OpenFlags::SQLITE_OPEN_READ_ONLY)
        .context("opening memory sqlite database read-only")?;
    let mut hits = Vec::new();

    hits.extend(search_episodes(&conn, &query, request.limit)?);
    hits.extend(search_semantic_facts(&conn, &query, request.limit)?);
    hits.truncate(request.limit);

    Ok(hits)
}

pub fn recall_memory(db_path: impl AsRef<Path>, request: MemoryRecallRequest) -> Result<Value> {
    let normalized_layer = normalize_recall_layer(request.layer.as_deref());
    let normalized_match = normalize_recall_match(&request.match_mode);
    let limit = request.limit;
    let session_id = request.session_id.as_deref();
    let kind = request.kind.as_deref().and_then(non_blank);

    if limit == 0 {
        return Ok(memory_recall_report(
            normalized_layer.unwrap_or("combined"),
            Vec::new(),
            session_id,
            normalized_match,
            normalized_layer == Some("l1"),
        ));
    }

    let conn = Connection::open_with_flags(db_path, OpenFlags::SQLITE_OPEN_READ_ONLY)
        .context("opening memory sqlite database read-only")?;

    match normalized_layer {
        Some("l1") => Ok(memory_recall_report(
            "l1",
            Vec::new(),
            session_id,
            normalized_match,
            true,
        )),
        Some("l2") => {
            let entries = recall_l2_entries(
                &conn,
                &request.user_id,
                session_id,
                kind,
                &request.query,
                normalized_match,
                limit,
            )?;
            Ok(memory_recall_report(
                "l2",
                recall_select_entries(entries, RECALL_LAYER_KEYS),
                session_id,
                normalized_match,
                false,
            ))
        }
        Some("l3") => {
            let entries = recall_l3_entries(
                &conn,
                &request.user_id,
                kind,
                request.min_confidence,
                &request.query,
                normalized_match,
                limit,
            )?;
            Ok(memory_recall_report(
                "l3",
                recall_select_entries(entries, RECALL_LAYER_KEYS),
                session_id,
                normalized_match,
                false,
            ))
        }
        _ => {
            let entries = recall_combined_entries(
                &conn,
                &request.user_id,
                session_id,
                &request.query,
                normalized_match,
                limit,
            )?;
            Ok(memory_recall_report(
                "combined",
                entries,
                session_id,
                normalized_match,
                true,
            ))
        }
    }
}

pub fn read_memory(db_path: impl AsRef<Path>, request: MemoryReadRequest) -> Result<Value> {
    let Some(layer) = normalize_recall_layer(Some(&request.layer)) else {
        return Ok(json!({
            "error": format!("UnifiedStore/read-entries: unknown layer: {}", request.layer),
        }));
    };
    let query = MemoryReadQuery::from_value(&request.query);
    let limit = request.limit;

    if limit == 0 || layer == "l1" {
        return Ok(memory_read_report(
            layer,
            Vec::new(),
            request.include_archived,
            layer == "l1",
        ));
    }

    let conn = Connection::open_with_flags(db_path, OpenFlags::SQLITE_OPEN_READ_ONLY)
        .context("opening memory sqlite database read-only")?;
    let user_id = query.user_id.as_deref().unwrap_or(&request.user_id);
    let entries = match layer {
        "l2" => read_l2_entries(&conn, user_id, &query, limit, request.include_archived)?,
        "l3" => read_l3_entries(&conn, user_id, &query, limit, request.include_archived)?,
        _ => Vec::new(),
    };

    Ok(memory_read_report(
        layer,
        entries,
        request.include_archived,
        false,
    ))
}

pub fn write_memory_entry(db_path: impl AsRef<Path>, request: MemoryWriteRequest) -> Result<Value> {
    let Some(layer) = normalize_recall_layer(Some(&request.layer)) else {
        return Ok(json!({
            "error": format!("UnifiedStore/write-entry: unknown layer: {}", request.layer),
        }));
    };

    if layer == "l1" {
        return Ok(json!({
            "error": "memory$write: layer l1 requires live memory manager state",
            "layer": layer,
            "l1-persisted?": false,
            "l1-live-skipped?": true,
        }));
    }

    let Some(raw_entry) = request.entry.as_object() else {
        return Ok(json!({
            "error": "memory$write: entry must be a map/object",
            "layer": layer,
        }));
    };
    let entry = normalize_memory_write_entry(layer, &request.user_id, raw_entry)?;
    if let Some(error) = entry.validation_error() {
        return Ok(json!({
            "error": error,
            "layer": layer,
            "entry": entry.as_entry_json(),
        }));
    }

    let conn = Connection::open_with_flags(db_path, OpenFlags::SQLITE_OPEN_READ_WRITE)
        .context("opening memory sqlite database read-write")?;
    let table = match layer {
        "l2" => "episodes",
        "l3" => "semantic_facts",
        _ => unreachable!("l1 returned before sqlite open"),
    };
    if !table_exists(&conn, table)? {
        return Ok(json!({
            "error": format!("memory$write: local sqlite table {table} is missing"),
            "layer": layer,
            "entry-id": entry.entry_id,
        }));
    }
    if let Some(existing) = match layer {
        "l2" => hydrate_episode(&conn, &entry.user_id, &entry.entry_id)?,
        "l3" => hydrate_semantic_fact(&conn, &entry.user_id, &entry.entry_id)?,
        _ => None,
    } {
        return Ok(json!({
            "entry-id": entry.entry_id,
            "layer": layer,
            "entry": existing,
            "duplicate?": true,
        }));
    }

    let saved = match layer {
        "l2" => insert_l2_memory_entry(&conn, &entry)?,
        "l3" => insert_l3_memory_entry(&conn, &entry)?,
        _ => unreachable!("l1 returned before insert"),
    };

    Ok(json!({
        "entry-id": entry.entry_id,
        "layer": layer,
        "entry": saved,
    }))
}

pub fn promote_memory_entry(
    db_path: impl AsRef<Path>,
    request: MemoryPromoteRequest,
) -> Result<Value> {
    let Some(from_layer) = normalize_recall_layer(Some(&request.from_layer)) else {
        return Ok(json!({
            "error": format!("UnifiedStore/promote: unknown from layer: {}", request.from_layer),
        }));
    };
    let Some(to_layer) = normalize_recall_layer(Some(&request.to_layer)) else {
        return Ok(json!({
            "error": format!("UnifiedStore/promote: unknown to layer: {}", request.to_layer),
            "from": from_layer,
        }));
    };

    if to_layer == "l1" {
        return Ok(json!({
            "error": "memory$promote: promoting into l1 requires live memory manager state",
            "from": from_layer,
            "to": to_layer,
            "l1-live-skipped?": true,
        }));
    }

    let Some(raw_entry) = request.entry.as_object() else {
        return Ok(json!({
            "error": "memory$promote: entry must be a map/object",
            "from": from_layer,
            "to": to_layer,
        }));
    };
    let Some(source_entry_id) = entry_string_field(raw_entry, &["id", "entry-id", "entry_id"])
    else {
        return Ok(json!({
            "error": "memory$promote: entry.id is required for provenance",
            "from": from_layer,
            "to": to_layer,
        }));
    };

    let content = entry_string_field(raw_entry, &["content"]).unwrap_or_default();
    let new_entry_id = match request
        .new_entry_id
        .as_deref()
        .and_then(|value| non_blank_owned(value.to_string()))
    {
        Some(value) => value,
        None => entry_id_for(
            to_layer,
            format!("{from_layer}:{source_entry_id}:{}", content.trim()),
        )?,
    };

    let mut promoted = raw_entry.clone();
    promoted.remove("db-id");
    promoted.remove("db_id");
    promoted.insert("id".to_string(), json!(new_entry_id));
    promoted.insert("layer".to_string(), json!(to_layer));
    let mut sources = match entry_array_field(raw_entry, &["sources"]) {
        Value::Array(values) => values,
        _ => Vec::new(),
    };
    sources.push(json!({
        "type": "promotion",
        "id": source_entry_id,
        "from-layer": from_layer,
    }));
    promoted.insert("sources".to_string(), Value::Array(sources));

    let mut report = write_memory_entry(
        db_path,
        MemoryWriteRequest {
            user_id: request.user_id,
            layer: to_layer.to_string(),
            entry: Value::Object(promoted.clone()),
        },
    )?;
    if let Value::Object(ref mut object) = report {
        object.insert("from".to_string(), json!(from_layer));
        object.insert("to".to_string(), json!(to_layer));
        object.insert("source-entry-id".to_string(), json!(source_entry_id));
        object.insert("promoted-entry".to_string(), Value::Object(promoted));
    }
    Ok(report)
}

pub fn purge_plan_memory(
    db_path: impl AsRef<Path>,
    request: MemoryPurgePlanRequest,
) -> Result<Value> {
    let conn = Connection::open_with_flags(db_path, OpenFlags::SQLITE_OPEN_READ_ONLY)
        .context("opening memory sqlite database read-only")?;
    let cap = request.cap;
    let in_db = db_sessions(&conn, &request.user_id)?;
    let on_disk = request
        .sessions_root
        .as_deref()
        .map(live_sessions_on_disk)
        .transpose()?
        .unwrap_or_default();
    let orphan_sids = in_db.difference(&on_disk).cloned().collect::<Vec<String>>();
    let l2_orphan = orphan_l2_episodes(&conn, &request.user_id, &orphan_sids, cap)?;
    let l3_stale = l3_stale_facts(&conn, &request.user_id, request.stale_days, cap)?;
    let l2_orphan_session_count = orphan_sids.len();
    let l2_orphan_episode_count = l2_orphan.as_array().map(Vec::len).unwrap_or(0);
    let l3_stale_count = l3_stale.as_array().map(Vec::len).unwrap_or(0);

    Ok(json!({
        "l2-orphan-sessions": orphan_sids,
        "l2-orphan-episodes": l2_orphan,
        "l3-stale-facts": l3_stale,
        "l3-orphan-facts": [],
        "counts": {
            "l2-orphan-sessions": l2_orphan_session_count,
            "l2-orphan-episodes": l2_orphan_episode_count,
            "l3-stale": l3_stale_count,
            "l3-orphan": 0,
        },
        "cap": cap,
        "stale-days": request.stale_days,
        "sessions-root": request
            .sessions_root
            .as_ref()
            .map(|path| path.display().to_string()),
        "registry-live-skipped?": true,
    }))
}

pub fn sweep_l2_memory(db_path: impl AsRef<Path>, request: MemorySweepL2Request) -> Result<Value> {
    let conn = Connection::open_with_flags(db_path, OpenFlags::SQLITE_OPEN_READ_WRITE)
        .context("opening memory sqlite database read-write")?;
    if !table_exists(&conn, "episodes")? {
        return Ok(json!({
            "error": "memory$sweep-l2: local sqlite table episodes is missing",
            "tombstoned": 0,
            "retention-days": request.retention_days,
        }));
    }

    let changed = conn
        .execute(
            r#"
            UPDATE episodes
               SET tombstoned_flag = 1
             WHERE user_id = ?1
               AND keep_flag = 0
               AND tombstoned_flag = 0
               AND timestamp < datetime('now', '-' || ?2 || ' days')
            "#,
            params![request.user_id, request.retention_days],
        )
        .context("sweeping l2 memory episodes")?;
    Ok(json!({
        "tombstoned": changed,
        "retention-days": request.retention_days,
    }))
}

pub fn consolidate_l2_memory(
    db_path: impl AsRef<Path>,
    request: MemoryConsolidateRequest,
) -> Result<Value> {
    let conn = Connection::open_with_flags(db_path, OpenFlags::SQLITE_OPEN_READ_WRITE)
        .context("opening memory sqlite database read-write")?;
    if !table_exists(&conn, "episodes")? {
        return Ok(memory_consolidate_error_report(
            &request,
            "memory$consolidate: local sqlite table episodes is missing",
        ));
    }
    if !table_exists(&conn, "semantic_facts")? {
        return Ok(memory_consolidate_error_report(
            &request,
            "memory$consolidate: local sqlite table semantic_facts is missing",
        ));
    }

    let window_ms = effective_consolidation_window_ms(request.window_ms);
    let min_batch = request.min_batch.max(1);
    let requested_reducer = normalize_keywordish(&request.reducer);
    let reducer = "heuristic";
    let episodes = consolidation_l2_episodes(
        &conn,
        &request.user_id,
        request.session_id.as_deref(),
        window_ms,
    )?;
    let mut grouped: BTreeMap<(Vec<String>, i64), Vec<ConsolidationEpisode>> = BTreeMap::new();
    for episode in episodes {
        grouped
            .entry((episode.tags.clone(), episode.bucket))
            .or_default()
            .push(episode);
    }

    let mut produced = 0usize;
    let mut consumed = 0usize;
    let mut auto_kept = 0usize;
    let mut batches = Vec::new();
    for ((tags, bucket), entries) in grouped {
        if entries.len() < min_batch {
            continue;
        }
        let summary = summarize_consolidation_batch(&tags, &entries);
        let sources = entries
            .iter()
            .map(|entry| {
                json!({
                    "type": "consolidation",
                    "id": entry.entry_id,
                    "db-id": entry.db_id,
                    "from-layer": "l2",
                })
            })
            .collect::<Vec<_>>();
        let entry_id = entry_id_for(
            "l3",
            format!(
                "l2-consolidation:{}:{}:{}:{}",
                request.user_id,
                request.session_id.as_deref().unwrap_or("*"),
                bucket,
                summary
            ),
        )?;
        let mut raw_entry = Map::new();
        raw_entry.insert("id".to_string(), json!(entry_id));
        raw_entry.insert("kind".to_string(), json!("summary"));
        raw_entry.insert("content".to_string(), json!(summary));
        raw_entry.insert("user-id".to_string(), json!(request.user_id));
        raw_entry.insert("tags".to_string(), json!(tags));
        raw_entry.insert("sources".to_string(), Value::Array(sources));
        raw_entry.insert("confidence".to_string(), json!(0.85));
        if let Some(session_id) = request.session_id.as_deref() {
            raw_entry.insert("session-id".to_string(), json!(session_id));
            raw_entry.insert(
                "metadata".to_string(),
                json!({
                    "session-id": session_id,
                    "window-ms": window_ms,
                    "bucket": bucket,
                }),
            );
        } else {
            raw_entry.insert(
                "metadata".to_string(),
                json!({
                    "window-ms": window_ms,
                    "bucket": bucket,
                }),
            );
        }
        let normalized = normalize_memory_write_entry("l3", &request.user_id, &raw_entry)?;
        if let Some(error) = normalized.validation_error() {
            return Ok(memory_consolidate_error_report(&request, error));
        }
        if hydrate_semantic_fact(&conn, &normalized.user_id, &normalized.entry_id)?.is_none() {
            insert_l3_memory_entry(&conn, &normalized)?;
            produced += 1;
        }
        let keep_ids = entries.iter().map(|entry| entry.db_id).collect::<Vec<_>>();
        auto_kept += keep_consolidation_source_episodes(&conn, &request.user_id, &keep_ids)?;
        consumed += entries.len();
        batches.push(json!({
            "tags": tags,
            "bucket": bucket,
            "consumed": entries.len(),
            "summary-bytes": normalized.content.len(),
        }));
    }

    let mut report = memory_consolidate_report(MemoryConsolidateReportInput {
        produced,
        consumed,
        auto_kept,
        batches,
        window_ms,
        min_batch,
        reducer,
        requested_reducer: &requested_reducer,
    });
    if requested_reducer == "llm" {
        if let Value::Object(ref mut object) = report {
            object.insert("llm-live-skipped?".to_string(), json!(true));
        }
    }
    Ok(json!({ "report": report }))
}

pub fn forget_memory_entry(
    db_path: impl AsRef<Path>,
    request: MemoryEntryMutationRequest,
) -> Result<Value> {
    let Some(layer) = normalize_recall_layer(Some(&request.layer)) else {
        return Ok(memory_mutation_error_report(
            &request.layer,
            &request.entry_id,
            format!("memory$forget: unknown layer: {}", request.layer),
        ));
    };

    if layer == "l1" {
        return Ok(json!({
            "ok": false,
            "layer": layer,
            "entry-id": request.entry_id,
            "l1-persisted?": false,
            "l1-live-skipped?": true,
        }));
    }

    let conn = Connection::open_with_flags(db_path, OpenFlags::SQLITE_OPEN_READ_WRITE)
        .context("opening memory sqlite database read-write")?;
    let ok = update_memory_entry_flag(
        &conn,
        layer,
        "tombstoned_flag",
        &request.user_id,
        &request.entry_id,
        true,
    )?;
    Ok(json!({
        "ok": ok,
        "layer": layer,
        "entry-id": request.entry_id,
    }))
}

pub fn set_memory_keep_flag(
    db_path: impl AsRef<Path>,
    request: MemoryEntryMutationRequest,
) -> Result<Value> {
    set_memory_policy_flag(db_path, request, "memory$keep!", "keep_flag")
}

pub fn set_memory_archive_flag(
    db_path: impl AsRef<Path>,
    request: MemoryEntryMutationRequest,
) -> Result<Value> {
    set_memory_policy_flag(db_path, request, "memory$archive!", "archived_flag")
}

fn set_memory_policy_flag(
    db_path: impl AsRef<Path>,
    request: MemoryEntryMutationRequest,
    command_name: &str,
    flag_column: &str,
) -> Result<Value> {
    let Some(layer) = normalize_recall_layer(Some(&request.layer)) else {
        return Ok(memory_mutation_error_report(
            &request.layer,
            &request.entry_id,
            format!("{command_name}: unknown layer: {}", request.layer),
        ));
    };

    if layer == "l1" {
        return Ok(memory_mutation_error_report(
            layer,
            &request.entry_id,
            format!("{command_name}: layer must be l2 or l3 for local sqlite toggle: l1"),
        ));
    }

    let conn = Connection::open_with_flags(db_path, OpenFlags::SQLITE_OPEN_READ_WRITE)
        .context("opening memory sqlite database read-write")?;
    let ok = update_memory_entry_flag(
        &conn,
        layer,
        flag_column,
        &request.user_id,
        &request.entry_id,
        request.value,
    )?;
    Ok(json!({
        "ok": ok,
        "layer": layer,
        "entry-id": request.entry_id,
        "value": request.value,
    }))
}

fn update_memory_entry_flag(
    conn: &Connection,
    layer: &str,
    flag_column: &str,
    user_id: &str,
    entry_id: &str,
    value: bool,
) -> Result<bool> {
    let (table, table_label) = match layer {
        "l2" => ("episodes", "episodes"),
        "l3" => ("semantic_facts", "semantic facts"),
        _ => return Ok(false),
    };
    if !table_exists(conn, table)? {
        return Ok(false);
    }

    let sql = format!("UPDATE {table} SET {flag_column} = ?1 WHERE user_id = ?2 AND entry_id = ?3");
    let changed = conn
        .execute(&sql, params![if value { 1 } else { 0 }, user_id, entry_id])
        .with_context(|| format!("updating {table_label} {flag_column}"))?;
    Ok(changed > 0)
}

fn memory_mutation_error_report(layer: &str, entry_id: &str, error: impl Into<String>) -> Value {
    json!({
        "ok": false,
        "layer": layer,
        "entry-id": entry_id,
        "error": error.into(),
    })
}

#[derive(Clone, Debug, PartialEq)]
struct ConsolidationEpisode {
    db_id: i64,
    entry_id: String,
    content: String,
    tags: Vec<String>,
    bucket: i64,
}

fn memory_consolidate_error_report(
    request: &MemoryConsolidateRequest,
    error: impl Into<String>,
) -> Value {
    json!({
        "error": error.into(),
        "report": memory_consolidate_report(MemoryConsolidateReportInput {
            produced: 0,
            consumed: 0,
            auto_kept: 0,
            batches: Vec::new(),
            window_ms: effective_consolidation_window_ms(request.window_ms),
            min_batch: request.min_batch.max(1),
            reducer: "heuristic",
            requested_reducer: &normalize_keywordish(&request.reducer),
        }),
    })
}

struct MemoryConsolidateReportInput<'a> {
    produced: usize,
    consumed: usize,
    auto_kept: usize,
    batches: Vec<Value>,
    window_ms: i64,
    min_batch: usize,
    reducer: &'a str,
    requested_reducer: &'a str,
}

fn memory_consolidate_report(input: MemoryConsolidateReportInput<'_>) -> Value {
    let MemoryConsolidateReportInput {
        produced,
        consumed,
        auto_kept,
        batches,
        window_ms,
        min_batch,
        reducer,
        requested_reducer,
    } = input;

    json!({
        "from-layer": "l2",
        "to-layer": "l3",
        "produced": produced,
        "consumed": consumed,
        "auto-kept": auto_kept,
        "batches": batches,
        "window-ms": window_ms,
        "min-batch": min_batch,
        "reducer": reducer,
        "requested-reducer": requested_reducer,
    })
}

fn effective_consolidation_window_ms(window_ms: i64) -> i64 {
    if window_ms > 0 {
        window_ms
    } else {
        600_000
    }
}

fn consolidation_l2_episodes(
    conn: &Connection,
    user_id: &str,
    session_id: Option<&str>,
    window_ms: i64,
) -> Result<Vec<ConsolidationEpisode>> {
    let mut sql = String::from(
        r#"
        SELECT id, entry_id, content, tags,
               CAST(COALESCE((CAST(strftime('%s', timestamp) AS INTEGER) * 1000) / ?1, 0) AS INTEGER)
        FROM episodes
        WHERE user_id = ?2
          AND archived_flag = 0
          AND tombstoned_flag = 0
        "#,
    );
    let mut values = vec![
        SqlValue::Integer(window_ms),
        SqlValue::Text(user_id.to_string()),
    ];
    push_optional_text_condition(&mut sql, &mut values, "session_id", session_id);
    sql.push_str(" ORDER BY timestamp DESC, id DESC LIMIT 1000");

    let mut stmt = conn.prepare(&sql)?;
    let rows = stmt.query_map(params_from_iter(values), |row| {
        let db_id: i64 = row.get(0)?;
        let entry_id = row
            .get::<_, Option<String>>(1)?
            .and_then(non_blank_owned)
            .unwrap_or_else(|| db_id.to_string());
        let content: String = row.get(2)?;
        let raw_tags: Option<String> = row.get(3)?;
        let bucket: i64 = row.get(4)?;
        Ok(ConsolidationEpisode {
            db_id,
            entry_id,
            content,
            tags: consolidation_tags(raw_tags.as_deref()),
            bucket,
        })
    })?;
    rows.collect::<rusqlite::Result<Vec<_>>>()
        .context("reading l2 episodes for memory consolidation")
}

fn consolidation_tags(raw_tags: Option<&str>) -> Vec<String> {
    let Value::Array(values) = json_array_or_empty(raw_tags) else {
        return Vec::new();
    };
    let mut tags = values
        .iter()
        .filter_map(json_value_to_string)
        .filter_map(non_blank_owned)
        .filter(|tag| !(tag.starts_with("event:") || tag.starts_with("kind:")))
        .collect::<Vec<_>>();
    tags.sort();
    tags.dedup();
    tags
}

fn summarize_consolidation_batch(tags: &[String], entries: &[ConsolidationEpisode]) -> String {
    let head = if tags.is_empty() {
        format!("[summary of {} events]", entries.len())
    } else {
        format!(
            "[summary of {} events] tags={}",
            entries.len(),
            clojure_string_vector(tags)
        )
    };
    let sample = entries
        .iter()
        .filter_map(|entry| non_blank(&entry.content).map(truncate_consolidation_sample))
        .take(3)
        .collect::<Vec<_>>();
    if sample.is_empty() {
        head
    } else {
        format!("{head}\n- {}", sample.join("\n- "))
    }
}

fn clojure_string_vector(values: &[String]) -> String {
    format!(
        "[{}]",
        values
            .iter()
            .map(|value| format!("{value:?}"))
            .collect::<Vec<_>>()
            .join(" ")
    )
}

fn truncate_consolidation_sample(value: &str) -> String {
    let mut chars = value.chars();
    let head = chars.by_ref().take(80).collect::<String>();
    if chars.next().is_some() {
        format!("{head}…")
    } else {
        head
    }
}

fn keep_consolidation_source_episodes(
    conn: &Connection,
    user_id: &str,
    db_ids: &[i64],
) -> Result<usize> {
    if db_ids.is_empty() {
        return Ok(0);
    }
    let placeholders = std::iter::repeat_n("?", db_ids.len())
        .collect::<Vec<_>>()
        .join(", ");
    let sql =
        format!("UPDATE episodes SET keep_flag = 1 WHERE user_id = ? AND id IN ({placeholders})");
    let mut values = Vec::with_capacity(db_ids.len() + 1);
    values.push(SqlValue::Text(user_id.to_string()));
    values.extend(db_ids.iter().copied().map(SqlValue::Integer));
    conn.execute(&sql, params_from_iter(values))
        .context("keeping l2 source episodes after memory consolidation")
}

#[derive(Clone, Debug, PartialEq)]
struct NormalizedMemoryWriteEntry {
    layer: &'static str,
    user_id: String,
    entry_id: String,
    id_source: &'static str,
    kind: String,
    content: String,
    session_id: Option<String>,
    role: Option<String>,
    source: Option<String>,
    confidence: f64,
    tags: Value,
    tags_text: Option<String>,
    sources: Value,
    sources_text: Option<String>,
    ttl: Value,
    data: Value,
    metadata: Value,
    metadata_text: Option<String>,
    keep: bool,
    archived: bool,
    tombstoned: bool,
}

impl NormalizedMemoryWriteEntry {
    fn validation_error(&self) -> Option<String> {
        if self.content.trim().is_empty() {
            return Some("memory$write: entry.content is required".to_string());
        }
        if self.layer == "l2" && self.session_id.is_none() {
            return Some(
                "memory$write: entry.session-id is required for local l2 writes without live session state"
                    .to_string(),
            );
        }
        if self.entry_id.trim().is_empty() {
            return Some("memory$write: entry.id could not be resolved".to_string());
        }
        None
    }

    fn as_entry_json(&self) -> Value {
        let mut entry = Map::new();
        entry.insert("id".to_string(), json!(self.entry_id));
        entry.insert("layer".to_string(), json!(self.layer));
        entry.insert("kind".to_string(), json!(self.kind));
        entry.insert("content".to_string(), json!(self.content));
        entry.insert("user-id".to_string(), json!(self.user_id));
        entry.insert("tags".to_string(), self.tags.clone());
        entry.insert("sources".to_string(), self.sources.clone());
        entry.insert("keep".to_string(), json!(self.keep));
        entry.insert("archived".to_string(), json!(self.archived));
        entry.insert("tombstoned".to_string(), json!(self.tombstoned));
        entry.insert("id-source".to_string(), json!(self.id_source));
        if let Some(session_id) = self.session_id.as_deref() {
            entry.insert("session-id".to_string(), json!(session_id));
        }
        if let Some(role) = self.role.as_deref() {
            entry.insert("role".to_string(), json!(role));
        }
        if let Some(source) = self.source.as_deref() {
            entry.insert("source".to_string(), json!(source));
        }
        if self.layer == "l3" {
            entry.insert("confidence".to_string(), json!(self.confidence));
        }
        if !self.ttl.is_null() {
            entry.insert("ttl".to_string(), self.ttl.clone());
        }
        if !self.data.is_null() {
            entry.insert("data".to_string(), self.data.clone());
        }
        if !self.metadata.is_null() {
            entry.insert("metadata".to_string(), self.metadata.clone());
        }
        Value::Object(entry)
    }
}

fn normalize_memory_write_entry(
    layer: &'static str,
    request_user_id: &str,
    raw_entry: &Map<String, Value>,
) -> Result<NormalizedMemoryWriteEntry> {
    let content = entry_string_field(raw_entry, &["content"]).unwrap_or_default();
    let user_id = entry_string_field(raw_entry, &["user-id", "user_id"])
        .unwrap_or_else(|| request_user_id.to_string());
    let supplied_id = entry_string_field(raw_entry, &["id", "entry-id", "entry_id"]);
    let (entry_id, id_source) = match supplied_id {
        Some(id) => (id, "provided"),
        None => (
            entry_id_for(layer, &content)?,
            if layer == "l3" {
                "content-address"
            } else {
                "local-content-hash"
            },
        ),
    };
    let kind = entry_string_field(raw_entry, &["kind"])
        .map(|kind| normalize_keywordish(&kind))
        .unwrap_or_default();
    let session_id = entry_string_field(raw_entry, &["session-id", "session_id"]);
    let role = entry_string_field(raw_entry, &["role"]);
    let source = entry_string_field(raw_entry, &["source"]);
    let confidence = entry_f64_field(raw_entry, &["confidence"]).unwrap_or(1.0);
    let tags = entry_array_field(raw_entry, &["tags"]);
    let tags_text = pack_memory_json_array(&tags)?;
    let sources = entry_array_field(raw_entry, &["sources"]);
    let sources_text = pack_memory_json_array(&sources)?;
    let ttl = entry_value_field(raw_entry, &["ttl"])
        .cloned()
        .unwrap_or(Value::Null);
    let data = entry_value_field(raw_entry, &["data"])
        .cloned()
        .unwrap_or(Value::Null);
    let metadata = entry_value_field(raw_entry, &["metadata"])
        .cloned()
        .unwrap_or(Value::Null);
    let metadata_text = pack_memory_entry_metadata(&ttl, &data, &metadata)?;

    Ok(NormalizedMemoryWriteEntry {
        layer,
        user_id,
        entry_id,
        id_source,
        kind,
        content,
        session_id,
        role,
        source,
        confidence,
        tags,
        tags_text,
        sources,
        sources_text,
        ttl,
        data,
        metadata,
        metadata_text,
        keep: entry_bool_field(raw_entry, &["keep"]).unwrap_or(false),
        archived: entry_bool_field(raw_entry, &["archived"]).unwrap_or(false),
        tombstoned: entry_bool_field(raw_entry, &["tombstoned"]).unwrap_or(false),
    })
}

fn insert_l2_memory_entry(conn: &Connection, entry: &NormalizedMemoryWriteEntry) -> Result<Value> {
    conn.execute(
        r#"
        INSERT INTO episodes
          (session_id, user_id, episode_type, role, content, metadata, tags, sources,
           entry_id, keep_flag, archived_flag, tombstoned_flag)
        VALUES
          (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)
        "#,
        params![
            entry.session_id.as_deref(),
            entry.user_id,
            entry.kind,
            entry.role.as_deref(),
            entry.content,
            entry.metadata_text.as_deref(),
            entry.tags_text.as_deref(),
            entry.sources_text.as_deref(),
            entry.entry_id,
            sqlite_int_bool(entry.keep),
            sqlite_int_bool(entry.archived),
            sqlite_int_bool(entry.tombstoned),
        ],
    )
    .context("writing l2 memory episode")?;
    hydrate_episode(conn, &entry.user_id, &entry.entry_id)?
        .with_context(|| format!("hydrating written l2 memory entry {}", entry.entry_id))
}

fn insert_l3_memory_entry(conn: &Connection, entry: &NormalizedMemoryWriteEntry) -> Result<Value> {
    conn.execute(
        r#"
        INSERT INTO semantic_facts
          (user_id, fact_type, content, source, confidence, metadata, tags, sources,
           entry_id, keep_flag, archived_flag, tombstoned_flag)
        VALUES
          (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)
        "#,
        params![
            entry.user_id,
            entry.kind,
            entry.content,
            entry.source.as_deref(),
            entry.confidence,
            entry.metadata_text.as_deref(),
            entry.tags_text.as_deref(),
            entry.sources_text.as_deref(),
            entry.entry_id,
            sqlite_int_bool(entry.keep),
            sqlite_int_bool(entry.archived),
            sqlite_int_bool(entry.tombstoned),
        ],
    )
    .context("writing l3 memory semantic fact")?;
    hydrate_semantic_fact(conn, &entry.user_id, &entry.entry_id)?
        .with_context(|| format!("hydrating written l3 memory entry {}", entry.entry_id))
}

fn entry_string_field(raw_entry: &Map<String, Value>, keys: &[&str]) -> Option<String> {
    keys.iter()
        .filter_map(|key| entry_value_field(raw_entry, &[*key]))
        .filter_map(json_value_to_string)
        .find_map(non_blank_owned)
}

fn entry_value_field<'a>(raw_entry: &'a Map<String, Value>, keys: &[&str]) -> Option<&'a Value> {
    keys.iter()
        .find_map(|key| object_get_keyish(raw_entry, key))
}

fn entry_bool_field(raw_entry: &Map<String, Value>, keys: &[&str]) -> Option<bool> {
    keys.iter()
        .filter_map(|key| entry_value_field(raw_entry, &[*key]))
        .find_map(json_value_to_bool)
}

fn entry_f64_field(raw_entry: &Map<String, Value>, keys: &[&str]) -> Option<f64> {
    keys.iter()
        .filter_map(|key| entry_value_field(raw_entry, &[*key]))
        .find_map(json_value_to_f64)
}

fn entry_array_field(raw_entry: &Map<String, Value>, keys: &[&str]) -> Value {
    let Some(value) = entry_value_field(raw_entry, keys) else {
        return Value::Array(Vec::new());
    };
    match value {
        Value::Array(values) => Value::Array(values.clone()),
        Value::String(text) => non_blank(text)
            .map(|text| Value::Array(vec![json!(text)]))
            .unwrap_or_else(|| Value::Array(Vec::new())),
        _ => Value::Array(Vec::new()),
    }
}

fn pack_memory_json_array(value: &Value) -> Result<Option<String>> {
    match value {
        Value::Array(values) if !values.is_empty() => Ok(Some(
            serde_json::to_string(values).context("serializing memory JSON array")?,
        )),
        _ => Ok(None),
    }
}

fn pack_memory_entry_metadata(
    ttl: &Value,
    data: &Value,
    metadata: &Value,
) -> Result<Option<String>> {
    let mut packed = Map::new();
    if !ttl.is_null() {
        packed.insert("ttl".to_string(), ttl.clone());
    }
    if !data.is_null() {
        packed.insert("data".to_string(), data.clone());
    }
    if !metadata.is_null() {
        packed.insert("metadata".to_string(), metadata.clone());
    }
    if packed.is_empty() {
        Ok(None)
    } else {
        serde_json::to_string(&Value::Object(packed))
            .map(Some)
            .context("serializing memory metadata")
    }
}

fn json_value_to_bool(value: &Value) -> Option<bool> {
    match value {
        Value::Bool(value) => Some(*value),
        Value::Number(value) => value.as_i64().map(|value| value != 0),
        Value::String(value) => match normalize_keywordish(value).as_str() {
            "true" | "yes" | "1" => Some(true),
            "false" | "no" | "0" => Some(false),
            _ => None,
        },
        _ => None,
    }
}

fn json_value_to_f64(value: &Value) -> Option<f64> {
    match value {
        Value::Number(value) => value.as_f64(),
        Value::String(value) => value.trim().parse::<f64>().ok(),
        _ => None,
    }
}

fn sqlite_int_bool(value: bool) -> i64 {
    if value {
        1
    } else {
        0
    }
}

pub fn inspect_memory(db_path: impl AsRef<Path>) -> Result<MemoryInspectReport> {
    let conn = Connection::open_with_flags(db_path, OpenFlags::SQLITE_OPEN_READ_ONLY)
        .context("opening memory sqlite database read-only")?;
    let schema_version = memory_schema_version(&conn)?;
    let sqlite_user_version = conn
        .pragma_query_value(None, "user_version", |row| row.get(0))
        .context("reading sqlite user_version pragma")?;
    let journal_mode = conn
        .pragma_query_value(None, "journal_mode", |row| row.get(0))
        .context("reading sqlite journal_mode pragma")?;
    let tables = MEMORY_TABLES
        .iter()
        .map(|table| memory_table_stats(&conn, table))
        .collect::<Result<Vec<_>>>()?;

    Ok(MemoryInspectReport {
        schema_version,
        sqlite_user_version,
        journal_mode,
        tables,
    })
}

pub fn memory_stats(db_path: impl AsRef<Path>, request: MemoryStatsRequest) -> Result<Value> {
    let db_path = db_path.as_ref();
    let conn = Connection::open_with_flags(db_path, OpenFlags::SQLITE_OPEN_READ_ONLY)
        .context("opening memory sqlite database read-only")?;
    let user_id = request.user_id.as_str();
    let session_id = request.session_id.as_deref();
    let (oldest_at, newest_at) = l2_min_max_timestamp(&conn, user_id);

    Ok(json!({
        "stats": {
            "db": {
                "path": db_path.display().to_string(),
                "bytes": file_size_bytes(db_path),
                "wal-bytes": Value::Null,
                "page-count": Value::Null,
                "page-size": Value::Null,
            },
            "l1": {
                "count": 0,
                "session-id": session_id,
                "session-entries": 0,
                "pinned": 0,
            },
            "l2": {
                "total": count_where(&conn, "SELECT COUNT(*) FROM episodes WHERE user_id = ?1 AND tombstoned_flag = 0", [user_id]),
                "current-session": session_id.map(|sid| count_where(&conn, "SELECT COUNT(*) FROM episodes WHERE user_id = ?1 AND session_id = ?2 AND tombstoned_flag = 0", [user_id, sid])),
                "sessions-known": count_where(&conn, "SELECT COUNT(DISTINCT session_id) FROM episodes WHERE user_id = ?1", [user_id]),
                "sessions-orphan": Value::Null,
                "keep-flagged": count_where(&conn, "SELECT COUNT(*) FROM episodes WHERE user_id = ?1 AND keep_flag = 1", [user_id]),
                "archived": count_where(&conn, "SELECT COUNT(*) FROM episodes WHERE user_id = ?1 AND archived_flag = 1", [user_id]),
                "tombstoned": count_where(&conn, "SELECT COUNT(*) FROM episodes WHERE user_id = ?1 AND tombstoned_flag = 1", [user_id]),
                "oldest-at": oldest_at,
                "newest-at": newest_at,
            },
            "l3": {
                "total": count_where(&conn, "SELECT COUNT(*) FROM semantic_facts WHERE user_id = ?1 AND tombstoned_flag = 0", [user_id]),
                "by-kind": l3_by_kind(&conn, user_id),
                "confidence-buckets": l3_confidence_buckets(&conn, user_id),
                "stale": Value::Null,
                "orphan": Value::Null,
                "archived": count_where(&conn, "SELECT COUNT(*) FROM semantic_facts WHERE user_id = ?1 AND archived_flag = 1", [user_id]),
                "tombstoned": count_where(&conn, "SELECT COUNT(*) FROM semantic_facts WHERE user_id = ?1 AND tombstoned_flag = 1", [user_id]),
            },
            "capture": {
                "running?": false,
                "backlog": Value::Null,
                "critical?": Value::Null,
                "reducer": "heuristic",
            },
            "audit": {
                "rows": count_where(&conn, "SELECT COUNT(*) FROM memory_audit WHERE user_id = ?1", [user_id]),
                "bytes": Value::Null,
            },
            "health": {
                "status": "ok",
                "warnings": [],
                "last-sweep-at": Value::Null,
                "last-consolidate-at": Value::Null,
            },
        }
    }))
}

pub fn extract_keywords(text: &str) -> Vec<String> {
    extract_keywords_with_limits(text, 3, 10)
}

pub fn extract_keywords_with_limits(
    text: &str,
    min_length: usize,
    max_keywords: usize,
) -> Vec<String> {
    if text.trim().is_empty() || max_keywords == 0 {
        return Vec::new();
    }

    let mut words: Vec<(String, usize, usize)> = Vec::new();
    for token in text
        .split(|ch: char| ch.is_whitespace() || is_fts_special_char(ch))
        .filter(|token| !token.is_empty())
    {
        let word = token.to_lowercase();
        if word.chars().count() < min_length || is_stop_word(&word) {
            continue;
        }
        if let Some((_, count, _)) = words.iter_mut().find(|(existing, _, _)| existing == &word) {
            *count += 1;
        } else {
            let index = words.len();
            words.push((word, 1, index));
        }
    }

    words.sort_by(|left, right| {
        right
            .1
            .cmp(&left.1)
            .then_with(|| left.2.cmp(&right.2))
            .then_with(|| left.0.cmp(&right.0))
    });
    words
        .into_iter()
        .take(max_keywords)
        .map(|(word, _, _)| word)
        .collect()
}

pub fn explain_memory_turn(
    db_path: impl AsRef<Path>,
    session_id: &str,
    agent_id: Option<&str>,
    turn_id: i64,
) -> Result<Value> {
    let conn = Connection::open_with_flags(db_path, OpenFlags::SQLITE_OPEN_READ_ONLY)
        .context("opening memory sqlite database read-only")?;
    if !table_exists(&conn, "memory_audit")? {
        return Ok(explain_turn_json(session_id, agent_id, turn_id, Vec::new()));
    }

    let rows = audit_rows_for_turn(&conn, session_id, agent_id, turn_id)?;
    let entries = explain_items_json(&conn, &rows)?;
    Ok(explain_turn_json_with_entries(
        session_id, agent_id, turn_id, &rows, entries,
    ))
}

pub fn explain_memory_session(db_path: impl AsRef<Path>, session_id: &str) -> Result<Value> {
    let conn = Connection::open_with_flags(db_path, OpenFlags::SQLITE_OPEN_READ_ONLY)
        .context("opening memory sqlite database read-only")?;
    if !table_exists(&conn, "memory_audit")? {
        return Ok(json!({
            "session-id": session_id,
            "turns": [],
        }));
    }

    let rows = audit_rows_for_session(&conn, session_id)?;
    let mut slots: Vec<MemoryAuditTurnSlot> = Vec::new();
    for row in rows {
        if let Some((_, _, _, slot_rows)) = slots
            .iter_mut()
            .find(|(agent, turn, _, _)| *agent == row.agent_id && *turn == row.turn_id)
        {
            slot_rows.push(row);
        } else {
            slots.push((
                row.agent_id.clone(),
                row.turn_id,
                row.total_turns,
                vec![row],
            ));
        }
    }
    slots.sort_by(|left, right| {
        left.2
            .unwrap_or(i64::MAX)
            .cmp(&right.2.unwrap_or(i64::MAX))
            .then_with(|| left.0.cmp(&right.0))
            .then_with(|| left.1.cmp(&right.1))
    });

    let turns = slots
        .into_iter()
        .map(|(agent_id, turn_id, total_turns, slot_rows)| {
            let entries = explain_items_json(&conn, &slot_rows)?;
            Ok(json!({
                "agent-id": agent_id,
                "turn-id": turn_id,
                "total-turns": total_turns,
                "entries": entries,
                "prompt-bytes": prompt_bytes(&slot_rows),
            }))
        })
        .collect::<Result<Vec<_>>>()?;

    Ok(json!({
        "session-id": session_id,
        "turns": turns,
    }))
}

fn search_episodes(conn: &Connection, query: &str, limit: usize) -> Result<Vec<MemorySearchHit>> {
    if !(table_exists(conn, "episodes")? && table_exists(conn, "episodes_fts")?) {
        return Ok(Vec::new());
    }

    let mut stmt = conn.prepare(
        r#"
        SELECT
          e.id,
          e.entry_id,
          e.episode_type,
          e.content,
          bm25(episodes_fts) AS rank
        FROM episodes_fts
        JOIN episodes e ON e.id = episodes_fts.rowid
        WHERE episodes_fts MATCH ?1
          AND e.archived_flag = 0
          AND e.tombstoned_flag = 0
        ORDER BY rank
        LIMIT ?2
        "#,
    )?;

    let rows = stmt.query_map(params![query, sqlite_limit(limit)], |row| {
        Ok(MemorySearchHit {
            layer: MemoryLayer::L2,
            db_id: row.get(0)?,
            entry_id: row.get(1)?,
            kind: row.get(2)?,
            content: row.get(3)?,
            rank: row.get(4)?,
        })
    })?;

    rows.collect::<rusqlite::Result<Vec<_>>>()
        .context("searching episodes FTS")
}

fn search_semantic_facts(
    conn: &Connection,
    query: &str,
    limit: usize,
) -> Result<Vec<MemorySearchHit>> {
    if !(table_exists(conn, "semantic_facts")? && table_exists(conn, "semantic_fts")?) {
        return Ok(Vec::new());
    }

    let mut stmt = conn.prepare(
        r#"
        SELECT
          f.id,
          f.entry_id,
          f.fact_type,
          f.content,
          bm25(semantic_fts) AS rank
        FROM semantic_fts
        JOIN semantic_facts f ON f.id = semantic_fts.rowid
        WHERE semantic_fts MATCH ?1
          AND f.confidence >= 0.0
          AND f.archived_flag = 0
          AND f.tombstoned_flag = 0
        ORDER BY f.confidence DESC, rank
        LIMIT ?2
        "#,
    )?;

    let rows = stmt.query_map(params![query, sqlite_limit(limit)], |row| {
        Ok(MemorySearchHit {
            layer: MemoryLayer::L3,
            db_id: row.get(0)?,
            entry_id: row.get(1)?,
            kind: row.get(2)?,
            content: row.get(3)?,
            rank: row.get(4)?,
        })
    })?;

    rows.collect::<rusqlite::Result<Vec<_>>>()
        .context("searching semantic facts FTS")
}

const RECALL_LAYER_KEYS: &[&str] = &[
    "id",
    "kind",
    "content",
    "role",
    "tags",
    "confidence",
    "session-id",
    "created-at",
];
const RECALL_COMBINED_KEYS: &[&str] = &[
    "id",
    "_layer",
    "kind",
    "content",
    "role",
    "tags",
    "_rrf_score",
    "session-id",
    "created-at",
];

fn recall_l2_entries(
    conn: &Connection,
    user_id: &str,
    session_id: Option<&str>,
    kind: Option<&str>,
    query: &str,
    match_mode: &str,
    limit: usize,
) -> Result<Vec<Value>> {
    if !table_exists(conn, "episodes")? {
        return Ok(Vec::new());
    }
    if let Some(query) = normalize_fts_query_with_match(query, match_mode) {
        recall_l2_fts_entries(conn, user_id, session_id, kind, &query, limit)
    } else {
        recall_l2_recent_entries(conn, user_id, session_id, kind, limit)
    }
}

fn recall_l2_fts_entries(
    conn: &Connection,
    user_id: &str,
    session_id: Option<&str>,
    kind: Option<&str>,
    query: &str,
    limit: usize,
) -> Result<Vec<Value>> {
    if !table_exists(conn, "episodes_fts")? {
        return Ok(Vec::new());
    }

    let mut sql = String::from(
        r#"
        SELECT e.id, e.session_id, e.user_id, e.timestamp, e.episode_type,
               e.role, e.content, e.metadata, e.tags, e.sources, e.entry_id,
               e.keep_flag, e.archived_flag, e.tombstoned_flag
        FROM episodes_fts
        JOIN episodes e ON e.id = episodes_fts.rowid
        WHERE episodes_fts MATCH ?1
          AND e.user_id = ?2
          AND e.archived_flag = 0
          AND e.tombstoned_flag = 0
        "#,
    );
    let mut values = vec![
        SqlValue::Text(query.to_string()),
        SqlValue::Text(user_id.to_string()),
    ];
    push_optional_text_condition(&mut sql, &mut values, "e.session_id", session_id);
    push_optional_keyword_condition(&mut sql, &mut values, "e.episode_type", kind);
    sql.push_str(" ORDER BY bm25(episodes_fts), e.timestamp DESC, e.id DESC LIMIT ?");
    values.push(SqlValue::Integer(sqlite_limit(limit)));

    let mut stmt = conn.prepare(&sql)?;
    let rows = stmt.query_map(params_from_iter(values), episode_json)?;
    rows.collect::<rusqlite::Result<Vec<_>>>()
        .context("recalling l2 episodes by FTS")
}

fn recall_l2_recent_entries(
    conn: &Connection,
    user_id: &str,
    session_id: Option<&str>,
    kind: Option<&str>,
    limit: usize,
) -> Result<Vec<Value>> {
    let mut sql = String::from(
        r#"
        SELECT id, session_id, user_id, timestamp, episode_type, role, content,
               metadata, tags, sources, entry_id, keep_flag, archived_flag,
               tombstoned_flag
        FROM episodes
        WHERE user_id = ?1
          AND archived_flag = 0
          AND tombstoned_flag = 0
        "#,
    );
    let mut values = vec![SqlValue::Text(user_id.to_string())];
    push_optional_text_condition(&mut sql, &mut values, "session_id", session_id);
    push_optional_keyword_condition(&mut sql, &mut values, "episode_type", kind);
    sql.push_str(" ORDER BY timestamp DESC, id DESC LIMIT ?");
    values.push(SqlValue::Integer(sqlite_limit(limit)));

    let mut stmt = conn.prepare(&sql)?;
    let rows = stmt.query_map(params_from_iter(values), episode_json)?;
    rows.collect::<rusqlite::Result<Vec<_>>>()
        .context("recalling recent l2 episodes")
}

fn recall_l3_entries(
    conn: &Connection,
    user_id: &str,
    kind: Option<&str>,
    min_confidence: Option<f64>,
    query: &str,
    match_mode: &str,
    limit: usize,
) -> Result<Vec<Value>> {
    if !table_exists(conn, "semantic_facts")? {
        return Ok(Vec::new());
    }
    if let Some(query) = normalize_fts_query_with_match(query, match_mode) {
        recall_l3_fts_entries(conn, user_id, kind, min_confidence, &query, limit)
    } else {
        recall_l3_recent_entries(conn, user_id, kind, min_confidence, limit)
    }
}

fn recall_l3_fts_entries(
    conn: &Connection,
    user_id: &str,
    kind: Option<&str>,
    min_confidence: Option<f64>,
    query: &str,
    limit: usize,
) -> Result<Vec<Value>> {
    if !table_exists(conn, "semantic_fts")? {
        return Ok(Vec::new());
    }

    let min_confidence = min_confidence.unwrap_or(0.0);
    let mut sql = String::from(
        r#"
        SELECT f.id, f.user_id, f.fact_type, f.content, f.source, f.confidence,
               f.created_at, f.access_count, f.metadata, f.tags, f.sources,
               f.entry_id, f.keep_flag, f.archived_flag, f.tombstoned_flag
        FROM semantic_fts
        JOIN semantic_facts f ON f.id = semantic_fts.rowid
        WHERE semantic_fts MATCH ?1
          AND f.user_id = ?2
          AND f.confidence >= ?3
          AND f.archived_flag = 0
          AND f.tombstoned_flag = 0
        "#,
    );
    let mut values = vec![
        SqlValue::Text(query.to_string()),
        SqlValue::Text(user_id.to_string()),
        SqlValue::Real(min_confidence),
    ];
    push_optional_keyword_condition(&mut sql, &mut values, "f.fact_type", kind);
    sql.push_str(
        " ORDER BY f.confidence DESC, bm25(semantic_fts), f.updated_at DESC, f.id DESC LIMIT ?",
    );
    values.push(SqlValue::Integer(sqlite_limit(limit)));

    let mut stmt = conn.prepare(&sql)?;
    let rows = stmt.query_map(params_from_iter(values), semantic_fact_json)?;
    rows.collect::<rusqlite::Result<Vec<_>>>()
        .context("recalling l3 semantic facts by FTS")
}

fn recall_l3_recent_entries(
    conn: &Connection,
    user_id: &str,
    kind: Option<&str>,
    min_confidence: Option<f64>,
    limit: usize,
) -> Result<Vec<Value>> {
    let min_confidence = min_confidence.unwrap_or(0.0);
    let mut sql = String::from(
        r#"
        SELECT id, user_id, fact_type, content, source, confidence, created_at,
               access_count, metadata, tags, sources, entry_id, keep_flag,
               archived_flag, tombstoned_flag
        FROM semantic_facts
        WHERE user_id = ?1
          AND confidence >= ?2
          AND archived_flag = 0
          AND tombstoned_flag = 0
        "#,
    );
    let mut values = vec![
        SqlValue::Text(user_id.to_string()),
        SqlValue::Real(min_confidence),
    ];
    push_optional_keyword_condition(&mut sql, &mut values, "fact_type", kind);
    sql.push_str(" ORDER BY updated_at DESC, id DESC LIMIT ?");
    values.push(SqlValue::Integer(sqlite_limit(limit)));

    let mut stmt = conn.prepare(&sql)?;
    let rows = stmt.query_map(params_from_iter(values), semantic_fact_json)?;
    rows.collect::<rusqlite::Result<Vec<_>>>()
        .context("recalling recent l3 semantic facts")
}

fn recall_combined_entries(
    conn: &Connection,
    user_id: &str,
    session_id: Option<&str>,
    query: &str,
    match_mode: &str,
    limit: usize,
) -> Result<Vec<Value>> {
    if normalize_fts_query_with_match(query, match_mode).is_none() {
        return Ok(Vec::new());
    }

    let l2 = recall_l2_entries(conn, user_id, session_id, None, query, match_mode, limit)?;
    let l3 = recall_l3_entries(conn, user_id, None, None, query, match_mode, limit)?;
    let mut ranked = Vec::new();
    ranked.extend(rrf_ranked_entries(l2, "l2", 0.4));
    ranked.extend(rrf_ranked_entries(l3, "l3", 0.6));
    ranked.sort_by(|left, right| {
        rrf_score(right)
            .partial_cmp(&rrf_score(left))
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| layer_order(left).cmp(&layer_order(right)))
            .then_with(|| recall_id(left).cmp(&recall_id(right)))
    });
    ranked.truncate(20);
    Ok(recall_select_entries(ranked, RECALL_COMBINED_KEYS))
}

fn rrf_ranked_entries(entries: Vec<Value>, layer: &str, weight: f64) -> Vec<Value> {
    entries
        .into_iter()
        .enumerate()
        .map(|(index, mut entry)| {
            let score = weight / (60.0 + (index + 1) as f64);
            if let Value::Object(ref mut object) = entry {
                object.insert("_layer".to_string(), json!(layer));
                object.insert("_rrf_score".to_string(), json!(score));
            }
            entry
        })
        .collect()
}

fn memory_recall_report(
    layer: &str,
    entries: Vec<Value>,
    session_id: Option<&str>,
    match_mode: &str,
    include_l1_note: bool,
) -> Value {
    json!({
        "layer": layer,
        "count": entries.len(),
        "entries": entries,
        "session-id": session_id,
        "match": match_mode,
        "l1-persisted?": false,
        "l1-live-skipped?": include_l1_note,
    })
}

#[derive(Clone, Debug, Default, PartialEq)]
struct MemoryReadQuery {
    id: Option<String>,
    text: Option<String>,
    session_id: Option<String>,
    user_id: Option<String>,
    kind: Option<String>,
    episode_type: Option<String>,
    fact_type: Option<String>,
    time_after: Option<String>,
    time_before: Option<String>,
    min_confidence: Option<f64>,
    match_mode: String,
}

impl MemoryReadQuery {
    fn from_value(value: &Value) -> Self {
        Self {
            id: query_string(value, &["id"]),
            text: query_string(value, &["text"]),
            session_id: query_string(value, &["session-id", "session_id"]),
            user_id: query_string(value, &["user-id", "user_id"]),
            kind: query_string(value, &["kind"]),
            episode_type: query_string(value, &["episode-type", "episode_type"]),
            fact_type: query_string(value, &["fact-type", "fact_type"]),
            time_after: query_string(value, &["time-after", "time_after"]),
            time_before: query_string(value, &["time-before", "time_before"]),
            min_confidence: query_f64(value, &["min-confidence", "min_confidence"]),
            match_mode: query_string(value, &["match"]).unwrap_or_else(|| "or".to_string()),
        }
    }

    fn l2_kind(&self) -> Option<&str> {
        self.kind
            .as_deref()
            .and_then(non_blank)
            .or_else(|| self.episode_type.as_deref().and_then(non_blank))
    }

    fn l3_kind(&self) -> Option<&str> {
        self.kind
            .as_deref()
            .and_then(non_blank)
            .or_else(|| self.fact_type.as_deref().and_then(non_blank))
    }
}

fn read_l2_entries(
    conn: &Connection,
    user_id: &str,
    query: &MemoryReadQuery,
    limit: usize,
    include_archived: bool,
) -> Result<Vec<Value>> {
    if !table_exists(conn, "episodes")? {
        return Ok(Vec::new());
    }
    if let Some(entry_id) = query.id.as_deref().and_then(non_blank) {
        return read_l2_by_entry_id(conn, user_id, entry_id, include_archived);
    }
    if let Some(text) = query.text.as_deref().and_then(non_blank) {
        return read_l2_fts_entries(conn, user_id, query, text, limit, include_archived);
    }
    read_l2_recent_entries_for_read(conn, user_id, query, limit, include_archived)
}

fn read_l2_by_entry_id(
    conn: &Connection,
    user_id: &str,
    entry_id: &str,
    include_archived: bool,
) -> Result<Vec<Value>> {
    let mut sql = String::from(
        r#"
        SELECT id, session_id, user_id, timestamp, episode_type, role, content,
               metadata, tags, sources, entry_id, keep_flag, archived_flag,
               tombstoned_flag
        FROM episodes
        WHERE user_id = ?1
          AND entry_id = ?2
          AND tombstoned_flag = 0
        "#,
    );
    if !include_archived {
        sql.push_str(" AND archived_flag = 0");
    }
    sql.push_str(" LIMIT 1");

    let mut stmt = conn.prepare(&sql)?;
    let rows = stmt.query_map(params![user_id, entry_id], episode_json)?;
    rows.collect::<rusqlite::Result<Vec<_>>>()
        .context("reading l2 episode by entry id")
}

fn read_l2_fts_entries(
    conn: &Connection,
    user_id: &str,
    query: &MemoryReadQuery,
    text: &str,
    limit: usize,
    include_archived: bool,
) -> Result<Vec<Value>> {
    if !table_exists(conn, "episodes_fts")? {
        return Ok(Vec::new());
    }
    let Some(fts_query) = normalize_fts_query_with_match(text, &query.match_mode) else {
        return Ok(Vec::new());
    };

    let mut sql = String::from(
        r#"
        SELECT e.id, e.session_id, e.user_id, e.timestamp, e.episode_type,
               e.role, e.content, e.metadata, e.tags, e.sources, e.entry_id,
               e.keep_flag, e.archived_flag, e.tombstoned_flag
        FROM episodes_fts
        JOIN episodes e ON e.id = episodes_fts.rowid
        WHERE episodes_fts MATCH ?
          AND e.user_id = ?
          AND e.tombstoned_flag = 0
        "#,
    );
    let mut values = vec![
        SqlValue::Text(fts_query),
        SqlValue::Text(user_id.to_string()),
    ];
    if !include_archived {
        sql.push_str(" AND e.archived_flag = 0");
    }
    push_optional_text_condition(
        &mut sql,
        &mut values,
        "e.session_id",
        query.session_id.as_deref(),
    );
    push_optional_keyword_condition(&mut sql, &mut values, "e.episode_type", query.l2_kind());
    push_optional_time_bound(
        &mut sql,
        &mut values,
        "e.timestamp",
        ">",
        query.time_after.as_deref(),
    );
    push_optional_time_bound(
        &mut sql,
        &mut values,
        "e.timestamp",
        "<",
        query.time_before.as_deref(),
    );
    sql.push_str(" ORDER BY bm25(episodes_fts), e.timestamp DESC, e.id DESC LIMIT ?");
    values.push(SqlValue::Integer(sqlite_limit(limit)));

    let mut stmt = conn.prepare(&sql)?;
    let rows = stmt.query_map(params_from_iter(values), episode_json)?;
    rows.collect::<rusqlite::Result<Vec<_>>>()
        .context("reading l2 episodes by FTS")
}

fn read_l2_recent_entries_for_read(
    conn: &Connection,
    user_id: &str,
    query: &MemoryReadQuery,
    limit: usize,
    include_archived: bool,
) -> Result<Vec<Value>> {
    let mut sql = String::from(
        r#"
        SELECT id, session_id, user_id, timestamp, episode_type, role, content,
               metadata, tags, sources, entry_id, keep_flag, archived_flag,
               tombstoned_flag
        FROM episodes
        WHERE tombstoned_flag = 0
        "#,
    );
    let mut values = Vec::new();
    if let Some(session_id) = query.session_id.as_deref().and_then(non_blank) {
        sql.push_str(" AND session_id = ?");
        values.push(SqlValue::Text(session_id.trim().to_string()));
    } else {
        sql.push_str(" AND user_id = ?");
        values.push(SqlValue::Text(user_id.to_string()));
    }
    if !include_archived {
        sql.push_str(" AND archived_flag = 0");
    }
    sql.push_str(" ORDER BY timestamp DESC, id DESC LIMIT ?");
    values.push(SqlValue::Integer(sqlite_limit(limit)));

    let mut stmt = conn.prepare(&sql)?;
    let rows = stmt.query_map(params_from_iter(values), episode_json)?;
    rows.collect::<rusqlite::Result<Vec<_>>>()
        .context("reading recent l2 episodes")
}

fn read_l3_entries(
    conn: &Connection,
    user_id: &str,
    query: &MemoryReadQuery,
    limit: usize,
    include_archived: bool,
) -> Result<Vec<Value>> {
    if !table_exists(conn, "semantic_facts")? {
        return Ok(Vec::new());
    }
    if let Some(entry_id) = query.id.as_deref().and_then(non_blank) {
        return read_l3_by_entry_id(conn, user_id, entry_id, include_archived);
    }
    if let Some(text) = query.text.as_deref().and_then(non_blank) {
        return read_l3_fts_entries(conn, user_id, query, text, limit, include_archived);
    }
    read_l3_recent_entries_for_read(conn, user_id, limit, include_archived)
}

fn read_l3_by_entry_id(
    conn: &Connection,
    user_id: &str,
    entry_id: &str,
    include_archived: bool,
) -> Result<Vec<Value>> {
    let mut sql = String::from(
        r#"
        SELECT id, user_id, fact_type, content, source, confidence, created_at,
               access_count, metadata, tags, sources, entry_id, keep_flag,
               archived_flag, tombstoned_flag
        FROM semantic_facts
        WHERE user_id = ?1
          AND entry_id = ?2
          AND tombstoned_flag = 0
        "#,
    );
    if !include_archived {
        sql.push_str(" AND archived_flag = 0");
    }
    sql.push_str(" LIMIT 1");

    let mut stmt = conn.prepare(&sql)?;
    let rows = stmt.query_map(params![user_id, entry_id], semantic_fact_json)?;
    rows.collect::<rusqlite::Result<Vec<_>>>()
        .context("reading l3 semantic fact by entry id")
}

fn read_l3_fts_entries(
    conn: &Connection,
    user_id: &str,
    query: &MemoryReadQuery,
    text: &str,
    limit: usize,
    include_archived: bool,
) -> Result<Vec<Value>> {
    if !table_exists(conn, "semantic_fts")? {
        return Ok(Vec::new());
    }
    let Some(fts_query) = normalize_fts_query_with_match(text, &query.match_mode) else {
        return Ok(Vec::new());
    };
    let min_confidence = query.min_confidence.unwrap_or(0.0);
    let mut sql = String::from(
        r#"
        SELECT f.id, f.user_id, f.fact_type, f.content, f.source, f.confidence,
               f.created_at, f.access_count, f.metadata, f.tags, f.sources,
               f.entry_id, f.keep_flag, f.archived_flag, f.tombstoned_flag
        FROM semantic_fts
        JOIN semantic_facts f ON f.id = semantic_fts.rowid
        WHERE semantic_fts MATCH ?
          AND f.confidence >= ?
          AND f.user_id = ?
          AND f.tombstoned_flag = 0
        "#,
    );
    let mut values = vec![
        SqlValue::Text(fts_query),
        SqlValue::Real(min_confidence),
        SqlValue::Text(user_id.to_string()),
    ];
    if !include_archived {
        sql.push_str(" AND f.archived_flag = 0");
    }
    push_optional_keyword_condition(&mut sql, &mut values, "f.fact_type", query.l3_kind());
    sql.push_str(" ORDER BY f.confidence DESC, bm25(semantic_fts), f.id DESC LIMIT ?");
    values.push(SqlValue::Integer(sqlite_limit(limit)));

    let mut stmt = conn.prepare(&sql)?;
    let rows = stmt.query_map(params_from_iter(values), semantic_fact_json)?;
    rows.collect::<rusqlite::Result<Vec<_>>>()
        .context("reading l3 semantic facts by FTS")
}

fn read_l3_recent_entries_for_read(
    conn: &Connection,
    user_id: &str,
    limit: usize,
    include_archived: bool,
) -> Result<Vec<Value>> {
    let mut sql = String::from(
        r#"
        SELECT id, user_id, fact_type, content, source, confidence, created_at,
               access_count, metadata, tags, sources, entry_id, keep_flag,
               archived_flag, tombstoned_flag
        FROM semantic_facts
        WHERE user_id = ?
          AND tombstoned_flag = 0
        "#,
    );
    let mut values = vec![SqlValue::Text(user_id.to_string())];
    if !include_archived {
        sql.push_str(" AND archived_flag = 0");
    }
    sql.push_str(" ORDER BY updated_at DESC, id DESC LIMIT ?");
    values.push(SqlValue::Integer(sqlite_limit(limit)));

    let mut stmt = conn.prepare(&sql)?;
    let rows = stmt.query_map(params_from_iter(values), semantic_fact_json)?;
    rows.collect::<rusqlite::Result<Vec<_>>>()
        .context("reading recent l3 semantic facts")
}

fn memory_read_report(
    layer: &str,
    entries: Vec<Value>,
    include_archived: bool,
    include_l1_note: bool,
) -> Value {
    json!({
        "layer": layer,
        "count": entries.len(),
        "entries": entries,
        "include-archived?": include_archived,
        "l1-persisted?": false,
        "l1-live-skipped?": include_l1_note,
    })
}

fn recall_select_entries(entries: Vec<Value>, keys: &[&str]) -> Vec<Value> {
    entries
        .into_iter()
        .map(|entry| recall_select_entry(entry, keys))
        .collect()
}

fn recall_select_entry(entry: Value, keys: &[&str]) -> Value {
    let Value::Object(object) = entry else {
        return Value::Null;
    };
    let mut selected = Map::new();
    for key in keys {
        if let Some(value) = object.get(*key) {
            selected.insert((*key).to_string(), value.clone());
        }
    }
    Value::Object(selected)
}

fn push_optional_text_condition(
    sql: &mut String,
    values: &mut Vec<SqlValue>,
    column: &str,
    value: Option<&str>,
) {
    let Some(value) = value.and_then(non_blank) else {
        return;
    };
    sql.push_str(" AND ");
    sql.push_str(column);
    sql.push_str(" = ?");
    values.push(SqlValue::Text(value.trim().to_string()));
}

fn push_optional_keyword_condition(
    sql: &mut String,
    values: &mut Vec<SqlValue>,
    column: &str,
    value: Option<&str>,
) {
    let Some(value) = value.and_then(non_blank) else {
        return;
    };
    sql.push_str(" AND ");
    sql.push_str(column);
    sql.push_str(" = ?");
    values.push(SqlValue::Text(normalize_keywordish(value)));
}

fn push_optional_time_bound(
    sql: &mut String,
    values: &mut Vec<SqlValue>,
    column: &str,
    operator: &str,
    value: Option<&str>,
) {
    let Some(value) = value.and_then(non_blank) else {
        return;
    };
    sql.push_str(" AND ");
    sql.push_str(column);
    sql.push(' ');
    sql.push_str(operator);
    sql.push_str(" ?");
    values.push(SqlValue::Text(value.trim().to_string()));
}

fn query_string(value: &Value, keys: &[&str]) -> Option<String> {
    let object = value.as_object()?;
    keys.iter()
        .filter_map(|key| object_get_keyish(object, key))
        .filter_map(json_value_to_string)
        .find_map(non_blank_owned)
}

fn query_f64(value: &Value, keys: &[&str]) -> Option<f64> {
    let object = value.as_object()?;
    keys.iter()
        .filter_map(|key| object_get_keyish(object, key))
        .find_map(|value| match value {
            Value::Number(number) => number.as_f64(),
            Value::String(text) => text.trim().parse::<f64>().ok(),
            _ => None,
        })
}

fn object_get_keyish<'a>(object: &'a Map<String, Value>, key: &str) -> Option<&'a Value> {
    object
        .get(key)
        .or_else(|| object.get(&key.replace('-', "_")))
        .or_else(|| object.get(&key.replace('_', "-")))
        .or_else(|| object.get(&format!(":{key}")))
}

fn json_value_to_string(value: &Value) -> Option<String> {
    match value {
        Value::Null => None,
        Value::String(value) => Some(value.clone()),
        Value::Number(value) => Some(value.to_string()),
        Value::Bool(value) => Some(value.to_string()),
        _ => None,
    }
}

fn non_blank_owned(value: String) -> Option<String> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        None
    } else {
        Some(trimmed.to_string())
    }
}

fn normalize_recall_layer(layer: Option<&str>) -> Option<&'static str> {
    let normalized = normalize_keywordish(layer.unwrap_or_default());
    match normalized.as_str() {
        "l1" => Some("l1"),
        "l2" => Some("l2"),
        "l3" => Some("l3"),
        _ => None,
    }
}

fn normalize_recall_match(match_mode: &str) -> &'static str {
    match normalize_keywordish(match_mode).as_str() {
        "and" => "and",
        "phrase" => "phrase",
        _ => "or",
    }
}

fn normalize_keywordish(value: &str) -> String {
    value
        .trim()
        .trim_start_matches(':')
        .trim_matches('"')
        .to_lowercase()
}

fn non_blank(value: &str) -> Option<&str> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        None
    } else {
        Some(trimmed)
    }
}

fn rrf_score(entry: &Value) -> f64 {
    entry
        .get("_rrf_score")
        .and_then(Value::as_f64)
        .unwrap_or(0.0)
}

fn layer_order(entry: &Value) -> i32 {
    match entry.get("_layer").and_then(Value::as_str) {
        Some("l3") => 0,
        Some("l2") => 1,
        _ => 2,
    }
}

fn recall_id(entry: &Value) -> String {
    entry
        .get("id")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

fn db_sessions(conn: &Connection, user_id: &str) -> Result<BTreeSet<String>> {
    if !table_exists(conn, "episodes")? {
        return Ok(BTreeSet::new());
    }
    let mut stmt = conn.prepare(
        "SELECT DISTINCT session_id FROM episodes
         WHERE user_id = ?1 AND tombstoned_flag = 0 AND keep_flag = 0
         ORDER BY session_id ASC",
    )?;
    let rows = stmt.query_map([user_id], |row| row.get::<_, String>(0))?;
    rows.collect::<rusqlite::Result<BTreeSet<_>>>()
        .context("reading memory DB sessions")
}

fn live_sessions_on_disk(root: &Path) -> Result<BTreeSet<String>> {
    let Ok(entries) = fs::read_dir(root) else {
        return Ok(BTreeSet::new());
    };
    let mut sessions = BTreeSet::new();
    for entry in entries {
        let entry = entry?;
        let file_type = entry.file_type()?;
        if !file_type.is_dir() {
            continue;
        }
        let name = entry.file_name().to_string_lossy().to_string();
        if !name.starts_with('.') {
            sessions.insert(name);
        }
    }
    Ok(sessions)
}

fn orphan_l2_episodes(
    conn: &Connection,
    user_id: &str,
    orphan_sids: &[String],
    cap: usize,
) -> Result<Value> {
    if cap == 0 || orphan_sids.is_empty() || !table_exists(conn, "episodes")? {
        return Ok(Value::Array(Vec::new()));
    }
    let placeholders = std::iter::repeat_n("?", orphan_sids.len())
        .collect::<Vec<_>>()
        .join(", ");
    let sql = format!(
        "SELECT COALESCE(entry_id, CAST(id AS TEXT)), session_id, timestamp, content
         FROM episodes
         WHERE user_id = ?
           AND tombstoned_flag = 0
           AND keep_flag = 0
           AND session_id IN ({placeholders})
         ORDER BY timestamp ASC, id ASC
         LIMIT ?"
    );
    let mut values = Vec::with_capacity(orphan_sids.len() + 2);
    values.push(SqlValue::Text(user_id.to_string()));
    values.extend(orphan_sids.iter().cloned().map(SqlValue::Text));
    values.push(SqlValue::Integer(sqlite_limit(cap)));

    let mut stmt = conn.prepare(&sql)?;
    let rows = stmt.query_map(params_from_iter(values), |row| {
        let content: String = row.get(3)?;
        Ok(json!({
            "entry-id": row.get::<_, String>(0)?,
            "session-id": row.get::<_, String>(1)?,
            "timestamp": row.get::<_, Option<String>>(2)?,
            "content-snippet": truncate_chars(&content, 80),
        }))
    })?;
    rows.collect::<rusqlite::Result<Vec<_>>>()
        .map(Value::Array)
        .context("reading orphan L2 episodes")
}

fn l3_stale_facts(conn: &Connection, user_id: &str, stale_days: i64, cap: usize) -> Result<Value> {
    if cap == 0 || !table_exists(conn, "semantic_facts")? {
        return Ok(Value::Array(Vec::new()));
    }
    let mut stmt = conn.prepare(
        "SELECT COALESCE(entry_id, CAST(id AS TEXT)), content, confidence, last_accessed
         FROM semantic_facts
         WHERE user_id = ?1
           AND tombstoned_flag = 0
           AND archived_flag = 0
           AND keep_flag = 0
           AND COALESCE(confidence, 1.0) < 0.5
           AND last_accessed IS NOT NULL
           AND last_accessed < datetime('now', '-' || ?2 || ' days')
         ORDER BY last_accessed ASC, id ASC
         LIMIT ?3",
    )?;
    let rows = stmt.query_map(params![user_id, stale_days, sqlite_limit(cap)], |row| {
        Ok(json!({
            "entry-id": row.get::<_, String>(0)?,
            "content": row.get::<_, String>(1)?,
            "confidence": row.get::<_, Option<f64>>(2)?.unwrap_or(1.0),
            "last-accessed": row.get::<_, Option<String>>(3)?,
            "reason": "stale",
        }))
    })?;
    rows.collect::<rusqlite::Result<Vec<_>>>()
        .map(Value::Array)
        .context("reading stale L3 facts")
}

fn truncate_chars(value: &str, max_chars: usize) -> String {
    value.chars().take(max_chars).collect()
}

fn audit_rows_for_turn(
    conn: &Connection,
    session_id: &str,
    agent_id: Option<&str>,
    turn_id: i64,
) -> Result<Vec<MemoryAuditRow>> {
    let sql = if agent_id.is_some() {
        r#"
        SELECT id, user_id, session_id, agent_id, turn_id, total_turns,
               entry_id, layer, byte_cost, created_at
        FROM memory_audit
        WHERE session_id = ?1 AND agent_id = ?2 AND turn_id = ?3
        ORDER BY id ASC
        "#
    } else {
        r#"
        SELECT id, user_id, session_id, agent_id, turn_id, total_turns,
               entry_id, layer, byte_cost, created_at
        FROM memory_audit
        WHERE session_id = ?1 AND turn_id = ?2
        ORDER BY id ASC
        "#
    };
    let mut stmt = conn.prepare(sql)?;
    let rows = if let Some(agent_id) = agent_id {
        stmt.query_map(params![session_id, agent_id, turn_id], audit_row_from_sql)?
            .collect::<rusqlite::Result<Vec<_>>>()
    } else {
        stmt.query_map(params![session_id, turn_id], audit_row_from_sql)?
            .collect::<rusqlite::Result<Vec<_>>>()
    }?;
    Ok(rows)
}

fn audit_rows_for_session(conn: &Connection, session_id: &str) -> Result<Vec<MemoryAuditRow>> {
    let mut stmt = conn.prepare(
        r#"
        SELECT id, user_id, session_id, agent_id, turn_id, total_turns,
               entry_id, layer, byte_cost, created_at
        FROM memory_audit
        WHERE session_id = ?1
        ORDER BY total_turns ASC, agent_id ASC, turn_id ASC, id ASC
        "#,
    )?;
    let rows = stmt
        .query_map([session_id], audit_row_from_sql)?
        .collect::<rusqlite::Result<Vec<_>>>()
        .context("reading memory audit rows for session")?;
    Ok(rows)
}

fn audit_row_from_sql(row: &rusqlite::Row<'_>) -> rusqlite::Result<MemoryAuditRow> {
    Ok(MemoryAuditRow {
        id: row.get(0)?,
        user_id: row.get(1)?,
        session_id: row.get(2)?,
        agent_id: row.get(3)?,
        turn_id: row.get(4)?,
        total_turns: row.get(5)?,
        entry_id: row.get(6)?,
        layer: row.get(7)?,
        byte_cost: row.get(8)?,
        created_at: row.get(9)?,
    })
}

fn explain_turn_json(
    session_id: &str,
    agent_id: Option<&str>,
    turn_id: i64,
    entries: Vec<Value>,
) -> Value {
    json!({
        "session-id": session_id,
        "agent-id": agent_id,
        "turn-id": turn_id,
        "user-id": Value::Null,
        "entries": entries,
        "prompt-bytes": 0,
        "recall-query": Value::Null,
    })
}

fn explain_turn_json_with_entries(
    session_id: &str,
    agent_id: Option<&str>,
    turn_id: i64,
    rows: &[MemoryAuditRow],
    entries: Vec<Value>,
) -> Value {
    json!({
        "session-id": session_id,
        "agent-id": agent_id,
        "turn-id": turn_id,
        "user-id": rows.first().map(|row| row.user_id.as_str()),
        "entries": entries,
        "prompt-bytes": prompt_bytes(rows),
        "recall-query": Value::Null,
    })
}

fn explain_items_json(conn: &Connection, rows: &[MemoryAuditRow]) -> Result<Vec<Value>> {
    rows.iter()
        .map(|row| {
            Ok(json!({
                "audit": audit_row_json(row),
                "entry": hydrate_audit_entry(conn, row)?,
            }))
        })
        .collect()
}

fn audit_row_json(row: &MemoryAuditRow) -> Value {
    json!({
        "id": row.id,
        "user_id": row.user_id,
        "session_id": row.session_id,
        "agent_id": row.agent_id,
        "turn_id": row.turn_id,
        "total_turns": row.total_turns,
        "entry_id": row.entry_id,
        "layer": row.layer,
        "byte_cost": row.byte_cost,
        "created_at": row.created_at,
    })
}

fn hydrate_audit_entry(conn: &Connection, row: &MemoryAuditRow) -> Result<Option<Value>> {
    match row.layer.as_str() {
        "l2" => hydrate_episode(conn, &row.user_id, &row.entry_id),
        "l3" => hydrate_semantic_fact(conn, &row.user_id, &row.entry_id),
        _ => Ok(None),
    }
}

fn hydrate_episode(conn: &Connection, user_id: &str, entry_id: &str) -> Result<Option<Value>> {
    if !table_exists(conn, "episodes")? {
        return Ok(None);
    }
    let sql = r#"
        SELECT id, session_id, user_id, timestamp, episode_type, role, content,
               metadata, tags, sources, entry_id, keep_flag, archived_flag,
               tombstoned_flag
        FROM episodes
        WHERE user_id = ?1 AND entry_id = ?2
        LIMIT 1
    "#;
    if let Some(entry) = optional_query_row(conn, sql, params![user_id, entry_id], episode_json)? {
        return Ok(Some(entry));
    }

    let Some(db_id) = parse_db_id(entry_id) else {
        return Ok(None);
    };
    let sql = r#"
        SELECT id, session_id, user_id, timestamp, episode_type, role, content,
               metadata, tags, sources, entry_id, keep_flag, archived_flag,
               tombstoned_flag
        FROM episodes
        WHERE user_id = ?1 AND id = ?2
        LIMIT 1
    "#;
    optional_query_row(conn, sql, params![user_id, db_id], episode_json)
}

fn hydrate_semantic_fact(
    conn: &Connection,
    user_id: &str,
    entry_id: &str,
) -> Result<Option<Value>> {
    if !table_exists(conn, "semantic_facts")? {
        return Ok(None);
    }
    let sql = r#"
        SELECT id, user_id, fact_type, content, source, confidence, created_at,
               access_count, metadata, tags, sources, entry_id, keep_flag,
               archived_flag, tombstoned_flag
        FROM semantic_facts
        WHERE user_id = ?1 AND entry_id = ?2
        LIMIT 1
    "#;
    if let Some(entry) =
        optional_query_row(conn, sql, params![user_id, entry_id], semantic_fact_json)?
    {
        return Ok(Some(entry));
    }

    let Some(db_id) = parse_db_id(entry_id) else {
        return Ok(None);
    };
    let sql = r#"
        SELECT id, user_id, fact_type, content, source, confidence, created_at,
               access_count, metadata, tags, sources, entry_id, keep_flag,
               archived_flag, tombstoned_flag
        FROM semantic_facts
        WHERE user_id = ?1 AND id = ?2
        LIMIT 1
    "#;
    optional_query_row(conn, sql, params![user_id, db_id], semantic_fact_json)
}

fn optional_query_row<P, F>(
    conn: &Connection,
    sql: &str,
    params: P,
    mapper: F,
) -> Result<Option<Value>>
where
    P: rusqlite::Params,
    F: FnOnce(&rusqlite::Row<'_>) -> rusqlite::Result<Value>,
{
    match conn.query_row(sql, params, mapper) {
        Ok(value) => Ok(Some(value)),
        Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
        Err(err) => Err(err).context("hydrating memory audit entry"),
    }
}

fn episode_json(row: &rusqlite::Row<'_>) -> rusqlite::Result<Value> {
    let db_id: i64 = row.get(0)?;
    let session_id: String = row.get(1)?;
    let user_id: String = row.get(2)?;
    let created_at: Option<String> = row.get(3)?;
    let kind: String = row.get(4)?;
    let role: Option<String> = row.get(5)?;
    let content: String = row.get(6)?;
    let metadata: Option<String> = row.get(7)?;
    let tags: Option<String> = row.get(8)?;
    let sources: Option<String> = row.get(9)?;
    let entry_id: Option<String> = row.get(10)?;
    let keep_flag: i64 = row.get(11)?;
    let archived_flag: i64 = row.get(12)?;
    let tombstoned_flag: i64 = row.get(13)?;
    let metadata_parts = unpack_metadata(metadata.as_deref());

    Ok(json!({
        "id": entry_id.unwrap_or_else(|| db_id.to_string()),
        "db-id": db_id,
        "layer": "l2",
        "kind": kind,
        "content": content,
        "role": role,
        "session-id": session_id,
        "user-id": user_id,
        "created-at": created_at,
        "tags": json_array_or_empty(tags.as_deref()),
        "sources": json_array_or_empty(sources.as_deref()),
        "ttl": metadata_parts.get("ttl").cloned().unwrap_or(Value::Null),
        "data": metadata_parts.get("data").cloned().unwrap_or(Value::Null),
        "metadata": metadata_parts.get("metadata").cloned().unwrap_or(Value::Null),
        "keep": sqlite_bool(keep_flag),
        "archived": sqlite_bool(archived_flag),
        "tombstoned": sqlite_bool(tombstoned_flag),
    }))
}

fn semantic_fact_json(row: &rusqlite::Row<'_>) -> rusqlite::Result<Value> {
    let db_id: i64 = row.get(0)?;
    let user_id: String = row.get(1)?;
    let kind: String = row.get(2)?;
    let content: String = row.get(3)?;
    let source: Option<String> = row.get(4)?;
    let confidence: Option<f64> = row.get(5)?;
    let created_at: Option<String> = row.get(6)?;
    let access_count: Option<i64> = row.get(7)?;
    let metadata: Option<String> = row.get(8)?;
    let tags: Option<String> = row.get(9)?;
    let sources: Option<String> = row.get(10)?;
    let entry_id: Option<String> = row.get(11)?;
    let keep_flag: i64 = row.get(12)?;
    let archived_flag: i64 = row.get(13)?;
    let tombstoned_flag: i64 = row.get(14)?;
    let metadata_parts = unpack_metadata(metadata.as_deref());

    Ok(json!({
        "id": entry_id.unwrap_or_else(|| db_id.to_string()),
        "db-id": db_id,
        "layer": "l3",
        "kind": kind,
        "content": content,
        "source": source,
        "confidence": confidence.unwrap_or(1.0),
        "access-count": access_count.unwrap_or(0),
        "user-id": user_id,
        "created-at": created_at,
        "tags": json_array_or_empty(tags.as_deref()),
        "sources": json_array_or_empty(sources.as_deref()),
        "ttl": metadata_parts.get("ttl").cloned().unwrap_or(Value::Null),
        "data": metadata_parts.get("data").cloned().unwrap_or(Value::Null),
        "metadata": metadata_parts.get("metadata").cloned().unwrap_or(Value::Null),
        "keep": sqlite_bool(keep_flag),
        "archived": sqlite_bool(archived_flag),
        "tombstoned": sqlite_bool(tombstoned_flag),
    }))
}

fn unpack_metadata(raw: Option<&str>) -> Map<String, Value> {
    let Some(raw) = raw else {
        return Map::new();
    };
    serde_json::from_str::<Map<String, Value>>(raw).unwrap_or_default()
}

fn json_array_or_empty(raw: Option<&str>) -> Value {
    let Some(raw) = raw else {
        return Value::Array(Vec::new());
    };
    match serde_json::from_str::<Value>(raw) {
        Ok(Value::Array(values)) => Value::Array(values),
        _ => Value::Array(Vec::new()),
    }
}

fn prompt_bytes(rows: &[MemoryAuditRow]) -> i64 {
    rows.iter().filter_map(|row| row.byte_cost).sum()
}

fn sqlite_bool(value: i64) -> bool {
    value != 0
}

fn parse_db_id(value: &str) -> Option<i64> {
    value.parse().ok()
}

const MEMORY_TABLES: &[&str] = &[
    "memory_metadata",
    "episodes",
    "episodes_fts",
    "semantic_facts",
    "semantic_fts",
    "memory_audit",
];

fn memory_table_stats(conn: &Connection, table: &str) -> Result<MemoryTableStats> {
    let present = table_exists(conn, table)?;
    let rows = if present {
        Some(count_static_table_rows(conn, table)?)
    } else {
        None
    };
    Ok(MemoryTableStats {
        name: table.to_string(),
        present,
        rows,
    })
}

fn memory_schema_version(conn: &Connection) -> Result<Option<String>> {
    if !table_exists(conn, "memory_metadata")? {
        return Ok(None);
    }

    let mut stmt =
        conn.prepare("SELECT value FROM memory_metadata WHERE key = 'schema_version'")?;
    match stmt.query_row([], |row| row.get(0)) {
        Ok(version) => Ok(Some(version)),
        Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
        Err(err) => Err(err).context("reading memory schema version"),
    }
}

fn table_exists(conn: &Connection, table: &str) -> Result<bool> {
    conn.query_row(
        "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type = 'table' AND name = ?1)",
        [table],
        |row| row.get::<_, bool>(0),
    )
    .with_context(|| format!("checking sqlite table {table}"))
}

fn count_static_table_rows(conn: &Connection, table: &str) -> Result<i64> {
    let sql = match table {
        "memory_metadata" => "SELECT COUNT(*) FROM memory_metadata",
        "episodes" => "SELECT COUNT(*) FROM episodes",
        "episodes_fts" => "SELECT COUNT(*) FROM episodes_fts",
        "semantic_facts" => "SELECT COUNT(*) FROM semantic_facts",
        "semantic_fts" => "SELECT COUNT(*) FROM semantic_fts",
        "memory_audit" => "SELECT COUNT(*) FROM memory_audit",
        _ => unreachable!("memory inspect only counts static table names"),
    };
    conn.query_row(sql, [], |row| row.get(0))
        .with_context(|| format!("counting rows in sqlite table {table}"))
}

fn count_where<P>(conn: &Connection, sql: &str, params: P) -> i64
where
    P: rusqlite::Params,
{
    conn.query_row(sql, params, |row| row.get::<_, i64>(0))
        .unwrap_or(0)
}

fn l2_min_max_timestamp(conn: &Connection, user_id: &str) -> (Option<String>, Option<String>) {
    conn.query_row(
        "SELECT MIN(timestamp), MAX(timestamp) FROM episodes WHERE user_id = ?1 AND tombstoned_flag = 0",
        [user_id],
        |row| Ok((row.get(0)?, row.get(1)?)),
    )
    .unwrap_or((None, None))
}

fn l3_by_kind(conn: &Connection, user_id: &str) -> Value {
    let mut kinds = Map::new();
    let Ok(mut stmt) = conn.prepare(
        "SELECT fact_type, COUNT(*) FROM semantic_facts
         WHERE user_id = ?1 AND tombstoned_flag = 0
         GROUP BY fact_type
         ORDER BY fact_type ASC",
    ) else {
        return Value::Object(kinds);
    };
    let Ok(rows) = stmt.query_map([user_id], |row| {
        Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?))
    }) else {
        return Value::Object(kinds);
    };
    for (kind, count) in rows.flatten() {
        kinds.insert(kind, json!(count));
    }
    Value::Object(kinds)
}

fn l3_confidence_buckets(conn: &Connection, user_id: &str) -> Value {
    conn.query_row(
        "SELECT
           SUM(CASE WHEN confidence >= 0.8 THEN 1 ELSE 0 END),
           SUM(CASE WHEN confidence >= 0.5 AND confidence < 0.8 THEN 1 ELSE 0 END),
           SUM(CASE WHEN confidence < 0.5 THEN 1 ELSE 0 END)
         FROM semantic_facts
         WHERE user_id = ?1 AND tombstoned_flag = 0",
        [user_id],
        |row| {
            Ok(json!({
                "high": row.get::<_, Option<i64>>(0)?.unwrap_or(0),
                "medium": row.get::<_, Option<i64>>(1)?.unwrap_or(0),
                "low": row.get::<_, Option<i64>>(2)?.unwrap_or(0),
            }))
        },
    )
    .unwrap_or_else(|_| json!({"high": 0, "medium": 0, "low": 0}))
}

fn file_size_bytes(path: &Path) -> Option<u64> {
    std::fs::metadata(path).ok().map(|metadata| metadata.len())
}

fn sqlite_limit(limit: usize) -> i64 {
    limit.min(i64::MAX as usize) as i64
}

fn normalize_fts_query(query: &str) -> Option<String> {
    normalize_fts_query_with_match(query, "or")
}

fn normalize_fts_query_with_match(query: &str, match_mode: &str) -> Option<String> {
    let words = query
        .split(|ch: char| ch.is_whitespace() || is_fts_special_char(ch))
        .filter_map(normalize_word)
        .collect::<Vec<_>>();

    match words.len() {
        0 => None,
        1 => words.into_iter().next(),
        _ => match normalize_recall_match(match_mode) {
            "phrase" => Some(format!("\"{}\"", words.join(" ").replace('"', "\"\""))),
            "and" => Some(words.join(" ")),
            _ => Some(words.join(" OR ")),
        },
    }
}

fn normalize_word(word: &str) -> Option<String> {
    let trimmed = word.trim();
    if trimmed.is_empty() {
        return None;
    }

    let upper = trimmed.to_ascii_uppercase();
    if matches!(upper.as_str(), "AND" | "OR" | "NOT" | "NEAR") {
        Some(trimmed.to_ascii_lowercase())
    } else {
        Some(trimmed.to_string())
    }
}

fn is_fts_special_char(ch: char) -> bool {
    matches!(
        ch,
        '#' | '@'
            | '$'
            | '%'
            | '^'
            | '&'
            | '*'
            | '('
            | ')'
            | '-'
            | '+'
            | '='
            | '['
            | ']'
            | '{'
            | '}'
            | '|'
            | '\\'
            | '/'
            | '<'
            | '>'
            | ':'
            | '"'
            | '\''
            | '~'
            | '`'
            | ','
            | '.'
            | '!'
            | '?'
            | ';'
    )
}

fn is_stop_word(word: &str) -> bool {
    matches!(
        word,
        "a" | "an"
            | "the"
            | "and"
            | "or"
            | "but"
            | "in"
            | "on"
            | "at"
            | "to"
            | "for"
            | "of"
            | "with"
            | "by"
            | "from"
            | "as"
            | "is"
            | "was"
            | "are"
            | "were"
            | "be"
            | "been"
            | "being"
            | "have"
            | "has"
            | "had"
            | "do"
            | "does"
            | "did"
            | "will"
            | "would"
            | "could"
            | "should"
            | "may"
            | "might"
            | "shall"
            | "can"
            | "need"
            | "dare"
            | "ought"
            | "used"
            | "it"
            | "its"
            | "this"
            | "that"
            | "these"
            | "those"
            | "i"
            | "me"
            | "my"
            | "we"
            | "our"
            | "you"
            | "your"
            | "he"
            | "him"
            | "his"
            | "she"
            | "her"
            | "they"
            | "them"
            | "their"
            | "what"
            | "which"
            | "who"
            | "whom"
            | "where"
            | "when"
            | "why"
            | "how"
            | "all"
            | "each"
            | "every"
            | "both"
            | "few"
            | "more"
            | "most"
            | "other"
            | "some"
            | "such"
            | "no"
            | "nor"
            | "not"
            | "only"
            | "own"
            | "same"
            | "so"
            | "than"
            | "too"
            | "very"
            | "just"
            | "about"
            | "above"
            | "after"
            | "again"
            | "also"
            | "any"
            | "because"
            | "before"
            | "below"
            | "between"
            | "during"
            | "into"
            | "out"
            | "over"
            | "then"
            | "there"
            | "through"
            | "under"
            | "until"
            | "up"
            | "while"
            | "if"
            | "else"
            | "here"
            | "am"
            | "let"
            | "get"
            | "got"
    )
}
