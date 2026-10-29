use std::path::{Path, PathBuf};
use std::process::Command;

use crate::scope;

pub const LAUNCH_AGENT_LABEL: &str = "com.appsdk.collab";
pub const LAUNCH_AGENTS_DIR: &str = "Library/LaunchAgents";
pub const AUTOSTART_LABEL_ENV: &str = "COLLAB_AUTOSTART_LABEL";

/// Label used for the login agent. Overridable so an isolated end-to-end run
/// can register a throwaway label (for example `com.appsdk.collab.e2e.<pid>`)
/// and never collide with, or restart, the real login agent.
fn launch_agent_label() -> String {
    std::env::var(AUTOSTART_LABEL_ENV).unwrap_or_else(|_| LAUNCH_AGENT_LABEL.to_string())
}

fn launch_agent_path() -> PathBuf {
    let home = std::env::var_os("HOME").expect("HOME must be set");
    let label = launch_agent_label();
    Path::new(&home)
        .join(LAUNCH_AGENTS_DIR)
        .join(format!("{label}.plist"))
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
    let label = launch_agent_label();
    let state_dir = std::env::var_os(scope::COLLAB_STATE_DIR_ENV).map(|value| {
        format!(
            "        <key>{}</key>\n        <string>{}</string>",
            scope::COLLAB_STATE_DIR_ENV,
            value.to_string_lossy()
        )
    });
    let state_dir = state_dir.as_deref().unwrap_or_default();
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
{state_dir}
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
        label = label,
        bin = collab_bin.display(),
        log = log_path,
        home = home,
        state_dir = state_dir,
    )
}

