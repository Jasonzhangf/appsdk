//! User maintenance must survive project replay failure. Reuse the existing
//! public integration executable. No production daemon
//! or terminal host is involved: this test owns its child, HOME and socket.
use serde_json::{json, Value};
use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixStream;
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

struct OwnedChild(Child);
impl Drop for OwnedChild {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

fn request(socket: &std::path::Path, value: Value) -> Value {
    let mut stream = UnixStream::connect(socket).unwrap();
    stream
        .set_read_timeout(Some(Duration::from_secs(5)))
        .unwrap();
    writeln!(stream, "{value}").unwrap();
    let mut line = String::new();
    BufReader::new(stream).read_line(&mut line).unwrap();
    serde_json::from_str(&line).unwrap()
}

#[test]
fn degraded_daemon_keeps_authenticated_user_shutdown_available() {
    let root = std::env::temp_dir().join(format!(
        "cm-{}-{}",
        std::process::id(),
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_millis()
    ));
    let home = root.join("home");
    let host = root.join("host");
    std::fs::create_dir_all(root.join(".agent-collab/server")).unwrap();
    std::fs::create_dir_all(home.join(".appsdk")).unwrap();
    let journal = root.join(".agent-collab/server/journal.jsonl");
    std::fs::write(&journal, b"{malformed-fixture}\n").unwrap();
    std::fs::write(
        home.join(".appsdk/config.toml"),
        "[maintenance]\npassword = 'test-owned-maintenance'\n",
    )
    .unwrap();
    let log = std::fs::File::create(root.join("daemon.log")).unwrap();
    let start = || {
        Command::new(env!("CARGO_BIN_EXE_collab"))
            .arg("serve")
            .current_dir(&root)
            .env_clear()
            .env("PATH", "/usr/bin:/bin:/usr/sbin:/sbin")
            .env("HOME", &home)
            .env("COLLAB_STATE_DIR", &host)
            .stdin(Stdio::null())
            .stdout(log.try_clone().unwrap())
            .stderr(log.try_clone().unwrap())
            .spawn()
            .unwrap()
    };
    let mut child = OwnedChild(start());
    let socket = host.join("server.sock");
    let deadline = Instant::now() + Duration::from_secs(30);
    while !socket.exists() && Instant::now() < deadline {
        assert!(
            child.0.try_wait().unwrap().is_none(),
            "isolated daemon exited"
        );
        std::thread::sleep(Duration::from_millis(20));
    }
    assert!(
        socket.exists(),
        "isolated daemon did not become ready: {}",
        std::fs::read_to_string(root.join("daemon.log")).unwrap_or_default()
    );
    assert_ne!(
        request(&socket, json!({"op":"Ping"}))["degraded"],
        true,
        "retired local corruption must not enter current runtime"
    );
    child.0.kill().unwrap();
    child.0.wait().unwrap();
    let journals = super::central_storage::runtime_journals(&host.join("projects"));
    assert_eq!(journals.len(), 1);
    let journal = journals[0].clone();
    std::fs::write(&journal, b"{malformed-fixture}\n").unwrap();
    std::fs::remove_file(&socket).ok();
    child.0 = start();
    let deadline = Instant::now() + Duration::from_secs(30);
    while !socket.exists() && Instant::now() < deadline {
        assert!(child.0.try_wait().unwrap().is_none());
        std::thread::sleep(Duration::from_millis(20));
    }
    assert_eq!(request(&socket, json!({"op":"Ping"}))["degraded"], true);
    // Keep another client connected: successful maintenance must also release it.
    let mut idle_client = UnixStream::connect(&socket).unwrap();
    idle_client
        .set_write_timeout(Some(Duration::from_millis(100)))
        .unwrap();
    // Stop reading while enough replies accumulate to backpressure its writer.
    let _ = idle_client.write_all(b"{\"op\":\"Ping\"}\n".repeat(100_000).as_slice());
    std::thread::sleep(Duration::from_millis(100));
    for password in [Value::Null, json!("wrong-fixture-password")] {
        let denied = request(&socket, json!({"op":"Shutdown","password":password}));
        assert_eq!(denied["error"], "MAINTENANCE_AUTHENTICATION_FAILED");
        assert!(!host.join("DOWN").exists());
        assert!(child.0.try_wait().unwrap().is_none());
    }
    assert_eq!(
        request(&socket, json!({"op":"MasterStatus"}))["error"],
        "PROJECT_RUNTIME_UNAVAILABLE: project replay failed"
    );
    let stopped = request(
        &socket,
        json!({"op":"Shutdown","password":"test-owned-maintenance"}),
    );
    assert_eq!(stopped["ok"], true, "{stopped}");
    assert_eq!(stopped["authorized"], true);
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        if let Some(status) = child.0.try_wait().unwrap() {
            assert!(status.success());
            break;
        }
        assert!(
            Instant::now() < deadline,
            "authorized maintenance did not exit"
        );
        std::thread::sleep(Duration::from_millis(20));
    }
    drop(idle_client);
    assert!(host.join("DOWN").exists());
    assert!(!socket.exists());
    assert_eq!(std::fs::read(&journal).unwrap(), b"{malformed-fixture}\n");
    std::fs::remove_dir_all(root).unwrap();
}
