use std::path::Path;
use std::process::Command;

/// Run the same pane command channel under a foreground process whose actual
/// executable name is recognized by the production agent observation.
pub fn command(root: &Path) -> String {
    command_named(root, "codex")
}

pub fn command_named(root: &Path, binary_name: &str) -> String {
    let python = Command::new("python3")
        .args([
            "-c",
            r#"import os, sys
executable = sys.executable
if sys.platform == "darwin":
    # A framework build can report its launcher instead of the running binary.
    import ctypes
    path = ctypes.create_string_buffer(4096)
    libproc = ctypes.CDLL("/usr/lib/libproc.dylib")
    assert libproc.proc_pidpath(os.getpid(), path, len(path)) > 0
    executable = os.fsdecode(path.value)
print(executable)
print(sys.base_prefix)"#,
        ])
        .output()
        .expect("find the pane runner's Python");
    assert!(python.status.success());
    let python = String::from_utf8(python.stdout).unwrap();
    let mut lines = python.lines();
    let executable = root.join(binary_name);
    if !executable.exists() {
        std::fs::copy(lines.next().unwrap(), &executable).unwrap();
    } else {
        let _ = lines.next();
    }
    let home = lines.next().unwrap();
    let quote = |value: &str| format!("'{}'", value.replace('\'', "'\\''"));
    format!(
        "env {} {}",
        quote(&format!("PYTHONHOME={home}")),
        quote(executable.to_str().unwrap())
    )
}