/// A supervised login agent owns the single default host state root. An
/// explicitly overridden state root is allowed only for the throwaway-label
/// seam used by isolated end-to-end runs, so a real login install never
/// registers a second daemon for an isolated runtime.
fn validate_state_override() -> anyhow::Result<()> {
    if std::env::var_os(scope::COLLAB_STATE_DIR_ENV).is_some()
        && std::env::var_os(AUTOSTART_LABEL_ENV).is_none()
    {
        anyhow::bail!(
            "AUTOSTART_UNAVAILABLE: {} is set, so this is an isolated Collab instance; login autostart is only installed for the default host state root",
            scope::COLLAB_STATE_DIR_ENV
        );
    }
    Ok(())
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
    // login autostart is a macOS LaunchAgent feature only. A non-macOS host has
    // no supervisor to own crash recovery, so report it as unavailable rather
    // than fail on the missing `launchctl` binary.
    if cfg!(not(target_os = "macos")) {
        anyhow::bail!("AUTOSTART_UNAVAILABLE: login autostart is only implemented on macOS");
    }

    validate_state_override()?;
    let collab_bin = collab_bin_path();
    let plist = render_plist(&collab_bin);
    let path = launch_agent_path();
    std::fs::create_dir_all(path.parent().expect("LaunchAgents dir"))?;
    // launchd opens StandardOutPath/StandardErrorPath before the daemon runs,
    // so the host state root must already exist or the agent fails to start.
    // `render_plist` derives the log path from this same override, keeping the
    // isolated supervised daemon out of the default-root legacy fence.
    std::fs::create_dir_all(scope::HostPaths::resolve()?.state_root())?;
    std::fs::write(&path, plist)?;
    if is_loaded() {
        // The agent is already loaded for this login session. A clean
        // `collab down` leaves the job loaded but stopped, so launchd would
        // have no child to relaunch after a crash. Resume the loaded job here;
        // a job that is already running is left untouched so the live daemon
        // is never replaced.
        if !is_running() {
            start_loaded_job()?;
        }
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

/// Start a loaded-but-stopped LaunchAgent job without killing a running one.
///
/// `launchctl start <label>` resumes the job in place, unlike
/// `launchctl kickstart -k` which first kills an existing instance. A real
/// launchd rejection is surfaced as an error instead of a silent success.
fn start_loaded_job() -> anyhow::Result<()> {
    let label = launch_agent_label();
    let output = Command::new("launchctl").args(["start", &label]).output()?;
    if !output.status.success() {
        anyhow::bail!(
            "launchctl start failed for {} ({}): {}{}",
            label,
            output.status,
            String::from_utf8_lossy(&output.stdout).trim(),
            String::from_utf8_lossy(&output.stderr).trim(),
        );
    }
    Ok(())
}

/// Returns true when the LaunchAgent job is currently loaded.
pub fn is_loaded() -> bool {
    let path = launch_agent_path();
    if !path.exists() {
        return false;
    }
    let label = launch_agent_label();
    let output = Command::new("launchctl").args(["list", &label]).output();
    matches!(output, Ok(o) if o.status.success())
}

/// Returns true when the loaded LaunchAgent job currently has a running PID.
///
/// `launchctl list <label>` prints the job as a dictionary such as
/// `"PID" = 69610;` while it runs, and omits the `PID` entry (or prints
/// `"PID" = "-";`) when the job is loaded but stopped.
pub fn is_running() -> bool {
    if !is_loaded() {
        return false;
    }
    let label = launch_agent_label();
    let output = Command::new("launchctl").args(["list", &label]).output();
    let Ok(output) = output else {
        return false;
    };
    if !output.status.success() {
        return false;
    }
    launchctl_list_reports_pid(&String::from_utf8_lossy(&output.stdout))
}

fn launchctl_list_reports_pid(text: &str) -> bool {
    text.lines().any(|line| {
        let line = line.trim();
        line.starts_with("\"PID\"")
            && line
                .split_once('=')
                .is_some_and(|(_, value)| value.chars().any(|c| c.is_ascii_digit()))
    })
}

#[cfg(test)]
mod plist_tests {
    use super::*;

    struct EnvGuard {
        key: &'static str,
        previous: Option<std::ffi::OsString>,
    }

    impl EnvGuard {
        fn set(key: &'static str, value: &Path) -> Self {
            let previous = std::env::var_os(key);
            std::env::set_var(key, value);
            Self { key, previous }
        }

        fn set_value(key: &'static str, value: &str) -> Self {
            let previous = std::env::var_os(key);
            std::env::set_var(key, value);
            Self { key, previous }
        }

        fn remove(key: &'static str) -> Self {
            let previous = std::env::var_os(key);
            std::env::remove_var(key);
            Self { key, previous }
        }
    }

    impl Drop for EnvGuard {
        fn drop(&mut self) {
            match &self.previous {
                Some(value) => std::env::set_var(self.key, value),
                None => std::env::remove_var(self.key),
            }
        }
    }

    #[test]
    fn plist_runs_serve_service_at_load() {
        let plist = render_plist(Path::new("/usr/local/bin/collab"));
        assert!(
            plist.contains("<key>RunAtLoad</key>\n    <true/>"),
            "{plist}"
        );
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
        assert!(
            !plist.contains("<key>KeepAlive</key>\n    <true/>"),
            "{plist}"
        );
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

    #[test]
    fn running_is_detected_from_the_launchd_pid_entry() {
        // `launchctl list <label>` prints a dictionary. A running job carries a
        // numeric PID; a loaded-but-stopped job (after a clean `collab down`)
        // omits it or uses "-". A parser that only checks for a non-empty line
        // would always report "running" and never resume a stopped job.
        assert!(launchctl_list_reports_pid(
            "{\n\t\"Label\" = \"com.appsdk.collab\";\n\t\"PID\" = 69610;\n}\n"
        ));
        assert!(!launchctl_list_reports_pid(
            "{\n\t\"Label\" = \"com.appsdk.collab\";\n\t\"LastExitStatus\" = 0;\n}\n"
        ));
        assert!(!launchctl_list_reports_pid("{\n\t\"PID\" = \"-\";\n}\n"));
    }

    #[test]
    fn isolated_label_passes_the_state_root_into_the_plist() {
        let _guard = crate::scope::TEST_ENV_LOCK
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let root = std::env::temp_dir().join(format!(
            "collab-autostart-state-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let home = root.join("home");
        let state = root.join("state");
        std::fs::create_dir_all(&home).unwrap();
        std::fs::create_dir_all(&state).unwrap();
        let _home = EnvGuard::set("HOME", &home);
        let _state = EnvGuard::set(scope::COLLAB_STATE_DIR_ENV, &state);
        let _label =
            EnvGuard::set_value(AUTOSTART_LABEL_ENV, "com.appsdk.collab.e2e.state-root-test");

        let plist = render_plist(Path::new("/tmp/collab"));
        assert!(
            plist.contains(&format!(
                "<key>{}</key>\n        <string>{}</string>",
                scope::COLLAB_STATE_DIR_ENV,
                state.display()
            )),
            "{plist}"
        );
        assert!(
            plist.contains(&format!(
                "<string>{}</string>",
                state.join("serve.log").display()
            )),
            "{plist}"
        );
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn state_root_override_requires_an_isolated_label() {
        let _guard = crate::scope::TEST_ENV_LOCK
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let state = std::env::temp_dir().join(format!(
            "collab-autostart-state-reject-{}",
            std::process::id()
        ));
        let _state = EnvGuard::set(scope::COLLAB_STATE_DIR_ENV, &state);
        let _label = EnvGuard::remove(AUTOSTART_LABEL_ENV);

        let error = validate_state_override().unwrap_err();
        assert!(
            error.to_string().starts_with("AUTOSTART_UNAVAILABLE:"),
            "{error:#}"
        );

        std::env::set_var(AUTOSTART_LABEL_ENV, "com.appsdk.collab.e2e.allowed");
        validate_state_override().expect("an isolated label permits an isolated state root");
    }
}
