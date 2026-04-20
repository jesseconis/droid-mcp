use crate::tools::{shlex_split, ParamType, ShellTool, ToolRegistry};
use crate::policy::Privilege;
use serde::Deserialize;
use std::path::Path;
use tokio::process::Command;

/// Top-level manifest file structure.
#[derive(Debug, Deserialize)]
pub struct Manifest {
    #[serde(default)]
    pub tool: Vec<ManifestTool>,
}

/// A single tool entry in the manifest.
/// Declares a binary to expose as an MCP tool.
/// The server scrapes the binary's help text at startup and embeds
/// it in the tool description so the agent sees the real interface.
#[derive(Debug, Deserialize)]
pub struct ManifestTool {
    /// MCP tool name (what the agent calls it by).
    pub name: String,

    /// Path or name of the binary to invoke.
    pub binary: String,

    /// Arguments to pass to get help text (e.g., ["-h"] or ["--help"]).
    /// Empty = no auto-scrape, use `fallback_help` instead.
    #[serde(default)]
    pub help_args: Vec<String>,

    /// One-line synopsis shown before the help text in the description.
    pub synopsis: String,

    /// Privilege level: "user" (default) or "root".
    #[serde(default = "default_privilege")]
    pub privilege: String,

    /// Whether the agent should confirm with the user before executing.
    #[serde(default)]
    pub confirm: bool,

    /// Execution timeout in seconds.
    #[serde(default = "default_timeout")]
    pub timeout: u64,

    /// Fixed subcommand prepended before the agent's args.
    /// e.g., subcommand = "list" with binary = "pm" runs "pm list <args>".
    #[serde(default)]
    pub subcommand: Option<String>,

    /// Fallback help text if help_args produces nothing or binary is missing.
    #[serde(default)]
    pub fallback_help: Option<String>,

    /// Max chars of help text to include in description (default: 2000).
    #[serde(default = "default_max_help")]
    pub max_help_chars: usize,
}

fn default_privilege() -> String {
    "user".into()
}
fn default_timeout() -> u64 {
    30
}
fn default_max_help() -> usize {
    2000
}

/// Load a manifest file and register all declared tools.
/// Runs each binary's help command to scrape real usage docs.
pub async fn load_and_register(
    path: &Path,
    registry: &mut ToolRegistry,
) -> Result<usize, String> {
    let content = std::fs::read_to_string(path)
        .map_err(|e| format!("failed to read manifest {}: {}", path.display(), e))?;

    let manifest: Manifest = toml::from_str(&content)
        .map_err(|e| format!("failed to parse manifest: {}", e))?;

    let mut registered = 0;

    for tool_def in manifest.tool {
        match register_one(&tool_def, registry).await {
            Ok(()) => {
                tracing::info!(
                    name = %tool_def.name,
                    binary = %tool_def.binary,
                    "manifest tool registered"
                );
                registered += 1;
            }
            Err(e) => {
                tracing::warn!(
                    name = %tool_def.name,
                    binary = %tool_def.binary,
                    error = %e,
                    "skipping manifest tool"
                );
            }
        }
    }

    Ok(registered)
}

async fn register_one(
    def: &ManifestTool,
    registry: &mut ToolRegistry,
) -> Result<(), String> {
    // 1. Verify the binary exists
    let binary_exists = Command::new("which")
        .arg(&def.binary)
        .output()
        .await
        .map(|o| o.status.success())
        .unwrap_or(false);

    if !binary_exists {
        return Err(format!("binary '{}' not found in PATH", def.binary));
    }

    // 2. Scrape help text
    let help_text = scrape_help(def).await;

    // 3. Build description: synopsis + real help text
    let description = match &help_text {
        Some(help) => format!("{}\n\n{}", def.synopsis, help),
        None => def.synopsis.clone(),
    };

    // 4. Build the tool
    let binary = def.binary.clone();
    let subcommand = def.subcommand.clone();
    let timeout = def.timeout;

    let mut builder = ShellTool::new(&def.name)
        .description(description)
        .param(
            "args",
            ParamType::String,
            format!(
                "Command-line arguments for `{}`{}. \
                 Pass flags and values exactly as shown in the usage above.",
                def.binary,
                match &def.subcommand {
                    Some(sub) => format!(" {} (subcommand is prepended automatically)", sub),
                    None => String::new(),
                }
            ),
            false,
        )
        .default_value("")
        .timeout(timeout)
        .direct(move |p| {
            let args_str = p.str_or("args", "");
            let mut argv = Vec::new();

            // Prepend fixed subcommand if configured
            if let Some(ref sub) = subcommand {
                argv.push(sub.clone());
            }

            // Split the agent's args string into individual arguments
            if !args_str.is_empty() {
                argv.extend(shlex_split(args_str));
            }

            (binary.clone(), argv)
        });

    if def.privilege == "root" {
        builder = builder.root();
    }
    if def.confirm {
        builder = builder.confirm();
    }

    registry.register(builder.build());
    Ok(())
}

/// Run the binary's help command and capture the output.
async fn scrape_help(def: &ManifestTool) -> Option<String> {
    // If no help_args specified, use fallback
    if def.help_args.is_empty() {
        return def.fallback_help.clone();
    }

    let output = tokio::time::timeout(
        std::time::Duration::from_secs(5),
        Command::new(&def.binary)
            .args(&def.help_args)
            .output(),
    )
    .await;

    let output = match output {
        Ok(Ok(o)) => o,
        Ok(Err(e)) => {
            tracing::debug!(
                binary = %def.binary,
                error = %e,
                "help scrape failed, using fallback"
            );
            return def.fallback_help.clone();
        }
        Err(_) => {
            tracing::debug!(binary = %def.binary, "help scrape timed out");
            return def.fallback_help.clone();
        }
    };

    // Some tools print help to stdout, others to stderr.
    // Some exit 0, others exit 1 for -h. We don't care about exit code.
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);

    let text = if !stdout.trim().is_empty() {
        stdout.to_string()
    } else if !stderr.trim().is_empty() {
        stderr.to_string()
    } else {
        return def.fallback_help.clone();
    };

    // Truncate if needed
    let truncated: String = text.chars().take(def.max_help_chars).collect();
    if truncated.len() < text.len() {
        Some(format!("{}\n[... truncated]", truncated))
    } else {
        Some(truncated)
    }
}
