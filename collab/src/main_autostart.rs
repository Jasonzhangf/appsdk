use std::path::{Path, PathBuf};
use std::process::Command;

use crate::scope;

pub const LAUNCH_AGENT_LABEL: &str = "com.appsdk.collab";
pub const LAUNCH_AGENTS_DIR: &str = "Library/LaunchAgents";

fn launch_agent_path() -> PathBuf {
    let home = std::env::var_os("HOME").expect("HOME must be set");
    Path::new(&home)
        .join(LAUNCH_AGENTS_DIR)
        .join(format!("{LAUNCH_AGENT_LABEL}.plist"))
}

/// Resolve the user"s installed `collab` binary: prefer `cargo bin`, fall back
/// to the current executable"s own directory when installed elsewhere.
fn collab_bin_path() -> PathBuf {
    if let Ok(cargo_bin) = std::env::var("CARGO_BIN") {
        return PathBuf::from(cargo_bin).join("collab");
    }
    let exe = std::env::current_exe().expect("current_exe available");
    exe.parent()
        .map(|p| p.join("collab"))
        .unwrap_or_else(|| PathBuf::from("collab"))
}

/// Render the per-user LaunchAgent plist. RunAtLoad guarantees login-time
/// autostart; KeepAlive restarts the service only when the job is loaded and
/// the process exits (crash recovery). An explicit `collab down` unloads the
/// job for the current session and does not permanently disable autostart.
fn render_plist(collab_bin: &Path) -> String {
    let log_path = scope::HostPaths::resolve()
        .map(|p| p.state_root().join("serve.log").display().to_string())
        .unwrap_or_else(|_| "/tmp/collab-serve.log".to_string());
    format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>Label</key>
    <string>{label}</string>
    <key>ProgramArguments</key>
    <array>
        <string>{bin}</string>
        <string>serve</string>
        <string>--service</string>
    </array>
    <key>RunAtLoad</key>
    <true/>
    <key>KeepAlive</key>
    <true/>
    <key>StandardOutPath</key>
    <string>{log}</string>
    <key>StandardErrorPath</key>
    <string>{log}</string>
    <key>ProcessType</key>
    <string>Interactive</string>
</dict>
</plist>
"#,
        label = LAUNCH_AGENT_LABEL,
        bin = collab_bin.display(),
        log = log_path,
    )
}

/// Write the durable service record binding the supervised daemon to the
/// exact canonical project root. `collab serve --service` reads this to pick
/// its scope root without a project cwd.
pub fn write_service_record(root: &Path) -> anyhow::Result<()> {
    let host_paths = scope::HostPaths::resolve()?;
    let state_root = host_paths.state_root();
    std::fs::create_dir_all(state_root)?;
    let record = serde_json::json!({
        "service_scope_root": root,
    });
    std::fs::write(
        state_root.join("service-record.json"),
        serde_json::to_vec_pretty(&record)?,
    )?;
    Ok(())
}

/// Install the LaunchAgent plist and load it so the daemon autostarts on login
/// and after crashes. Idempotent: overwrites an existing plist and reuses the
/// loaded job if present.
pub fn install_and_load() -> anyhow::Result<PathBuf> {
    let collab_bin = collab_bin_path();
    let plist = render_plist(&collab_bin);
    let path = launch_agent_path();
    std::fs::create_dir_all(path.parent().expect("LaunchAgents dir"))?;
    std::fs::write(&path, plist)?;
    let _: std::process::ExitStatus = Command::new("launchctl")
        .args(["load", "-w", &path.display().to_string()])
        .status()?;
    Ok(path)
}

/// Unload the LaunchAgent job for the current session. This is the action
/// behind `collab down`: the plist file is retained, so the next login reloads
/// it and autostart resumes; an explicit `collab up` also reloads it.
pub fn unload_job() -> anyhow::Result<()> {
    let path = launch_agent_path();
    if !path.exists() {
        return Ok(());
    }
    let _: std::process::ExitStatus = Command::new("launchctl")
        .args(["unload", "-w", &path.display().to_string()])
        .status()?;
    Ok(())
}

/// Returns true when the LaunchAgent job is currently loaded.
pub fn is_loaded() -> bool {
    let path = launch_agent_path();
    if !path.exists() {
        return false;
    }
    let label = LAUNCH_AGENT_LABEL;
    let output = Command::new("launchctl").args(["list", label]).output();
    matches!(output, Ok(o) if o.status.success())
}
