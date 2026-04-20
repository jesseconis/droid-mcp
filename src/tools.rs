use crate::mcp::{InputSchema, PropertySchema, ToolDefinition, ToolResult};
use crate::policy::{Policy, Privilege};
use serde_json::Value;
use std::collections::HashMap;
use std::sync::Arc;
use tokio::process::Command;

// ── Tool trait ─────────────────────────────────────────────────────────

#[async_trait::async_trait]
pub trait Tool: Send + Sync {
    fn definition(&self) -> ToolDefinition;
    fn privilege(&self) -> Privilege;
    async fn invoke(&self, params: &serde_json::Map<String, Value>) -> ToolResult;
}

// ── Parameter types ────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub enum ParamType {
    String,
    Integer,
    Boolean,
}

impl ParamType {
    fn as_str(&self) -> &'static str {
        match self {
            Self::String => "string",
            Self::Integer => "integer",
            Self::Boolean => "boolean",
        }
    }
}

#[derive(Debug, Clone)]
pub struct ParamDef {
    pub name: String,
    pub param_type: ParamType,
    pub description: String,
    pub required: bool,
    pub default: Option<Value>,
    pub enum_values: Option<Vec<String>>,
}

// ── Helper for extracting typed params ─────────────────────────────────

pub struct Params<'a>(pub &'a serde_json::Map<String, Value>);

impl<'a> Params<'a> {
    pub fn str(&self, key: &str) -> Option<&'a str> {
        self.0.get(key).and_then(|v| v.as_str())
    }

    pub fn str_or(&self, key: &str, default: &'a str) -> &'a str {
        self.str(key).unwrap_or(default)
    }

    pub fn int(&self, key: &str) -> Option<i64> {
        self.0.get(key).and_then(|v| v.as_i64())
    }

    pub fn int_or(&self, key: &str, default: i64) -> i64 {
        self.int(key).unwrap_or(default)
    }

    pub fn bool(&self, key: &str) -> Option<bool> {
        self.0.get(key).and_then(|v| v.as_bool())
    }
}

// ── Validator function type ────────────────────────────────────────────

type ValidatorFn = Box<dyn Fn(&Value) -> Result<(), String> + Send + Sync>;

// ── Command builder types ──────────────────────────────────────────────

