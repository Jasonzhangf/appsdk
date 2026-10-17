use std::env;
use std::fs;
use std::path::PathBuf;
use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

fn binary() -> PathBuf {
    env::var_os("APPSDK_TEST_BINARY")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(env!("CARGO_BIN_EXE_appsdk")))
}

/// Unique scratch area under the worktree `target/`; removed on drop.
struct Scratch {
    base: PathBuf,
}

impl Scratch {
    fn new() -> Self {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let base = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("target")
            .join(format!(
                "appsdk-guidance-ablation-{}-{}",
                std::process::id(),
                nanos
            ));
        fs::create_dir_all(base.join("appsdk-home")).unwrap();
        fs::create_dir_all(base.join("collab-state")).unwrap();
        Self { base }
    }

    fn root(&self) -> PathBuf {
        self.base.join("project")
    }

    fn cmd(&self) -> Command {
        let mut command = Command::new(binary());
        command
            .env("APPSDK_HOME", self.base.join("appsdk-home"))
            .env("COLLAB_STATE_DIR", self.base.join("collab-state"));
        command
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.base);
    }
}

/// Red test: a compiled Guidance artifact without `manifest_hash` must still be
/// consumable by the public `guide status` command.
#[test]
fn guide_status_does_not_require_compiled_manifest_hash() {
    let scratch = Scratch::new();
    let root = scratch.root();
    let root_arg = root.to_str().unwrap();

    let new = scratch.cmd().args(["new", root_arg]).output().unwrap();
    assert!(
        new.status.success(),
        "new failed: {}{}",
        String::from_utf8_lossy(&new.stdout),
        String::from_utf8_lossy(&new.stderr)
    );
    let compile = scratch
        .cmd()
        .args(["guide", "compile", root_arg])
        .output()
        .unwrap();
    assert!(
        compile.status.success(),
        "guide compile failed: {}{}",
        String::from_utf8_lossy(&compile.stdout),
        String::from_utf8_lossy(&compile.stderr)
    );

    let compiled_path = root.join(".appsdk/guidance/compiled.json");
    let compiled: serde_json::Value =
        serde_json::from_slice(&fs::read(&compiled_path).unwrap()).unwrap();
    assert!(compiled.get("manifest_hash").is_none());
    assert!(compiled.get("project_contract_digest").is_none());
    for source in compiled["sources"].as_array().unwrap() {
        assert!(source.get("digest").is_none());
        assert!(source.get("contract_digest").is_none());
    }

    let status = scratch
        .cmd()
        .args(["guide", "status", root_arg])
        .output()
        .unwrap();
    let stdout = String::from_utf8_lossy(&status.stdout).to_string();
    let stderr = String::from_utf8_lossy(&status.stderr).to_string();

    assert!(
        status.status.success(),
        "guide status must succeed when a valid compiled artifact lacks manifest_hash\nexit={:?}\nstdout={}\nstderr={}",
        status.status.code(),
        stdout,
        stderr
    );
    serde_json::from_str::<serde_json::Value>(&stdout).unwrap_or_else(|e| {
        panic!("status stdout is not JSON: {e}\nstdout={stdout}\nstderr={stderr}")
    });
}
