//! Public startup regression for historical identity-map shapes.
//!
//! The candidate `collab serve` child is test-owned and fully isolated: a
//! temporary state root and HOME, an empty inherited environment, and no
//! production daemon, launchd, tmux or Rex discovery variables.

use serde_json::{json, Value};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

#[path = "support/daemon_readiness.rs"]
mod daemon_readiness;

fn unique_root(label: &str) -> PathBuf {
    let nonce = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    PathBuf::from(format!(
        "/tmp/collab-identity-map-startup-{label}-{}-{nonce}",
        std::process::id()
    ))
}

fn encoded_storage_path(base: &Path, value: &str) -> PathBuf {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    const CHUNK_BYTES: usize = 96;
    let bytes = value.as_bytes();
    let mut path = base.to_path_buf();
    for (index, chunk) in bytes.chunks(CHUNK_BYTES).enumerate() {
        let mut component = if index == 0 {
            format!("v1-{}-", bytes.len())
        } else {
            String::new()
        };
        for byte in chunk {
            component.push(HEX[(byte >> 4) as usize] as char);
            component.push(HEX[(byte & 0x0f) as usize] as char);
        }
        path.push(component);
    }
    path
}

fn binding(name: &str, runtime: &str, project: &str) -> Value {
    json!({
        "project_scope": project,
        "app_scope_id": "appserver-cli",
        "agent_id": name.to_lowercase(),
        "runtime_id": runtime,
        "binding_id": format!("binding-{runtime}")
    })
}

fn binding_value(name: &str, runtime: &str, project: &str) -> Value {
    json!({
        "binding": binding(name, runtime, project),
        "transport": {
            "kind": "appserver",
            "endpoint": format!("{runtime}-endpoint"),
            "namespace": "test",
            "session_id": format!("{runtime}-session"),
            "thread_id": format!("{runtime}-thread"),
            "capabilities": ["test"],
            "self_check": "ok"
        },
        "tab_id": {
            "project_scope": project,
            "app_scope_id": "appserver-cli"
        },
        "host": {
            "agent_kind": null,
            "host_type": "native",
            "profile": null
        },
        "invocation_cwd": project,
        "physical_name": null,
        "layout_binding_id": null
    })
}

/// One active slot with `display_slot: null` and a `registration_number`, one
/// active slot with a numeric display slot and a distinct number.
fn seeded_map(project: &str) -> Value {
    let mut alice_value = binding_value("Alice", "runtime-a", project);
    alice_value["registration_number"] = json!(4242);
    let mut bob_value = binding_value("Bob", "runtime-b", project);
    bob_value["registration_number"] = json!(7);
    json!({
        "schema_version": 1,
        "slots": [
            {
                "identity_key": {
                    "project_scope": project,
                    "app_scope_id": "appserver-cli",
                    "name": "Alice"
                },
                "display_slot": null,
                "identity_status": "active",
                "generation": 1,
                "current": alice_value,
                "previous": null
            },
            {
                "identity_key": {
                    "project_scope": project,
                    "app_scope_id": "appserver-cli",
                    "name": "Bob"
                },
                "display_slot": 3,
                "identity_status": "active",
                "generation": 1,
                "current": bob_value,
                "previous": null
            }
        ],
        "effects": [],
        "pending_resets": []
    })
}

struct StartupFixture {
    root: PathBuf,
    state: PathBuf,
    project: PathBuf,
    socket: PathBuf,
    map_path: PathBuf,
    daemon: Option<Child>,
}

impl StartupFixture {
    fn new(label: &str) -> Self {
        let root = unique_root(label);
        let state = root.join("host-state");
        let project = root.join("project");
        std::fs::create_dir_all(&state).unwrap();
        std::fs::create_dir_all(project.join(".agent-collab")).unwrap();
        std::fs::create_dir_all(root.join("home")).unwrap();
        let project = project.canonicalize().unwrap();
        Self {
            socket: state.join("server.sock"),
            map_path: state.join("identity-map.json"),
            root,
            state,
            project,
            daemon: None,
        }
    }

    fn project_scope(&self) -> String {
        self.project.to_string_lossy().into_owned()
    }

    fn write_current_state(&self) {
        let path = encoded_storage_path(
            &encoded_storage_path(&self.state.join("projects"), &self.project_scope()),
            "appserver-cli",
        )
        .join(".agent-collab/server/state.json");
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, br#"{"schema_version":1,"state":{}}"#).unwrap();
    }

    fn write_map(&self, map: &Value) -> Vec<u8> {
        let bytes = serde_json::to_vec_pretty(map).unwrap();
        std::fs::write(&self.map_path, &bytes).unwrap();
        bytes
    }

