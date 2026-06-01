#![forbid(unsafe_code)]

use anyhow::Result;
use by_contracts::{parse_map, EdnMap, EdnValue};
use std::path::Path;

#[derive(Clone, Debug, PartialEq)]
pub struct ConfigDocument {
    pub raw: EdnMap,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct LlmConfig {
    pub default_provider: Option<String>,
    pub default_model: Option<String>,
    pub available_providers: Vec<String>,
}

impl ConfigDocument {
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
}

pub fn read_config(path: impl AsRef<Path>) -> Result<ConfigDocument> {
    let raw = std::fs::read_to_string(path)?;
    Ok(ConfigDocument {
        raw: parse_map(&raw)?,
    })
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
