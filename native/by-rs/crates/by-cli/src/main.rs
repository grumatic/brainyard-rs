#![forbid(unsafe_code)]

use anyhow::{bail, Context, Result};
use clap::{Parser, Subcommand};
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

#[derive(Debug, Parser)]
#[command(name = "by-rs")]
#[command(about = "Brainyard Rust companion CLI")]
#[command(version = env!("BY_BUILD_VERSION"))]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Debug, Subcommand)]
enum Commands {
    /// Ask a one-shot question. Bedrock supports dry-run shaping or explicit live calls.
    Ask {
        /// Agent ID. Reserved for later full agent execution parity.
        #[arg(long, short = 'a', default_value = "coact-agent", value_name = "ID")]
        agent: String,
        /// LM provider. Only bedrock is implemented in by-rs for now.
        #[arg(
            long,
            short = 'p',
            default_value = "claude-code",
            value_name = "PROVIDER"
        )]
        provider: String,
        /// Model name override. Required for Bedrock when config does not provide a default.
        #[arg(long, short = 'm', value_name = "MODEL")]
        model: Option<String>,
        /// Max agent iterations. Reserved for later full agent execution parity.
        #[arg(long, short = 'n', value_name = "N")]
        max_iterations: Option<usize>,
        /// AWS region for Bedrock. Falls back to AWS_REGION, AWS_DEFAULT_REGION, then us-east-1.
        #[arg(long, value_name = "REGION")]
        region: Option<String>,
        /// AWS profile for Bedrock. Falls back to AWS_PROFILE, then AWS_DEFAULT_PROFILE.
        #[arg(long = "aws-profile", value_name = "PROFILE")]
        aws_profile: Option<String>,
        /// Bedrock inference temperature. Dropped automatically for incompatible models.
        #[arg(long, default_value_t = 0.0, value_name = "FLOAT")]
        temperature: f64,
        /// Bedrock maxTokens value.
        #[arg(long = "max-tokens", value_name = "N")]
        max_tokens: Option<u32>,
        /// Disable Bedrock prompt-cache cachePoint shaping.
        #[arg(long)]
        no_prompt_cache: bool,
        /// Print the provider request JSON without network access.
        #[arg(long)]
        dry_run: bool,
        /// Call Bedrock Converse over the network.
        #[arg(long)]
        live: bool,
        /// Question to ask. Also accepts the legacy positional provider:model token.
        #[arg(value_name = "QUESTION", num_args = 1..)]
        question: Vec<String>,
    },
    /// List agents from a registry fixture.
    Agents {
        /// Path to an exported registry JSON fixture.
        #[arg(long, value_name = "PATH")]
        fixture: PathBuf,
    },
    /// List models from a registry fixture.
    Models {
        /// Path to an exported registry JSON fixture.
        #[arg(long, value_name = "PATH")]
        fixture: PathBuf,
        /// Filter to a single provider, e.g. bedrock or openai.
        #[arg(long, value_name = "PROVIDER")]
        provider: Option<String>,
    },
    /// List tool and command descriptors from a registry fixture.
    Tools {
        /// Path to an exported registry or tools JSON fixture.
        #[arg(long, value_name = "PATH")]
        fixture: PathBuf,
        /// Filter to a single tool type, e.g. tool, command, agent, or skill.
        #[arg(long = "type", value_name = "TYPE")]
        tool_type: Option<String>,
        /// Filter to one exact tool ID.
        #[arg(long, value_name = "ID")]
        id: Option<String>,
    },
    /// Inspect Brainyard configuration without mutating it.
    Config {
        #[command(subcommand)]
        command: ConfigCommand,
    },
    /// Search Brainyard memory SQLite stores without mutating them.
    Memory {
        #[command(subcommand)]
        command: MemoryCommand,
    },
    /// Inspect Brainyard session directories.
    Sessions {
        #[command(subcommand)]
        command: SessionCommand,
    },
    /// Render local TUI previews without entering the alternate screen.
    Tui {
        #[command(subcommand)]
        command: TuiCommand,
    },
}

#[derive(Debug, Subcommand)]
enum ConfigCommand {
    /// Show selected config defaults from config.edn.
    Show {
        /// Config file path. Defaults to ~/.brainyard/config.edn.
        #[arg(long, value_name = "PATH")]
        path: Option<PathBuf>,
    },
}

