use serde_json::{json, Value};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

struct Scratch(PathBuf);
impl Drop for Scratch {
    fn drop(&mut self) {
        std::fs::remove_dir_all(&self.0).unwrap();
    }
}

fn isolated(binary: &str, root: &Path) -> Command {
    let mut command = Command::new(binary);
    command
        .current_dir(root)
        .env("HOME", root)
        .env("CODEX_HOME", root)
        .env("COLLAB_STATE_DIR", root.join("state"))
        .env_remove("CODEX_SESSION_ID")
        .env_remove("CODEX_THREAD_ID")
        .env_remove("CODEX_APP_TOOLS_PIPE_PATH")
        .env_remove("TMUX")
        .env_remove("TMUX_PANE")
        .env_remove("COLLAB_WORKER");
    command
}

#[test]
fn cli_and_mcp_reject_removed_migration_without_creating_state() {
    let root = Scratch(std::env::temp_dir().join(format!(
        "collab-removed-migration-{}-{:032x}",
        std::process::id(),
        rand::random::<u128>()
    )));
    std::fs::create_dir(&root.0).unwrap();
    let cli = env!("CARGO_BIN_EXE_collab");
    let help = isolated(cli, &root.0).arg("--help").output().unwrap();
    assert!(help.status.success());
    let help = String::from_utf8(help.stdout).unwrap();
    assert!(!help
        .lines()
        .any(|line| line.trim_start().starts_with("migrate ")));
    for action in ["--help", "inspect", "plan", "apply", "verify"] {
        let output = isolated(cli, &root.0)
            .args(["migrate", action])
            .output()
            .unwrap();
        let error = String::from_utf8(output.stderr).unwrap();
        assert!(!output.status.success(), "{action}: {error}");
        assert!(
            error.contains("unrecognized subcommand 'migrate'"),
            "{action}: {error}"
        );
    }
    let mut mcp = isolated(env!("CARGO_BIN_EXE_collab-mcp"), &root.0)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let mut input = mcp.stdin.take().unwrap();
    for request in [
        json!({"jsonrpc":"2.0","id":1,"method":"tools/list"}),
        json!({"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"collab_migrate","arguments":{"action":"inspect"}}}),
    ] {
        writeln!(input, "{request}").unwrap();
    }
    drop(input);
    let output = mcp.wait_with_output().unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let responses: Vec<Value> = String::from_utf8(output.stdout)
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();
    let listed = responses.iter().find(|value| value["id"] == 1).unwrap();
    let tools = listed["result"]["tools"].as_array().unwrap();
    assert!(tools.iter().any(|tool| tool["name"] == "collab_context"));
    assert!(tools.iter().all(|tool| tool["name"] != "collab_migrate"));
    let refused = responses.iter().find(|value| value["id"] == 2).unwrap();
    assert_eq!(refused["result"]["isError"], true, "{refused}");
    assert!(
        refused["result"]["content"][0]["text"]
            .as_str()
            .unwrap()
            .contains("unknown tool collab_migrate"),
        "{refused}"
    );
    assert!(!root.0.join("state").exists());
    assert!(!root.0.join(".agent-collab").exists());
}
