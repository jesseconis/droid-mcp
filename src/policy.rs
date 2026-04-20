use serde::Deserialize;
use std::collections::{HashMap, HashSet};
use std::path::Path;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Privilege {
    /// Runs as the Termux user (u0_a280).
    User,
    /// Runs via `su -c` — requires root.
    Root,
}

/// Fine-grained ACL for individual tools.
#[derive(Debug, Clone, Deserialize)]
pub struct ToolAcl {
    /// Whether this tool is enabled at all.
    #[serde(default = "default_true")]
    pub enabled: bool,

    /// If set, only these subcommands/argument patterns are allowed.
    /// For tools that wrap a binary with subcommands (pm, am), this
    /// restricts which subcommand the first positional arg can be.
    #[serde(default)]
    pub allowed_subcommands: Vec<String>,

    /// Subcommands that require the agent to confirm with the user first.
    /// The MCP response will include a confirmation prompt.
    #[serde(default)]
    pub confirm_subcommands: Vec<String>,

    /// Subcommands that are never allowed, even if in `allowed_subcommands`.
    #[serde(default)]
    pub denied_subcommands: Vec<String>,
}

fn default_true() -> bool {
    true
}

/// Top-level policy configuration loaded from TOML.
#[derive(Debug, Clone, Deserialize)]
pub struct PolicyConfig {
    /// Global settings.
    #[serde(default)]
    pub global: GlobalPolicy,

    /// Per-tool ACL overrides. Key is the tool name.
    #[serde(default)]
    pub tools: HashMap<String, ToolAcl>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct GlobalPolicy {
    /// Allow root tools at all? If false, all root tools are disabled.
    #[serde(default = "default_true")]
    pub allow_root: bool,

    /// Tools that are globally disabled by name.
    #[serde(default)]
    pub disabled_tools: Vec<String>,

    /// Maximum output size in bytes (truncate beyond this).
    #[serde(default = "default_max_output")]
    pub max_output_bytes: usize,
}

fn default_max_output() -> usize {
    512_000 // 500KB
}

impl Default for GlobalPolicy {
    fn default() -> Self {
        Self {
            allow_root: true,
            disabled_tools: Vec::new(),
            max_output_bytes: default_max_output(),
        }
    }
}

impl Default for PolicyConfig {
    fn default() -> Self {
        Self {
            global: GlobalPolicy::default(),
            tools: HashMap::new(),
        }
    }
}

/// Runtime policy enforcer.
#[derive(Debug)]
pub struct Policy {
    config: PolicyConfig,
    disabled_set: HashSet<String>,
}

impl Policy {
    pub fn from_config(config: PolicyConfig) -> Self {
        let disabled_set: HashSet<String> =
            config.global.disabled_tools.iter().cloned().collect();
        Self { config, disabled_set }
    }

    pub fn permissive() -> Self {
        Self::from_config(PolicyConfig::default())
    }

    pub fn load(path: &Path) -> Result<Self, String> {
        let content = std::fs::read_to_string(path)
            .map_err(|e| format!("failed to read policy file: {}", e))?;
        let config: PolicyConfig = toml::from_str(&content)
            .map_err(|e| format!("failed to parse policy: {}", e))?;
        Ok(Self::from_config(config))
    }

    /// Check if a tool is allowed to run.
    pub fn is_allowed(&self, tool_name: &str, privilege: &Privilege) -> bool {
        // Global root kill-switch
        if *privilege == Privilege::Root && !self.config.global.allow_root {
            return false;
        }

        // Globally disabled?
        if self.disabled_set.contains(tool_name) {
            return false;
        }

        // Per-tool ACL
        if let Some(acl) = self.config.tools.get(tool_name) {
            return acl.enabled;
        }

        true
    }

    /// For tools with subcommand ACLs (like pm, am), check if a specific
    /// subcommand is allowed. Returns Ok(needs_confirm) or Err(reason).
    pub fn check_subcommand(
        &self,
        tool_name: &str,
        subcommand: &str,
    ) -> Result<bool, String> {
        let acl = match self.config.tools.get(tool_name) {
            Some(acl) => acl,
            None => return Ok(false), // No ACL = allowed, no confirm
        };

        // Denied?
        if acl.denied_subcommands.iter().any(|s| s == subcommand) {
            return Err(format!(
                "subcommand '{}' is denied for tool '{}'",
                subcommand, tool_name
            ));
        }

        // If allowed_subcommands is non-empty, it's a whitelist
        if !acl.allowed_subcommands.is_empty()
            && !acl.allowed_subcommands.iter().any(|s| s == subcommand)
        {
            return Err(format!(
                "subcommand '{}' not in allowed list for tool '{}'",
                subcommand, tool_name
            ));
        }

        // Needs confirmation?
        let needs_confirm = acl.confirm_subcommands.iter().any(|s| s == subcommand);
        Ok(needs_confirm)
    }

    pub fn max_output_bytes(&self) -> usize {
        self.config.global.max_output_bytes
    }
}