#[derive(Debug, Subcommand)]
enum MemoryCommand {
    /// Print read-only SQLite memory schema and row-count metadata.
    Inspect {
        /// Brainyard memory SQLite database path.
        #[arg(long, value_name = "PATH")]
        db: PathBuf,
    },
    /// Search L2 episodes and L3 semantic facts using compatible FTS5 tables.
    Search {
        /// Brainyard memory SQLite database path.
        #[arg(long, value_name = "PATH")]
        db: PathBuf,
        /// Natural-language query. Multi-word queries default to OR recall.
        #[arg(long, value_name = "TEXT")]
        query: String,
        /// Maximum number of combined hits to print.
        #[arg(long, default_value_t = 20, value_name = "N")]
        limit: usize,
    },
}

#[derive(Debug, Subcommand)]
enum SessionCommand {
    /// List session directories without mutating them.
    List {
        /// Session root. Defaults to ~/.brainyard/sessions.
        #[arg(long, value_name = "PATH")]
        root: Option<PathBuf>,
    },
    /// Delete one persisted session directory.
    Prune {
        /// Session root. Defaults to ~/.brainyard/sessions.
        #[arg(long, value_name = "PATH")]
        root: Option<PathBuf>,
        /// Session ID to delete.
        #[arg(long = "session-id", short = 's', value_name = "ID")]
        session_id: Option<String>,
        /// Session ID to delete.
        #[arg(value_name = "SESSION_ID")]
        positional_session_id: Option<String>,
    },
}

#[derive(Debug, Subcommand)]
enum TuiCommand {
    /// Print a static compatibility snapshot of the fullscreen chrome.
    Snapshot {
        /// Agent label shown in the scroll area.
        #[arg(long, default_value = "coact-agent", value_name = "ID")]
        agent: String,
        /// Provider/model label shown in the scroll area.
        #[arg(
            long,
            default_value = "bedrock:amazon.nova-lite-v1:0",
            value_name = "MODEL"
        )]
        model: String,
        /// Snapshot rows.
        #[arg(long, default_value_t = 24, value_name = "N")]
        rows: usize,
        /// Snapshot columns.
        #[arg(long, default_value_t = 80, value_name = "N")]
        cols: usize,
    },
}

fn main() {
    if let Err(err) = run() {
        eprintln!("error: {err:#}");
        std::process::exit(1);
    }
}

fn run() -> Result<()> {
    let cli = Cli::parse();
    match cli.command {
        Commands::Ask {
            agent: _agent,
            provider,
            model,
            max_iterations: _max_iterations,
            region,
            aws_profile,
            temperature,
            max_tokens,
            no_prompt_cache,
            dry_run,
            live,
            question,
        } => print_ask(AskRequest {
            provider,
            model,
            region,
            aws_profile,
            temperature,
            max_tokens,
            no_prompt_cache,
            dry_run,
            live,
            question,
        }),
        Commands::Agents { fixture } => print_agents(fixture),
        Commands::Models { fixture, provider } => print_models(fixture, provider),
        Commands::Tools {
            fixture,
            tool_type,
            id,
        } => print_tools(fixture, tool_type, id),
        Commands::Config { command } => match command {
            ConfigCommand::Show { path } => print_config(path),
        },
        Commands::Memory { command } => match command {
            MemoryCommand::Inspect { db } => print_memory_inspect(db),
            MemoryCommand::Search { db, query, limit } => print_memory_search(db, query, limit),
        },
        Commands::Sessions { command } => match command {
            SessionCommand::List { root } => print_sessions(root),
            SessionCommand::Prune {
                root,
                session_id,
                positional_session_id,
            } => prune_session(root, session_id, positional_session_id),
        },
        Commands::Tui { command } => match command {
            TuiCommand::Snapshot {
                agent,
                model,
                rows,
                cols,
            } => print_tui_snapshot(agent, model, rows, cols),
        },
    }
}

#[derive(Debug)]
struct AskRequest {
    provider: String,
    model: Option<String>,
    region: Option<String>,
    aws_profile: Option<String>,
    temperature: f64,
    max_tokens: Option<u32>,
    no_prompt_cache: bool,
    dry_run: bool,
    live: bool,
    question: Vec<String>,
}

