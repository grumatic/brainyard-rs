#![forbid(unsafe_code)]

use std::path::Path;

use anyhow::{Context, Result};
use rusqlite::{params, Connection, OpenFlags};

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

fn sqlite_limit(limit: usize) -> i64 {
    limit.min(i64::MAX as usize) as i64
}

fn normalize_fts_query(query: &str) -> Option<String> {
    let words = query
        .split(|ch: char| ch.is_whitespace() || is_fts_special_char(ch))
        .filter_map(normalize_word)
        .collect::<Vec<_>>();

    match words.len() {
        0 => None,
        1 => words.into_iter().next(),
        _ => Some(words.join(" OR ")),
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
