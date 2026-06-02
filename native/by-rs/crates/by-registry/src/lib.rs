#![forbid(unsafe_code)]

use anyhow::{anyhow, Context, Result};
use serde::Deserialize;
use serde_json::{json, Value};
use std::path::Path;

pub const ORACLE_REGISTRY_JSON: &str = include_str!("../../../fixtures/oracle/registry.json");

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RegistryFixture {
    pub tools: Vec<ToolDescriptor>,
    pub agents: Vec<AgentDescriptor>,
    pub models: Vec<ModelDescriptor>,
    pub mcp_servers: Vec<McpServerDescriptor>,
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

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct McpServerDescriptor {
    pub name: String,
    pub transport: String,
    pub config: Value,
    pub enabled: bool,
    pub auto_register_tools: bool,
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

pub fn load_embedded_oracle_registry() -> Result<RegistryFixture> {
    load_registry_str(ORACLE_REGISTRY_JSON)
}

pub fn load_tools_path(path: impl AsRef<Path>) -> Result<Vec<ToolDescriptor>> {
    let path = path.as_ref();
    let raw = std::fs::read_to_string(path)
        .with_context(|| format!("failed to read tools fixture {}", path.display()))?;
    load_tools_str(&raw)
}

pub fn load_mcp_servers_path(path: impl AsRef<Path>) -> Result<Vec<McpServerDescriptor>> {
    let path = path.as_ref();
    let raw = std::fs::read_to_string(path)
        .with_context(|| format!("failed to read MCP servers fixture {}", path.display()))?;
    load_mcp_servers_str(&raw)
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
            mcp_servers: Vec::new(),
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
            let mcp_servers = take_mcp_servers(&mut object)?.unwrap_or_default();
            Ok(RegistryFixture {
                tools,
                agents,
                models,
                mcp_servers,
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

pub fn load_mcp_servers_str(input: &str) -> Result<Vec<McpServerDescriptor>> {
    let value: Value = serde_json::from_str(input).context("invalid MCP servers JSON")?;
    mcp_servers_from_value(value)
}

fn take_array(object: &mut serde_json::Map<String, Value>, key: &str) -> Result<Vec<Value>> {
    match object.remove(key) {
        Some(Value::Array(items)) => Ok(items),
        Some(_) => Err(anyhow!("registry field '{key}' must be an array")),
        None => Ok(Vec::new()),
    }
}

fn take_mcp_servers(
    object: &mut serde_json::Map<String, Value>,
) -> Result<Option<Vec<McpServerDescriptor>>> {
    for key in ["mcpServers", "mcp-servers", "mcp_servers"] {
        if let Some(value) = object.remove(key) {
            return mcp_servers_from_value(value).map(Some);
        }
    }
    Ok(None)
}

fn mcp_servers_from_value(value: Value) -> Result<Vec<McpServerDescriptor>> {
    match value {
        Value::Array(items) => items
            .into_iter()
            .map(raw_mcp_server_from_value)
            .collect::<Result<Vec<_>>>(),
        Value::Object(object) => object
            .into_iter()
            .map(|(name, value)| raw_mcp_server_from_named_value(name, value))
            .collect::<Result<Vec<_>>>(),
        _ => Err(anyhow!(
            "MCP servers fixture must be a JSON object or array"
        )),
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

fn raw_mcp_server_from_value(value: Value) -> Result<McpServerDescriptor> {
    raw_mcp_server_from_named_value(String::new(), value)
}

fn raw_mcp_server_from_named_value(
    fallback_name: String,
    value: Value,
) -> Result<McpServerDescriptor> {
    let raw: RawMcpServer =
        serde_json::from_value(value).context("invalid MCP server descriptor")?;
    let name = raw
        .name
        .or(raw.id)
        .filter(|value| !value.trim().is_empty())
        .unwrap_or(fallback_name);
    if name.trim().is_empty() {
        return Err(anyhow!("MCP server descriptor requires 'name' or 'id'"));
    }

    Ok(McpServerDescriptor {
        name,
        transport: raw
            .transport
            .filter(|value| !value.trim().is_empty())
            .ok_or_else(|| anyhow!("MCP server descriptor requires 'transport'"))?,
        config: raw.config.unwrap_or_else(|| json!({})),
        enabled: raw.enabled.unwrap_or(false),
        auto_register_tools: raw.auto_register_tools.unwrap_or(false),
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

#[derive(Debug, Deserialize)]
struct RawMcpServer {
    name: Option<String>,
    id: Option<String>,
    transport: Option<String>,
    config: Option<Value>,
    enabled: Option<bool>,
    #[serde(
        rename = "autoRegisterTools",
        alias = "auto-register-tools",
        alias = "auto_register_tools"
    )]
    auto_register_tools: Option<bool>,
}
