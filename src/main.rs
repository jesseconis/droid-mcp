mod builtins;
mod manifest;
mod mcp;
mod policy;
mod server;
mod tools;
mod transport_sse;
mod transport_stdio;

use clap::Parser;
use policy::Policy;
use server::McpServer;
use tools::ToolRegistry;

#[derive(Parser)]
#[command(name = "droid-mcp", about = "MCP server for Android development via Termux")]
struct Cli {
    /// Transport mode: "sse" (HTTP, default) or "stdio" (for SSH piping)
    #[arg(short, long, default_value = "sse")]
    transport: String,

    /// Bind address for SSE transport
    #[arg(short, long, default_value = "0.0.0.0:3100")]
    bind: String,

    /// Path to policy TOML file (optional, defaults to permissive)
    #[arg(short, long)]
    policy: Option<String>,

    /// Path to tool manifest TOML file (default: tools.toml in working directory)
    #[arg(short, long, default_value = "tools.toml")]
    manifest: String,

    /// Skip manifest loading (use only built-in curated tools)
    #[arg(long)]
    no_manifest: bool,

    /// Enable verbose logging
    #[arg(short, long)]
    verbose: bool,
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let cli = Cli::parse();

    // Init logging — stderr so it doesn't interfere with stdio transport
    let filter = if cli.verbose { "debug" } else { "info" };
    tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_writer(std::io::stderr)
        .init();

    // Load policy
    let policy = match &cli.policy {
        Some(path) => {
            tracing::info!(path = %path, "loading policy");
            Policy::load(std::path::Path::new(path))?
        }
        None => {
            tracing::info!("using permissive default policy");
            Policy::permissive()
        }
    };

    // Build tool registry
    let mut registry = ToolRegistry::new(policy);

    // 1. Register curated built-in tools (multi-step, custom validators)
    builtins::register_all(&mut registry);
    let builtin_count = registry.definitions().len();
    tracing::info!(count = builtin_count, "built-in tools registered");

    // 2. Load manifest tools (passthrough with scraped help text)
    if !cli.no_manifest {
        let manifest_path = std::path::Path::new(&cli.manifest);
        if manifest_path.exists() {
            tracing::info!(path = %cli.manifest, "loading tool manifest");
            match manifest::load_and_register(manifest_path, &mut registry).await {
                Ok(count) => {
                    tracing::info!(count = count, "manifest tools registered");
                }
                Err(e) => {
                    tracing::error!(error = %e, "failed to load manifest");
                    return Err(e.into());
                }
            }
        } else {
            tracing::warn!(
                path = %cli.manifest,
                "manifest file not found, running with built-in tools only"
            );
        }
    }

    let total = registry.definitions().len();
    tracing::info!(total = total, "total tools available");

    // Create MCP server
    let server = McpServer::new(registry);

    // Start transport
    match cli.transport.as_str() {
        "sse" => {
            tracing::info!(
                addr = %cli.bind,
                "starting SSE transport — connect clients to http://{}/sse",
                cli.bind
            );
            transport_sse::serve_sse(server, &cli.bind).await?;
        }
        "stdio" => {
            transport_stdio::serve_stdio(server).await?;
        }
        other => {
            eprintln!("unknown transport: {} (use 'sse' or 'stdio')", other);
            std::process::exit(1);
        }
    }

    Ok(())
}
