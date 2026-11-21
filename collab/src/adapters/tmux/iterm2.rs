use super::{tmux_command, validate_endpoint, TmuxEndpoint};
use std::collections::BTreeSet;

/// Documents an attach to this exact session on this instance of the server.
/// A pane's terminal ID and the tmux pane ID belong to different namespaces.
pub(crate) fn panes_for_endpoint(endpoint: &TmuxEndpoint) -> Result<BTreeSet<String>, String> {
    validate_endpoint(endpoint)?;
    #[cfg(feature = "lifecycle-test-hooks")]
    if std::env::var_os("COLLAB_TEST_ITERM2_ATTACHMENT").as_deref()
        == Some(std::ffi::OsStr::new("fixture"))
    {
        // Contract suites exercise the daemon with an isolated adapter fixture.
        // The real iTerm2 acceptance runs an ordinary build, without this seam.
        return Ok(BTreeSet::from([format!("fixture-iterm2-{}", endpoint.tmux_session_id)]));
    }
    let output = match tmux_command(
        &endpoint.socket_path,
        &["list-clients", "-F", "#{pid}|#{session_id}|#{client_tty}"],
    ) {
        Ok(output) => output,
        Err(error) if super::endpoint_is_absent(&error) => return Ok(BTreeSet::new()),
        Err(error) => return Err(format!("TMUX_CLIENT_OBSERVATION_FAILED: {error}")),
    };
    let inventory = std::str::from_utf8(&output.stdout)
        .map_err(|_| "TMUX_CLIENT_OBSERVATION_FAILED: non-UTF-8 response".to_owned())?;
    let ttys = current_client_ttys(inventory, endpoint)?;
    if ttys.is_empty() {
        return Ok(BTreeSet::new());
    }
    native_panes(&ttys)
}

fn current_client_ttys<'a>(
    inventory: &'a str,
    endpoint: &TmuxEndpoint,
) -> Result<BTreeSet<&'a str>, String> {
    let mut ttys = BTreeSet::new();
    for row in inventory.lines().filter(|row| !row.is_empty()) {
        let mut fields = row.splitn(3, '|');
        let (Some(pid), Some(session), Some(tty)) = (fields.next(), fields.next(), fields.next())
        else {
            return Err("TMUX_CLIENT_OBSERVATION_FAILED: invalid client row".into());
        };
        if pid.parse::<u32>().ok() == Some(endpoint.server_pid)
            && session == endpoint.tmux_session_id
            && tty.starts_with("/dev/")
            && !tty.chars().any(char::is_control)
        {
            ttys.insert(tty);
        }
    }
    Ok(ttys)
}

#[cfg(target_os = "macos")]
fn native_panes(ttys: &BTreeSet<&str>) -> Result<BTreeSet<String>, String> {
    use std::process::Command;

    // Check running before sending Apple events, so observation cannot launch
    // iTerm2. Pass exact client TTYs as data and return only matching pane IDs.
    const SCRIPT: &str = r#"on run argv
        set matched to {}
        if application id "com.googlecode.iterm2" is running then
            with timeout of 3 seconds
                tell application id "com.googlecode.iterm2"
                    repeat with w in windows
                        repeat with t in tabs of w
                            repeat with s in sessions of t
                                if (tty of s) is in argv then set end of matched to (id of s as text)
                            end repeat
                        end repeat
                    end repeat
                end tell
            end timeout
        end if
        set AppleScript's text item delimiters to linefeed
        return matched as text
    end run"#;
    let response = Command::new("/usr/bin/osascript")
        .args(["-e", SCRIPT, "--"])
        .args(ttys)
        .output()
        .map_err(|error| format!("ITERM2_PANE_OBSERVATION_FAILED: {error}"))?;
    if !response.status.success() {
        return Err(format!(
            "ITERM2_PANE_OBSERVATION_FAILED: {}",
            String::from_utf8_lossy(&response.stderr).trim()
        ));
    }
    let ids = std::str::from_utf8(&response.stdout)
        .map_err(|_| "ITERM2_PANE_OBSERVATION_FAILED: non-UTF-8 response".to_owned())?;
    Ok(verified_pane_ids(ids))
}

#[cfg(any(target_os = "macos", test))]
fn verified_pane_ids(ids: &str) -> BTreeSet<String> {
    ids
        .lines()
        .map(str::trim)
        .filter(|id| !id.is_empty() && !id.chars().any(char::is_control))
        .map(str::to_owned)
        .collect()
}

#[cfg(not(target_os = "macos"))]
fn native_panes(_: &BTreeSet<&str>) -> Result<BTreeSet<String>, String> {
    Ok(BTreeSet::new())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn multiple_verified_views_of_the_same_session_are_preserved() {
        assert_eq!(
            verified_pane_ids("second\nfirst\nsecond\n"),
            BTreeSet::from(["first".into(), "second".into()])
        );
    }

    #[test]
    fn client_ttys_match_the_current_server_and_exact_session() {
        let endpoint = TmuxEndpoint {
            socket_path: "/tmp/tmux.sock".into(),
            server_pid: 17,
            tmux_session_id: "$2".into(),
            pane_id: "%1".into(),
            pane_pid: 18,
            codex_session_id: None,
            codex_thread_id: None,
        };
        let rows = "16|$2|/dev/ttys001\n17|$20|/dev/ttys002\n17|$2|/dev/ttys003\n17|$2|/dev/ttys004\n";
        assert_eq!(
            current_client_ttys(rows, &endpoint).unwrap(),
            BTreeSet::from(["/dev/ttys003", "/dev/ttys004"])
        );
        assert!(current_client_ttys("17|$2\n", &endpoint).is_err());
    }
}