fn print_ask(args: AskRequest) -> Result<()> {
    if args.dry_run && args.live {
        bail!("choose only one of --dry-run or --live");
    }
    if !args.dry_run && !args.live {
        bail!("by-rs ask requires --dry-run or --live");
    }

    let (provider, model, question) =
        resolve_ask_positionals(args.provider, args.model, args.question)?;

    let llm_config = read_default_llm_config()?;
    let resolved_provider = match provider.as_str() {
        "claude-code" => llm_config
            .as_ref()
            .and_then(|config| config.default_provider.clone())
            .unwrap_or(provider),
        _ => provider,
    };
    let model = model
        .or_else(|| {
            llm_config
                .as_ref()
                .and_then(|config| config.default_model.clone())
        })
        .context("--model is required for bedrock ask")?;

    if resolved_provider != "bedrock" {
        bail!("by-rs ask currently supports provider 'bedrock' only");
    }

    let runtime = by_llm::resolve_bedrock_runtime_options(by_llm::BedrockRuntimeInputs {
        explicit_region: args.region,
        aws_region: std::env::var("AWS_REGION").ok(),
        aws_default_region: std::env::var("AWS_DEFAULT_REGION").ok(),
        explicit_profile: args.aws_profile,
        aws_profile: std::env::var("AWS_PROFILE").ok(),
        aws_default_profile: std::env::var("AWS_DEFAULT_PROFILE").ok(),
    });

    let config = by_llm::BedrockConfig {
        model: model.clone(),
        temperature: Some(args.temperature),
        max_tokens: args.max_tokens,
        prompt_cache: !args.no_prompt_cache && by_llm::bedrock_supports_prompt_cache(&model),
        drop_temperature: by_llm::bedrock_drops_temperature(&model),
    };
    let messages = vec![by_llm::ChatMessage::user(question)];

    if args.dry_run {
        let request = by_llm::build_bedrock_request(&config, &messages);
        let dry_run = serde_json::json!({
            "provider": "bedrock",
            "operation": "Converse",
            "network": false,
            "region": runtime.region,
            "aws_profile": runtime.aws_profile,
            "request": request,
        });
        println!("{}", serde_json::to_string_pretty(&dry_run)?);
        return Ok(());
    }

    let response = tokio::runtime::Runtime::new()
        .context("failed to create Tokio runtime for Bedrock live call")?
        .block_on(by_llm::converse_bedrock(by_llm::BedrockConverseRequest {
            config,
            runtime,
            messages,
            cache_zones: Vec::new(),
        }))?;

    if response.text.is_empty() {
        println!("{}", serde_json::to_string_pretty(&response.projected)?);
    } else {
        println!("{}", response.text);
    }
    Ok(())
}

fn resolve_ask_positionals(
    provider: String,
    model: Option<String>,
    mut positionals: Vec<String>,
) -> Result<(String, Option<String>, String)> {
    let (provider, model) = match take_legacy_provider_model(&mut positionals) {
        Some((legacy_provider, legacy_model)) => (legacy_provider, Some(legacy_model)),
        None => (provider, model),
    };
    let question = positionals
        .into_iter()
        .next()
        .context("question argument is required")?;
    Ok((provider, model, question))
}

fn take_legacy_provider_model(positionals: &mut Vec<String>) -> Option<(String, String)> {
    let index = positionals
        .iter()
        .position(|value| legacy_provider_model(value))?;
    let token = positionals.remove(index);
    let (provider, model) = token.split_once(':')?;
    Some((provider.to_string(), model.to_string()))
}

fn legacy_provider_model(value: &str) -> bool {
    let Some((provider, model)) = value.split_once(':') else {
        return false;
    };
    if model.contains(':') {
        return false;
    }

    provider_token(provider) && model_token(model)
}

fn provider_token(value: &str) -> bool {
    let mut chars = value.chars();
    matches!(chars.next(), Some(ch) if ch.is_ascii_lowercase())
        && chars.all(provider_or_model_rest_char)
}

fn model_token(value: &str) -> bool {
    let mut chars = value.chars();
    matches!(chars.next(), Some(ch) if ch.is_ascii_lowercase() || ch.is_ascii_digit())
        && chars.all(provider_or_model_rest_char)
}

fn provider_or_model_rest_char(ch: char) -> bool {
    ch.is_ascii_lowercase() || ch.is_ascii_digit() || matches!(ch, '.' | '_' | '-')
}