enum CommandSource {
    /// Simple template: "cat /sys/class/net/{interface}/connect_info"
    /// Parameters in {braces} are substituted after validation.
    Template(String),
    /// Full script built dynamically from parameters.
    Script(Box<dyn Fn(Params<'_>) -> String + Send + Sync>),
    /// Read a file path (no shell, just read and return contents).
    ReadFile(Box<dyn Fn(Params<'_>) -> String + Send + Sync>),
    /// Direct binary invocation — no shell involved.
    /// Returns (binary, args_vec). Safer for passthrough tools
    /// because args are never interpreted by sh.
    Direct(Box<dyn Fn(Params<'_>) -> (String, Vec<String>) + Send + Sync>),
}

// ── ShellTool: the reusable builder ────────────────────────────────────

pub struct ShellTool {
    name: String,
    description: String,
    params: Vec<ParamDef>,
    privilege: Privilege,
    needs_confirmation: bool,
    command: CommandSource,
    validators: HashMap<String, ValidatorFn>,
    timeout_secs: u64,
}

impl ShellTool {
    pub fn new(name: impl Into<String>) -> ShellToolBuilder {
        ShellToolBuilder {
            name: name.into(),
            description: String::new(),
            params: Vec::new(),
            privilege: Privilege::User,
            needs_confirmation: false,
            command: None,
            validators: HashMap::new(),
            timeout_secs: 30,
        }
    }
}

pub struct ShellToolBuilder {
    name: String,
    description: String,
    params: Vec<ParamDef>,
    privilege: Privilege,
    needs_confirmation: bool,
    command: Option<CommandSource>,
    validators: HashMap<String, ValidatorFn>,
    timeout_secs: u64,
}

impl ShellToolBuilder {
    pub fn description(mut self, desc: impl Into<String>) -> Self {
        self.description = desc.into();
        self
    }

    /// Add a parameter definition.
    pub fn param(
        mut self,
        name: impl Into<String>,
        param_type: ParamType,
        description: impl Into<String>,
        required: bool,
    ) -> Self {
        self.params.push(ParamDef {
            name: name.into(),
            param_type,
            description: description.into(),
            required,
            default: None,
            enum_values: None,
        });
        self
    }

    /// Add a parameter with enum constraints.
    pub fn param_enum(
        mut self,
        name: impl Into<String>,
        description: impl Into<String>,
        values: Vec<&str>,
        required: bool,
    ) -> Self {
        self.params.push(ParamDef {
            name: name.into(),
            param_type: ParamType::String,
            description: description.into(),
            required,
            default: None,
            enum_values: Some(values.into_iter().map(String::from).collect()),
        });
        self
    }

    /// Set a default value for the last added parameter.
    pub fn default_value(mut self, value: impl Into<Value>) -> Self {
        if let Some(last) = self.params.last_mut() {
            last.default = Some(value.into());
        }
        self
    }

    /// Mark this tool as requiring root privileges.
    pub fn root(mut self) -> Self {
        self.privilege = Privilege::Root;
        self
    }

    /// Mark as needing explicit user confirmation before execution.
    pub fn confirm(mut self) -> Self {
        self.needs_confirmation = true;
        self
    }

    /// Simple template command: `"cat /sys/class/net/{interface}/connect_info"`
    /// Parameters in `{braces}` are substituted. All values are shell-escaped.
    pub fn template(mut self, tpl: impl Into<String>) -> Self {
        self.command = Some(CommandSource::Template(tpl.into()));
        self
    }

    /// Dynamic script builder for complex commands.
    pub fn script<F>(mut self, f: F) -> Self
    where
        F: Fn(Params<'_>) -> String + Send + Sync + 'static,
    {
        self.command = Some(CommandSource::Script(Box::new(f)));
        self
    }

    /// Read a file path (no shell invocation).
    pub fn read_file<F>(mut self, f: F) -> Self
    where
        F: Fn(Params<'_>) -> String + Send + Sync + 'static,
    {
        self.command = Some(CommandSource::ReadFile(Box::new(f)));
        self
    }

    /// Direct binary invocation — no shell. The closure returns
    /// (binary_path, vec_of_args). Use this for passthrough tools
    /// where the agent constructs the args string and you want to
    /// avoid shell interpretation entirely.
    pub fn direct<F>(mut self, f: F) -> Self
    where
        F: Fn(Params<'_>) -> (String, Vec<String>) + Send + Sync + 'static,
    {
        self.command = Some(CommandSource::Direct(Box::new(f)));
        self
    }

    /// Add a validator for a specific parameter.
    pub fn validate<F>(mut self, param_name: impl Into<String>, f: F) -> Self
    where
        F: Fn(&Value) -> Result<(), String> + Send + Sync + 'static,
    {
        self.validators.insert(param_name.into(), Box::new(f));
        self
    }

    /// Set execution timeout in seconds (default: 30).
    pub fn timeout(mut self, secs: u64) -> Self {
        self.timeout_secs = secs;
        self
    }

    pub fn build(self) -> ShellTool {
        assert!(self.command.is_some(), "tool '{}' has no command source", self.name);
        ShellTool {
            name: self.name,
            description: self.description,
            params: self.params,
            privilege: self.privilege,
            needs_confirmation: self.needs_confirmation,
            command: self.command.unwrap(),
            validators: self.validators,
            timeout_secs: self.timeout_secs,
        }
    }
}

// ── Shell escaping ─────────────────────────────────────────────────────

pub(crate) fn shell_escape(s: &str) -> String {
    // POSIX single-quote escaping: wrap in '', escape internal ' as '\''
    if s.is_empty() {
        return "''".to_string();
    }
    if s.chars().all(|c| c.is_alphanumeric() || "._-/".contains(c)) {
        return s.to_string();
    }
    format!("'{}'", s.replace('\'', "'\\''"))
}

/// Split a string into args respecting single/double quotes.
/// Used by Direct command source to split the agent's `args` string
/// into individual arguments without invoking a shell.
pub(crate) fn shlex_split(s: &str) -> Vec<String> {
    let mut args = Vec::new();
    let mut current = String::new();
    let mut in_single = false;
    let mut in_double = false;
    let mut escape = false;

    for c in s.chars() {
        if escape {
            current.push(c);
            escape = false;
            continue;
        }
        match c {
            '\\' if !in_single => escape = true,
            '\'' if !in_double => in_single = !in_single,
            '"' if !in_single => in_double = !in_double,
            ' ' | '\t' if !in_single && !in_double => {
                if !current.is_empty() {
                    args.push(std::mem::take(&mut current));
                }
            }
            _ => current.push(c),
        }
    }
    if !current.is_empty() {
        args.push(current);
    }
    args
}

// ── Tool trait impl for ShellTool ──────────────────────────────────────

#[async_trait::async_trait]
impl Tool for ShellTool {
    fn definition(&self) -> ToolDefinition {
        let mut properties = HashMap::new();
        let mut required = Vec::new();

        for p in &self.params {
            let mut prop = PropertySchema {
                prop_type: p.param_type.as_str().to_string(),
                description: p.description.clone(),
                default: p.default.clone(),
                enum_values: p.enum_values.clone(),
            };
            if let Some(ref def) = p.default {
                prop.default = Some(def.clone());
            }
            properties.insert(p.name.clone(), prop);
            if p.required {
                required.push(p.name.clone());
            }
        }

        let mut desc = self.description.clone();
        if self.privilege == Privilege::Root {
            desc.push_str(" [requires root]");
        }
        if self.needs_confirmation {
            desc.push_str(" [requires confirmation]");
        }

        ToolDefinition {
            name: self.name.clone(),
            description: desc,
            input_schema: InputSchema {
                schema_type: "object".into(),
                properties,
                required,
            },
        }
    }

    fn privilege(&self) -> Privilege {
        self.privilege.clone()
    }

    async fn invoke(&self, params: &serde_json::Map<String, Value>) -> ToolResult {
        // 1. Validate required params
        for p in &self.params {
            if p.required && !params.contains_key(&p.name) {
                return ToolResult::error(format!("missing required parameter: {}", p.name));
            }
        }

        // 2. Run param validators
        for (name, validator) in &self.validators {
            if let Some(val) = params.get(name) {
                if let Err(e) = validator(val) {
                    return ToolResult::error(format!("validation failed for '{}': {}", name, e));
                }
            }
        }

        // 3. Apply defaults — build a merged param map
        let mut merged = params.clone();
        for p in &self.params {
            if !merged.contains_key(&p.name) {
                if let Some(ref def) = p.default {
                    merged.insert(p.name.clone(), def.clone());
                }
            }
        }

        let pp = Params(&merged);

        // 4. Build and execute command
        match &self.command {
            CommandSource::Template(tpl) => {
                let mut cmd_str = tpl.clone();
                for p in &self.params {
                    if let Some(val) = merged.get(&p.name) {
                        let raw = match val {
                            Value::String(s) => s.clone(),
                            other => other.to_string(),
                        };
                        cmd_str = cmd_str.replace(
                            &format!("{{{}}}", p.name),
                            &shell_escape(&raw),
                        );
                    }
                }
                self.exec_shell(&cmd_str).await
            }
            CommandSource::Script(f) => {
                let cmd_str = f(pp);
                self.exec_shell(&cmd_str).await
            }
            CommandSource::ReadFile(f) => {
                let path = f(pp);
                match tokio::fs::read_to_string(&path).await {
                    Ok(content) => ToolResult::text(content),
                    Err(e) => ToolResult::error(format!("failed to read {}: {}", path, e)),
                }
            }
            CommandSource::Direct(f) => {
                let (binary, args) = f(pp);
                self.exec_direct(&binary, &args).await
            }
        }
    }
}

impl ShellTool {
    async fn exec_shell(&self, cmd: &str) -> ToolResult {
        let shell_cmd = if self.privilege == Privilege::Root {
            format!("su -c {}", shell_escape(cmd))
        } else {
            cmd.to_string()
        };

        tracing::info!(tool = %self.name, cmd = %shell_cmd, "executing via shell");

        let result = tokio::time::timeout(
            std::time::Duration::from_secs(self.timeout_secs),
            Command::new("sh")
                .arg("-c")
                .arg(&shell_cmd)
                .output(),
        )
        .await;

        Self::handle_output(result, self.timeout_secs)
    }

    /// Execute a binary directly without a shell. For root tools,
    /// we go through `su -c` but shell-escape each arg individually.
    async fn exec_direct(&self, binary: &str, args: &[String]) -> ToolResult {
        let result = if self.privilege == Privilege::Root {
            // Build a properly escaped command string for su -c
            let escaped_args: Vec<String> = args.iter().map(|a| shell_escape(a)).collect();
            let full_cmd = if escaped_args.is_empty() {
                binary.to_string()
            } else {
                format!("{} {}", binary, escaped_args.join(" "))
            };
            tracing::info!(tool = %self.name, cmd = %full_cmd, "executing direct (root)");
            tokio::time::timeout(
                std::time::Duration::from_secs(self.timeout_secs),
                Command::new("su").arg("-c").arg(&full_cmd).output(),
            )
            .await
        } else {
            tracing::info!(
                tool = %self.name,
                binary = %binary,
                args = ?args,
                "executing direct"
            );
            tokio::time::timeout(
                std::time::Duration::from_secs(self.timeout_secs),
                Command::new(binary).args(args).output(),
            )
            .await
        };

        Self::handle_output(result, self.timeout_secs)
    }

    fn handle_output(
        result: Result<Result<std::process::Output, std::io::Error>, tokio::time::error::Elapsed>,
        timeout_secs: u64,
    ) -> ToolResult {
        match result {
            Ok(Ok(output)) => {
                let stdout = String::from_utf8_lossy(&output.stdout);
                let stderr = String::from_utf8_lossy(&output.stderr);

                if output.status.success() {
                    if stdout.is_empty() && !stderr.is_empty() {
                        ToolResult::text(format!("(stderr) {}", stderr))
                    } else {
                        ToolResult::text(stdout.into_owned())
                    }
                } else {
                    let mut msg = format!("exit code: {}", output.status);
                    if !stderr.is_empty() {
                        msg.push_str(&format!("\nstderr: {}", stderr));
                    }
                    if !stdout.is_empty() {
                        msg.push_str(&format!("\nstdout: {}", stdout));
                    }
                    ToolResult::error(msg)
                }
            }
            Ok(Err(e)) => ToolResult::error(format!("failed to spawn: {}", e)),
            Err(_) => ToolResult::error(format!("command timed out after {}s", timeout_secs)),
        }
    }
}

// ── Tool Registry ──────────────────────────────────────────────────────

pub struct ToolRegistry {
    tools: HashMap<String, Arc<dyn Tool>>,
    policy: Arc<Policy>,
}

impl ToolRegistry {
    pub fn new(policy: Policy) -> Self {
        Self {
            tools: HashMap::new(),
            policy: Arc::new(policy),
        }
    }

    /// Register a ShellTool built via the builder pattern.
    pub fn register(&mut self, tool: ShellTool) {
        let name = tool.name.clone();
        self.tools.insert(name, Arc::new(tool));
    }

    /// Register any type implementing the Tool trait (for custom tools).
    pub fn register_custom(&mut self, tool: impl Tool + 'static) {
        let name = tool.definition().name.clone();
        self.tools.insert(name, Arc::new(tool));
    }

    pub fn definitions(&self) -> Vec<ToolDefinition> {
        self.tools.values().map(|t| t.definition()).collect()
    }

    pub async fn call(
        &self,
        name: &str,
        params: &serde_json::Map<String, Value>,
    ) -> ToolResult {
        let tool = match self.tools.get(name) {
            Some(t) => t.clone(),
            None => return ToolResult::error(format!("unknown tool: {}", name)),
        };

        // Check privilege policy
        let priv_level = tool.privilege();
        if !self.policy.is_allowed(name, &priv_level) {
            return ToolResult::error(format!(
                "tool '{}' blocked by policy (requires {:?})",
                name, priv_level
            ));
        }

        tool.invoke(params).await
    }
}

// ── Common validators ──────────────────────────────────────────────────

/// Package name: only alphanumeric + dots, e.g. "com.jesse.hostctl"
pub fn validate_package_name(v: &Value) -> Result<(), String> {
    let s = v.as_str().ok_or("must be a string")?;
    if s.is_empty() {
        return Err("cannot be empty".into());
    }
    if !s.chars().all(|c| c.is_alphanumeric() || c == '.') {
        return Err("invalid package name: only alphanumeric and '.' allowed".into());
    }
    if !s.contains('.') {
        return Err("package name must contain at least one '.'".into());
    }
    Ok(())
}

/// File path: no traversal, must be under allowed prefixes.
pub fn validate_safe_path(v: &Value) -> Result<(), String> {
    let s = v.as_str().ok_or("must be a string")?;
    if s.contains("..") {
        return Err("path traversal not allowed".into());
    }
    if s.contains('\0') {
        return Err("null bytes not allowed".into());
    }
    Ok(())
}

/// Network interface name: alphanumeric + limited special chars.
pub fn validate_interface_name(v: &Value) -> Result<(), String> {
    let s = v.as_str().ok_or("must be a string")?;
    if !s.chars().all(|c| c.is_alphanumeric() || c == '_' || c == '-') {
        return Err("invalid interface name".into());
    }
    if s.len() > 16 {
        return Err("interface name too long".into());
    }
    Ok(())
}
