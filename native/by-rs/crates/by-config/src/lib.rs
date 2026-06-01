#![forbid(unsafe_code)]

use anyhow::Result;
use by_contracts::{parse_map, EdnMap, EdnValue};
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
