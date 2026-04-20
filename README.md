# droid-mcp

> [!WARNING]
> 🤖 AI generated .:. your mileage may vary...

MCP (Model Context Protocol) server for rooted Android development via Termux. Exposes device capabilities — network, packages, activities, logcat, sensors, TTS, and more — as structured tools that any MCP-compatible agent can call.

## Architecture

```
┌─────────────────────────────────────────────────────┐
│  Desktop (Arch)                                     │
│                                                     │
│  ┌──────────────┐   ┌──────────────────────────┐    │
│  │  Claude Code  │   │  VS Code + Continue/Cline│    │
│  │  Claude Desktop│   │  Any MCP client          │    │
│  └──────┬───────┘   └──────────┬───────────────┘    │
│         │                      │                    │
│         │  SSE: http://phone:3100/sse               │
│         │  -or- stdio over SSH                      │
└─────────┼──────────────────────┼────────────────────┘
          │                      │
    ┌─────▼──────────────────────▼────────────────────┐
    │  Android (Termux, rooted)                       │
    │                                                 │
    │  droid-mcp binary                               │
    │  ├── SSE transport (axum, port 3100)            │
    │  ├── stdio transport (for SSH piping)           │
    │  ├── Tool Registry                              │
    │  │   ├── ShellTool: net_connect_info            │
    │  │   ├── ShellTool: getprop                     │
    │  │   ├── ShellTool: app_logcat    [root]        │
    │  │   ├── ShellTool: pm_install    [root+confirm]│
    │  │   ├── ShellTool: tts_speak                   │
    │  │   └── ...                                    │
    │  └── Policy engine (TOML config)                │
    │      ├── Root kill-switch                       │
    │      ├── Per-tool enable/disable                │
    │      └── Subcommand ACLs                        │
    └─────────────────────────────────────────────────┘
```

## Two transports, one binary

**SSE (default):** The server listens on HTTP. Any MCP client connects to the SSE endpoint. This is IDE-agnostic — Claude Desktop, Claude Code, VS Code extensions, or custom agents all work the same way.

**stdio:** The binary reads JSON-RPC from stdin and writes to stdout. Useful when the MCP client can shell out via SSH, but the device isn't reachable over HTTP (or you prefer not to expose a port).

You can run both simultaneously by starting two instances, or just pick the one that fits your setup.

## Building

### Option A: Build natively in Termux (simpler)

```bash
# In Termux on the device
pkg install rust
git clone <your-repo> && cd droid-mcp
cargo build --release
cp target/release/droid-mcp ~/
```

### Option B: Cross-compile from Arch (faster builds)

```bash
# Install the Android NDK or use the prebuilt linker
# Termux uses Bionic libc, so the target triple is:
rustup target add aarch64-linux-android

# You need an aarch64-linux-android linker. The Android NDK provides one.
# If you have the NDK at ~/android-ndk:
export CARGO_TARGET_AARCH64_LINUX_ANDROID_LINKER=\
  ~/android-ndk/toolchains/llvm/prebuilt/linux-x86_64/bin/aarch64-linux-android33-clang

cargo build --release --target aarch64-linux-android

# Deploy via your sshfs mount or scp
scp -P 8022 target/aarch64-linux-android/release/droid-mcp \
    u0_a280@192.168.1.33:/data/data/com.termux/files/home/
```

**Tip:** Cross-compilation for Termux can be finicky with libc linking. If you hit issues, Option A (building in Termux) just works — `cargo` on aarch64 Termux produces a native binary with no cross-compilation headaches.

## Running

```bash
# SSE mode (default) — accessible from any device on the network
droid-mcp --bind 0.0.0.0:3100

# SSE with a policy file
droid-mcp --bind 0.0.0.0:3100 --policy policy.toml

# stdio mode — for SSH-based MCP clients
droid-mcp --transport stdio

# Verbose logging
droid-mcp -v
```

### Running as a background service

