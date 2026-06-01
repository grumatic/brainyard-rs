#![forbid(unsafe_code)]

use anyhow::{bail, Context, Result};
use clap::{Parser, Subcommand};
use std::path::PathBuf;

#[derive(Debug, Parser)]
#[command(name = "by-rs")]
#[command(about = "Brainyard Rust companion CLI")]
#[command(version)]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Debug, Subcommand)]
enum Commands {
    /// Ask a one-shot question. Currently supports local dry-run request shaping.
    Ask {
        /// Agent ID. Reserved for later full agent execution parity.
        #[arg(long, short = 'a', default_value = "coact-agent", value_name = "ID")]
        agent: String,
        /// LM provider. Only bedrock dry-run is implemented in by-rs for now.
        #[arg(
            long,
            short = 'p',
            default_value = "claude-code",
            value_name = "PROVIDER"
        )]
        provider: String,
        /// Model name override. Required for Bedrock dry-run.
        #[arg(long, short = 'm', value_name = "MODEL")]
        model: Option<String>,
        /// Max agent iterations. Reserved for later full agent execution parity.
        #[arg(long, short = 'n', value_name = "N")]
        max_iterations: Option<usize>,
        /// Print the provider request JSON without network access.
        #[arg(long)]
        dry_run: bool,
        /// Question to ask.
        #[arg(value_name = "QUESTION")]
        question: String,
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
            dry_run,
            question,
        } => print_ask(provider, model, dry_run, question),
        Commands::Agents { fixture } => print_agents(fixture),
        Commands::Models { fixture, provider } => print_models(fixture, provider),
        Commands::Config { command } => match command {
            ConfigCommand::Show { path } => print_config(path),
        },
        Commands::Memory { command } => match command {
            MemoryCommand::Search { db, query, limit } => print_memory_search(db, query, limit),
        },
        Commands::Sessions { command } => match command {
            SessionCommand::List { root } => print_sessions(root),
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

fn print_ask(
    provider: String,
    model: Option<String>,
    dry_run: bool,
    question: String,
) -> Result<()> {
    if !dry_run {
        bail!("by-rs ask currently supports --dry-run only");
    }

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
        .context("--model is required for bedrock dry-run")?;

    if resolved_provider != "bedrock" {
        bail!("by-rs ask --dry-run currently supports provider 'bedrock' only");
    }

    let config = by_llm::BedrockConfig {
        model: model.clone(),
        temperature: Some(0.0),
        max_tokens: None,
        prompt_cache: by_llm::bedrock_supports_prompt_cache(&model),
        drop_temperature: by_llm::bedrock_drops_temperature(&model),
    };
    let request = by_llm::build_bedrock_request(&config, &[by_llm::ChatMessage::user(question)]);
    let dry_run = serde_json::json!({
        "provider": "bedrock",
        "operation": "Converse",
        "network": false,
        "request": request,
    });
    println!("{}", serde_json::to_string_pretty(&dry_run)?);
    Ok(())
}

fn print_agents(fixture: PathBuf) -> Result<()> {
    let registry = by_registry::load_registry_path(&fixture)?;
    for agent in registry.agents {
        println!(
            "{}\t{}\t{}",
            agent.id,
            agent.name,
            agent.description.unwrap_or_default()
        );
    }
    Ok(())
}

fn print_models(fixture: PathBuf, provider: Option<String>) -> Result<()> {
    let registry = by_registry::load_registry_path(&fixture)?;
    for model in registry.models {
        if provider
            .as_deref()
            .is_some_and(|provider| provider != model.provider)
        {
            continue;
        }
        println!(
            "{}\t{}",
            model_label_with_region(&model),
            model.description.unwrap_or_default()
        );
    }
    Ok(())
}

fn model_label_with_region(model: &by_registry::ModelDescriptor) -> String {
    match model.region.as_deref() {
        Some(region) if !region.is_empty() => format!("{} ({})", model.label(), region),
        _ => model.label(),
    }
}

fn print_config(path: Option<PathBuf>) -> Result<()> {
    let path = match path {
        Some(path) => path,
        None => default_config_path().context("could not determine default config path")?,
    };
    let config = by_config::read_config(&path)?;
    let llm = config.llm();
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

fn print_sessions(root: Option<PathBuf>) -> Result<()> {
    let root = match root {
        Some(root) => root,
        None => default_sessions_root().context("could not determine default session root")?,
    };
    for session in by_persist::list_sessions(&root)? {
        println!(
            "{}\t{}\t{}",
            session.id,
            session.label.unwrap_or_default(),
            session.last_active.unwrap_or_default()
        );
    }
    Ok(())
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
    std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".brainyard/config.edn"))
}

fn default_sessions_root() -> Option<PathBuf> {
    std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".brainyard/sessions"))
}
