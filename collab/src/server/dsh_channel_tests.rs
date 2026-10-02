use super::*;
use crate::proto::{DshCandidate, TransportCandidates, TransportKind};
use std::io::{BufRead, BufReader, Read, Write};
use std::os::unix::net::UnixListener as StdUnixListener;
use std::os::unix::net::UnixStream as StdUnixStream;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

static SEQ: AtomicU64 = AtomicU64::new(0);

fn scratch(name: &str) -> PathBuf {
    let id = SEQ.fetch_add(1, Ordering::Relaxed);
    let path = std::env::temp_dir().join(format!("collab-dsh-{name}-{}-{id}", std::process::id()));
    std::fs::create_dir_all(&path).expect("create dsh fixture directory");
    path
}

/// A fake dsh-gateway control plane: NDJSON over a UNIX socket.
///
/// `respond` receives one parsed request and returns the single reply line, or
/// `None` to accept the connection and stay silent. `None` is how a hanging
/// gateway is modelled; the connection is held open until the client gives up.
struct Gateway {
    socket: PathBuf,
    stop: Arc<AtomicBool>,
    handle: Option<std::thread::JoinHandle<()>>,
}

impl Gateway {
    fn start(
        name: &str,
        respond: impl Fn(serde_json::Value) -> Option<String> + Send + 'static,
    ) -> Self {
        let dir = scratch(name);
        let socket = dir.join("control.sock");
        let listener = StdUnixListener::bind(&socket).expect("bind control socket");
        let stop = Arc::new(AtomicBool::new(false));
        let flag = Arc::clone(&stop);
        let handle = std::thread::spawn(move || {
            while !flag.load(Ordering::SeqCst) {
                let Ok((stream, _)) = listener.accept() else {
                    return;
                };
                if flag.load(Ordering::SeqCst) {
                    return;
                }
                let mut reader = BufReader::new(&stream);
                let mut line = String::new();
                if reader.read_line(&mut line).unwrap_or(0) == 0 {
                    continue;
                }
                let request: serde_json::Value =
                    serde_json::from_str(line.trim()).expect("control request is JSON");
                match respond(request) {
                    Some(reply) => {
                        let mut stream = &stream;
                        let _ = stream.write_all(reply.as_bytes());
                        let _ = stream.write_all(b"\n");
                        let _ = stream.flush();
                    }
                    None => {
                        // Hold the connection open and never answer: the client
                        // must decide on its own timeout.
                        let mut buffer = [0u8; 64];
                        let source = reader.get_mut();
                        while matches!(source.read(&mut buffer), Ok(read) if read > 0) {}
                    }
                }
            }
        });
        Self {
            socket,
            stop,
            handle: Some(handle),
        }
    }

    fn endpoint(&self) -> String {
        format!("unix://{}", self.socket.display())
    }
}

impl Drop for Gateway {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        // Wake a pending accept so the loop can observe the stop flag.
        let _ = StdUnixStream::connect(&self.socket);
        if let Some(handle) = self.handle.take() {
            let _ = handle.join();
        }
        if let Some(dir) = self.socket.parent() {
            let _ = std::fs::remove_dir_all(dir);
        }
    }
}

fn candidate(socket: &Path, root: &Path) -> DshCandidate {
    candidate_ids(socket, root, "rt-1", "ses-1")
}

fn candidate_ids(socket: &Path, root: &Path, runtime_id: &str, agent_id: &str) -> DshCandidate {
    DshCandidate {
        endpoint: format!("unix://{}", socket.display()),
        runtime_id: runtime_id.into(),
        agent_id: agent_id.into(),
        session_id: agent_id.into(),
        cwd: root.display().to_string(),
    }
}

/// A gateway answering `agent-facts` for whichever agent was asked about, the
/// way a real gateway projects the registry record for the requested id.
fn facts_reply(request: &serde_json::Value, cwd: &Path) -> String {
    let nonce = request
        .pointer("/params/nonce")
        .and_then(serde_json::Value::as_str)
        .unwrap_or_default();
    let runtime_id = request
        .pointer("/params/runtimeId")
        .and_then(serde_json::Value::as_str)
        .unwrap_or("rt-1");
    let agent_id = request
        .pointer("/params/agentId")
        .and_then(serde_json::Value::as_str)
        .unwrap_or("ses-1");
    serde_json::json!({
        "ok": true,
        "result": {
            "nonce": nonce,
            "runtimeId": runtime_id,
            "agentId": agent_id,
            // The design projects sessionId from agentId; it is not stored twice.
            "sessionId": agent_id,
            "cwd": cwd.display().to_string(),
            "status": "running",
        }
    })
    .to_string()
}