    fn serve_command(&self, label: &str) -> Command {
        let mut command = Command::new(env!("CARGO_BIN_EXE_collab"));
        command
            .arg("serve")
            .current_dir(&self.project)
            .env_clear()
            .env("PATH", "/usr/bin:/bin:/usr/sbin:/sbin")
            .env("LC_ALL", "en_US.UTF-8")
            .env("HOME", self.root.join("home"))
            .env("CODEX_HOME", self.root.join("home"))
            .env("COLLAB_STATE_DIR", &self.state)
            .stdin(Stdio::null());
        let log = std::fs::File::create(self.root.join(format!("{label}.log"))).unwrap();
        command.stdout(log.try_clone().unwrap()).stderr(log);
        command
    }

    fn log_text(&self, label: &str) -> String {
        std::fs::read_to_string(self.root.join(format!("{label}.log"))).unwrap_or_default()
    }

    fn write_map_for_project(&self, map: &Value) -> Vec<u8> {
        let mut map = map.clone();
        for slot in map["slots"].as_array_mut().unwrap() {
            slot["identity_key"]["project_scope"] = json!(self.project_scope());
            for binding_name in ["current", "previous"] {
                let binding = &mut slot[binding_name];
                if binding.is_null() {
                    continue;
                }
                binding["binding"]["project_scope"] = json!(self.project_scope());
                binding["tab_id"]["project_scope"] = json!(self.project_scope());
                binding["invocation_cwd"] = json!(self.project_scope());
            }
        }
        let project_storage = self
            .state
            .join("projects")
            .join(encoded_scope_component(&self.project_scope()))
            .join("apps")
            .join(encoded_scope_component("appserver-cli"))
            .join(".agent-collab")
            .join("server");
        std::fs::create_dir_all(&project_storage).unwrap();
        std::fs::write(
            project_storage.join("state.json"),
            br#"{"schema_version":1,"state":{}}"#,
        )
        .unwrap();
        self.write_map(&map)
    }

    fn start_daemon(&mut self) {
        assert!(self.daemon.is_none(), "fixture daemon already owned");
        let child = self
            .serve_command("serve")
            .spawn()
            .expect("spawn test-owned collab serve");
        self.daemon = Some(child);
        daemon_readiness::wait_for_ping(
            self.daemon.as_mut().unwrap(),
            &self.socket,
            Duration::from_secs(15),
        );
    }

    fn stop_daemon(&mut self) {
        let Some(mut child) = self.daemon.take() else {
            return;
        };
        if child.try_wait().unwrap().is_none() {
            let result = unsafe { libc::kill(child.id() as i32, libc::SIGTERM) };
            assert_eq!(result, 0, "terminate the test-owned serve child");
        }
        child.wait().expect("reap test-owned serve child");
        let deadline = Instant::now() + Duration::from_secs(10);
        while self.socket.exists() {
            assert!(
                Instant::now() < deadline,
                "test-owned serve child still owns {}",
                self.socket.display()
            );
            std::thread::sleep(Duration::from_millis(25));
        }
    }

    fn cleanup(&self) {
        std::fs::remove_dir_all(&self.root).unwrap();
    }
}

impl Drop for StartupFixture {
    fn drop(&mut self) {
        if let Some(mut child) = self.daemon.take() {
            if child.try_wait().ok().flatten().is_none() {
                let _ = unsafe { libc::kill(child.id() as i32, libc::SIGTERM) };
            }
            let _ = child.wait();
        }
    }
}

fn encoded_scope_component(value: &str) -> String {
    let bytes = value.as_bytes();
    let mut component = format!("v1-{}-", bytes.len());
    for byte in bytes {
        component.push_str(&format!("{byte:02x}"));
    }
    component
}

#[test]
fn historical_identity_map_shapes_start_and_survive_public_ping() {
    let mut fixture = StartupFixture::new("valid");
    let seeded_bytes = fixture.write_map_for_project(&seeded_map(""));
    let seeded: Value = serde_json::from_slice(&seeded_bytes).unwrap();
    fixture.write_current_state();

    fixture.start_daemon();

    let after = std::fs::read(&fixture.map_path).unwrap();
    assert_eq!(
        after, seeded_bytes,
        "startup must not rewrite or drop the seeded compatibility data"
    );
    let observed: Value = serde_json::from_slice(&after).unwrap();
    assert_eq!(observed, seeded);
    assert_eq!(observed["slots"][0]["display_slot"], Value::Null);
    assert_eq!(
        observed["slots"][0]["current"]["registration_number"],
        json!(4242)
    );
    assert_eq!(observed["slots"][1]["display_slot"], json!(3));
    assert_eq!(
        observed["slots"][1]["current"]["registration_number"],
        json!(7)
    );

    fixture.stop_daemon();
    fixture.cleanup();
}