fn print_agents(fixture: PathBuf) -> Result<()> {
    let registry = by_registry::load_registry_path(&fixture)?;
    if registry.agents.is_empty() {
        println!("No agents registered.");
        return Ok(());
    }

    println!("{} agent(s) available:", registry.agents.len());
    print_agents_table(registry.agents);
    Ok(())
}

fn print_models(fixture: PathBuf, provider: Option<String>) -> Result<()> {
    let registry = by_registry::load_registry_path(&fixture)?;
    if registry.models.is_empty() {
        println!("No models registered.");
        return Ok(());
    }

    let provider_filter = provider.as_deref();
    let listed = print_models_table(registry.models, provider_filter);
    if let Some(provider) = provider_filter.filter(|_| listed == 0) {
        println!("No models found for provider: {provider}");
    } else {
        println!(
            "{} model(s) listed.{}",
            listed,
            provider_filter
                .map(|provider| format!(" (filtered to {provider})"))
                .unwrap_or_default()
        );
    }
    Ok(())
}

fn print_tools(fixture: PathBuf, tool_type: Option<String>, id: Option<String>) -> Result<()> {
    let tools = by_registry::load_tools_path(&fixture)?;
    if tools.is_empty() {
        println!("No tools registered.");
        return Ok(());
    }

    let listed = print_tools_table(tools, tool_type.as_deref(), id.as_deref());
    if listed == 0 {
        match (tool_type.as_deref(), id.as_deref()) {
            (_, Some(id)) => println!("No tools found for id: {id}"),
            (Some(tool_type), None) => println!("No tools found for type: {tool_type}"),
            (None, None) => println!("No tools registered."),
        }
    } else {
        println!(
            "{} tool(s) listed.{}{}",
            listed,
            tool_type
                .as_deref()
                .map(|tool_type| format!(" (filtered to type {tool_type})"))
                .unwrap_or_default(),
            id.as_deref()
                .map(|id| format!(" (filtered to id {id})"))
                .unwrap_or_default()
        );
    }
    Ok(())
}

fn model_id_with_region(model: &by_registry::ModelDescriptor) -> String {
    match model.region.as_deref() {
        Some(region) if !region.is_empty() => format!("{} ({})", model.id, region),
        _ => model.id.clone(),
    }
}

fn print_agents_table(mut agents: Vec<by_registry::AgentDescriptor>) {
    agents.sort_by(|left, right| left.id.cmp(&right.id));
    let id_width = agents
        .iter()
        .map(|agent| agent.id.chars().count())
        .max()
        .unwrap_or(10)
        .max(10);

    println!();
    println!("  {:<id_width$}  DESCRIPTION", "AGENT");
    println!("  {:<id_width$}  -----------", "-".repeat(id_width));
    for agent in agents {
        let description = truncate_lines(agent.description.as_deref().unwrap_or(""), 2);
        let description = indent_continuation_lines(&description, id_width + 4);
        println!("  {:<id_width$}  {}", agent.id, description);
    }
    println!();
}

fn print_models_table(
    models: Vec<by_registry::ModelDescriptor>,
    provider_filter: Option<&str>,
) -> usize {
    let mut rows: Vec<_> = models
        .into_iter()
        .filter(|model| provider_filter.is_none_or(|provider| provider == model.provider))
        .map(|model| {
            let model_id = model_id_with_region(&model);
            (
                model.provider,
                model_id,
                model.description.unwrap_or_default(),
            )
        })
        .collect();
    rows.sort_by(|left, right| left.0.cmp(&right.0).then(left.1.cmp(&right.1)));

    let provider_width = rows
        .iter()
        .map(|row| row.0.chars().count())
        .max()
        .unwrap_or(10)
        .max(10);
    let model_width = rows
        .iter()
        .map(|row| row.1.chars().count())
        .max()
        .unwrap_or(18)
        .max(18);

    println!();
    println!(
        "  {:<provider_width$}  {:<model_width$}  DESCRIPTION",
        "PROVIDER", "MODEL"
    );
    println!(
        "  {:<provider_width$}  {:<model_width$}  -----------",
        "-".repeat(provider_width),
        "-".repeat(model_width)
    );
    for (provider, model, description) in &rows {
        println!(
            "  {:<provider_width$}  {:<model_width$}  {}",
            provider,
            model,
            truncate_description(description, 60)
        );
    }
    println!();

    rows.len()
}