fn wake_reply(message_id: &str) -> Option<String> {
    Some(
        serde_json::json!({
            "ok": true,
            "result": {"messageId": message_id, "runtimeId": "rt-1", "agentId": "ses-1"}
        })
        .to_string(),
    )
}

fn test_server(root: &Path) -> Arc<Server> {
    let server_dir = root.join(".agent-collab").join("server");
    std::fs::create_dir_all(&server_dir).unwrap();
    let journal_path = server_dir.join("journal.jsonl");
    let journal = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&journal_path)
        .unwrap();
    Arc::new(Server {
        config: crate::config::Config::default(),
        root: root.to_path_buf(),
        storage_root: root.to_path_buf(),
        journal_path,
        host_paths: HostPaths::for_state_root(root.join("host-state")).unwrap(),
        state: Mutex::new(State::default()),
        journal: Mutex::new(journal),
        // dsh validation must never consult the App Server candidate check.
        appserver_candidate_check: Arc::new(|_| {
            Err("APPSERVER_CHECK_REACHED: dsh validation must not consult it".into())
        }),
        appserver_notification_sink: Arc::new(|_, _, _, _, _, _| {
            Ok(serde_json::json!({"accepted": true}))
        }),
        appserver_thread_status: Arc::new(|_, _| Ok(serde_json::json!({}))),
        appserver_thread_archive: Arc::new(|_, _| Ok(serde_json::json!({}))),
        mailbox_notify: Notify::new(),
    })
}

// ---------------------------------------------------------------- registration