#[test]
fn repeated_current_previous_route_facts_do_not_block_startup_or_rewrite_map() {
    let mut fixture = StartupFixture::new("same-current-previous");
    let mut map = seeded_map("");
    let current = map["slots"][0]["current"].clone();
    map["slots"][0]["previous"] = current;
    map["slots"][0]["current"]["registration_number"] = json!(2);
    map["slots"][0]["previous"]["registration_number"] = json!(0);
    let seeded_bytes = fixture.write_map_for_project(&map);
    fixture.write_current_state();

    fixture.start_daemon();

    assert_eq!(
        std::fs::read(&fixture.map_path).unwrap(),
        seeded_bytes,
        "startup must preserve historical data and must not normalize the map"
    );
    fixture.stop_daemon();
    fixture.cleanup();
}

#[test]
fn generated_peer_identity_starts_and_preserves_map() {
    let mut fixture = StartupFixture::new("generated-peer");
    let mut map = seeded_map("");
    map["slots"][0]["identity_key"]["name"] = json!("peer-17");
    map["slots"][0]["display_slot"] = Value::Null;
    map["slots"][0]["current"]["registration_number"] = json!(17);
    map["slots"][0]["previous"] = Value::Null;
    let seeded_bytes = fixture.write_map_for_project(&map);
    fixture.write_current_state();

    fixture.start_daemon();

    assert_eq!(
        std::fs::read(&fixture.map_path).unwrap(),
        seeded_bytes,
        "startup must preserve the generated identity slot without normalization"
    );
    fixture.stop_daemon();
    fixture.cleanup();
}

#[test]
fn malformed_identity_maps_fail_before_socket_readiness_and_stay_unchanged() {
    let cases: [(&str, Value, &str); 4] = [
        (
            "unknown-field",
            {
                let mut map = seeded_map("/tmp/collab-identity-map-startup");
                map["slots"][0]["unexpected"] = json!(1);
                map
            },
            "IDENTITY_MAP_DECODE_FAILED",
        ),
        (
            "unsupported-version",
            {
                let mut map = seeded_map("/tmp/collab-identity-map-startup");
                map["schema_version"] = json!(2);
                map
            },
            "IDENTITY_MAP_VERSION_UNSUPPORTED",
        ),
        (
            "wrong-registration-number-type",
            {
                let mut map = seeded_map("/tmp/collab-identity-map-startup");
                map["slots"][0]["current"]["registration_number"] = json!("not-a-number");
                map
            },
            "IDENTITY_MAP_DECODE_FAILED",
        ),
        (
            "malformed-generated-name",
            {
                let mut map = seeded_map("/tmp/collab-identity-map-startup");
                map["slots"][0]["identity_key"]["name"] = json!("peer-01");
                map["slots"][0]["display_slot"] = Value::Null;
                map
            },
            "NAME_INVALID",
        ),
    ];

    for (label, map, expected_error) in cases {
        let fixture = StartupFixture::new(label);
        let seeded_bytes = fixture.write_map_for_project(&map);

        let output = fixture
            .serve_command(label)
            .output()
            .expect("run test-owned collab serve");
        assert!(
            !output.status.success(),
            "{label}: malformed map must fail startup"
        );
        let log = fixture.log_text(label);
        assert!(
            log.contains(expected_error),
            "{label}: expected {expected_error} in serve log: {log}"
        );
        assert!(
            !fixture.socket.exists(),
            "{label}: malformed map must fail before socket readiness"
        );
        assert_eq!(
            std::fs::read(&fixture.map_path).unwrap(),
            seeded_bytes,
            "{label}: rejected input must remain unchanged"
        );
        fixture.cleanup();
    }
}

#[test]
fn missing_display_slot_field_is_still_a_decode_error() {
    let fixture = StartupFixture::new("missing-display-slot");
    let mut map = seeded_map(&fixture.project_scope());
    map["slots"][1]
        .as_object_mut()
        .unwrap()
        .remove("display_slot");
    let seeded_bytes = fixture.write_map(&map);

    let output = fixture
        .serve_command("missing-display-slot")
        .output()
        .expect("run test-owned collab serve");
    assert!(!output.status.success());
    let log = fixture.log_text("missing-display-slot");
    assert!(
        log.contains("IDENTITY_MAP_DECODE_FAILED"),
        "missing required field must still decode-fail: {log}"
    );
    assert_eq!(std::fs::read(&fixture.map_path).unwrap(), seeded_bytes);
    fixture.cleanup();
}