```bash
# Using Termux:Boot (auto-start on device boot)
mkdir -p ~/.termux/boot
cat > ~/.termux/boot/start-mcp.sh << 'EOF'
#!/data/data/com.termux/files/usr/bin/sh
termux-wake-lock
exec ~/droid-mcp --bind 0.0.0.0:3100 --policy ~/policy.toml 2>>~/mcp.log
EOF
chmod +x ~/.termux/boot/start-mcp.sh
```

## Connecting agents

### Claude Desktop / Claude Code

Add to `~/.config/claude/claude_desktop_config.json` (or `claude_code_config.json`):

```json
{
  "mcpServers": {
    "android": {
      "url": "http://192.168.1.33:3100/sse"
    }
  }
}
```

**Or via stdio over SSH** (no port exposure needed):

```json
{
  "mcpServers": {
    "android": {
      "command": "ssh",
      "args": [
        "-F", "/dev/null",
        "u0_a280@192.168.1.33",
        "-p", "8022",
        "/data/data/com.termux/files/home/droid-mcp",
        "--transport", "stdio"
      ]
    }
  }
}
```

### VS Code (Continue, Cline, etc.)

These extensions have their own MCP configuration. Point them at the SSE endpoint:

```
http://192.168.1.33:3100/sse
```

### Generic / custom agent

Any MCP client library can connect. The protocol is standard:

1. `GET /sse` — open an SSE stream, receive `endpoint` event with POST URL
2. `POST /message?sessionId=<id>` — send JSON-RPC messages
3. Receive JSON-RPC responses as `message` events on the SSE stream

## Security model

### The policy file

`policy.toml` controls what tools are available and what operations are allowed:

```toml
[global]
allow_root = true            # Master kill-switch for all root tools
disabled_tools = ["shell_exec"]  # Disable the escape-hatch tool
max_output_bytes = 512000

[tools.pm_install]
enabled = true               # Can be set to false to block installs

[tools.pm_uninstall]
enabled = false              # Block uninstalls entirely
```

### Privilege levels

Tools are tagged with a privilege level at definition time:

- **User** — runs as the Termux user (`u0_a280`). No escalation.
- **Root** — runs via `su -c`. Requires a rooted device with su binary.

The policy engine checks the privilege level before execution. Setting `allow_root = false` disables all root tools globally.

### Confirmation-required tools

Some tools are marked `.confirm()` in the builder. These tools include `[requires confirmation]` in their description, signaling to the agent that it should ask the user before executing. The MCP protocol doesn't have a native confirmation mechanism, so this is a convention — the agent sees the flag and is expected to respect it.

### Input validation

Every parameter can have a validator. The built-in validators cover:
- **Package names**: must be alphanumeric + dots, must contain a dot
- **File paths**: no `..` traversal, no null bytes
- **Interface names**: alphanumeric only, max 16 chars

Add custom validators with `.validate("param_name", |v| ...)`.

### The shell_exec escape hatch

The `shell_exec` tool lets the agent run arbitrary commands. This is intentionally included because an agent doing development work needs flexibility, but you should consider:
- Disabling it in `policy.toml` for untrusted agents
- Running the MCP server in a restricted network segment
- Using SSH key auth (not password) for the stdio transport

## Adding new tools

The `ShellTool` builder in `src/builtins.rs` is the pattern for exposing any binary:

```rust
// Simple: read a file, substituting a parameter
registry.register(
    ShellTool::new("my_tool")
        .description("What this tool does")
        .param("arg1", ParamType::String, "Description", true)
        .validate("arg1", validate_package_name)
        .template("some_binary --flag {arg1}")
        .build(),
);

// Complex: dynamic script with multiple params
registry.register(
    ShellTool::new("complex_tool")
        .description("Does something complex")
        .param("package", ParamType::String, "Package name", true)
        .param("verbose", ParamType::Boolean, "Verbose output", false)
        .default_value(false)
        .root()           // Needs su
        .confirm()        // Agent should ask user first
        .timeout(60)      // Custom timeout (default: 30s)
        .script(|p| {
            let pkg = p.str("package").unwrap();
            let verbose = if p.bool("verbose").unwrap_or(false) { "-v" } else { "" };
            format!("dumpsys package {} {}", pkg, verbose)
        })
        .build(),
);

// Read a sysfs/procfs file
registry.register(
    ShellTool::new("read_sensor")
        .description("Read a sysfs sensor value")
        .param("path", ParamType::String, "Sysfs path under /sys/", true)
        .validate("path", |v| {
            let s = v.as_str().ok_or("must be string")?;
            if !s.starts_with("/sys/") { return Err("must be under /sys/".into()); }
            validate_safe_path(v)
        })
        .read_file(|p| p.str("path").unwrap().to_string())
        .build(),
);
```

