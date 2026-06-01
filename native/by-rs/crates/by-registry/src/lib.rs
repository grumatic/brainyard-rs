#![forbid(unsafe_code)]

use anyhow::{anyhow, Context, Result};
use serde::Deserialize;
use std::path::Path;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RegistryFixture {
    pub agents: Vec<AgentDescriptor>,
    pub models: Vec<ModelDescriptor>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AgentDescriptor {
    pub id: String,
    pub name: String,
    pub description: Option<String>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ModelDescriptor {
    pub provider: String,
    pub id: String,
    pub description: Option<String>,
    pub region: Option<String>,
}

impl ModelDescriptor {
    pub fn label(&self) -> String {
        format!("{}:{}", self.provider, self.id)
    }
}

pub fn load_registry_path(path: impl AsRef<Path>) -> Result<RegistryFixture> {
    let path = path.as_ref();
    let raw = std::fs::read_to_string(path)
        .with_context(|| format!("failed to read registry fixture {}", path.display()))?;
    load_registry_str(&raw)
}

pub fn load_registry_str(input: &str) -> Result<RegistryFixture> {
    let value: serde_json::Value = serde_json::from_str(input).context("invalid registry JSON")?;
    match value {
        serde_json::Value::Array(items) => Ok(RegistryFixture {
            agents: items
                .into_iter()
                .map(raw_agent_from_value)
                .collect::<Result<Vec<_>>>()?,
            models: Vec::new(),
        }),
        serde_json::Value::Object(mut object) => {
            let agents = take_array(&mut object, "agents")?
                .into_iter()
                .map(raw_agent_from_value)
                .collect::<Result<Vec<_>>>()?;
            let models = take_array(&mut object, "models")?
                .into_iter()
                .map(raw_model_from_value)
                .collect::<Result<Vec<_>>>()?;
            Ok(RegistryFixture { agents, models })
        }
        _ => Err(anyhow!("registry fixture must be a JSON object or array")),
    }
}

fn take_array(
    object: &mut serde_json::Map<String, serde_json::Value>,
    key: &str,
) -> Result<Vec<serde_json::Value>> {
    match object.remove(key) {
        Some(serde_json::Value::Array(items)) => Ok(items),
        Some(_) => Err(anyhow!("registry field '{key}' must be an array")),
        None => Ok(Vec::new()),
    }
}

fn raw_agent_from_value(value: serde_json::Value) -> Result<AgentDescriptor> {
    let raw: RawAgent = serde_json::from_value(value).context("invalid agent descriptor")?;
    let id = raw
        .id
        .or_else(|| raw.name.clone())
        .ok_or_else(|| anyhow!("agent descriptor requires 'id' or 'name'"))?;
    let name = raw.name.unwrap_or_else(|| id.clone());
    Ok(AgentDescriptor {
        id,
        name,
        description: raw.description,
    })
}

fn raw_model_from_value(value: serde_json::Value) -> Result<ModelDescriptor> {
    let raw: RawModel = serde_json::from_value(value).context("invalid model descriptor")?;
    Ok(ModelDescriptor {
        provider: raw
            .provider
            .ok_or_else(|| anyhow!("model descriptor requires 'provider'"))?,
        id: raw
            .id
            .or(raw.model)
            .ok_or_else(|| anyhow!("model descriptor requires 'id' or 'model'"))?,
        description: raw.description,
        region: raw.region,
    })
}

#[derive(Debug, Deserialize)]
struct RawAgent {
    id: Option<String>,
    name: Option<String>,
    description: Option<String>,
}

#[derive(Debug, Deserialize)]
struct RawModel {
    provider: Option<String>,
    id: Option<String>,
    model: Option<String>,
    description: Option<String>,
    region: Option<String>,
}
