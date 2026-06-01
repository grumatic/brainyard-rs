#![forbid(unsafe_code)]

use anyhow::{anyhow, Context, Result};
use serde::Deserialize;
use serde_json::{json, Value};
use std::path::Path;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RegistryFixture {
    pub tools: Vec<ToolDescriptor>,
    pub agents: Vec<AgentDescriptor>,
    pub models: Vec<ModelDescriptor>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ToolDescriptor {
    pub id: String,
    pub tool_type: String,
    pub description: Option<String>,
    pub input_schema: Value,
    pub output_schema: Value,
    pub aliases: Vec<String>,
    pub tool_use_control: Option<Value>,
    pub agent_tools: Option<Value>,
    pub config_schema: Option<Value>,
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

pub fn load_tools_path(path: impl AsRef<Path>) -> Result<Vec<ToolDescriptor>> {
    let path = path.as_ref();
    let raw = std::fs::read_to_string(path)
        .with_context(|| format!("failed to read tools fixture {}", path.display()))?;
    load_tools_str(&raw)
}

pub fn load_registry_str(input: &str) -> Result<RegistryFixture> {
    let value: Value = serde_json::from_str(input).context("invalid registry JSON")?;
    match value {
        Value::Array(items) => Ok(RegistryFixture {
            tools: Vec::new(),
            agents: items
                .into_iter()
                .map(raw_agent_from_value)
                .collect::<Result<Vec<_>>>()?,
            models: Vec::new(),
        }),
        Value::Object(mut object) => {
            let tools = take_array(&mut object, "tools")?
                .into_iter()
                .map(raw_tool_from_value)
                .collect::<Result<Vec<_>>>()?;
            let agents = take_array(&mut object, "agents")?
                .into_iter()
                .map(raw_agent_from_value)
                .collect::<Result<Vec<_>>>()?;
            let models = take_array(&mut object, "models")?
                .into_iter()
                .map(raw_model_from_value)
                .collect::<Result<Vec<_>>>()?;
            Ok(RegistryFixture {
                tools,
                agents,
                models,
            })
        }
        _ => Err(anyhow!("registry fixture must be a JSON object or array")),
    }
}

pub fn load_tools_str(input: &str) -> Result<Vec<ToolDescriptor>> {
    let value: Value = serde_json::from_str(input).context("invalid tools JSON")?;
    match value {
        Value::Array(items) => items
            .into_iter()
            .map(raw_tool_from_value)
            .collect::<Result<Vec<_>>>(),
        Value::Object(mut object) => take_array(&mut object, "tools")?
            .into_iter()
            .map(raw_tool_from_value)
            .collect::<Result<Vec<_>>>(),
        _ => Err(anyhow!("tools fixture must be a JSON object or array")),
    }
}

fn take_array(object: &mut serde_json::Map<String, Value>, key: &str) -> Result<Vec<Value>> {
    match object.remove(key) {
        Some(Value::Array(items)) => Ok(items),
        Some(_) => Err(anyhow!("registry field '{key}' must be an array")),
        None => Ok(Vec::new()),
    }
}

fn raw_tool_from_value(value: Value) -> Result<ToolDescriptor> {
    let raw: RawTool = serde_json::from_value(value).context("invalid tool descriptor")?;
    let id = raw
        .id
        .ok_or_else(|| anyhow!("tool descriptor requires 'id'"))?;
    Ok(ToolDescriptor {
        id,
        tool_type: raw.tool_type.unwrap_or_else(|| "tool".to_string()),
        description: raw.description,
        input_schema: raw.input_schema.unwrap_or_else(|| json!(["map"])),
        output_schema: raw.output_schema.unwrap_or_else(|| json!(["map"])),
        aliases: raw.aliases.unwrap_or_default(),
        tool_use_control: raw.tool_use_control,
        agent_tools: raw.agent_tools,
        config_schema: raw.config_schema,
    })
}

fn raw_agent_from_value(value: Value) -> Result<AgentDescriptor> {
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

fn raw_model_from_value(value: Value) -> Result<ModelDescriptor> {
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
struct RawTool {
    id: Option<String>,
    #[serde(rename = "type")]
    tool_type: Option<String>,
    description: Option<String>,
    #[serde(rename = "inputSchema", alias = "input-schema")]
    input_schema: Option<Value>,
    #[serde(rename = "outputSchema", alias = "output-schema")]
    output_schema: Option<Value>,
    aliases: Option<Vec<String>>,
    #[serde(rename = "toolUseControl", alias = "tool-use-control")]
    tool_use_control: Option<Value>,
    #[serde(rename = "agentTools", alias = "agent-tools")]
    agent_tools: Option<Value>,
    #[serde(rename = "configSchema", alias = "config-schema")]
    config_schema: Option<Value>,
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