### Custom tool trait

For tools that need more than shell commands (e.g., parsing output, making decisions, streaming), implement the `Tool` trait directly:

```rust
struct MyCustomTool;

#[async_trait::async_trait]
impl Tool for MyCustomTool {
    fn definition(&self) -> ToolDefinition {
        ToolDefinition {
            name: "my_custom_tool".into(),
            description: "Does something special".into(),
            input_schema: InputSchema {
                schema_type: "object".into(),
                properties: HashMap::new(),
                required: vec![],
            },
        }
    }

    fn privilege(&self) -> Privilege {
        Privilege::User
    }

    async fn invoke(&self, params: &serde_json::Map<String, Value>) -> ToolResult {
        // Custom logic here
        ToolResult::text("result")
    }
}

// Register it
registry.register_custom(MyCustomTool);
```

## Tool inventory

| Tool | Privilege | Confirm | Description |
|------|-----------|---------|-------------|
| `net_connect_info` | User | No | WiFi connection info from kernel |
| `net_interfaces` | User | No | List network interfaces |
| `wifi_info` | User | No | WiFi details via Termux API |
| `wifi_scan` | User | No | Scan nearby WiFi networks |
| `getprop` | User | No | Read Android system properties |
| `device_info` | User | No | Device/OS/hardware summary |
| `am_start` | Root | No | Launch an app/activity |
| `am_force_stop` | Root | Yes | Force-stop an app |
| `pm_list` | Root | No | List installed packages |
| `pm_path` | Root | No | Get APK path for a package |
| `pm_dump` | Root | No | Detailed package info |
| `pm_install` | Root | Yes | Install an APK |
| `pm_uninstall` | Root | Yes | Uninstall a package |
| `app_logcat` | Root | No | Logcat filtered by app PID |
| `app_pid` | Root | No | Get PID of a running app |
| `ps_list` | User | No | List processes |
| `tts_speak` | User | No | Speak text aloud |
| `battery_status` | User | No | Battery info |
| `clipboard_get` | User | No | Read clipboard |
| `clipboard_set` | User | No | Set clipboard |
| `notification_send` | User | No | Show a notification |
| `device_location` | User | No | GPS/network location |
| `vibrate` | User | No | Vibrate device |
| `toast` | User | No | Show toast message |
| `camera_photo` | User | No | Take a photo |
| `sensor_read` | User | No | Read sensor data |
| `shell_exec` | User* | No | Arbitrary shell command |
| `file_read` | User | No | Read a file |
| `file_write` | User | Yes | Write a file |

\* `shell_exec` has an opt-in `as_root` flag that escalates internally.

## Design notes

**Why Rust?** The binary runs on a resource-constrained device. A single static binary with no runtime dependencies is ideal for Termux. Startup is instant, memory is minimal, and there's no JVM to contend with.

**Why not the Android SDK / ADB?** This server runs *on* the device, not on a host talking to the device via ADB. The agent has direct access to the device's filesystem, processes, and hardware through Termux and root. ADB would add an unnecessary layer of indirection for your workflow.

**Why SSE over WebSocket?** MCP specifies SSE as its HTTP transport. It's simpler, unidirectional (server → client events, client → server POST), and works through more proxies. The MCP ecosystem standardized on it.

**Harness/IDE agnostic:** The SSE transport is a plain HTTP endpoint. Any MCP client — Claude Desktop, Claude Code, Continue, Cline, a custom Python script using the `mcp` SDK — connects the same way. The stdio transport works with any client that can spawn a subprocess (SSH in this case). You're not locked into any IDE.
