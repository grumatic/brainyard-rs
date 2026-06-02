#![forbid(unsafe_code)]

use anyhow::{bail, Context, Result};
use clap::{ArgAction, Parser, Subcommand};
use std::io::{self, IsTerminal, Write};
use std::path::{Path, PathBuf};
use std::process::{Command as ProcessCommand, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

const RESUME_LATEST_SENTINEL: &str = "--by-resume-latest--";
const RUN_PREVIEW_ROWS: usize = 24;
const RUN_PREVIEW_COLS: usize = 80;
static SESSION_ID_COUNTER: AtomicU64 = AtomicU64::new(0);
const TMUX_NEED_SESSION_GUIDANCE: &str =
    "You passed --with-tmux, but you're not currently inside a tmux session.
For tmux side panes (activity, log) and popup dialogs, start a tmux
session and re-run `by` from inside it:

    tmux new -s brainyard
    by --with-tmux

Or drop --with-tmux to run the in-process TUI without tmux integration:

    by\n";
const TMUX_NEED_TMUX_GUIDANCE: &str = "You passed --with-tmux, but `tmux` is not on $PATH.
Install tmux, then re-run from inside a tmux session:

    # macOS
    brew install tmux
    # Debian/Ubuntu
    sudo apt-get install tmux

Or drop --with-tmux to run the in-process TUI without tmux integration:

    by\n";
const TMUX_SERVER_DEAD_GUIDANCE: &str =
    "You passed --with-tmux and $TMUX is set, but the tmux server isn't
responding (it may have been killed or the system was suspended).
Start a fresh tmux session:

    tmux new -s brainyard
    by --with-tmux\n";

#[derive(Debug, Parser)]
#[command(name = "by")]
#[command(about = "Brainyard Agent CLI")]
#[command(version = env!("BY_BUILD_VERSION"))]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Debug, Subcommand)]
enum Commands {
    /// Start interactive TUI agent session (default).
    Run {
        /// Agent ID.
        #[arg(long, short = 'a', default_value = "coact-agent", value_name = "ID")]
        agent: String,
        /// LM provider.
        #[arg(
            long,
            short = 'p',
            default_value = "claude-code",
            value_name = "PROVIDER"
        )]
        provider: String,
        /// Model name override.
        #[arg(long, short = 'm', value_name = "MODEL")]
        model: Option<String>,
        /// User identity for sessions/memory.
        #[arg(long = "user-id", short = 'u', value_name = "ID")]
        user_id: Option<String>,
        /// Inline mode (no alt screen).
        #[arg(long, short = 'i', action = ArgAction::SetTrue)]
        inline: bool,
        /// Disable inline mode.
        #[arg(long = "no-inline", action = ArgAction::SetTrue, hide = true)]
        no_inline: bool,
        /// Verbose output.
        #[arg(long, short = 'v', action = ArgAction::SetTrue)]
        verbose: bool,
        /// Disable verbose output.
        #[arg(long = "no-verbose", action = ArgAction::SetTrue, hide = true)]
        no_verbose: bool,
        /// Require tmux side panes / popups.
        #[arg(long = "with-tmux", action = ArgAction::SetTrue)]
        with_tmux: bool,
        /// Disable required tmux side panes / popups.
        #[arg(long = "no-with-tmux", action = ArgAction::SetTrue, hide = true)]
        no_with_tmux: bool,
        /// Max agent iterations.
        #[arg(long, short = 'n', value_name = "N")]
        max_iterations: Option<usize>,
        /// AWS region for Bedrock one-turn MVP.
        #[arg(long, value_name = "REGION", hide = true)]
        region: Option<String>,
        /// AWS profile for Bedrock one-turn MVP.
        #[arg(long = "aws-profile", value_name = "PROFILE", hide = true)]
        aws_profile: Option<String>,
        /// Bedrock maxTokens value for one-turn MVP.
        #[arg(long = "max-tokens", value_name = "N", hide = true)]
        bedrock_max_tokens: Option<u32>,
        /// Disable Bedrock prompt-cache cachePoint shaping for one-turn MVP.
        #[arg(long = "no-prompt-cache", hide = true)]
        no_prompt_cache: bool,
        /// Print the Bedrock one-turn request JSON without network access.
        #[arg(long = "dry-run", hide = true)]
        dry_run: bool,
        /// Call Bedrock Converse over the network for one-turn MVP.
        #[arg(long = "live", hide = true)]
        live: bool,
        /// Replay a Bedrock Converse response fixture without network access.
        #[arg(long = "fixture-response", value_name = "PATH", hide = true)]
        fixture_response: Option<PathBuf>,
        /// Resume a persisted session; bare --resume means latest.
        #[arg(
            long,
            short = 'r',
            value_name = "ID",
            num_args = 0..=1,
            default_missing_value = "--by-resume-latest--"
        )]
        resume: Option<String>,
        /// Pick a persisted session to resume from an interactive menu.
        #[arg(long = "select-resume", action = ArgAction::SetTrue)]
        select_resume: bool,
        /// Disable interactive resume selection.
        #[arg(long = "no-select-resume", action = ArgAction::SetTrue, hide = true)]
        no_select_resume: bool,
        /// Deprecated no-op; sessions start fresh by default.
        #[arg(long = "new", action = ArgAction::SetTrue)]
        new: bool,
        /// Disable deprecated new-session flag.
        #[arg(long = "no-new", action = ArgAction::SetTrue, hide = true)]
        no_new: bool,
        /// Bare agent id or legacy provider:model token.
        #[arg(value_name = "ARG", num_args = 0..)]
        positional: Vec<String>,
    },
    /// Ask a one-shot question. Bedrock supports dry-run shaping or explicit live calls.
    Ask {
        /// Agent ID. Dry-run metadata follows Clojure default-agent precedence.
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
        /// Max agent iterations. Included as a dry-run agent-session override.
        #[arg(long, short = 'n', value_name = "N")]
        max_iterations: Option<usize>,
        /// User identity for sessions/memory. Reserved for later full agent execution parity.
        #[arg(long = "user-id", short = 'u', value_name = "ID")]
        user_id: Option<String>,
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
        /// Replay a Bedrock Converse response fixture without network access.
        #[arg(long = "fixture-response", value_name = "PATH", hide = true)]
        fixture_response: Option<PathBuf>,
        /// Question to ask. Also accepts the legacy positional provider:model token.
        #[arg(value_name = "QUESTION", num_args = 1..)]
        question: Vec<String>,
    },
    /// List available agents.
    Agents {
        /// Path to an exported registry JSON fixture.
        #[arg(long, value_name = "PATH", hide = true)]
        fixture: Option<PathBuf>,
    },
    /// List available LLM models.
    Models {
        /// Path to an exported registry JSON fixture.
        #[arg(long, value_name = "PATH", hide = true)]
        fixture: Option<PathBuf>,
        /// Filter to a single provider, e.g. bedrock or openai.
        #[arg(long, short = 'p', value_name = "PROVIDER")]
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
        command: Option<ConfigCommand>,
        /// Non-interactive mode; apply profile defaults.
        #[arg(long, action = ArgAction::SetTrue)]
        auto: bool,
        /// Disable non-interactive mode.
        #[arg(long = "no-auto", action = ArgAction::SetTrue, hide = true)]
        no_auto: bool,
        /// Named profile (dev, ci, offline, cloud).
        #[arg(long, value_name = "S")]
        profile: Option<String>,
        /// Run phases 1-2 only; skip config-agent prompt.
        #[arg(long = "skip-handoff", action = ArgAction::SetTrue)]
        skip_handoff: bool,
        /// Do not skip config-agent prompt.
        #[arg(long = "no-skip-handoff", action = ArgAction::SetTrue, hide = true)]
        no_skip_handoff: bool,
        /// Force rung re-evaluation even if existing LLM is reachable.
        #[arg(long = "re-bootstrap", action = ArgAction::SetTrue)]
        re_bootstrap: bool,
        /// Do not force rung re-evaluation.
        #[arg(long = "no-re-bootstrap", action = ArgAction::SetTrue, hide = true)]
        no_re_bootstrap: bool,
        /// Compute the config but do not write it.
        #[arg(long = "dry-run", action = ArgAction::SetTrue)]
        dry_run: bool,
        /// Allow writes when bootstrap parity is implemented.
        #[arg(long = "no-dry-run", action = ArgAction::SetTrue, hide = true)]
        no_dry_run: bool,
        /// Override bootstrap-log path.
        #[arg(long, value_name = "S")]
        log: Option<PathBuf>,
    },
    /// Search Brainyard memory SQLite stores without mutating them.
    Memory {
        #[command(subcommand)]
        command: MemoryCommand,
    },
    /// Inspect configured MCP servers from fixture data without connecting.
    #[command(hide = true)]
    Mcp {
        #[command(subcommand)]
        command: McpCommand,
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
        /// Brainyard memory SQLite database path. Defaults to ~/.brainyard/memory/<user-id>.db.
        #[arg(long, value_name = "PATH")]
        db: Option<PathBuf>,
        /// User identity for default memory database path.
        #[arg(long = "user-id", short = 'u', value_name = "ID")]
        user_id: Option<String>,
    },
    /// Search L2 episodes and L3 semantic facts using compatible FTS5 tables.
    Search {
        /// Brainyard memory SQLite database path. Defaults to ~/.brainyard/memory/<user-id>.db.
        #[arg(long, value_name = "PATH")]
        db: Option<PathBuf>,
        /// User identity for default memory database path.
        #[arg(long = "user-id", short = 'u', value_name = "ID")]
        user_id: Option<String>,
        /// Natural-language query. Multi-word queries default to OR recall.
        #[arg(long, value_name = "TEXT")]
        query: String,
        /// Maximum number of combined hits to print.
        #[arg(long, default_value_t = 20, value_name = "N")]
        limit: usize,
    },
}

