use crate::tools::*;

/// Register curated tools that need custom logic beyond what
/// the manifest's passthrough pattern can express.
///
/// Most tools should go in tools.toml instead. Only put tools here if they:
/// - Chain multiple commands (e.g., pidof → logcat)
/// - Read sysfs/procfs files directly (no binary invocation)
/// - Need complex parameter validation or transformation
pub fn register_all(registry: &mut ToolRegistry) {
    // ── Kernel sysfs reads (no binary, just file reads) ────────────

    registry.register(
        ShellTool::new("net_connect_info")
            .description(
                "Read WiFi/network connection info directly from the kernel. \
                 Returns detailed connection state: SSID, BSSID, signal strength, \
                 bitrate, auth type, frequency, channel width.",
            )
            .param("interface", ParamType::String, "Network interface name", false)
            .default_value("wlan0")
            .validate("interface", validate_interface_name)
            .read_file(|p| {
                format!(
                    "/sys/class/net/{}/connect_info",
                    p.str_or("interface", "wlan0")
                )
            })
            .build(),
    );

    // ── Multi-step: pidof + logcat in one call ─────────────────────

    registry.register(
        ShellTool::new("app_logcat")
            .description(
                "Get recent logcat output for a running app, filtered by PID. \
                 This is a convenience tool that chains `pidof` and `logcat` — \
                 use the individual `app_pid` and `logcat` tools if you need \
                 more control over flags.",
            )
            .param("package", ParamType::String, "Package name (e.g., com.jesse.hostctl)", true)
            .param("lines", ParamType::Integer, "Number of recent log lines (default: 100)", false)
            .default_value(100)
            .param("level", ParamType::String, "Min log level: V, D, I, W, E, F", false)
            .validate("package", validate_package_name)
            .root()
            .timeout(15)
            .script(|p| {
                let pkg = p.str("package").unwrap();
                let lines = p.int_or("lines", 100);
                let level_filter = match p.str("level") {
                    Some(l) => format!(" *:{}", l),
                    None => String::new(),
                };
                format!(
                    "PID=$(pidof {pkg}) && \
                     if [ -z \"$PID\" ]; then echo 'Process not running'; exit 1; fi && \
                     logcat --pid $PID -d -t {lines}{level_filter}"
                )
            })
            .build(),
    );

    // ── Device summary (multiple getprop calls + procfs reads) ─────

    registry.register(
        ShellTool::new("device_info")
            .description(
                "Get a consolidated device summary: model, Android version, \
                 kernel, memory, and storage in one call.",
            )
            .script(|_| {
                [
                    "echo '=== Device ==='",
                    "getprop ro.product.model",
                    "getprop ro.product.brand",
                    "echo '=== Android ==='",
                    "getprop ro.build.version.release",
                    "getprop ro.build.version.sdk",
                    "echo '=== Kernel ==='",
                    "uname -a",
                    "echo '=== Memory ==='",
                    "cat /proc/meminfo | head -3",
                    "echo '=== Storage ==='",
                    "df -h /data /storage/emulated/0 2>/dev/null",
                ]
                .join(" && ")
            })
            .build(),
    );

    // ── Network interfaces (combines ls + ip) ──────────────────────

    registry.register(
        ShellTool::new("net_interfaces")
            .description("List all network interfaces with their addresses and state")
            .template("ls /sys/class/net/ && echo '---' && ip -brief addr show")
            .build(),
    );

    // ── Arbitrary shell (escape hatch) ─────────────────────────────

    registry.register(
        ShellTool::new("shell_exec")
            .description(
                "Execute an arbitrary shell command in the Termux environment. \
                 Use when no specific tool covers what you need. \
                 Commands run as the Termux user by default.",
            )
            .param("command", ParamType::String, "Shell command to execute", true)
            .param("as_root", ParamType::Boolean, "Run as root via su", false)
            .default_value(false)
            .script(|p| {
                let cmd = p.str("command").unwrap();
                if p.bool("as_root").unwrap_or(false) {
                    format!("su -c {}", crate::tools::shell_escape(cmd))
                } else {
                    cmd.to_string()
                }
            })
            .timeout(60)
            .build(),
    );

    // ── File operations (need path validation) ─────────────────────

    registry.register(
        ShellTool::new("file_read")
            .description("Read contents of a file on the device")
            .param("path", ParamType::String, "File path to read", true)
            .param("lines", ParamType::Integer, "Max lines (0 = all, default 200)", false)
            .default_value(200)
            .validate("path", validate_safe_path)
            .script(|p| {
                let path = p.str("path").unwrap();
                let lines = p.int_or("lines", 200);
                if lines == 0 {
                    format!("cat '{}'", path)
                } else {
                    format!("head -n {} '{}'", lines, path)
                }
            })
            .build(),
    );

    registry.register(
        ShellTool::new("file_write")
            .description("Write content to a file on the device [DESTRUCTIVE]")
            .param("path", ParamType::String, "File path to write", true)
            .param("content", ParamType::String, "File contents", true)
            .param("append", ParamType::Boolean, "Append instead of overwrite", false)
            .default_value(false)
            .validate("path", validate_safe_path)
            .confirm()
            .script(|p| {
                let path = p.str("path").unwrap();
                let content = p.str("content").unwrap();
                let op = if p.bool("append").unwrap_or(false) { ">>" } else { ">" };
                format!(
                    "cat <<'DROID_MCP_EOF' {} '{}'\n{}\nDROID_MCP_EOF",
                    op, path, content
                )
            })
            .build(),
    );
}
