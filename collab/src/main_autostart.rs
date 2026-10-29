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
/// autostart; `KeepAlive.SuccessfulExit=false` restarts the service only after
/// an unsuccessful exit (crash recovery), never after a clean `collab down`.
/// That is what lets a manual `down` stay stopped for the current session
/// without the persistent `launchctl unload -w` disable that would also cancel
/// the next login. The plist stays installed, so the next login re-runs it.
fn render_plist(collab_bin: &Path) -> String {
    let log_path = scope::HostPaths::resolve()
        .map(|p| p.state_root().join("serve.log").display().to_string())
        .unwrap_or_else(|_| "/tmp/collab-serve.log".to_string());
    let home = std::env::var("HOME").unwrap_or_default();
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
    <key>EnvironmentVariables</key>
    <dict>
        <key>HOME</key>
        <string>{home}</string>
    </dict>
    <key>RunAtLoad</key>
    <true/>
    <key>KeepAlive</key>
    <dict>
        <key>SuccessfulExit</key>
        <false/>
    </dict>
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
        home = home,
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
    // The login agent must own the single real host daemon. An explicitly
    // overridden state root is an isolated instance (tests, temporary runs);
    // registering it for login autostart would make two daemons compete for one
    // login session and would outlive the caller that started it.
    if std::env::var_os(scope::COLLAB_STATE_DIR_ENV).is_some() {
        anyhow::bail!(
            "AUTOSTART_UNAVAILABLE: {} is set, so this is an isolated Collab instance; login autostart is only installed for the default host state root",
            scope::COLLAB_STATE_DIR_ENV
        );
    }
    let collab_bin = collab_bin_path();
    let plist = render_plist(&collab_bin);
    let path = launch_agent_path();
    std::fs::create_dir_all(path.parent().expect("LaunchAgents dir"))?;
    // launchd opens StandardOutPath/StandardErrorPath before the daemon runs,
    // so the host state root must already exist or the agent fails to start.
    std::fs::create_dir_all(scope::HostPaths::resolve()?.state_root())?;
    std::fs::write(&path, plist)?;
    if is_loaded() {
        // The agent is already loaded for this login session; rewriting the
        // plist is enough and must not restart the running daemon.
        return Ok(path);
    }
    let output = Command::new("launchctl")
        .args(["load", "-w", &path.display().to_string()])
        .output()?;
    if !output.status.success() {
        // Report the real launchd failure instead of claiming the autostart
        // was installed.
        anyhow::bail!(
            "launchctl load failed for {} ({}): {}{}",
            path.display(),
            output.status,
            String::from_utf8_lossy(&output.stdout).trim(),
            String::from_utf8_lossy(&output.stderr).trim(),
        );
    }
    Ok(path)
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

#[cfg(test)]
mod plist_tests {
    use super::*;

    #[test]
    fn plist_runs_serve_service_at_load() {
        let plist = render_plist(Path::new("/usr/local/bin/collab"));
        assert!(plist.contains("<key>RunAtLoad</key>\n    <true/>"), "{plist}");
        assert!(plist.contains("<string>serve</string>"), "{plist}");
        assert!(plist.contains("<string>--service</string>"), "{plist}");
        assert!(plist.contains(LAUNCH_AGENT_LABEL), "{plist}");
    }

    #[test]
    fn keepalive_restarts_only_on_unsuccessful_exit() {
        // A clean `collab down` exits 0; launchd must NOT relaunch it, or the
        // daemon would come straight back and manual close would be impossible.
        // Crash recovery still needs a relaunch, so SuccessfulExit=false.
        let plist = render_plist(Path::new("/usr/local/bin/collab"));
        assert!(
            plist.contains("<key>KeepAlive</key>\n    <dict>\n        <key>SuccessfulExit</key>\n        <false/>"),
            "{plist}"
        );
        // Guard against the old always-restart boolean form coming back.
        assert!(!plist.contains("<key>KeepAlive</key>\n    <true/>"), "{plist}");
    }

    #[test]
    fn plist_pins_the_installing_users_home() {
        // The agent must resolve the same host state root and configuration as
        // the installer, rather than whatever HOME launchd happens to give it.
        let home = std::env::var("HOME").expect("HOME");
        let plist = render_plist(Path::new("/usr/local/bin/collab"));
        assert!(
            plist.contains("<key>EnvironmentVariables</key>\n    <dict>"),
            "{plist}"
        );
        assert!(
            plist.contains(&format!("<key>HOME</key>\n        <string>{home}</string>")),
            "{plist}"
        );
    }
}