#[derive(Debug, Subcommand)]
enum McpCommand {
    /// List configured MCP servers without connecting.
    Servers {
        /// Path to an exported registry or MCP servers JSON fixture.
        #[arg(long, value_name = "PATH")]
        fixture: Option<PathBuf>,
    },
    /// Print one configured MCP server entry without connecting.
    Config {
        /// Path to an exported registry or MCP servers JSON fixture.
        #[arg(long, value_name = "PATH")]
        fixture: Option<PathBuf>,
        /// MCP server name.
        #[arg(value_name = "SERVER_NAME")]
        server_name: String,
    },
    /// Project an initialize response fixture into server-info shape.
    Info {
        /// MCP server name used by the agent command result.
        #[arg(long = "server-name", value_name = "SERVER_NAME")]
        server_name: String,
        /// Path to an initialize JSON-RPC response or raw result fixture.
        #[arg(long = "fixture-response", value_name = "PATH")]
        fixture_response: PathBuf,
        /// JSON-RPC request id in the fixture response.
        #[arg(long = "request-id", default_value_t = 1)]
        request_id: u64,
    },
    /// Project an initialize/capabilities fixture into capabilities shape.
    Capabilities {
        /// MCP server name used by the agent command result.
        #[arg(long = "server-name", value_name = "SERVER_NAME")]
        server_name: String,
        /// Path to an initialize JSON-RPC response, raw result, or raw capabilities fixture.
        #[arg(long = "fixture-response", value_name = "PATH")]
        fixture_response: PathBuf,
        /// JSON-RPC request id in the fixture response.
        #[arg(long = "request-id", default_value_t = 1)]
        request_id: u64,
    },
    /// Project a ping request or successful health-check fixture.
    Health {
        /// MCP server name used by the agent command result.
        #[arg(long = "server-name", value_name = "SERVER_NAME")]
        server_name: String,
        /// Optional ping JSON-RPC response fixture.
        #[arg(long = "fixture-response", value_name = "PATH")]
        fixture_response: Option<PathBuf>,
        /// JSON-RPC request id.
        #[arg(long = "request-id", default_value_t = 1)]
        request_id: u64,
        /// Timestamp to include in the projected health result.
        #[arg(long = "timestamp-ms", default_value_t = 0)]
        timestamp_ms: u64,
    },
    /// Project the Clojure disconnected-server command result shape.
    Disconnected {
        /// MCP server name used by the agent command result.
        #[arg(long = "server-name", value_name = "SERVER_NAME")]
        server_name: String,
    },
    /// Project an MCP lifecycle command success result without side effects.
    Lifecycle {
        /// Operation: start, stop, or restart.
        #[arg(long, value_name = "OP")]
        op: String,
        /// MCP server name used by the agent command result.
        #[arg(long = "server-name", value_name = "SERVER_NAME")]
        server_name: String,
    },
    /// Project a tools/list JSON-RPC fixture into the agent command result shape.
    Tools {
        /// MCP server name that produced the tools/list response.
        #[arg(long = "server-name", value_name = "SERVER_NAME")]
        server_name: String,
        /// Path to a tools/list JSON-RPC response or raw result fixture.
        #[arg(long = "fixture-response", value_name = "PATH")]
        fixture_response: PathBuf,
        /// JSON-RPC request id in the fixture response.
        #[arg(long = "request-id", default_value_t = 1)]
        request_id: u64,
    },
    /// Project a resources/list request or response fixture.
    Resources {
        /// MCP server name used by the agent command result.
        #[arg(long = "server-name", value_name = "SERVER_NAME")]
        server_name: String,
        /// Optional resources/list JSON-RPC response or raw result fixture.
        #[arg(long = "fixture-response", value_name = "PATH")]
        fixture_response: Option<PathBuf>,
        /// JSON-RPC request id.
        #[arg(long = "request-id", default_value_t = 1)]
        request_id: u64,
    },
    /// Project a prompts/list request or response fixture.
    Prompts {
        /// MCP server name used by the agent command result.
        #[arg(long = "server-name", value_name = "SERVER_NAME")]
        server_name: String,
        /// Optional prompts/list JSON-RPC response or raw result fixture.
        #[arg(long = "fixture-response", value_name = "PATH")]
        fixture_response: Option<PathBuf>,
        /// JSON-RPC request id.
        #[arg(long = "request-id", default_value_t = 1)]
        request_id: u64,
    },
    /// Project tools/list into auto-registered agent tool descriptors.
    RegisteredTools {
        /// MCP server name that produced the tools/list response.
        #[arg(long = "server-name", value_name = "SERVER_NAME")]
        server_name: String,
        /// Path to a tools/list JSON-RPC response or raw result fixture.
        #[arg(long = "fixture-response", value_name = "PATH")]
        fixture_response: PathBuf,
        /// JSON-RPC request id in the fixture response.
        #[arg(long = "request-id", default_value_t = 1)]
        request_id: u64,
    },
    /// Project a single tools/call request or response fixture.
    CallTool {
        /// MCP server name used by the agent command result.
        #[arg(long = "server-name", value_name = "SERVER_NAME")]
        server_name: String,
        /// Native MCP tool name.
        #[arg(long = "tool-name", value_name = "TOOL_NAME")]
        tool_name: String,
        /// Tool arguments as JSON. Accepts map, [{name,value}], or compact vector.
        #[arg(long = "tool-args", default_value = "{}", value_name = "JSON")]
        tool_args: String,
        /// Optional tools/call JSON-RPC response or raw result fixture.
        #[arg(long = "fixture-response", value_name = "PATH")]
        fixture_response: Option<PathBuf>,
        /// JSON-RPC request id.
        #[arg(long = "request-id", default_value_t = 1)]
        request_id: u64,
    },
    /// Project a resources/read request or response fixture.
    ReadResource {
        /// MCP server name used by the agent command result.
        #[arg(long = "server-name", value_name = "SERVER_NAME")]
        server_name: String,
        /// Resource URI to read.
        #[arg(long = "resource-uri", value_name = "URI")]
        resource_uri: String,
        /// Optional resources/read JSON-RPC response or raw result fixture.
        #[arg(long = "fixture-response", value_name = "PATH")]
        fixture_response: Option<PathBuf>,
        /// JSON-RPC request id.
        #[arg(long = "request-id", default_value_t = 1)]
        request_id: u64,
    },
    /// Project a prompts/get request or response fixture.
    GetPrompt {
        /// MCP server name used by the agent command result.
        #[arg(long = "server-name", value_name = "SERVER_NAME")]
        server_name: String,
        /// Prompt name to fetch.
        #[arg(long = "prompt-name", value_name = "PROMPT_NAME")]
        prompt_name: String,
        /// Prompt arguments as a JSON object.
        #[arg(long = "arguments", default_value = "{}", value_name = "JSON")]
        arguments: String,
        /// Optional prompts/get JSON-RPC response or raw result fixture.
        #[arg(long = "fixture-response", value_name = "PATH")]
        fixture_response: Option<PathBuf>,
        /// JSON-RPC request id.
        #[arg(long = "request-id", default_value_t = 1)]
        request_id: u64,
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
    let raw_args = std::env::args().collect::<Vec<_>>();
    let user_args = &raw_args[1..];
    exit_compat_short_h_error_if_requested(user_args);

    if print_compat_help_if_requested(user_args.iter().cloned()) {
        return Ok(());
    }

    let cli = Cli::parse_from(normalize_default_run_args(raw_args));
    match cli.command {
        Commands::Run {
            agent,
            provider,
            model,
            user_id,
            inline: _inline,
            no_inline: _no_inline,
            verbose: _verbose,
            no_verbose: _no_verbose,
            with_tmux,
            no_with_tmux,
            max_iterations,
            region,
            aws_profile,
            bedrock_max_tokens,
            no_prompt_cache,
            dry_run,
            live,
            fixture_response,
            resume,
            select_resume,
            no_select_resume,
            new: _new,
            no_new: _no_new,
            positional,
        } => {
            let one_turn_requested =
                run_one_turn_mode_requested(dry_run, live, fixture_response.as_ref());
            if one_turn_requested {
                validate_run_one_turn_modes(dry_run, live, fixture_response.as_ref())?;
            }
            let session_selection = preflight_run_session_selection(
                select_resume && !no_select_resume,
                resume.as_deref(),
            )?;
            preflight_run_tmux(with_tmux && !no_with_tmux);
            let session_selection =
                prepare_run_session(session_selection, &agent, user_id.as_deref())?;
            if one_turn_requested {
                return print_run_bedrock_one_turn(RunBedrockOneTurnRequest {
                    agent,
                    provider,
                    model,
                    max_iterations,
                    user_id,
                    region,
                    aws_profile,
                    max_tokens: bedrock_max_tokens,
                    no_prompt_cache,
                    dry_run,
                    live,
                    fixture_response,
                    question: positional,
                    session_selection,
                });
            }
            print_run_preview(agent, provider, model.as_deref(), &session_selection)
        }
        Commands::Ask {
            agent,
            provider,
            model,
            max_iterations,
            user_id,
            region,
            aws_profile,
            temperature,
            max_tokens,
            no_prompt_cache,
            dry_run,
            live,
            fixture_response,
            question,
        } => print_ask(AskRequest {
            agent,
            provider,
            model,
            max_iterations,
            user_id,
            region,
            aws_profile,
            temperature,
            max_tokens,
            no_prompt_cache,
            dry_run,
            live,
            fixture_response,
            question,
        }),
        Commands::Agents { fixture } => print_agents(fixture),
        Commands::Models { fixture, provider } => print_models(fixture, provider),
        Commands::Tools {
            fixture,
            tool_type,
            id,
        } => print_tools(fixture, tool_type, id),
        Commands::Config {
            command,
            auto,
            no_auto,
            profile,
            skip_handoff,
            no_skip_handoff,
            re_bootstrap,
            no_re_bootstrap,
            dry_run,
            no_dry_run,
            log,
        } => match command {
            Some(ConfigCommand::Show { path }) => print_config(path),
            None => print_config_bootstrap_projection(ConfigBootstrapOptions {
                auto,
                no_auto,
                profile,
                skip_handoff,
                no_skip_handoff,
                re_bootstrap,
                no_re_bootstrap,
                dry_run,
                no_dry_run,
                log,
            }),
        },
        Commands::Memory { command } => match command {
            MemoryCommand::Inspect { db, user_id } => print_memory_inspect(db, user_id),
            MemoryCommand::Search {
                db,
                user_id,
                query,
                limit,
            } => print_memory_search(db, user_id, query, limit),
        },
        Commands::Mcp { command } => match command {
            McpCommand::Servers { fixture } => print_mcp_servers(fixture),
            McpCommand::Config {
                fixture,
                server_name,
            } => print_mcp_config(fixture, server_name),
            McpCommand::Info {
                server_name,
                fixture_response,
                request_id,
            } => print_mcp_info(server_name, fixture_response, request_id),
            McpCommand::Capabilities {
                server_name,
                fixture_response,
                request_id,
            } => print_mcp_capabilities(server_name, fixture_response, request_id),
            McpCommand::Health {
                server_name,
                fixture_response,
                request_id,
                timestamp_ms,
            } => print_mcp_health(server_name, fixture_response, request_id, timestamp_ms),
            McpCommand::Disconnected { server_name } => print_mcp_disconnected(server_name),
            McpCommand::Lifecycle { op, server_name } => print_mcp_lifecycle(op, server_name),
            McpCommand::Tools {
                server_name,
                fixture_response,
                request_id,
            } => print_mcp_tools(server_name, fixture_response, request_id),
            McpCommand::Resources {
                server_name,
                fixture_response,
                request_id,
            } => print_mcp_resources(server_name, fixture_response, request_id),
            McpCommand::Prompts {
                server_name,
                fixture_response,
                request_id,
            } => print_mcp_prompts(server_name, fixture_response, request_id),
            McpCommand::RegisteredTools {
                server_name,
                fixture_response,
                request_id,
            } => print_mcp_registered_tools(server_name, fixture_response, request_id),
            McpCommand::CallTool {
                server_name,
                tool_name,
                tool_args,
                fixture_response,
                request_id,
            } => print_mcp_call_tool(
                server_name,
                tool_name,
                tool_args,
                fixture_response,
                request_id,
            ),
            McpCommand::ReadResource {
                server_name,
                resource_uri,
                fixture_response,
                request_id,
            } => print_mcp_read_resource(server_name, resource_uri, fixture_response, request_id),
            McpCommand::GetPrompt {
                server_name,
                prompt_name,
                arguments,
                fixture_response,
                request_id,
            } => print_mcp_get_prompt(
                server_name,
                prompt_name,
                arguments,
                fixture_response,
                request_id,
            ),
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

fn normalize_default_run_args<I>(args: I) -> Vec<String>
where
    I: IntoIterator,
    I::Item: Into<String>,
{
    let mut args = args.into_iter().map(Into::into).collect::<Vec<_>>();
    let Some(first_user_arg) = args.get(1) else {
        args.push("run".to_string());
        return args;
    };

    if is_known_subcommand(first_user_arg)
        || is_help_flag(first_user_arg)
        || is_version_flag(first_user_arg)
    {
        return args;
    }

    args.insert(1, "run".to_string());
    args
}

fn is_known_subcommand(arg: &str) -> bool {
    matches!(
        arg,
        "run"
            | "ask"
            | "agents"
            | "models"
            | "tools"
            | "config"
            | "memory"
            | "mcp"
            | "sessions"
            | "tui"
    )
}

fn print_compat_help_if_requested<I>(args: I) -> bool
where
    I: IntoIterator,
    I::Item: Into<String>,
{
    let args: Vec<String> = args.into_iter().map(Into::into).collect();
    match args.as_slice() {
        [flag] if is_help_flag(flag) => {
            print!("{}", top_level_help());
            true
        }
        [command, flag] if command == "run" && is_help_flag(flag) => {
            print!("{}", run_help());
            true
        }
        [command, flag] if command == "ask" && is_help_flag(flag) => {
            print!("{}", ask_help());
            true
        }
        [command, flag] if command == "agents" && is_help_flag(flag) => {
            print!("{}", agents_help());
            true
        }
        [command, flag] if command == "models" && is_help_flag(flag) => {
            print!("{}", models_help());
            true
        }
        [command, flag] if command == "config" && is_help_flag(flag) => {
            print!("{}", config_help());
            true
        }
        [command, flag] if command == "sessions" && is_help_flag(flag) => {
            print!("{}", sessions_help());
            true
        }
        [command, subcommand, flag]
            if command == "sessions" && subcommand == "list" && is_help_flag(flag) =>
        {
            print!("{}", sessions_list_help());
            true
        }
        [command, subcommand, flag]
            if command == "sessions" && subcommand == "prune" && is_help_flag(flag) =>
        {
            print!("{}", sessions_prune_help());
            true
        }
        _ => false,
    }
}

fn is_help_flag(flag: &str) -> bool {
    matches!(flag, "--help" | "-?")
}

fn exit_compat_short_h_error_if_requested(args: &[String]) {
    if !args.iter().any(|arg| arg == "-h") {
        return;
    }

    let (error, help) = match args {
        [flag] if flag == "-h" => (
            "Global option error: Unknown option: \"-h\"",
            top_level_help(),
        ),
        [command, flag] if command == "run" && flag == "-h" => (
            "Option error: Unknown option: \"-h\"",
            run_help().to_string(),
        ),
        [command, flag] if command == "ask" && flag == "-h" => (
            "Option error: Unknown option: \"-h\"",
            ask_help().to_string(),
        ),
        [command, flag] if command == "agents" && flag == "-h" => (
            "Option error: Unknown option: \"-h\"",
            agents_help().to_string(),
        ),
        [command, flag] if command == "models" && flag == "-h" => (
            "Option error: Unknown option: \"-h\"",
            models_help().to_string(),
        ),
        [command, flag] if command == "config" && flag == "-h" => (
            "Option error: Unknown option: \"-h\"",
            config_help().to_string(),
        ),
        [command, flag] if command == "sessions" && flag == "-h" => (
            "Global option error: Unknown option: \"-h\"",
            sessions_help().to_string(),
        ),
        [command, subcommand, flag]
            if command == "sessions" && subcommand == "list" && flag == "-h" =>
        {
            (
                "Option error: Unknown option: \"-h\"",
                sessions_list_help().to_string(),
            )
        }
        [command, subcommand, flag]
            if command == "sessions" && subcommand == "prune" && flag == "-h" =>
        {
            (
                "Option error: Unknown option: \"-h\"",
                sessions_prune_help().to_string(),
            )
        }
        _ => return,
    };

    eprintln!("** ERROR: **");
    eprintln!("{error}");
    eprintln!();
    eprintln!();
    eprint!("{help}");
    std::process::exit(255);
}

fn is_version_flag(flag: &str) -> bool {
    matches!(flag, "--version" | "-V")
}

fn top_level_help() -> String {
    format!(
        concat!(
            "NAME:\n",
            " by - Brainyard Agent CLI\n",
            "\n",
            "USAGE:\n",
            " by [global-options] command [command options] [arguments...]\n",
            "\n",
            "VERSION:\n",
            " {}\n",
            "\n",
            "COMMANDS:\n",
            "   run                  Start interactive TUI agent session (default)\n",
            "   ask                  Ask a one-shot question (non-interactive)\n",
            "   agents               List available agents\n",
            "   models               List available LLM models (provider/model)\n",
            "   config               Bootstrap pipeline (detect → ladder → handoff)\n",
            "   sessions             List or prune persisted agent sessions\n",
            "\n",
            "GLOBAL OPTIONS:\n",
            "   -?, --help\n",
            "\n",
        ),
        env!("BY_BUILD_VERSION")
    )
}

fn run_help() -> &'static str {
    concat!(
        "NAME:\n",
        " by run - Start interactive TUI agent session (default)\n",
        "\n",
        "USAGE:\n",
        " by run [command options] [arguments...]\n",
        "\n",
        "OPTIONS:\n",
        "   -a, --agent S             coact-agent  Agent ID\n",
        "   -p, --provider S          claude-code  LM provider (claude-code, anthropic, openai, ollama)\n",
        "   -m, --model S                          Model name override\n",
        "   -u, --user-id S                        User identity for sessions/memory (default: $BY_USER_ID, else OS login name)\n",
        "   -i, --[no-]inline                      Inline mode (no alt screen)\n",
        "   -v, --[no-]verbose                     Verbose output\n",
        "       --[no-]with-tmux                   Require tmux side panes / popups (exit 1 if not in a tmux session)\n",
        "   -n, --max-iterations N                 Max agent iterations\n",
        "   -r, --resume S                         Resume a persisted session: bare = latest; --resume <id> = that session\n",
        "       --[no-]select-resume               Pick a persisted session to resume from an interactive menu\n",
        "       --[no-]new                         (deprecated; sessions start fresh by default — accepted as a no-op)\n",
        "   -?, --help\n",
        "\n",
    )
}

fn ask_help() -> &'static str {
    concat!(
        "NAME:\n",
        " by ask - Ask a one-shot question (non-interactive)\n",
        "\n",
        "USAGE:\n",
        " by ask [command options] [arguments...]\n",
        "\n",
        "OPTIONS:\n",
        "   -a, --agent S           coact-agent  Agent ID\n",
        "   -p, --provider S        claude-code  LM provider (claude-code, anthropic, openai, ollama)\n",
        "   -m, --model S                        Model name override\n",
        "   -u, --user-id S                      User identity for sessions/memory (default: $BY_USER_ID, else OS login name)\n",
        "   -n, --max-iterations N               Max agent iterations\n",
        "   -?, --help\n",
        "\n",
    )
}

fn agents_help() -> &'static str {
    concat!(
        "NAME:\n",
        " by agents - List available agents\n",
        "\n",
        "USAGE:\n",
        " by agents [command options] [arguments...]\n",
        "\n",
        "OPTIONS:\n",
        "   -?, --help\n",
        "\n",
    )
}

fn models_help() -> &'static str {
    concat!(
        "NAME:\n",
        " by models - List available LLM models (provider/model)\n",
        "\n",
        "USAGE:\n",
        " by models [command options] [arguments...]\n",
        "\n",
        "OPTIONS:\n",
        "   -p, --provider S  Filter to a single provider (e.g. anthropic, openai, bedrock)\n",
        "   -?, --help\n",
        "\n",
    )
}