fn print_tools_table(
    tools: Vec<by_registry::ToolDescriptor>,
    tool_type_filter: Option<&str>,
    id_filter: Option<&str>,
) -> usize {
    let mut rows: Vec<_> = tools
        .into_iter()
        .filter(|tool| tool_type_filter.is_none_or(|tool_type| tool_type == tool.tool_type))
        .filter(|tool| id_filter.is_none_or(|id| id == tool.id))
        .map(|tool| {
            (
                tool.id,
                tool.tool_type,
                tool.description.unwrap_or_default(),
            )
        })
        .collect();
    rows.sort_by(|left, right| left.1.cmp(&right.1).then(left.0.cmp(&right.0)));

    let tool_width = rows
        .iter()
        .map(|row| row.0.chars().count())
        .max()
        .unwrap_or(12)
        .max(12);
    let type_width = rows
        .iter()
        .map(|row| row.1.chars().count())
        .max()
        .unwrap_or(8)
        .max(8);

    println!();
    println!(
        "  {:<tool_width$}  {:<type_width$}  DESCRIPTION",
        "TOOL", "TYPE"
    );
    println!(
        "  {:<tool_width$}  {:<type_width$}  -----------",
        "-".repeat(tool_width),
        "-".repeat(type_width)
    );
    for (id, tool_type, description) in &rows {
        println!(
            "  {:<tool_width$}  {:<type_width$}  {}",
            id,
            tool_type,
            truncate_description(description, 80)
        );
    }
    println!();

    rows.len()
}

fn truncate_lines(value: &str, max_lines: usize) -> String {
    let mut lines: Vec<_> = value
        .lines()
        .enumerate()
        .map(|(index, line)| {
            if index == 0 {
                line.to_string()
            } else {
                line.trim_start().to_string()
            }
        })
        .collect();
    if lines.len() <= max_lines {
        return lines.join("\n");
    }

    lines.truncate(max_lines);
    if let Some(last) = lines.last_mut() {
        *last = format!("{} ...", last.trim_end());
    }
    lines.join("\n")
}