#[test]
fn a_verified_dsh_candidate_is_admitted_as_the_dsh_transport() {
    let root = scratch("admit");
    let gateway = Gateway::start("admit", {
        let root = root.clone();
        move |request| Some(facts_reply(&request, &root))
    });
    let server = test_server(&root);
    let selected = validate_transport_candidates(
        &server,
        &TransportCandidates {
            appserver: None,
            tmux: None,
            dsh: Some(candidate(&gateway.socket, &root)),
        },
        root.to_str().unwrap(),
    )
    .expect("a gateway-verified dsh candidate must be admitted");

    assert_eq!(selected.kind, TransportKind::Dsh);
    assert_eq!(
        selected.endpoint.as_deref(),
        Some(gateway.endpoint().as_str())
    );
    assert_eq!(selected.namespace.as_deref(), Some("rt-1"));
    assert_eq!(selected.session_id.as_deref(), Some("ses-1"));
    assert_eq!(selected.thread_id.as_deref(), Some("ses-1"));
    assert!(selected.tmux_endpoint.is_none());
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn dsh_must_not_be_combined_with_a_pane_candidate() {
    // Design F1: dsh is mutually exclusive. The ambiguous set is refused rather
    // than ranked, so neither candidate can silently shadow the other.
    let root = scratch("mutex");
    let gateway = Gateway::start("mutex", |_| None);
    let server = test_server(&root);
    let tmux = crate::proto::TmuxCandidate {
        endpoint: crate::proto::TmuxEndpoint {
            socket_path: "/tmp/collab-dsh-mutex-tmux.sock".into(),
            server_pid: 1,
            tmux_session_id: "$1".into(),
            pane_id: "%1".into(),
            pane_pid: 2,
            codex_session_id: Some("session-1".into()),
            codex_thread_id: Some("thread-1".into()),
        },
        cwd: root.display().to_string(),
    };

    let error = validate_transport_candidates(
        &server,
        &TransportCandidates {
            appserver: None,
            tmux: Some(tmux),
            dsh: Some(candidate(&gateway.socket, &root)),
        },
        root.to_str().unwrap(),
    )
    .expect_err("dsh plus tmux must be refused, not ranked");

    assert!(error.starts_with("DSH_ENDPOINT_REJECTED:"), "{error}");
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn dsh_candidate_cwd_must_be_the_project_root() {
    let root = scratch("scope");
    let other = scratch("scope-other");
    let gateway = Gateway::start("scope", |_| None);
    let server = test_server(&root);
    let error = validate_transport_candidates(
        &server,
        &TransportCandidates {
            appserver: None,
            tmux: None,
            dsh: Some(candidate(&gateway.socket, &other)),
        },
        root.to_str().unwrap(),
    )
    .expect_err("a dsh candidate outside the project root must be refused");

    assert!(error.starts_with("DSH_ENDPOINT_REJECTED:"), "{error}");
    let _ = std::fs::remove_dir_all(&root);
    let _ = std::fs::remove_dir_all(&other);
}

#[test]
fn a_gateway_reporting_another_runtime_is_rejected() {
    let root = scratch("runtime");
    let gateway = Gateway::start("runtime", {
        let root = root.clone();
        move |request| {
            let nonce = request
                .pointer("/params/nonce")
                .and_then(serde_json::Value::as_str)
                .unwrap_or_default();
            Some(
                serde_json::json!({
                    "ok": true,
                    "result": {
                        "nonce": nonce,
                        "runtimeId": "rt-OTHER",
                        "agentId": "ses-1",
                        "sessionId": "ses-1",
                        "cwd": root.display().to_string(),
                        "status": "running",
                    }
                })
                .to_string(),
            )
        }
    });
    let server = test_server(&root);
    let error = validate_transport_candidates(
        &server,
        &TransportCandidates {
            appserver: None,
            tmux: None,
            dsh: Some(candidate(&gateway.socket, &root)),
        },
        root.to_str().unwrap(),
    )
    .expect_err("a runtime mismatch must be rejected");

    assert!(error.starts_with("DSH_ENDPOINT_REJECTED:"), "{error}");
    assert!(error.contains("rt-OTHER"), "{error}");
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn a_reply_that_does_not_echo_the_challenge_nonce_is_rejected() {
    let root = scratch("nonce");
    let gateway = Gateway::start("nonce", {
        let root = root.clone();
        move |_| {
            Some(
                serde_json::json!({
                    "ok": true,
                    "result": {
                        "nonce": "00000000000000000000000000000000",
                        "runtimeId": "rt-1",
                        "agentId": "ses-1",
                        "sessionId": "ses-1",
                        "cwd": root.display().to_string(),
                        "status": "running",
                    }
                })
                .to_string(),
            )
        }
    });
    let server = test_server(&root);
    let error = validate_transport_candidates(
        &server,
        &TransportCandidates {
            appserver: None,
            tmux: None,
            dsh: Some(candidate(&gateway.socket, &root)),
        },
        root.to_str().unwrap(),
    )
    .expect_err("a replayed reply must not satisfy a fresh challenge");

    assert!(error.starts_with("DSH_ENDPOINT_REJECTED:"), "{error}");
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn every_challenge_carries_a_distinct_nonce() {
    let seen: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
    let record = |seen: &Arc<Mutex<Vec<String>>>, request: &serde_json::Value| -> String {
        let nonce = request
            .pointer("/params/nonce")
            .and_then(serde_json::Value::as_str)
            .unwrap_or_default()
            .to_owned();
        seen.lock().unwrap().push(nonce.clone());
        nonce
    };

    let first = Arc::clone(&seen);
    let gateway_a = Gateway::start("fresh-nonce-a", move |request| {
        let nonce = record(&first, &request);
        Some(
            serde_json::json!({
                "ok": true,
                "result": {
                    "nonce": nonce,
                    "runtimeId": "rt-1",
                    "agentId": "ses-1",
                    "sessionId": "ses-1",
                    "cwd": "/tmp",
                    "status": "running",
                }
            })
            .to_string(),
        )
    });
    let _ = crate::client::adapters::dsh::facts(&gateway_a.endpoint(), "rt-1", "ses-1")
        .expect("first challenge");
    drop(gateway_a);

    let second = Arc::clone(&seen);
    let gateway_b = Gateway::start("fresh-nonce-b", move |request| {
        let nonce = record(&second, &request);
        Some(
            serde_json::json!({
                "ok": true,
                "result": {
                    "nonce": nonce,
                    "runtimeId": "rt-1",
                    "agentId": "ses-1",
                    "sessionId": "ses-1",
                    "cwd": "/tmp",
                    "status": "running",
                }
            })
            .to_string(),
        )
    });
    let _ = crate::client::adapters::dsh::facts(&gateway_b.endpoint(), "rt-1", "ses-1")
        .expect("second challenge");

    let nonces = seen.lock().unwrap().clone();
    assert_eq!(nonces.len(), 2, "both challenges must reach a gateway");
    assert_ne!(nonces[0], nonces[1], "nonces must never be reused");
    assert_eq!(nonces[0].len(), 32, "nonce is 32 hex characters");
    assert!(nonces[0].chars().all(|c| c.is_ascii_hexdigit()));
}

// -------------------------------------------------------------- failure classes

#[test]
fn an_unreachable_gateway_is_blocked_and_never_absent() {
    let dir = scratch("unreachable");
    let socket = dir.join("missing.sock");
    let error = crate::client::adapters::dsh::facts(
        &format!("unix://{}", socket.display()),
        "rt-1",
        "ses-1",
    )
    .expect_err("a missing control socket must fail closed");

    assert!(
        error.to_string().starts_with("DSH_ENDPOINT_BLOCKED:"),
        "{error}"
    );
    assert!(
        !error.is_definitely_absent(),
        "an unreachable gateway proves nothing about the agent"
    );
    assert!(error.is_definitely_undelivered());
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_silent_gateway_is_unknown_and_never_absent() {
    let gateway = Gateway::start("silent", |_| None);
    let error = crate::client::adapters::dsh::facts(&gateway.endpoint(), "rt-1", "ses-1")
        .expect_err("a hanging gateway must fail closed");

    assert!(
        error.to_string().starts_with("DSH_ENDPOINT_UNKNOWN:"),
        "{error}"
    );
    assert!(
        !error.is_definitely_absent(),
        "a timeout is not proof that the agent is gone"
    );
    assert!(
        !error.is_definitely_undelivered(),
        "a timeout may still have been queued, so it must never be resent"
    );
}

#[test]
fn a_malformed_gateway_reply_is_unknown() {
    let gateway = Gateway::start("malformed", |_| Some("not json at all".into()));
    let error = crate::client::adapters::dsh::facts(&gateway.endpoint(), "rt-1", "ses-1")
        .expect_err("a malformed reply must fail closed");

    assert!(
        error.to_string().starts_with("DSH_ENDPOINT_UNKNOWN:"),
        "{error}"
    );
    assert!(!error.is_definitely_absent());
}

#[test]
fn a_gateway_refusal_carries_its_stable_code() {
    let gateway = Gateway::start("refused", |_| {
        Some(
            serde_json::json!({
                "ok": false,
                "error": {"code": "unknown-agent", "message": "no such agent"}
            })
            .to_string(),
        )
    });
    let error = crate::client::adapters::dsh::facts(&gateway.endpoint(), "rt-1", "ses-1")
        .expect_err("a gateway refusal must fail closed");

    assert!(
        error.to_string().starts_with("DSH_ENDPOINT_REJECTED:"),
        "{error}"
    );
    assert!(error.to_string().contains("unknown-agent"), "{error}");
    assert!(
        error.is_definitely_absent(),
        "an explicit unknown-agent denial is proof of absence"
    );
}

#[test]
fn a_dsh_endpoint_must_be_an_absolute_unix_path() {
    for bad in [
        "http://example.invalid/control.sock",
        "unix://relative.sock",
        "unix://",
    ] {
        let error = crate::client::adapters::dsh::control_socket(bad)
            .expect_err("only unix://<absolute path> is a dsh endpoint");
        assert!(
            error.to_string().starts_with("DSH_ENDPOINT_REJECTED:"),
            "{bad}: {error}"
        );
    }
}

// -------------------------------------------------------------------- presence

fn dsh_transport(socket: &Path) -> SelectedTransport {
    SelectedTransport {
        kind: TransportKind::Dsh,
        endpoint: Some(format!("unix://{}", socket.display())),
        namespace: Some("rt-1".into()),
        session_id: Some("ses-1".into()),
        thread_id: Some("ses-1".into()),
        tmux_endpoint: None,
        capabilities: vec!["enqueue_wake".into()],
        self_check: "test".into(),
    }
}

#[test]
fn dsh_presence_is_present_while_running_and_cold_when_inactive() {
    for (status, expected) in [
        ("running", IdentityPresence::Present),
        ("inactive", IdentityPresence::Cold),
    ] {
        let gateway = Gateway::start("presence", move |request| {
            let nonce = request
                .pointer("/params/nonce")
                .and_then(serde_json::Value::as_str)
                .unwrap_or_default();
            Some(
                serde_json::json!({
                    "ok": true,
                    "result": {
                        "nonce": nonce,
                        "runtimeId": "rt-1",
                        "agentId": "ses-1",
                        "sessionId": "ses-1",
                        "cwd": "/tmp",
                        "status": status,
                    }
                })
                .to_string(),
            )
        });
        let observed = dsh_identity_presence(&dsh_transport(&gateway.socket));
        assert_eq!(observed, expected, "status {status}");
    }
}

#[test]
fn dsh_presence_is_unknown_when_the_gateway_cannot_be_reached() {
    let dir = scratch("presence-down");
    let socket = dir.join("missing.sock");
    // Invariant I7: only explicit text judges a peer dead. An unreachable
    // gateway must never produce Missing, which would retire the record.
    assert_eq!(
        dsh_identity_presence(&dsh_transport(&socket)),
        IdentityPresence::Unknown
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn dsh_presence_is_missing_only_on_an_explicit_denial() {
    let gateway = Gateway::start("presence-absent", |_| {
        Some(
            serde_json::json!({
                "ok": false,
                "error": {"code": "unknown-agent", "message": "no such agent"}
            })
            .to_string(),
        )
    });
    assert_eq!(
        dsh_identity_presence(&dsh_transport(&gateway.socket)),
        IdentityPresence::Missing
    );
}

#[test]
fn a_gateway_timeout_leaves_dsh_presence_unknown_not_missing() {
    let gateway = Gateway::start("presence-timeout", |_| None);
    assert_eq!(
        dsh_identity_presence(&dsh_transport(&gateway.socket)),
        IdentityPresence::Unknown
    );
}

// ------------------------------------------------------------ wake and delivery

#[test]
fn dsh_wake_mode_covers_the_whole_sink_mode_domain() {
    // Mirrors the exhaustive domain in mailbox.rs / part_05.rs. A missing value
    // here is how `explicit-notification` silently took the immediate path.
    assert_eq!(
        dsh_wake_mode("immediate", "m").unwrap(),
        ("followup", false)
    );
    assert_eq!(
        dsh_wake_mode("explicit-notification", "m").unwrap(),
        ("followup", false)
    );
    assert_eq!(dsh_wake_mode("queued", "m").unwrap(), ("inject", false));
    assert_eq!(
        dsh_wake_mode("daemon-live-closure", "m").unwrap(),
        ("followup", true)
    );
    assert_eq!(
        dsh_wake_mode("restart-replay-pending", "m").unwrap(),
        ("followup", true)
    );
}

#[test]
fn dsh_wake_mode_refuses_a_mode_outside_the_domain() {
    let error = dsh_wake_mode("invented", "m-7").expect_err("unknown modes must not be guessed");
    assert!(error.starts_with("DSH_ENDPOINT_REJECTED:"), "{error}");
    assert!(error.contains("m-7"), "{error}");
}

#[test]
fn dsh_wake_enqueue_maps_the_mode_and_returns_a_queue_receipt() {
    let seen: Arc<Mutex<Vec<serde_json::Value>>> = Arc::new(Mutex::new(Vec::new()));
    let observer = Arc::clone(&seen);
    let gateway = Gateway::start("enqueue", move |request| {
        observer.lock().unwrap().push(request.clone());
        Some(
            serde_json::json!({
                "ok": true,
                "result": {"messageId": "msg-1", "runtimeId": "rt-1", "agentId": "ses-1"}
            })
            .to_string(),
        )
    });
    let receipt = crate::client::adapters::dsh::notify(
        &gateway.endpoint(),
        "rt-1",
        "ses-1",
        "followup",
        "codex-%1",
        "hello",
    )
    .expect("enqueue must be admitted");

    assert_eq!(receipt["transport"], "dsh");
    assert_eq!(receipt["message_id"], "msg-1");
    assert_eq!(receipt["consumed"], false);
    let requests = seen.lock().unwrap().clone();
    assert_eq!(requests[0]["method"], "enqueue");
    assert_eq!(requests[0]["params"]["mode"], "followup");
    assert_eq!(requests[0]["params"]["agentId"], "ses-1");
    assert_eq!(requests[0]["params"]["content"][0]["type"], "text");
    assert_eq!(requests[0]["params"]["content"][0]["text"], "hello");
}

#[test]
fn dsh_delivery_classes_separate_unreachable_from_timeout() {
    use crate::client::adapters::{AdapterError, NotificationDeliveryClass};
    let not_delivered = AdapterError::notification_class_from_display(
        "DSH_ENDPOINT_BLOCKED: gateway control socket is unreachable",
    );
    let refused = AdapterError::notification_class_from_display(
        "DSH_ENDPOINT_REJECTED: gateway refused enqueue with not-ready",
    );
    let unknown = AdapterError::notification_class_from_display(
        "DSH_ENDPOINT_UNKNOWN: gateway did not answer enqueue within 2s",
    );
    assert_eq!(not_delivered, NotificationDeliveryClass::KnownNotDelivered);
    assert_eq!(refused, NotificationDeliveryClass::KnownNotDelivered);
    // MAX_WAKE_ATTEMPTS = 1: an unknown outcome must never be resent.
    assert_eq!(unknown, NotificationDeliveryClass::Unknown);
}

#[test]
fn a_dsh_subscription_is_armed_under_the_dsh_method() {
    // The registration path installs a default `direct-message` lease. A method
    // string the matcher does not know would silently leave every dsh peer in
    // `mailbox-only-no-subscription`.
    let subscription = crate::server::state::NotificationSubscription {
        id: "default-direct-message".into(),
        worker_id: "dsh-abc".into(),
        event: "direct-message".into(),
        subject: None,
        target: "ses-1".into(),
        method: TransportKind::Dsh.as_str().into(),
        trigger_ms: None,
        trigger_times_ms: Vec::new(),
        interval_ms: None,
        repeat_count: 1,
        fired_count: 0,
        expires_ms: 10_000,
        status: "armed".into(),
        created_ms: 0,
        updated_ms: 0,
        status_reason: None,
    };
    assert_eq!(subscription.method, "dsh");
    assert!(
        subscription.matches("dsh-abc", "direct-message", None, 1_000),
        "an armed dsh lease must match"
    );
}

#[test]
fn every_transport_kind_round_trips_through_its_method_string() {
    for kind in TransportKind::ALL {
        assert_eq!(
            TransportKind::from_method(kind.as_str()),
            Some(kind.clone()),
            "{kind:?}"
        );
    }
    assert_eq!(TransportKind::from_method("carrier-pigeon"), None);
}

#[test]
fn dsh_candidate_serializes_additively_without_disturbing_pane_candidates() {
    let root = scratch("serde");
    let candidates = TransportCandidates {
        appserver: None,
        tmux: None,
        dsh: Some(candidate(&root.join("control.sock"), &root)),
    };
    let encoded = serde_json::to_value(&candidates).unwrap();
    assert_eq!(encoded["dsh"]["runtime_id"], "rt-1");
    assert!(
        encoded.get("tmux").is_none(),
        "absent candidates stay absent"
    );

    // An older client that sends only appserver/tmux must still deserialize.
    let legacy: TransportCandidates =
        serde_json::from_str(r#"{"appserver":null,"tmux":null}"#).unwrap();
    assert!(legacy.dsh.is_none());
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn a_dsh_control_socket_is_only_ever_asked_for_read_only_or_enqueue_work() {
    // Least privilege: collab must never reach the operator surface of the same
    // control plane (interrupt, shutdown, hold-ack, release-ack, queue, ...).
    let seen: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
    let observer = Arc::clone(&seen);
    let gateway = Gateway::start("least-privilege", move |request| {
        let method = request
            .get("method")
            .and_then(serde_json::Value::as_str)
            .unwrap_or_default()
            .to_owned();
        observer.lock().unwrap().push(method);
        Some(
            serde_json::json!({
                "ok": true,
                "result": {"messageId": "msg-1", "runtimeId": "rt-1", "agentId": "ses-1"}
            })
            .to_string(),
        )
    });
    let _ = crate::client::adapters::dsh::notify(
        &gateway.endpoint(),
        "rt-1",
        "ses-1",
        "inject",
        "codex-%1",
        "body",
    );
    assert_eq!(seen.lock().unwrap().clone(), vec!["enqueue".to_owned()]);
}

#[test]
fn a_dsh_wake_never_reaches_the_tmux_sink() {
    // The pre-existing sink was `if kind != AppServer`, which sent a new kind
    // into the tmux sink and failed with "Collab notifications require tmux".
    let root = scratch("sink-route");
    let gateway = Gateway::start("sink-route", |_| wake_reply("msg-9"));
    let transport = dsh_transport(&gateway.socket);
    let receipt = (default_appserver_notification_sink())(
        &transport,
        Some("codex-%1"),
        "hello",
        "msg-9",
        true,
        "immediate",
    )
    .expect("a dsh wake must be routed to the gateway, not to tmux");

    assert_eq!(receipt["transport"], "dsh");
    assert_eq!(receipt["sink_mode"], "immediate");
    assert!(receipt.get("unexpected_internal_mode").is_none());
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn a_dsh_wake_forwards_the_sink_mode_it_was_given() {
    let seen: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
    let observer = Arc::clone(&seen);
    let gateway = Gateway::start("sink-mode", move |request| {
        observer.lock().unwrap().push(
            request
                .pointer("/params/mode")
                .and_then(serde_json::Value::as_str)
                .unwrap_or_default()
                .to_owned(),
        );
        wake_reply("msg-1")
    });
    let transport = dsh_transport(&gateway.socket);
    let receipt = (default_appserver_notification_sink())(
        &transport,
        None,
        "hello",
        "msg-1",
        false,
        "explicit-notification",
    )
    .expect("explicit-notification must map, not fall through");

    assert_eq!(receipt["sink_mode"], "explicit-notification");
    assert_eq!(
        seen.lock().unwrap().clone(),
        vec!["followup".to_owned()],
        "explicit-notification maps to followup"
    );
}

#[test]
fn a_dsh_thread_status_reports_the_gateway_facts() {
    let root = scratch("status");
    let gateway = Gateway::start("status", {
        let root = root.clone();
        move |request| Some(facts_reply(&request, &root))
    });
    let transport = dsh_transport(&gateway.socket);
    let view = (default_appserver_thread_status())(&transport, "ses-1").expect("dsh status");

    assert_eq!(view["transport"], "dsh");
    assert_eq!(view["agentId"], "ses-1");
    assert_eq!(view["status"], "running");
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn a_dsh_thread_status_rejects_a_different_agent_than_the_endpoint() {
    let root = scratch("status-mismatch");
    let gateway = Gateway::start("status-mismatch", |_| None);
    let transport = dsh_transport(&gateway.socket);
    let error = (default_appserver_thread_status())(&transport, "ses-OTHER")
        .expect_err("a mismatched agent must be refused without probing");
    assert!(error.starts_with("DSH_ENDPOINT_REJECTED:"), "{error}");
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn a_dsh_agent_is_never_archived_by_collab() {
    let root = scratch("archive");
    let gateway = Gateway::start("archive", |_| None);
    let transport = dsh_transport(&gateway.socket);
    let error = (default_appserver_thread_archive())(&transport, "ses-1")
        .expect_err("the gateway owns the agent lifecycle");
    assert!(error.starts_with("TRANSPORT_UNSUPPORTED:"), "{error}");
    assert!(error.contains("gateway"), "{error}");
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn a_dsh_route_is_reverified_by_a_fresh_challenge_not_a_pane_compare() {
    let root = scratch("route");
    let gateway = Gateway::start("route", {
        let root = root.clone();
        move |request| Some(facts_reply(&request, &root))
    });
    let transport = dsh_transport(&gateway.socket);
    assert_eq!(dsh_identity_presence(&transport), IdentityPresence::Present);
    let _ = std::fs::remove_dir_all(&root);
}

// ------------------------------------------- real registration and wake entry

/// Register a dsh peer through the real registration entry point.
fn register_dsh(
    server: &Server,
    worker_id: &str,
    socket: &Path,
    runtime_id: &str,
    agent_id: &str,
) -> Resp {
    handle_register_with_app_scope(
        server,
        worker_id.into(),
        format!("token-{worker_id}"),
        server.root.display().to_string(),
        Some(crate::identity::AppServerId::new("tui-default").unwrap()),
        Some(TransportCandidates {
            appserver: None,
            tmux: None,
            dsh: Some(candidate_ids(socket, &server.root, runtime_id, agent_id)),
        }),
    )
}

/// Two dsh peers, each behind its own gateway: `a` sends, `b` is woken.
/// Every control request either gateway receives is recorded in `seen`.
fn dsh_peers(
    root: &Path,
    b_enqueue: impl Fn() -> Option<String> + Send + 'static,
    seen: Arc<Mutex<Vec<serde_json::Value>>>,
) -> (Arc<Server>, Gateway, Gateway) {
    let observer_a = Arc::clone(&seen);
    let gateway_a = Gateway::start("peers-a", {
        let root = root.to_path_buf();
        move |request| {
            observer_a.lock().unwrap().push(request.clone());
            Some(facts_reply(&request, &root))
        }
    });
    let observer_b = Arc::clone(&seen);
    let gateway_b = Gateway::start("peers-b", {
        let root = root.to_path_buf();
        move |request| {
            observer_b.lock().unwrap().push(request.clone());
            if request.get("method").and_then(serde_json::Value::as_str) == Some("agent-facts") {
                Some(facts_reply(&request, &root))
            } else {
                b_enqueue()
            }
        }
    });
    let mut server = test_server(root);
    {
        let server = Arc::get_mut(&mut server).expect("unique test server");
        server.appserver_notification_sink = default_appserver_notification_sink();
        server.config.notifications.enabled = true;
    }
    // Distinct gateway agents: a route key is (session, agent), so two peers
    // must not claim the same one.
    assert!(
        register_dsh(&server, "dsh-a", &gateway_a.socket, "rt-a", "ses-a").ok,
        "register a"
    );
    assert!(
        register_dsh(&server, "dsh-b", &gateway_b.socket, "rt-b", "ses-b").ok,
        "register b"
    );
    (server, gateway_a, gateway_b)
}

fn subscribe_and_send(server: &Server) -> Resp {
    let subscription = handle_notification_subscribe(
        server,
        "dsh-b".into(),
        "token-dsh-b".into(),
        "direct-message".into(),
        None,
        None,
        Vec::new(),
        None,
        1,
        3600,
    );
    assert!(subscription.ok, "subscription failed: {subscription:?}");
    handle_send(
        server,
        "dsh-a".into(),
        "dsh-b".into(),
        "notify".into(),
        Some("dsh wake".into()),
        "execute collab recv for this message".into(),
        None,
        "immediate".into(),
    )
}

#[test]
fn a_dsh_peer_registers_and_is_woken_through_the_real_send_entry() {
    let root = scratch("e2e-wake");
    let seen: Arc<Mutex<Vec<serde_json::Value>>> = Arc::new(Mutex::new(Vec::new()));
    let (server, gateway_a, gateway_b) =
        dsh_peers(&root, || wake_reply("msg-42"), Arc::clone(&seen));

    let sent = subscribe_and_send(&server);
    assert!(sent.ok, "send failed: {sent:?}");
    assert_eq!(
        sent.data["notification"], "dsh-wake-enqueued",
        "a dsh wake is queue admission, not agent execution"
    );
    assert_eq!(sent.data["consumed"], false);
    assert_eq!(sent.data["durable"], true);

    let requests = seen.lock().unwrap().clone();
    let enqueues = requests
        .iter()
        .filter(|request| request["method"] == "enqueue")
        .collect::<Vec<_>>();
    assert_eq!(enqueues.len(), 1, "exactly one wake per send: {requests:?}");
    let enqueue = enqueues[0];
    assert_eq!(enqueue["params"]["runtimeId"], "rt-b");
    assert_eq!(enqueue["params"]["agentId"], "ses-b");
    assert_eq!(
        enqueue["params"]["mode"], "followup",
        "the immediate sink mode maps to a followup wake"
    );
    assert_eq!(enqueue["params"]["content"][0]["type"], "text");
    assert!(
        enqueue["params"]["content"][0]["text"]
            .as_str()
            .is_some_and(|text| !text.is_empty()),
        "the wake must carry a body: {enqueue:?}"
    );
    drop((gateway_a, gateway_b));
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn a_dsh_wake_receipt_carries_the_gateway_message_id() {
    let root = scratch("e2e-receipt");
    let seen: Arc<Mutex<Vec<serde_json::Value>>> = Arc::new(Mutex::new(Vec::new()));
    let (server, gateway_a, gateway_b) =
        dsh_peers(&root, || wake_reply("msg-77"), Arc::clone(&seen));

    let sent = subscribe_and_send(&server);
    assert!(sent.ok, "send failed: {sent:?}");
    // The durable id is the gateway's queue admission receipt, not a claim that
    // the agent ran.
    assert_eq!(sent.data["notification"], "dsh-wake-enqueued");
    assert!(
        sent.data["msg_id"]
            .as_str()
            .is_some_and(|id| !id.is_empty()),
        "the receipt must carry the durable message id: {sent:?}"
    );
    drop((gateway_a, gateway_b));
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn a_refused_dsh_wake_is_labelled_as_a_dsh_rejection() {
    let root = scratch("e2e-refused");
    let seen: Arc<Mutex<Vec<serde_json::Value>>> = Arc::new(Mutex::new(Vec::new()));
    let (server, gateway_a, gateway_b) = dsh_peers(
        &root,
        || {
            Some(
                serde_json::json!({
                    "ok": false,
                    "error": {"code": "not-ready", "message": "agent is restarting"}
                })
                .to_string(),
            )
        },
        Arc::clone(&seen),
    );

    let sent = subscribe_and_send(&server);
    let error = sent.error.clone().unwrap_or_default();
    assert!(
        error.starts_with("DSH_NOTIFICATION_REJECTED:"),
        "a refused dsh wake must not be reported as a tmux rejection: {error}"
    );
    assert!(error.contains("not-ready"), "{error}");
    assert_eq!(sent.data["notification"], "subscribed-not-sent");
    assert_eq!(sent.data["repair_required"], true);
    drop((gateway_a, gateway_b));
    let _ = std::fs::remove_dir_all(&root);
}