fn config_help() -> &'static str {
    concat!(
        "NAME:\n",
        " by config - Bootstrap pipeline (detect → ladder → handoff)\n",
        "\n",
        "USAGE:\n",
        " by config [command options] [arguments...]\n",
        "\n",
        "OPTIONS:\n",
        "       --[no-]auto          Non-interactive mode; apply profile defaults\n",
        "       --profile S          Named profile (dev, ci, offline, cloud)\n",
        "       --[no-]skip-handoff  Run phases 1-2 only; skip config-agent prompt\n",
        "       --[no-]re-bootstrap  Force rung re-evaluation even if existing LLM is reachable\n",
        "       --[no-]dry-run       Compute the config but do not write it\n",
        "       --log S              Override bootstrap-log path\n",
        "   -?, --help\n",
        "\n",
    )
}

fn sessions_help() -> String {
    format!(
        concat!(
            "NAME:\n",
            " by sessions - List or prune persisted agent sessions\n",
            "\n",
            "USAGE:\n",
            " by sessions [global-options] command [command options] [arguments...]\n",
            "\n",
            "VERSION:\n",
            " {}\n",
            "\n",
            "COMMANDS:\n",
            "   list                 List all persisted sessions\n",
            "   prune                Delete a persisted session\n",
            "\n",
            "GLOBAL OPTIONS:\n",
            "   -?, --help\n",
            "\n",
        ),
        env!("BY_BUILD_VERSION")
    )
}

fn sessions_list_help() -> &'static str {
    concat!(
        "NAME:\n",
        " by sessions list - List all persisted sessions\n",
        "\n",
        "USAGE:\n",
        " by sessions list [command options] [arguments...]\n",
        "\n",
        "OPTIONS:\n",
        "   -?, --help\n",
        "\n",
    )
}

fn sessions_prune_help() -> &'static str {
    concat!(
        "NAME:\n",
        " by sessions prune - Delete a persisted session\n",
        "\n",
        "USAGE:\n",
        " by sessions prune [command options] [arguments...]\n",
        "\n",
        "OPTIONS:\n",
        "   -s, --session-id S  Session ID\n",
        "   -?, --help\n",
        "\n",
    )
}

#[derive(Debug)]
struct AskRequest {
    agent: String,
    provider: String,
    model: Option<String>,
    max_iterations: Option<usize>,
    user_id: Option<String>,
    region: Option<String>,
    aws_profile: Option<String>,
    temperature: f64,
    max_tokens: Option<u32>,
    no_prompt_cache: bool,
    dry_run: bool,
    live: bool,
    fixture_response: Option<PathBuf>,
    question: Vec<String>,
}

#[derive(Debug)]
struct RunBedrockOneTurnRequest {
    agent: String,
    provider: String,
    model: Option<String>,
    max_iterations: Option<usize>,
    user_id: Option<String>,
    region: Option<String>,
    aws_profile: Option<String>,
    max_tokens: Option<u32>,
    no_prompt_cache: bool,
    dry_run: bool,
    live: bool,
    fixture_response: Option<PathBuf>,
    question: Vec<String>,
    session_selection: RunSessionSelection,
}

fn run_one_turn_mode_requested(
    dry_run: bool,
    live: bool,
    fixture_response: Option<&PathBuf>,
) -> bool {
    dry_run || live || fixture_response.is_some()
}