fn indent_continuation_lines(value: &str, continuation_width: usize) -> String {
    value
        .lines()
        .enumerate()
        .map(|(index, line)| {
            if index == 0 {
                line.to_string()
            } else {
                format!("{}{}", " ".repeat(continuation_width), line)
            }
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn truncate_description(value: &str, max_chars: usize) -> String {
    if value.chars().count() <= max_chars {
        return value.to_string();
    }
    let keep = max_chars.saturating_sub(3);
    format!("{}...", value.chars().take(keep).collect::<String>())
}

fn print_config(path: Option<PathBuf>) -> Result<()> {
    let explicit_path = path.is_some();
    let path = match path {
        Some(path) => path,
        None => default_config_path().context("could not determine default config path")?,
    };
    let config = if !explicit_path && !path.exists() {
        by_config::ConfigDocument::empty()
    } else {
        by_config::read_config(&path)?
    };
    let llm = config.llm();
    let agent = config.agent();
    println!(
        "agent.default-agent\t{}",
        agent.default_agent.unwrap_or_default()
    );
    println!(
        "llm.default-provider\t{}",
        llm.default_provider.unwrap_or_default()
    );
    println!(
        "llm.default-model\t{}",
        llm.default_model.unwrap_or_default()
    );
    println!(
        "llm.available-providers\t{}",
        llm.available_providers.join(",")
    );
    Ok(())
}

fn print_memory_search(db: PathBuf, query: String, limit: usize) -> Result<()> {
    let request = by_memory::MemorySearchRequest { query, limit };
    for hit in by_memory::search_memory(&db, request)? {
        println!("{}\t{}\t{}", hit.layer.as_str(), hit.kind, hit.content);
    }
    Ok(())
}

fn print_memory_inspect(db: PathBuf) -> Result<()> {
    let report = by_memory::inspect_memory(&db)?;
    println!("Memory database: {}", db.display());
    println!(
        "schema-version: {}",
        report.schema_version.as_deref().unwrap_or("-")
    );
    println!("sqlite-user-version: {}", report.sqlite_user_version);
    println!("journal-mode: {}", report.journal_mode);
    println!();
    println!("  {:<18}  {:<7}  ROWS", "TABLE", "PRESENT");
    println!("  {:<18}  {:<7}  ----", "------------------", "-------");
    for table in report.tables {
        println!(
            "  {:<18}  {:<7}  {}",
            table.name,
            if table.present { "yes" } else { "no" },
            table
                .rows
                .map(|rows| rows.to_string())
                .unwrap_or_else(|| "-".to_string())
        );
    }
    Ok(())
}

fn print_sessions(root: Option<PathBuf>) -> Result<()> {
    let root = match root {
        Some(root) => root,
        None => default_sessions_root().context("could not determine default session root")?,
    };
    let sessions = by_persist::list_sessions(&root)?;
    if sessions.is_empty() {
        println!("No persisted sessions.");
        return Ok(());
    }

    println!(
        "{:<30} {:<14} {:<18} {:<10} last-attached",
        "session-id", "label", "agent", "size"
    );
    println!("{}", "-".repeat(88));
    for session in sessions {
        let last = session
            .last_attached_at_millis
            .or(session.started_at_millis)
            .and_then(format_age_millis)
            .unwrap_or_else(|| "-".to_string());
        println!(
            "{:<30} {:<14} {:<18} {:<10} {}",
            session.id,
            session.label.unwrap_or_else(|| "-".to_string()),
            session.agent.unwrap_or_else(|| "-".to_string()),
            format_bytes(session.bytes),
            last
        );
    }
    Ok(())
}

fn prune_session(
    root: Option<PathBuf>,
    session_id: Option<String>,
    positional_session_id: Option<String>,
) -> Result<()> {
    let root = match root {
        Some(root) => root,
        None => default_sessions_root().context("could not determine default session root")?,
    };
    let target = positional_session_id
        .or(session_id)
        .context("Usage: by-rs sessions prune [--root PATH] <session-id>")?;

    if by_persist::delete_session_dir(&root, &target)? {
        println!("Deleted session: {target}");
    } else {
        println!("Session not found: {target}");
    }
    Ok(())
}

fn format_bytes(bytes: u64) -> String {
    const KB: u64 = 1024;
    const MB: u64 = 1024 * 1024;
    if bytes < KB {
        format!("{bytes} B")
    } else if bytes < MB {
        format!("{:.1} KB", bytes as f64 / KB as f64)
    } else {
        format!("{:.1} MB", bytes as f64 / MB as f64)
    }
}

fn format_age_millis(epoch_millis: i64) -> Option<String> {
    let now_millis = current_epoch_millis()?;
    let age = now_millis.saturating_sub(epoch_millis).max(0);
    let mins = age / 60_000;
    let hrs = mins / 60;
    let days = hrs / 24;

    Some(if days > 1 {
        format!("{days}d ago")
    } else if hrs > 1 {
        format!("{hrs}h ago")
    } else if mins > 1 {
        format!("{mins}m ago")
    } else {
        "just now".to_string()
    })
}

fn current_epoch_millis() -> Option<i64> {
    let duration = SystemTime::now().duration_since(UNIX_EPOCH).ok()?;
    i64::try_from(duration.as_millis()).ok()
}

fn print_tui_snapshot(agent: String, model: String, rows: usize, cols: usize) -> Result<()> {
    let frame = by_tui::StaticFrame {
        rows,
        cols,
        agent,
        model,
        status: "idle".to_string(),
    };
    println!("{}", by_tui::render_static_frame(&frame));
    Ok(())
}

fn read_default_llm_config() -> Result<Option<by_config::LlmConfig>> {
    let Some(path) = default_config_path() else {
        return Ok(None);
    };
    if !path.exists() {
        return Ok(None);
    }
    by_config::read_config(&path)
        .map(|config| Some(config.llm()))
        .with_context(|| format!("failed to read default config {}", path.display()))
}

fn default_config_path() -> Option<PathBuf> {
    let working_dir = std::env::current_dir().ok()?;
    let user_dir = std::env::var_os("HOME").map(PathBuf::from);
    let project_dir_override = std::env::var_os("BRAINYARD_PROJECT_DIR").map(PathBuf::from);
    let dirs = by_config::BrainyardDirs::resolve(working_dir, user_dir, project_dir_override);
    by_config::resolve_default_config_path(&dirs)
}

fn default_sessions_root() -> Option<PathBuf> {
    std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".brainyard/sessions"))
}
