#![forbid(unsafe_code)]

use anyhow::Result;
use by_contracts::{parse_map, EdnMap, EdnValue};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

#[derive(Clone, Debug, PartialEq)]
pub struct ConfigDocument {
    pub raw: EdnMap,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct BrainyardDirs {
    pub user_dir: Option<PathBuf>,
    pub project_dir: PathBuf,
    pub working_dir: PathBuf,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct LlmConfig {
    pub default_provider: Option<String>,
    pub default_model: Option<String>,
    pub available_providers: Vec<String>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AgentConfig {
    pub default_agent: Option<String>,
    pub max_iterations: Option<usize>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PermissionsConfig {
    pub mode: Option<String>,
    pub allowed_dirs: Vec<String>,
}

pub const USER_ID_FALLBACK: &str = "by-user";

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct UserIdInputs<'a> {
    pub explicit: Option<&'a str>,
    pub by_user_id_env: Option<&'a str>,
    pub by_user_id_property: Option<&'a str>,
    pub os_user_name: Option<&'a str>,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct DotenvValues {
    pub values: BTreeMap<String, String>,
    pub loaded_paths: Vec<DotenvLoadedPath>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DotenvLoadedPath {
    pub path: PathBuf,
    pub keys: Vec<String>,
}

impl DotenvValues {
    pub fn get(&self, key: &str) -> Option<&str> {
        self.values.get(key).map(String::as_str)
    }
}

impl ConfigDocument {
    pub fn empty() -> Self {
        Self {
            raw: EdnMap::default(),
        }
    }

    pub fn llm(&self) -> LlmConfig {
        let llm = match self.raw.get("llm") {
            Some(EdnValue::Map(map)) => Some(map),
            _ => None,
        };

        LlmConfig {
            default_provider: llm.and_then(|map| text_value(map.get("default-provider"))),
            default_model: llm.and_then(|map| text_value(map.get("default-model"))),
            available_providers: llm
                .and_then(|map| vector_text_values(map.get("available-providers")))
                .unwrap_or_default(),
        }
    }

    pub fn agent(&self) -> AgentConfig {
        let agent = match self.raw.get("agent") {
            Some(EdnValue::Map(map)) => Some(map),
            _ => None,
        };

        AgentConfig {
            default_agent: agent.and_then(|map| text_value(map.get("default-agent"))),
            max_iterations: agent.and_then(agent_max_iterations),
        }
    }

    pub fn permissions(&self) -> PermissionsConfig {
        let agent_config = match self.raw.get("agent") {
            Some(EdnValue::Map(agent)) => match agent.get("config") {
                Some(EdnValue::Map(config)) => Some(config),
                _ => None,
            },
            _ => None,
        };
        let permissions = match self.raw.get("permissions") {
            Some(EdnValue::Map(map)) => Some(map),
            _ => None,
        };

        PermissionsConfig {
            mode: permissions
                .and_then(|map| text_value(map.get("mode")))
                .or_else(|| agent_config.and_then(|map| text_value(map.get("permission-mode")))),
            allowed_dirs: permissions
                .and_then(|map| vector_text_values(map.get("allowed-dirs")))
                .or_else(|| {
                    agent_config.and_then(|map| vector_text_values(map.get("allowed-dirs")))
                })
                .unwrap_or_default(),
        }
    }
}

pub fn read_config(path: impl AsRef<Path>) -> Result<ConfigDocument> {
    let raw = std::fs::read_to_string(path)?;
    Ok(ConfigDocument {
        raw: parse_map(&raw)?,
    })
}

impl BrainyardDirs {
    pub fn resolve(
        working_dir: impl AsRef<Path>,
        user_dir: Option<impl AsRef<Path>>,
        project_dir_override: Option<impl AsRef<Path>>,
    ) -> Self {
        let working_dir = working_dir.as_ref().to_path_buf();
        let user_dir = user_dir.map(|path| path.as_ref().to_path_buf());
        let project_dir = project_dir_override
            .map(|path| path.as_ref().to_path_buf())
            .or_else(|| find_git_root(&working_dir))
            .unwrap_or_else(|| working_dir.clone());

        Self {
            user_dir,
            project_dir,
            working_dir,
        }
    }
}

pub fn resolve_default_config_path(dirs: &BrainyardDirs) -> Option<PathBuf> {
    let project_config = dirs.project_dir.join(".brainyard/config.edn");
    if project_config.is_file() {
        return Some(project_config);
    }

    dirs.user_dir
        .as_ref()
        .map(|user_dir| user_dir.join(".brainyard/config.edn"))
}

pub fn user_config_dir(dirs: &BrainyardDirs) -> Option<PathBuf> {
    dirs.user_dir
        .as_ref()
        .map(|user_dir| user_dir.join(".brainyard"))
}

pub fn project_config_dir(dirs: &BrainyardDirs) -> PathBuf {
    dirs.project_dir.join(".brainyard")
}

pub fn default_allowed_dirs(dirs: &BrainyardDirs) -> Vec<PathBuf> {
    let mut paths = Vec::new();
    push_distinct_path(&mut paths, PathBuf::from("/tmp"));
    push_distinct_path(&mut paths, dirs.project_dir.clone());
    if let Some(user_config_dir) = user_config_dir(dirs) {
        push_distinct_path(&mut paths, user_config_dir);
    }
    paths
}

pub fn default_memory_db_path(dirs: &BrainyardDirs, user_id: &str) -> Option<PathBuf> {
    dirs.user_dir.as_ref().map(|user_dir| {
        user_dir
            .join(".brainyard/memory")
            .join(format!("{user_id}.db"))
    })
}

pub fn default_sessions_root(dirs: &BrainyardDirs) -> Option<PathBuf> {
    dirs.user_dir
        .as_ref()
        .map(|user_dir| user_dir.join(".brainyard/sessions"))
}

pub fn resolve_user_id(inputs: UserIdInputs<'_>) -> String {
    first_non_blank([
        inputs.explicit,
        inputs.by_user_id_env,
        inputs.by_user_id_property,
        inputs.os_user_name,
        Some(USER_ID_FALLBACK),
    ])
    .expect("USER_ID_FALLBACK is non-blank")
}

pub fn resolve_process_user_id(explicit: Option<&str>) -> String {
    let dotenv = load_process_dotenv().unwrap_or_default();
    resolve_process_user_id_with_dotenv(explicit, &dotenv)
}

pub fn resolve_process_user_id_with_dotenv(
    explicit: Option<&str>,
    dotenv: &DotenvValues,
) -> String {
    let by_user_id = process_env_or_dotenv(dotenv, "BY_USER_ID");
    let os_user_name = std::env::var("USER")
        .ok()
        .or_else(|| std::env::var("USERNAME").ok());

    resolve_user_id(UserIdInputs {
        explicit,
        by_user_id_env: by_user_id.as_deref(),
        by_user_id_property: None,
        os_user_name: os_user_name.as_deref(),
    })
}

pub fn process_env_or_dotenv(dotenv: &DotenvValues, key: &str) -> Option<String> {
    match std::env::var_os(key) {
        Some(value) => Some(value.to_string_lossy().into_owned()),
        None => dotenv.get(key).map(ToOwned::to_owned),
    }
}

pub fn load_process_dotenv() -> Result<DotenvValues> {
    let working_dir = std::env::current_dir()?;
    let home_dir = std::env::var_os("HOME").map(PathBuf::from);
    let explicit_env_file = std::env::var_os("BY_ENV_FILE")
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
        .filter(|path| path.is_file());
    let skip = std::env::var_os("BY_NO_DOTENV").is_some_and(|value| !value.is_empty());

    load_dotenv_values(
        &working_dir,
        home_dir.as_deref(),
        explicit_env_file.as_deref(),
        skip,
        &|key| std::env::var_os(key).is_some(),
    )
}

pub fn load_dotenv_values(
    working_dir: &Path,
    home_dir: Option<&Path>,
    explicit_env_file: Option<&Path>,
    skip: bool,
    env_contains_key: &dyn Fn(&str) -> bool,
) -> Result<DotenvValues> {
    if skip {
        return Ok(DotenvValues::default());
    }

    let mut values = BTreeMap::new();
    let mut loaded_paths = Vec::new();

    for path in dotenv_candidate_paths(working_dir, home_dir, explicit_env_file) {
        if !path.is_file() {
            continue;
        }

        let raw = std::fs::read_to_string(&path)?;
        let mut keys = Vec::new();
        for line in raw.lines() {
            let Some((key, value)) = parse_dotenv_line(line) else {
                continue;
            };
            if values.contains_key(&key) || env_contains_key(&key) {
                continue;
            }
            values.insert(key.clone(), value);
            keys.push(key);
        }
        if !keys.is_empty() {
            loaded_paths.push(DotenvLoadedPath { path, keys });
        }
    }

    Ok(DotenvValues {
        values,
        loaded_paths,
    })
}

fn first_non_blank<'a>(values: impl IntoIterator<Item = Option<&'a str>>) -> Option<String> {
    values
        .into_iter()
        .flatten()
        .map(str::trim)
        .find(|value| !value.is_empty())
        .map(ToOwned::to_owned)
}

fn dotenv_candidate_paths(
    working_dir: &Path,
    home_dir: Option<&Path>,
    explicit_env_file: Option<&Path>,
) -> Vec<PathBuf> {
    if let Some(explicit_env_file) = explicit_env_file {
        return vec![explicit_env_file.to_path_buf()];
    }

    let mut paths = Vec::new();
    let mut dir = Some(working_dir);
    while let Some(current) = dir {
        push_distinct_path(&mut paths, current.join(".env"));
        dir = current.parent();
    }
    if let Some(home_dir) = home_dir {
        push_distinct_path(&mut paths, home_dir.join(".brainyard/.env"));
    }
    paths
}

fn push_distinct_path(paths: &mut Vec<PathBuf>, path: PathBuf) {
    if !paths.iter().any(|existing| existing == &path) {
        paths.push(path);
    }
}

fn parse_dotenv_line(line: &str) -> Option<(String, String)> {
    let trimmed = line.trim();
    if trimmed.is_empty() || trimmed.starts_with('#') {
        return None;
    }

    let (key, value) = trimmed.split_once('=')?;
    let key = key.trim();
    let key = key.strip_prefix("export ").unwrap_or(key).trim();
    if key.is_empty() {
        return None;
    }

    let value = strip_matching_quotes(value.trim()).to_string();
    Some((key.to_string(), value))
}

fn strip_matching_quotes(value: &str) -> &str {
    if value.len() < 2 {
        return value;
    }

    let bytes = value.as_bytes();
    if matches!(
        (bytes.first(), bytes.last()),
        (Some(b'"'), Some(b'"')) | (Some(b'\''), Some(b'\''))
    ) {
        &value[1..value.len() - 1]
    } else {
        value
    }
}

fn find_git_root(start_dir: &Path) -> Option<PathBuf> {
    let mut dir = Some(start_dir);
    while let Some(current) = dir {
        if current.join(".git").is_dir() {
            return Some(current.to_path_buf());
        }
        dir = current.parent();
    }
    None
}

fn text_value(value: Option<&EdnValue>) -> Option<String> {
    match value {
        Some(EdnValue::String(value))
        | Some(EdnValue::Keyword(value))
        | Some(EdnValue::Symbol(value)) => Some(value.clone()),
        _ => None,
    }
}

fn agent_max_iterations(agent: &EdnMap) -> Option<usize> {
    // Match the Clojure migration path: legacy [:agent :max-iterations]
    // is relocated over [:agent :config :max-iterations] when both exist.
    usize_value(agent.get("max-iterations")).or_else(|| {
        let Some(EdnValue::Map(config)) = agent.get("config") else {
            return None;
        };
        usize_value(config.get("max-iterations"))
    })
}

fn usize_value(value: Option<&EdnValue>) -> Option<usize> {
    match value {
        Some(EdnValue::Integer(value)) => (*value).try_into().ok(),
        _ => None,
    }
}

fn vector_text_values(value: Option<&EdnValue>) -> Option<Vec<String>> {
    match value {
        Some(EdnValue::Vector(values)) => Some(
            values
                .iter()
                .filter_map(|value| text_value(Some(value)))
                .collect(),
        ),
        _ => None,
    }
}