fn print_run_bedrock_one_turn(args: RunBedrockOneTurnRequest) -> Result<()> {
    validate_run_one_turn_modes(args.dry_run, args.live, args.fixture_response.as_ref())?;
    let (provider, model, question) =
        resolve_run_one_turn_positionals(args.provider, args.model, args.question);
    let question = question.context(
        "by-rs run Bedrock MVP requires a prompt when --dry-run, --live, or --fixture-response is used",
    )?;

    let default_config = read_default_config()?;
    let llm_config = default_config.as_ref().map(by_config::ConfigDocument::llm);
    let agent_config = default_config
        .as_ref()
        .map(by_config::ConfigDocument::agent);
    let resolved_agent = resolve_ask_agent(
        args.agent,
        agent_config
            .as_ref()
            .and_then(|config| config.default_agent.as_deref()),
    );
    let resolved_max_iterations = resolve_ask_max_iterations(&resolved_agent, args.max_iterations)?;
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
        .context("--model is required for run Bedrock MVP")?;

    if resolved_provider != "bedrock" {
        bail!("by-rs run one-turn MVP currently supports provider 'bedrock' only");
    }

    let session_id = args
        .session_selection
        .session_id
        .as_deref()
        .context("run session was not prepared")?
        .to_string();
    let messages = vec![by_llm::ChatMessage::user(question.clone())];
    let dotenv = by_config::load_process_dotenv()?;
    let catalog_region = bedrock_catalog_region(&model)?;
    let runtime = by_llm::resolve_bedrock_runtime_options(by_llm::BedrockRuntimeInputs {
        explicit_region: args.region,
        catalog_region,
        aws_region: by_config::process_env_or_dotenv(&dotenv, "AWS_REGION"),
        aws_default_region: by_config::process_env_or_dotenv(&dotenv, "AWS_DEFAULT_REGION"),
        explicit_profile: args.aws_profile,
        aws_profile: by_config::process_env_or_dotenv(&dotenv, "AWS_PROFILE"),
        aws_default_profile: by_config::process_env_or_dotenv(&dotenv, "AWS_DEFAULT_PROFILE"),
    });

    let config = by_llm::BedrockConfig {
        model: model.clone(),
        temperature: Some(0.0),
        max_tokens: args.max_tokens,
        prompt_cache: !args.no_prompt_cache && by_llm::bedrock_supports_prompt_cache(&model),
        drop_temperature: by_llm::bedrock_drops_temperature(&model),
    };

    if let Some(path) = args.fixture_response {
        let raw: serde_json::Value = serde_json::from_str(
            &std::fs::read_to_string(&path)
                .with_context(|| format!("failed to read response fixture {}", path.display()))?,
        )
        .with_context(|| format!("failed to parse response fixture {}", path.display()))?;
        let projected = by_llm::reshape_provider_response("bedrock", raw)?;
        let text = by_llm::projected_response_text(&projected);
        let assistant_content = if text.is_empty() {
            serde_json::to_string_pretty(&projected)?
        } else {
            text
        };
        persist_run_bedrock_exchange(&session_id, &question, &assistant_content)?;
        println!("{assistant_content}");
        return Ok(());
    }

    if args.dry_run {
        let request = by_llm::build_bedrock_request(&config, &messages);
        let user_id =
            by_config::resolve_process_user_id_with_dotenv(args.user_id.as_deref(), &dotenv);
        let dry_run = serde_json::json!({
            "provider": "bedrock",
            "operation": "Converse",
            "network": false,
            "session_id": session_id,
            "resume": args.session_selection.resume,
            "agent_session": ask_agent_session(&resolved_agent, user_id, resolved_max_iterations),
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
    let assistant_content = if response.text.is_empty() {
        serde_json::to_string_pretty(&response.projected)?
    } else {
        response.text
    };
    persist_run_bedrock_exchange(&session_id, &question, &assistant_content)?;
    println!("{assistant_content}");
    Ok(())
}

fn validate_run_one_turn_modes(
    dry_run: bool,
    live: bool,
    fixture_response: Option<&PathBuf>,
) -> Result<()> {
    if dry_run && live {
        bail!("choose only one of --dry-run or --live");
    }
    if fixture_response.is_some() && (dry_run || live) {
        bail!("choose only one of --dry-run, --live, or --fixture-response");
    }
    if !dry_run && !live && fixture_response.is_none() {
        bail!("by-rs run Bedrock MVP requires --dry-run or --live");
    }
    Ok(())
}

fn resolve_run_one_turn_positionals(
    provider: String,
    model: Option<String>,
    mut positionals: Vec<String>,
) -> (String, Option<String>, Option<String>) {
    let (provider, model) = match take_legacy_provider_model(&mut positionals) {
        Some((legacy_provider, legacy_model)) => (legacy_provider, Some(legacy_model)),
        None => (provider, model),
    };
    let joined = positionals.join(" ");
    let trimmed = joined.trim();
    let question = (!trimmed.is_empty()).then(|| trimmed.to_string());
    (provider, model, question)
}

fn persist_run_bedrock_exchange(session_id: &str, question: &str, answer: &str) -> Result<()> {
    let root = default_sessions_root().context("could not determine default session root")?;
    by_persist::append_session_message(&root, session_id, "user", question)?;
    by_persist::append_session_message(&root, session_id, "assistant", answer)?;
    Ok(())
}

fn print_ask(args: AskRequest) -> Result<()> {
    let (provider, model, question) =
        resolve_ask_positionals(args.provider, args.model, args.question);
    let Some(question) = question.filter(|question| !question.trim().is_empty()) else {
        print_missing_ask_question_and_exit();
    };

    if args.dry_run && args.live {
        bail!("choose only one of --dry-run or --live");
    }
    if args.fixture_response.is_some() && (args.dry_run || args.live) {
        bail!("choose only one of --dry-run, --live, or --fixture-response");
    }
    if !args.dry_run && !args.live && args.fixture_response.is_none() {
        bail!("by-rs ask requires --dry-run or --live");
    }

    let default_config = read_default_config()?;
    let llm_config = default_config.as_ref().map(by_config::ConfigDocument::llm);
    let agent_config = default_config
        .as_ref()
        .map(by_config::ConfigDocument::agent);
    let resolved_agent = resolve_ask_agent(
        args.agent,
        agent_config
            .as_ref()
            .and_then(|config| config.default_agent.as_deref()),
    );
    let resolved_max_iterations = resolve_ask_max_iterations(&resolved_agent, args.max_iterations)?;
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
        .context("--model is required for ask")?;

    if let Some(path) = args.fixture_response {
        let raw: serde_json::Value = serde_json::from_str(
            &std::fs::read_to_string(&path)
                .with_context(|| format!("failed to read response fixture {}", path.display()))?,
        )
        .with_context(|| format!("failed to parse response fixture {}", path.display()))?;
        let projected = by_llm::reshape_provider_response(&resolved_provider, raw)?;
        let text = by_llm::projected_response_text(&projected);
        if text.is_empty() {
            println!("{}", serde_json::to_string_pretty(&projected)?);
        } else {
            println!("{text}");
        }
        return Ok(());
    }

    let messages = vec![by_llm::ChatMessage::user(question)];
    let dotenv = by_config::load_process_dotenv()?;

    if args.dry_run && resolved_provider != "bedrock" {
        let drop_temperature = by_llm::bedrock_drops_temperature(&model);
        let config = by_llm::ProviderChatConfig {
            model,
            temperature: Some(args.temperature),
            max_tokens: args.max_tokens,
            drop_temperature,
        };
        let projection = by_llm::build_provider_request(&resolved_provider, &config, &messages)?;
        let user_id =
            by_config::resolve_process_user_id_with_dotenv(args.user_id.as_deref(), &dotenv);
        let dry_run = serde_json::json!({
            "provider": resolved_provider,
            "operation": projection.operation,
            "network": false,
            "agent_session": ask_agent_session(&resolved_agent, user_id, resolved_max_iterations),
            "request": projection.request,
        });
        println!("{}", serde_json::to_string_pretty(&dry_run)?);
        return Ok(());
    }

    if resolved_provider != "bedrock" {
        bail!("by-rs ask currently supports provider 'bedrock' only");
    }

    let catalog_region = bedrock_catalog_region(&model)?;
    let runtime = by_llm::resolve_bedrock_runtime_options(by_llm::BedrockRuntimeInputs {
        explicit_region: args.region,
        catalog_region,
        aws_region: by_config::process_env_or_dotenv(&dotenv, "AWS_REGION"),
        aws_default_region: by_config::process_env_or_dotenv(&dotenv, "AWS_DEFAULT_REGION"),
        explicit_profile: args.aws_profile,
        aws_profile: by_config::process_env_or_dotenv(&dotenv, "AWS_PROFILE"),
        aws_default_profile: by_config::process_env_or_dotenv(&dotenv, "AWS_DEFAULT_PROFILE"),
    });

    let config = by_llm::BedrockConfig {
        model: model.clone(),
        temperature: Some(args.temperature),
        max_tokens: args.max_tokens,
        prompt_cache: !args.no_prompt_cache && by_llm::bedrock_supports_prompt_cache(&model),
        drop_temperature: by_llm::bedrock_drops_temperature(&model),
    };

    if args.dry_run {
        let request = by_llm::build_bedrock_request(&config, &messages);
        let user_id =
            by_config::resolve_process_user_id_with_dotenv(args.user_id.as_deref(), &dotenv);
        let dry_run = serde_json::json!({
            "provider": "bedrock",
            "operation": "Converse",
            "network": false,
            "agent_session": ask_agent_session(&resolved_agent, user_id, resolved_max_iterations),
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

fn resolve_ask_max_iterations(
    agent_id: &str,
    cli_max_iterations: Option<usize>,
) -> Result<Option<usize>> {
    if cli_max_iterations.is_some() {
        return Ok(cli_max_iterations);
    }

    Ok(by_registry::load_embedded_oracle_registry()?
        .agents
        .into_iter()
        .find(|agent| agent.id == agent_id)
        .and_then(|agent| agent.max_iterations)
        .or(Some(by_config::DEFAULT_AGENT_MAX_ITERATIONS)))
}

fn resolve_ask_agent(cli_agent: String, config_default_agent: Option<&str>) -> String {
    if cli_agent != "coact-agent" && !cli_agent.trim().is_empty() {
        return cli_agent;
    }

    config_default_agent
        .filter(|agent| !agent.trim().is_empty())
        .map(ToOwned::to_owned)
        .unwrap_or(cli_agent)
}

fn ask_agent_session(
    agent_id: &str,
    user_id: String,
    max_iterations: Option<usize>,
) -> serde_json::Value {
    let mut session = serde_json::Map::new();
    session.insert("agent_id".to_string(), serde_json::json!(agent_id));
    session.insert("user_id".to_string(), serde_json::json!(user_id));
    if let Some(max_iterations) = max_iterations {
        session.insert(
            "max_iterations".to_string(),
            serde_json::json!(max_iterations),
        );
    }
    serde_json::Value::Object(session)
}

fn print_missing_ask_question_and_exit() -> ! {
    println!("Error: question argument is required.");
    println!("Usage: by ask [options] QUESTION");
    std::process::exit(1);
}

fn bedrock_catalog_region(model: &str) -> Result<Option<String>> {
    Ok(by_registry::load_embedded_oracle_registry()?
        .models
        .into_iter()
        .find(|entry| entry.provider == "bedrock" && entry.id == model)
        .and_then(|entry| entry.region))
}

fn resolve_ask_positionals(
    provider: String,
    model: Option<String>,
    mut positionals: Vec<String>,
) -> (String, Option<String>, Option<String>) {
    let (provider, model) = match take_legacy_provider_model(&mut positionals) {
        Some((legacy_provider, legacy_model)) => (legacy_provider, Some(legacy_model)),
        None => (provider, model),
    };
    let question = positionals.into_iter().next();
    (provider, model, question)
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

fn print_agents(fixture: Option<PathBuf>) -> Result<()> {
    let registry = load_registry_or_embedded(fixture)?;
    if registry.agents.is_empty() {
        println!("No agents registered.");
        return Ok(());
    }

    println!("{} agent(s) available:", registry.agents.len());
    print_agents_table(registry.agents);
    Ok(())
}

fn print_models(fixture: Option<PathBuf>, provider: Option<String>) -> Result<()> {
    let registry = load_registry_or_embedded(fixture)?;
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

fn load_registry_or_embedded(fixture: Option<PathBuf>) -> Result<by_registry::RegistryFixture> {
    match fixture {
        Some(path) => by_registry::load_registry_path(path),
        None => by_registry::load_embedded_oracle_registry(),
    }
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

fn print_mcp_servers(fixture: Option<PathBuf>) -> Result<()> {
    let mut servers = load_mcp_servers_or_embedded(fixture)?;
    for server in &servers {
        by_mcp::validate_server_config(&server.transport, &server.config)
            .with_context(|| format!("invalid MCP config for server '{}'", server.name))?;
    }
    servers.sort_by(|left, right| left.name.cmp(&right.name));

    let projected_servers = servers
        .iter()
        .map(|server| {
            serde_json::json!({
                "name": server.name,
                "connected": false,
                "transport": server.transport,
            })
        })
        .collect::<Vec<_>>();
    let output = serde_json::json!({
        "result": {
            "servers": projected_servers,
            "total": servers.len(),
            "connected": 0,
        }
    });
    println!("{}", serde_json::to_string_pretty(&output)?);
    Ok(())
}

fn print_mcp_config(fixture: Option<PathBuf>, server_name: String) -> Result<()> {
    let servers = load_mcp_servers_or_embedded(fixture)?;
    if server_name.trim().is_empty() {
        let output = by_mcp::project_error_command_result("server-name is required")?;
        println!("{}", serde_json::to_string_pretty(&output)?);
        return Ok(());
    }

    let Some(server) = servers
        .into_iter()
        .find(|server| server.name == server_name)
    else {
        let output = by_mcp::project_error_command_result(&format!(
            "MCP server '{server_name}' not found in configuration"
        ))?;
        println!("{}", serde_json::to_string_pretty(&output)?);
        return Ok(());
    };
    by_mcp::validate_server_config(&server.transport, &server.config)
        .with_context(|| format!("invalid MCP config for server '{}'", server.name))?;

    let output = serde_json::json!({
        "result": {
            "name": server.name,
            "config": {
                "transport": server.transport,
                "config": server.config,
                "enabled": server.enabled,
                "auto-register-tools": server.auto_register_tools,
            }
        }
    });
    println!("{}", serde_json::to_string_pretty(&output)?);
    Ok(())
}

fn print_mcp_info(server_name: String, fixture_response: PathBuf, request_id: u64) -> Result<()> {
    if server_name.trim().is_empty() {
        let output = by_mcp::project_error_command_result("server-name is required")?;
        println!("{}", serde_json::to_string_pretty(&output)?);
        return Ok(());
    }

    let raw = std::fs::read_to_string(&fixture_response).with_context(|| {
        format!(
            "failed to read MCP initialize response fixture {}",
            fixture_response.display()
        )
    })?;
    if let Some(error) = by_mcp::extract_jsonrpc_error_message_from_json(&raw, request_id)
        .with_context(|| {
            format!(
                "failed to project MCP initialize response fixture {}",
                fixture_response.display()
            )
        })?
    {
        let output = by_mcp::project_server_info_error_command_result(&server_name, &error)?;
        println!("{}", serde_json::to_string_pretty(&output)?);
        return Ok(());
    }
    let result = extract_mcp_fixture_result(&raw, request_id).with_context(|| {
        format!(
            "failed to project MCP initialize response fixture {}",
            fixture_response.display()
        )
    })?;
    let output = by_mcp::project_server_info_command_result(&server_name, result)?;
    println!("{}", serde_json::to_string_pretty(&output)?);
    Ok(())
}

fn print_mcp_capabilities(
    server_name: String,
    fixture_response: PathBuf,
    request_id: u64,
) -> Result<()> {
    if server_name.trim().is_empty() {
        let output = by_mcp::project_error_command_result("server-name is required")?;
        println!("{}", serde_json::to_string_pretty(&output)?);
        return Ok(());
    }

    let raw = std::fs::read_to_string(&fixture_response).with_context(|| {
        format!(
            "failed to read MCP capabilities response fixture {}",
            fixture_response.display()
        )
    })?;
    if let Some(error) = by_mcp::extract_jsonrpc_error_message_from_json(&raw, request_id)
        .with_context(|| {
            format!(
                "failed to project MCP capabilities response fixture {}",
                fixture_response.display()
            )
        })?
    {
        let output =
            by_mcp::project_server_capabilities_error_command_result(&server_name, &error)?;
        println!("{}", serde_json::to_string_pretty(&output)?);
        return Ok(());
    }
    let result = extract_mcp_fixture_result(&raw, request_id).with_context(|| {
        format!(
            "failed to project MCP capabilities response fixture {}",
            fixture_response.display()
        )
    })?;
    let nested_capabilities = result.get("capabilities").cloned();
    let capabilities = nested_capabilities.unwrap_or(result);
    let output = by_mcp::project_server_capabilities_command_result(&server_name, capabilities)?;
    println!("{}", serde_json::to_string_pretty(&output)?);
    Ok(())
}

fn print_mcp_health(
    server_name: String,
    fixture_response: Option<PathBuf>,
    request_id: u64,
    timestamp_ms: u64,
) -> Result<()> {
    if server_name.trim().is_empty() {
        let output = by_mcp::project_error_command_result("server-name is required")?;
        println!("{}", serde_json::to_string_pretty(&output)?);
        return Ok(());
    }

    if let Some(fixture_response) = fixture_response {
        let raw = std::fs::read_to_string(&fixture_response).with_context(|| {
            format!(
                "failed to read MCP ping response fixture {}",
                fixture_response.display()
            )
        })?;
        if let Some(error) = by_mcp::extract_jsonrpc_error_message_from_json(&raw, request_id)
            .with_context(|| {
                format!(
                    "failed to project MCP ping response fixture {}",
                    fixture_response.display()
                )
            })?
        {
            let output = by_mcp::project_server_unhealthy_command_result(
                &server_name,
                &error,
                timestamp_ms,
            )?;
            println!("{}", serde_json::to_string_pretty(&output)?);
            return Ok(());
        }
        extract_mcp_ping_fixture_result(&raw, request_id).with_context(|| {
            format!(
                "failed to project MCP ping response fixture {}",
                fixture_response.display()
            )
        })?;
        let output =
            by_mcp::project_server_health_command_result(&server_name, "healthy", timestamp_ms)?;
        println!("{}", serde_json::to_string_pretty(&output)?);
    } else {
        let output = by_mcp::ping_request(request_id);
        println!("{}", serde_json::to_string_pretty(&output)?);
    }

    Ok(())
}

fn print_mcp_disconnected(server_name: String) -> Result<()> {
    let output = by_mcp::project_disconnected_server_command_result(&server_name)?;
    println!("{}", serde_json::to_string_pretty(&output)?);
    Ok(())
}

fn print_mcp_lifecycle(op: String, server_name: String) -> Result<()> {
    if server_name.trim().is_empty() {
        let output = by_mcp::project_error_command_result("server-name is required")?;
        println!("{}", serde_json::to_string_pretty(&output)?);
        return Ok(());
    }

    let output = by_mcp::project_lifecycle_command_result(&server_name, &op)?;
    println!("{}", serde_json::to_string_pretty(&output)?);
    Ok(())
}

fn print_mcp_tools(server_name: String, fixture_response: PathBuf, request_id: u64) -> Result<()> {
    let raw = std::fs::read_to_string(&fixture_response).with_context(|| {
        format!(
            "failed to read MCP tools/list response fixture {}",
            fixture_response.display()
        )
    })?;
    if let Some(error) = by_mcp::extract_jsonrpc_error_message_from_json(&raw, request_id)
        .with_context(|| {
            format!(
                "failed to project MCP tools/list response fixture {}",
                fixture_response.display()
            )
        })?
    {
        let output = by_mcp::project_tools_list_error_command_result(&error)?;
        println!("{}", serde_json::to_string_pretty(&output)?);
        return Ok(());
    }
    let result = extract_mcp_fixture_result(&raw, request_id).with_context(|| {
        format!(
            "failed to project MCP tools/list response fixture {}",
            fixture_response.display()
        )
    })?;
    let tools = by_mcp::tools_from_list_result(&server_name, &result)?;
    let output = by_mcp::project_tools_list_command_result(&tools);
    println!("{}", serde_json::to_string_pretty(&output)?);
    Ok(())
}

fn print_mcp_resources(
    server_name: String,
    fixture_response: Option<PathBuf>,
    request_id: u64,
) -> Result<()> {
    if server_name.trim().is_empty() {
        let output = by_mcp::project_error_command_result("server-name is required")?;
        println!("{}", serde_json::to_string_pretty(&output)?);
        return Ok(());
    }

    if let Some(fixture_response) = fixture_response {
        let raw = std::fs::read_to_string(&fixture_response).with_context(|| {
            format!(
                "failed to read MCP resources/list response fixture {}",
                fixture_response.display()
            )
        })?;
        if let Some(error) = by_mcp::extract_jsonrpc_error_message_from_json(&raw, request_id)
            .with_context(|| {
                format!(
                    "failed to project MCP resources/list response fixture {}",
                    fixture_response.display()
                )
            })?
        {
            let output =
                by_mcp::project_server_resources_error_command_result(&server_name, &error)?;
            println!("{}", serde_json::to_string_pretty(&output)?);
            return Ok(());
        }
        let result = extract_mcp_fixture_result(&raw, request_id).with_context(|| {
            format!(
                "failed to project MCP resources/list response fixture {}",
                fixture_response.display()
            )
        })?;
        let output = by_mcp::project_server_resources_command_result(&server_name, result)?;
        println!("{}", serde_json::to_string_pretty(&output)?);
    } else {
        let output = by_mcp::list_resources_request(request_id);
        println!("{}", serde_json::to_string_pretty(&output)?);
    }

    Ok(())
}

fn print_mcp_prompts(
    server_name: String,
    fixture_response: Option<PathBuf>,
    request_id: u64,
) -> Result<()> {
    if server_name.trim().is_empty() {
        let output = by_mcp::project_error_command_result("server-name is required")?;
        println!("{}", serde_json::to_string_pretty(&output)?);
        return Ok(());
    }

    if let Some(fixture_response) = fixture_response {
        let raw = std::fs::read_to_string(&fixture_response).with_context(|| {
            format!(
                "failed to read MCP prompts/list response fixture {}",
                fixture_response.display()
            )
        })?;
        if let Some(error) = by_mcp::extract_jsonrpc_error_message_from_json(&raw, request_id)
            .with_context(|| {
                format!(
                    "failed to project MCP prompts/list response fixture {}",
                    fixture_response.display()
                )
            })?
        {
            let output = by_mcp::project_server_prompts_error_command_result(&server_name, &error)?;
            println!("{}", serde_json::to_string_pretty(&output)?);
            return Ok(());
        }
        let result = extract_mcp_fixture_result(&raw, request_id).with_context(|| {
            format!(
                "failed to project MCP prompts/list response fixture {}",
                fixture_response.display()
            )
        })?;
        let output = by_mcp::project_server_prompts_command_result(&server_name, result)?;
        println!("{}", serde_json::to_string_pretty(&output)?);
    } else {
        let output = by_mcp::list_prompts_request(request_id);
        println!("{}", serde_json::to_string_pretty(&output)?);
    }

    Ok(())
}

fn print_mcp_registered_tools(
    server_name: String,
    fixture_response: PathBuf,
    request_id: u64,
) -> Result<()> {
    let raw = std::fs::read_to_string(&fixture_response).with_context(|| {
        format!(
            "failed to read MCP tools/list response fixture {}",
            fixture_response.display()
        )
    })?;
    let result = extract_mcp_fixture_result(&raw, request_id).with_context(|| {
        format!(
            "failed to project MCP tools/list response fixture {}",
            fixture_response.display()
        )
    })?;
    let tools = by_mcp::tools_from_list_result(&server_name, &result)?;
    let output = by_mcp::project_registered_tools_command_result(&tools);
    println!("{}", serde_json::to_string_pretty(&output)?);
    Ok(())
}

fn print_mcp_call_tool(
    server_name: String,
    tool_name: String,
    tool_args: String,
    fixture_response: Option<PathBuf>,
    request_id: u64,
) -> Result<()> {
    let tool_args_json: serde_json::Value =
        serde_json::from_str(&tool_args).context("failed to parse --tool-args JSON")?;
    if server_name.trim().is_empty() {
        let output = by_mcp::project_tool_call_validation_error_command_result(
            &server_name,
            &tool_name,
            tool_args_json,
            "server-name is required",
        )?;
        println!("{}", serde_json::to_string_pretty(&output)?);
        return Ok(());
    }
    if tool_name.trim().is_empty() {
        let output = by_mcp::project_tool_call_validation_error_command_result(
            &server_name,
            &tool_name,
            tool_args_json,
            "tool-name is required",
        )?;
        println!("{}", serde_json::to_string_pretty(&output)?);
        return Ok(());
    }
    let calls = by_mcp::tool_calls_from_value(&serde_json::json!([
        {
            "server-name": server_name,
            "tool-name": tool_name,
            "tool-args": tool_args_json,
        }
    ]))?;
    let call = calls
        .first()
        .context("single call projection should produce one call")?;

    if let Some(fixture_response) = fixture_response {
        let raw = std::fs::read_to_string(&fixture_response).with_context(|| {
            format!(
                "failed to read MCP tools/call response fixture {}",
                fixture_response.display()
            )
        })?;
        if let Some(error) = by_mcp::extract_jsonrpc_error_message_from_json(&raw, request_id)
            .with_context(|| {
                format!(
                    "failed to project MCP tools/call response fixture {}",
                    fixture_response.display()
                )
            })?
        {
            let output = by_mcp::project_tool_call_errors_command_result(&calls, &[error])?;
            println!("{}", serde_json::to_string_pretty(&output)?);
            return Ok(());
        }
        let result = extract_mcp_fixture_result(&raw, request_id).with_context(|| {
            format!(
                "failed to project MCP tools/call response fixture {}",
                fixture_response.display()
            )
        })?;
        let output = by_mcp::project_tool_calls_command_result(&calls, &[result])?;
        println!("{}", serde_json::to_string_pretty(&output)?);
    } else {
        let output = by_mcp::tool_call_request_from_call(request_id, call)?;
        println!("{}", serde_json::to_string_pretty(&output)?);
    }

    Ok(())
}

fn print_mcp_read_resource(
    server_name: String,
    resource_uri: String,
    fixture_response: Option<PathBuf>,
    request_id: u64,
) -> Result<()> {
    if server_name.trim().is_empty() {
        let output = by_mcp::project_error_command_result("server-name is required")?;
        println!("{}", serde_json::to_string_pretty(&output)?);
        return Ok(());
    }
    if resource_uri.trim().is_empty() {
        let output = by_mcp::project_error_command_result("resource-uri is required")?;
        println!("{}", serde_json::to_string_pretty(&output)?);
        return Ok(());
    }

    if let Some(fixture_response) = fixture_response {
        let raw = std::fs::read_to_string(&fixture_response).with_context(|| {
            format!(
                "failed to read MCP resources/read response fixture {}",
                fixture_response.display()
            )
        })?;
        if let Some(error) = by_mcp::extract_jsonrpc_error_message_from_json(&raw, request_id)
            .with_context(|| {
                format!(
                    "failed to project MCP resources/read response fixture {}",
                    fixture_response.display()
                )
            })?
        {
            let output = by_mcp::project_read_resource_error_command_result(
                &server_name,
                &resource_uri,
                &error,
            )?;
            println!("{}", serde_json::to_string_pretty(&output)?);
            return Ok(());
        }
        let result = extract_mcp_fixture_result(&raw, request_id).with_context(|| {
            format!(
                "failed to project MCP resources/read response fixture {}",
                fixture_response.display()
            )
        })?;
        let output =
            by_mcp::project_read_resource_command_result(&server_name, &resource_uri, result)?;
        println!("{}", serde_json::to_string_pretty(&output)?);
    } else {
        let output = by_mcp::read_resource_request(request_id, &resource_uri)?;
        println!("{}", serde_json::to_string_pretty(&output)?);
    }

    Ok(())
}

fn print_mcp_get_prompt(
    server_name: String,
    prompt_name: String,
    arguments: String,
    fixture_response: Option<PathBuf>,
    request_id: u64,
) -> Result<()> {
    if server_name.trim().is_empty() {
        let output = by_mcp::project_error_command_result("server-name is required")?;
        println!("{}", serde_json::to_string_pretty(&output)?);
        return Ok(());
    }
    if prompt_name.trim().is_empty() {
        let output = by_mcp::project_error_command_result("prompt-name is required")?;
        println!("{}", serde_json::to_string_pretty(&output)?);
        return Ok(());
    }

    let prompt_arguments = parse_prompt_arguments(&arguments)?;

    if let Some(fixture_response) = fixture_response {
        let raw = std::fs::read_to_string(&fixture_response).with_context(|| {
            format!(
                "failed to read MCP prompts/get response fixture {}",
                fixture_response.display()
            )
        })?;
        if let Some(error) = by_mcp::extract_jsonrpc_error_message_from_json(&raw, request_id)
            .with_context(|| {
                format!(
                    "failed to project MCP prompts/get response fixture {}",
                    fixture_response.display()
                )
            })?
        {
            let output = by_mcp::project_get_prompt_error_command_result(
                &server_name,
                &prompt_name,
                &error,
            )?;
            println!("{}", serde_json::to_string_pretty(&output)?);
            return Ok(());
        }
        let result = extract_mcp_fixture_result(&raw, request_id).with_context(|| {
            format!(
                "failed to project MCP prompts/get response fixture {}",
                fixture_response.display()
            )
        })?;
        let output = by_mcp::project_get_prompt_command_result(&server_name, &prompt_name, result)?;
        println!("{}", serde_json::to_string_pretty(&output)?);
    } else {
        let output = by_mcp::get_prompt_request(request_id, &prompt_name, prompt_arguments)?;
        println!("{}", serde_json::to_string_pretty(&output)?);
    }

    Ok(())
}

fn parse_prompt_arguments(raw: &str) -> Result<serde_json::Value> {
    if raw.trim().is_empty() {
        return Ok(serde_json::json!({}));
    }

    let Ok(value) = serde_json::from_str::<serde_json::Value>(raw) else {
        return Ok(serde_json::json!({}));
    };

    if value.is_object() {
        Ok(value)
    } else {
        bail!("--arguments JSON must be an object");
    }
}

fn extract_mcp_fixture_result(raw: &str, request_id: u64) -> Result<serde_json::Value> {
    if let Some(result) = by_mcp::extract_jsonrpc_result_from_json(raw, request_id)? {
        return Ok(result);
    }

    let value: serde_json::Value =
        serde_json::from_str(raw).context("invalid MCP response fixture JSON")?;
    if value.get("tools").is_some()
        || value.get("resources").is_some()
        || value.get("prompts").is_some()
        || value.get("content").is_some()
        || value.get("contents").is_some()
        || value.get("messages").is_some()
        || value.get("serverInfo").is_some()
        || value.get("server-info").is_some()
        || value.get("capabilities").is_some()
    {
        Ok(value)
    } else if let Some(result) = value.get("result") {
        Ok(result.clone())
    } else {
        bail!("MCP fixture must contain a raw result object or JSON-RPC result");
    }
}

fn extract_mcp_ping_fixture_result(raw: &str, request_id: u64) -> Result<()> {
    if let Some(result) = by_mcp::extract_jsonrpc_result_from_json(raw, request_id)? {
        if result.is_object() {
            return Ok(());
        }
        bail!("MCP ping result must be an object");
    }

    let value: serde_json::Value =
        serde_json::from_str(raw).context("invalid MCP ping fixture JSON")?;
    if value.is_object() {
        Ok(())
    } else {
        bail!("MCP ping fixture must contain a raw object result or JSON-RPC result");
    }
}

fn load_mcp_servers_or_embedded(
    fixture: Option<PathBuf>,
) -> Result<Vec<by_registry::McpServerDescriptor>> {
    match fixture {
        Some(path) => load_mcp_servers_fixture_path(path),
        None => Ok(by_registry::load_embedded_oracle_registry()?.mcp_servers),
    }
}

fn load_mcp_servers_fixture_path(path: PathBuf) -> Result<Vec<by_registry::McpServerDescriptor>> {
    let raw = std::fs::read_to_string(&path)
        .with_context(|| format!("failed to read MCP fixture {}", path.display()))?;
    let value: serde_json::Value = serde_json::from_str(&raw)
        .with_context(|| format!("failed to parse MCP fixture {}", path.display()))?;
    let is_registry_object = value.as_object().is_some_and(|object| {
        [
            "mcpServers",
            "mcp-servers",
            "mcp_servers",
            "tools",
            "agents",
            "models",
        ]
        .iter()
        .any(|key| object.contains_key(*key))
    });

    if is_registry_object {
        Ok(by_registry::load_registry_str(&raw)?.mcp_servers)
    } else {
        by_registry::load_mcp_servers_str(&raw)
    }
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

#[derive(Debug)]
struct ConfigBootstrapOptions {
    auto: bool,
    no_auto: bool,
    profile: Option<String>,
    skip_handoff: bool,
    no_skip_handoff: bool,
    re_bootstrap: bool,
    no_re_bootstrap: bool,
    dry_run: bool,
    no_dry_run: bool,
    log: Option<PathBuf>,
}

fn print_config_bootstrap_projection(opts: ConfigBootstrapOptions) -> Result<()> {
    let auto = opts.auto && !opts.no_auto;
    if !auto && !std::io::stdin().is_terminal() {
        eprintln!("Non-interactive stdin detected. Use --auto for non-interactive runs.");
        std::process::exit(2);
    }
    validate_config_profile(opts.profile.as_deref());

    let dirs = process_dirs();
    let config_dirs = dirs.as_ref().map(config_dirs_projection);
    let defaults = dirs.as_ref().map(config_defaults_projection);
    let projection = serde_json::json!({
        "operation": "config",
        "status": "not-implemented",
        "network": false,
        "writes": false,
        "auto": auto,
        "profile": opts.profile,
        "skip_handoff": opts.skip_handoff && !opts.no_skip_handoff,
        "re_bootstrap": opts.re_bootstrap && !opts.no_re_bootstrap,
        "dry_run": opts.dry_run && !opts.no_dry_run,
        "log": opts.log.map(|path| path.display().to_string()),
        "dirs": config_dirs,
        "defaults": defaults,
        "note": "by-rs keeps config bootstrap read-only until Clojure parity is proven; use `by-rs config show` for config inspection."
    });
    println!("{}", serde_json::to_string_pretty(&projection)?);
    Ok(())
}

fn config_dirs_projection(dirs: &by_config::BrainyardDirs) -> serde_json::Value {
    serde_json::json!({
        "working_dir": display_path(&dirs.working_dir),
        "project_dir": display_path(&dirs.project_dir),
        "user_dir": dirs.user_dir.as_ref().map(|path| display_path(path)),
        "project_config_dir": display_path(&by_config::project_config_dir(dirs)),
        "user_config_dir": by_config::user_config_dir(dirs)
            .as_ref()
            .map(|path| display_path(path)),
    })
}

fn config_defaults_projection(dirs: &by_config::BrainyardDirs) -> serde_json::Value {
    let allowed_dirs = by_config::default_allowed_dirs(dirs)
        .into_iter()
        .map(|path| display_path(&path))
        .collect::<Vec<_>>();

    serde_json::json!({
        "permissions": {
            "mode": "ask-each-time",
            "allowed_dirs": allowed_dirs,
        }
    })
}

fn display_path(path: &std::path::Path) -> String {
    path.display().to_string()
}

fn validate_config_profile(profile: Option<&str>) {
    const KNOWN_PROFILES: [&str; 4] = ["ci", "cloud", "dev", "offline"];
    let Some(profile) = profile else {
        return;
    };
    if KNOWN_PROFILES.contains(&profile) {
        return;
    }

    eprintln!("Unknown profile: :{profile}");
    eprintln!("  Known profiles: {}", KNOWN_PROFILES.join(", "));
    std::process::exit(2);
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
    let permissions = config.permissions();
    println!(
        "agent.default-agent\t{}",
        agent.default_agent.unwrap_or_default()
    );
    println!(
        "agent.max-iterations\t{}",
        agent
            .max_iterations
            .map(|value| value.to_string())
            .unwrap_or_default()
    );
    println!("permissions.mode\t{}", permissions.mode.unwrap_or_default());
    println!(
        "permissions.allowed-dirs\t{}",
        permissions.allowed_dirs.join(",")
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

fn print_memory_search(
    db: Option<PathBuf>,
    user_id: Option<String>,
    query: String,
    limit: usize,
) -> Result<()> {
    let db = resolve_memory_db_path(db, user_id)?;
    let request = by_memory::MemorySearchRequest { query, limit };
    for hit in by_memory::search_memory(&db, request)? {
        println!("{}\t{}\t{}", hit.layer.as_str(), hit.kind, hit.content);
    }
    Ok(())
}

fn print_memory_inspect(db: Option<PathBuf>, user_id: Option<String>) -> Result<()> {
    let db = resolve_memory_db_path(db, user_id)?;
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

fn resolve_memory_db_path(db: Option<PathBuf>, user_id: Option<String>) -> Result<PathBuf> {
    if let Some(db) = db {
        return Ok(db);
    }

    let dotenv = by_config::load_process_dotenv().unwrap_or_default();
    let user_id = by_config::resolve_process_user_id_with_dotenv(user_id.as_deref(), &dotenv);
    let dirs = process_dirs()
        .context("could not determine default memory database path; pass --db or set HOME")?;

    by_config::default_memory_db_path(&dirs, &user_id)
        .context("could not determine default memory database path; pass --db or set HOME")
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct RunSessionSelection {
    session_id: Option<String>,
    resume: bool,
}

impl RunSessionSelection {
    fn fresh() -> Self {
        Self {
            session_id: None,
            resume: false,
        }
    }

    fn resume(session_id: String) -> Self {
        Self {
            session_id: Some(session_id),
            resume: true,
        }
    }
}

fn preflight_run_session_selection(
    select_resume: bool,
    resume: Option<&str>,
) -> Result<RunSessionSelection> {
    if select_resume {
        let root = default_sessions_root().context("could not determine default session root")?;
        let sessions = sorted_sessions_by_activity(&root)?;
        let picked = pick_session_interactive(&sessions)?;
        return Ok(run_selection_from_pick(picked));
    }

    preflight_run_resume(resume)
}

fn preflight_run_resume(resume: Option<&str>) -> Result<RunSessionSelection> {
    let Some(resume) = resume else {
        return Ok(RunSessionSelection::fresh());
    };
    if resume == RESUME_LATEST_SENTINEL {
        let root = default_sessions_root().context("could not determine default session root")?;
        let sessions = sorted_sessions_by_activity(&root)?;
        return Ok(select_latest_resume_session(&sessions));
    }

    let root = default_sessions_root().context("could not determine default session root")?;
    let sessions = by_persist::list_sessions(&root)?;
    if !sessions.iter().any(|session| session.id == resume) {
        eprintln!("Error: no persisted session named '{resume}'.");
        std::process::exit(1);
    }

    Ok(RunSessionSelection::resume(resume.to_string()))
}

fn run_selection_from_pick(picked: Option<String>) -> RunSessionSelection {
    picked
        .map(RunSessionSelection::resume)
        .unwrap_or_else(RunSessionSelection::fresh)
}

fn select_latest_resume_session(sessions: &[by_persist::SessionSummary]) -> RunSessionSelection {
    run_selection_from_pick(sessions.first().map(|session| session.id.clone()))
}

fn prepare_run_session(
    selection: RunSessionSelection,
    agent: &str,
    user_id: Option<&str>,
) -> Result<RunSessionSelection> {
    let root = default_sessions_root().context("could not determine default session root")?;
    let now_millis = current_epoch_millis().context("could not determine current time")?;
    let session_id = selection
        .session_id
        .clone()
        .unwrap_or_else(|| new_run_session_id(now_millis));

    let update = if selection.resume {
        by_persist::SessionMetaUpdate {
            last_attached_at_millis: Some(now_millis),
            ..by_persist::SessionMetaUpdate::default()
        }
    } else {
        let dotenv = by_config::load_process_dotenv().unwrap_or_default();
        let user_id = by_config::resolve_process_user_id_with_dotenv(user_id, &dotenv);
        by_persist::SessionMetaUpdate {
            user_id: Some(user_id),
            agent_id: Some(agent.to_string()),
            defagent_id: Some(agent.to_string()),
            started_at_millis: Some(now_millis),
            last_attached_at_millis: Some(now_millis),
            working_dir: Some(std::env::current_dir()?.display().to_string()),
            ..by_persist::SessionMetaUpdate::default()
        }
    };
    by_persist::save_session_meta(root, &session_id, &update)?;

    Ok(RunSessionSelection {
        session_id: Some(session_id),
        resume: selection.resume,
    })
}

fn new_run_session_id(now_millis: i64) -> String {
    if let Some(session_id) = std::env::var("BRAINYARD_SESSION_ID")
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
    {
        return session_id;
    }

    let suffix_seed = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| u64::from(duration.subsec_nanos()))
        .unwrap_or(0);
    let counter = SESSION_ID_COUNTER.fetch_add(1, Ordering::Relaxed);
    let suffix = (suffix_seed ^ u64::from(std::process::id()) ^ counter) % 10_000;
    format!("agt-{now_millis}-{suffix}")
}

fn sorted_sessions_by_activity(root: &Path) -> Result<Vec<by_persist::SessionSummary>> {
    let mut sessions = by_persist::list_sessions(root)?;
    sessions.sort_by(|left, right| {
        let left_ts = left
            .last_attached_at_millis
            .or(left.started_at_millis)
            .unwrap_or(0);
        let right_ts = right
            .last_attached_at_millis
            .or(right.started_at_millis)
            .unwrap_or(0);
        right_ts.cmp(&left_ts).then_with(|| left.id.cmp(&right.id))
    });
    Ok(sessions)
}

fn pick_session_interactive(sessions: &[by_persist::SessionSummary]) -> Result<Option<String>> {
    let stdin = io::stdin();
    let mut input = stdin.lock();
    let mut output = io::stdout();
    pick_session_with_io(sessions, &mut input, &mut output)
}

fn pick_session_with_io<R: io::BufRead, W: Write>(
    sessions: &[by_persist::SessionSummary],
    input: &mut R,
    output: &mut W,
) -> Result<Option<String>> {
    let n = sessions.len();
    if n == 0 {
        return Ok(None);
    }

    writeln!(output)?;
    writeln!(
        output,
        "{n} persisted session(s) — pick one to resume, or [N] for a new session:"
    )?;
    writeln!(output, "{}", "-".repeat(72))?;
    writeln!(
        output,
        " {:>3}  {:<30} {:<14} {:<18} {:<10} last",
        "#", "session-id", "label", "agent", "size"
    )?;
    writeln!(output, "{}", "-".repeat(88))?;
    for (index, session) in sessions.iter().enumerate() {
        let last = session
            .last_attached_at_millis
            .or(session.started_at_millis)
            .and_then(format_age_millis)
            .unwrap_or_else(|| "-".to_string());
        writeln!(
            output,
            " {:>3}  {:<30} {:<14} {:<18} {:<10} {}",
            index + 1,
            session.id,
            session.label.as_deref().unwrap_or("-"),
            session.agent.as_deref().unwrap_or("-"),
            format_bytes(session.bytes),
            last
        )?;
    }
    writeln!(output)?;
    write!(output, "Choice [1- {n} ] / (N)ew: ")?;
    output.flush()?;

    let mut line = String::new();
    let bytes_read = input.read_line(&mut line)?;
    if bytes_read == 0 {
        return Ok(None);
    }

    let choice = line.trim();
    if choice.is_empty() || matches!(choice, "n" | "N" | "new") {
        return Ok(None);
    }

    let Some(index) = choice.parse::<usize>().ok() else {
        return Ok(None);
    };
    if index == 0 || index > n {
        return Ok(None);
    }

    Ok(Some(sessions[index - 1].id.clone()))
}

fn pick_session_to_prune_interactive(
    sessions: &[by_persist::SessionSummary],
) -> Result<Option<String>> {
    let stdin = io::stdin();
    let mut input = stdin.lock();
    let mut output = io::stdout();
    pick_session_to_prune_with_io(sessions, &mut input, &mut output)
}

fn pick_session_to_prune_with_io<R: io::BufRead, W: Write>(
    sessions: &[by_persist::SessionSummary],
    input: &mut R,
    output: &mut W,
) -> Result<Option<String>> {
    let n = sessions.len();
    if n == 0 {
        return Ok(None);
    }

    writeln!(output)?;
    writeln!(
        output,
        "{n} persisted session(s) — pick one to prune, or (C)ancel:"
    )?;
    writeln!(output, "{}", "-".repeat(88))?;
    writeln!(
        output,
        " {:>3}  {:<30} {:<14} {:<18} {:<10} last",
        "#", "session-id", "label", "agent", "size"
    )?;
    writeln!(output, "{}", "-".repeat(88))?;
    for (index, session) in sessions.iter().enumerate() {
        let last = session
            .last_attached_at_millis
            .or(session.started_at_millis)
            .and_then(format_age_millis)
            .unwrap_or_else(|| "-".to_string());
        writeln!(
            output,
            " {:>3}  {:<30} {:<14} {:<18} {:<10} {}",
            index + 1,
            session.id,
            session.label.as_deref().unwrap_or("-"),
            session.agent.as_deref().unwrap_or("-"),
            format_bytes(session.bytes),
            last
        )?;
    }
    writeln!(output)?;
    write!(output, "Choice [1- {n} ] / (C)ancel: ")?;
    output.flush()?;

    let mut line = String::new();
    let bytes_read = input.read_line(&mut line)?;
    if bytes_read == 0 {
        return Ok(None);
    }

    let choice = line.trim();
    if choice.is_empty() || matches!(choice, "c" | "C" | "cancel") {
        return Ok(None);
    }

    let Some(index) = choice.parse::<usize>().ok() else {
        return Ok(None);
    };
    if index == 0 || index > n {
        return Ok(None);
    }

    Ok(Some(sessions[index - 1].id.clone()))
}

fn preflight_run_tmux(with_tmux: bool) {
    if !with_tmux {
        return;
    }

    let Some(_tmux) = find_command_on_path("tmux") else {
        eprint!("{TMUX_NEED_TMUX_GUIDANCE}");
        std::process::exit(1);
    };

    let tmux_env = std::env::var("TMUX").unwrap_or_default();
    if tmux_env.trim().is_empty() {
        eprint!("{TMUX_NEED_SESSION_GUIDANCE}");
        std::process::exit(1);
    }

    if !tmux_server_alive("tmux", &tmux_env) {
        eprint!("{TMUX_SERVER_DEAD_GUIDANCE}");
        std::process::exit(1);
    }
}

fn find_command_on_path(command: &str) -> Option<PathBuf> {
    std::env::var_os("PATH").and_then(|paths| {
        std::env::split_paths(&paths)
            .map(|dir| dir.join(command))
            .find(|path| is_executable_file(path))
    })
}

#[cfg(unix)]
fn is_executable_file(path: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt;

    path.metadata()
        .map(|metadata| metadata.is_file() && metadata.permissions().mode() & 0o111 != 0)
        .unwrap_or(false)
}

#[cfg(not(unix))]
fn is_executable_file(path: &Path) -> bool {
    path.is_file()
}

fn tmux_server_alive(tmux: &str, tmux_env: &str) -> bool {
    let socket = tmux_env.split(',').next().unwrap_or_default();
    if socket.trim().is_empty() {
        return false;
    }

    let Ok(mut child) = ProcessCommand::new(tmux)
        .args(["-S", socket, "display", "-p", "#{client_pid}"])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
    else {
        return false;
    };

    let deadline = Instant::now() + Duration::from_millis(200);
    loop {
        match child.try_wait() {
            Ok(Some(status)) => return status.success(),
            Ok(None) if Instant::now() >= deadline => {
                let _ = child.kill();
                let _ = child.wait();
                return false;
            }
            Ok(None) => std::thread::sleep(Duration::from_millis(10)),
            Err(_) => return false,
        }
    }
}

fn print_sessions(root: Option<PathBuf>) -> Result<()> {
    let root = match root {
        Some(root) => root,
        None => default_sessions_root().context("could not determine default session root")?,
    };
    let report = by_persist::list_sessions_with_warnings(&root)?;
    for warning in &report.warnings {
        eprintln!(
            "[persist] skipping unreadable meta.edn for {}: {}",
            warning.session_id, warning.message
        );
    }
    let sessions = report.sessions;
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
    let target = match positional_session_id.or(session_id) {
        Some(target) => Some(target),
        None if io::stdin().is_terminal() && io::stdout().is_terminal() => {
            let sessions = sorted_sessions_by_activity(&root)?;
            pick_session_to_prune_interactive(&sessions)?
        }
        None => bail!("Usage: by sessions prune <session-id>"),
    };

    let Some(target) = target else {
        println!("Cancelled.");
        return Ok(());
    };

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

fn print_run_preview(
    agent: String,
    provider: String,
    model: Option<&str>,
    selection: &RunSessionSelection,
) -> Result<()> {
    let frame = by_tui::StaticFrame {
        rows: RUN_PREVIEW_ROWS,
        cols: RUN_PREVIEW_COLS,
        agent,
        model: run_preview_model_label(&provider, model),
        status: run_preview_status(selection),
    };
    println!("{}", by_tui::render_static_frame(&frame));
    Ok(())
}

fn run_preview_model_label(provider: &str, model: Option<&str>) -> String {
    match model {
        Some(model) => format!("{provider}:{model}"),
        None => provider.to_string(),
    }
}

fn run_preview_status(selection: &RunSessionSelection) -> String {
    if selection.resume {
        let session_id = selection.session_id.as_deref().unwrap_or("latest");
        format!("resume {session_id}")
    } else {
        "preview".to_string()
    }
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

fn read_default_config() -> Result<Option<by_config::ConfigDocument>> {
    let Some(path) = default_config_path() else {
        return Ok(None);
    };
    if !path.exists() {
        return Ok(None);
    }
    by_config::read_config(&path)
        .map(Some)
        .with_context(|| format!("failed to read default config {}", path.display()))
}

fn process_dirs() -> Option<by_config::BrainyardDirs> {
    let working_dir = std::env::current_dir().ok()?;
    let user_dir = std::env::var_os("HOME").map(PathBuf::from);
    let project_dir_override = std::env::var_os("BRAINYARD_PROJECT_DIR").map(PathBuf::from);
    Some(by_config::BrainyardDirs::resolve(
        working_dir,
        user_dir,
        project_dir_override,
    ))
}

fn default_config_path() -> Option<PathBuf> {
    process_dirs().and_then(|dirs| by_config::resolve_default_config_path(&dirs))
}

fn default_sessions_root() -> Option<PathBuf> {
    process_dirs().and_then(|dirs| by_config::default_sessions_root(&dirs))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    fn session(id: &str, label: &str, agent: &str, bytes: u64) -> by_persist::SessionSummary {
        by_persist::SessionSummary {
            id: id.to_string(),
            label: Some(label.to_string()),
            agent: Some(agent.to_string()),
            bytes,
            created_at: None,
            started_at: None,
            started_at_millis: None,
            last_active: None,
            last_attached_at_millis: None,
            path: PathBuf::from(id),
        }
    }

    #[test]
    fn resume_picker_renders_clojure_prompt_and_selects_number() {
        let sessions = vec![
            session("newer", "New", "main-agent", 1),
            session("older", "Old", "coact-agent", 2048),
        ];
        let mut input = Cursor::new(b"2\n".to_vec());
        let mut output = Vec::new();

        let picked = pick_session_with_io(&sessions, &mut input, &mut output).unwrap();

        assert_eq!(picked, Some("older".to_string()));
        let stdout = String::from_utf8(output).unwrap();
        assert!(stdout
            .contains("2 persisted session(s) — pick one to resume, or [N] for a new session:"));
        assert!(stdout.contains("Choice [1- 2 ] / (N)ew: "));
        assert!(stdout.contains("newer"));
        assert!(stdout.contains("older"));
        assert!(
            stdout.find("newer").unwrap() < stdout.find("older").unwrap(),
            "sessions should render in caller-provided order: {stdout}"
        );
    }

    #[test]
    fn resume_picker_new_returns_fresh_selection() {
        let sessions = vec![session("alpha", "Alpha", "coact-agent", 0)];
        let mut input = Cursor::new(b"N\n".to_vec());
        let mut output = Vec::new();

        let picked = pick_session_with_io(&sessions, &mut input, &mut output).unwrap();

        assert_eq!(picked, None);
        assert_eq!(
            run_selection_from_pick(picked),
            RunSessionSelection::fresh()
        );
        let stdout = String::from_utf8(output).unwrap();
        assert!(stdout.contains("Choice [1- 1 ] / (N)ew: "));
    }

    #[test]
    fn latest_resume_selection_resumes_first_sorted_session() {
        let sessions = vec![
            session("newer", "New", "main-agent", 1),
            session("older", "Old", "coact-agent", 2048),
        ];

        assert_eq!(
            select_latest_resume_session(&sessions),
            RunSessionSelection::resume("newer".to_string())
        );
    }

    #[test]
    fn latest_resume_selection_without_sessions_starts_fresh() {
        assert_eq!(
            select_latest_resume_session(&[]),
            RunSessionSelection::fresh()
        );
    }

    #[test]
    fn prune_picker_renders_clojure_prompt_and_selects_number() {
        let sessions = vec![
            session("newer", "New", "main-agent", 1),
            session("older", "Old", "coact-agent", 2048),
        ];
        let mut input = Cursor::new(b"2\n".to_vec());
        let mut output = Vec::new();

        let picked = pick_session_to_prune_with_io(&sessions, &mut input, &mut output).unwrap();

        assert_eq!(picked, Some("older".to_string()));
        let stdout = String::from_utf8(output).unwrap();
        assert!(stdout.contains("2 persisted session(s) — pick one to prune, or (C)ancel:"));
        assert!(stdout.contains("Choice [1- 2 ] / (C)ancel: "));
        assert!(stdout.contains("newer"));
        assert!(stdout.contains("older"));
        assert!(
            stdout.find("newer").unwrap() < stdout.find("older").unwrap(),
            "sessions should render in caller-provided order: {stdout}"
        );
    }

    #[test]
    fn prune_picker_cancel_returns_none() {
        let sessions = vec![session("alpha", "Alpha", "coact-agent", 0)];
        let mut input = Cursor::new(b"C\n".to_vec());
        let mut output = Vec::new();

        let picked = pick_session_to_prune_with_io(&sessions, &mut input, &mut output).unwrap();

        assert_eq!(picked, None);
        let stdout = String::from_utf8(output).unwrap();
        assert!(stdout.contains("Choice [1- 1 ] / (C)ancel: "));
    }
}
