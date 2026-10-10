use super::*;
use std::os::unix::net::UnixListener;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::time::Duration;

fn test_root(name: &str) -> PathBuf {
    let root = PathBuf::from(format!(
        "/tmp/collab-startup-{name}-{}-{}",
        std::process::id(),
        now_ms()
    ));
    std::fs::create_dir_all(root.join(".agent-collab/server")).expect("create startup test root");
    root
}

#[test]
fn legacy_host_fence_is_isolated_with_a_custom_state_root() {
    let root = test_root("legacy-fence-path");
    let default_root = root.join(".collab");
    let custom_root = root.join("host-state");
    assert_eq!(
        legacy_host_daemon_lock_path_for(&custom_root, &default_root),
        custom_root.join("legacy-host.lock")
    );
    assert_eq!(
        legacy_host_daemon_lock_path_for(&default_root, &default_root),
        PathBuf::from(LEGACY_HOST_DAEMON_LOCK_PATH)
    );

    std::fs::remove_dir_all(root).expect("remove custom state root");
}

#[tokio::test]
async fn timer_scheduler_does_not_overlap_ticks() {
    let (stop_tx, stop_rx) = tokio::sync::mpsc::channel(1);
    let in_flight = Arc::new(AtomicBool::new(false));
    let release = Arc::new(AtomicBool::new(false));
    let starts = Arc::new(AtomicUsize::new(0));
    let scheduler = tokio::spawn(run_timer_scheduler(stop_rx, 1, || vec![()], {
        let in_flight = in_flight.clone();
        let starts = starts.clone();
        let release = release.clone();
        move |_| {
            let in_flight = in_flight.clone();
            let starts = starts.clone();
            let release = release.clone();
            tokio::task::spawn_blocking(move || {
                assert!(
                    !in_flight.swap(true, Ordering::SeqCst),
                    "timer tick overlapped an in-flight tick"
                );
                starts.fetch_add(1, Ordering::SeqCst);
                while !release.load(Ordering::SeqCst) {
                    std::thread::sleep(Duration::from_millis(1));
                }
                in_flight.store(false, Ordering::SeqCst);
            })
        }
    }));

    tokio::time::timeout(Duration::from_secs(1), async {
        while starts.load(Ordering::SeqCst) == 0 {
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("scheduler did not start its first tick");
    tokio::time::sleep(Duration::from_millis(5)).await;
    assert_eq!(
        starts.load(Ordering::SeqCst),
        1,
        "scheduler queued another tick while the first was in flight"
    );
    release.store(true, Ordering::SeqCst);
    let _ = stop_tx.try_send(());
    tokio::time::timeout(Duration::from_secs(1), scheduler)
        .await
        .expect("scheduler did not stop")
        .expect("scheduler task failed");
}

#[tokio::test]
async fn pid_publication_failure_removes_the_owned_socket() {
    let _startup_test_lock = startup_test_lock();
    let root = test_root("pid-failure");
    let host_paths = HostPaths::for_state_root(root.join("host-state")).unwrap();
    host_paths.ensure_root().unwrap();
    std::fs::create_dir(host_paths.pid_path()).expect("occupy pid path");

    let error = run_with_host_paths(Scope { root: root.clone() }, host_paths.clone())
        .await
        .expect_err("a directory at server.pid must fail startup");
    assert!(error.to_string().contains("directory"), "{error:#}");
    assert!(
        !host_paths.socket_path().exists(),
        "startup failure must remove the socket it just published"
    );

    std::fs::remove_dir_all(root).expect("remove startup test root");
}

#[tokio::test]
async fn startup_rejects_a_reachable_legacy_project_daemon() {
    let _startup_test_lock = startup_test_lock();
    let root = test_root("legacy-socket");
    let host_paths = HostPaths::for_state_root(root.join("host-state")).unwrap();
    host_paths.ensure_root().unwrap();
    let legacy_socket = root.join(".agent-collab/server/server.sock");
    let listener = UnixListener::bind(&legacy_socket).expect("bind legacy socket");

    let error = run_with_host_paths(Scope { root: root.clone() }, host_paths.clone())
        .await
        .expect_err("a reachable legacy daemon must be fenced");
    assert!(error.to_string().contains("DAEMON_MIGRATION_REQUIRED"));
    assert!(error.to_string().contains("legacy project daemon"));
    assert!(!host_paths.socket_path().exists());
    assert!(legacy_socket.exists());

    drop(listener);
    std::fs::remove_dir_all(root).expect("remove startup test root");
}

#[tokio::test]
async fn startup_rejects_a_held_legacy_project_lock() {
    let _startup_test_lock = startup_test_lock();
    let root = test_root("legacy-lock");
    let host_paths = HostPaths::for_state_root(root.join("host-state")).unwrap();
    host_paths.ensure_root().unwrap();
    let legacy_lock = root.join(".agent-collab/server/daemon.lock");
    let lock_file = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .open(&legacy_lock)
        .expect("open legacy lock");
    use std::os::unix::io::AsRawFd;
    let rc = unsafe { libc::flock(lock_file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) };
    assert_eq!(rc, 0, "test must hold the legacy lock");

    let error = run_with_host_paths(Scope { root: root.clone() }, host_paths.clone())
        .await
        .expect_err("a held legacy writer lock must be fenced");
    assert!(error.to_string().contains("DAEMON_MIGRATION_REQUIRED"));
    assert!(error.to_string().contains("legacy project daemon lock"));
    assert!(!host_paths.socket_path().exists());

    drop(lock_file);
    std::fs::remove_dir_all(root).expect("remove startup test root");
}

#[tokio::test]
async fn running_host_daemon_holds_legacy_writer_fence() {
    let _startup_test_lock = startup_test_lock();
    let root = test_root("legacy-fence");
    let host_paths = HostPaths::for_state_root(root.join("host-state")).unwrap();
    host_paths.ensure_root().unwrap();
    let legacy_host_lock = legacy_host_daemon_lock_path(&host_paths);
    let socket = host_paths.socket_path();
    let running = tokio::spawn(run_with_host_paths(
        Scope { root: root.clone() },
        host_paths.clone(),
    ));

    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    while tokio::time::Instant::now() < deadline && !crate::client::alive(&socket) {
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    assert!(
        crate::client::alive(&socket),
        "host daemon did not publish an accepting socket"
    );

    let legacy_socket = root.join(".agent-collab/server/server.sock");
    let legacy_project_lock = root.join(".agent-collab/server/daemon.lock");
    let journal = root.join(".agent-collab/server/journal.jsonl");
    let journal_before = std::fs::read(&journal).expect("host daemon must open project journal");

    let old_host_attempt = acquire_daemon_lock(&legacy_host_lock, &legacy_socket);
    assert!(old_host_attempt
        .expect_err("old writer must not reacquire its host lock")
        .to_string()
        .contains("server already running"));
    let old_project_attempt = acquire_daemon_lock(&legacy_project_lock, &legacy_socket);
    assert!(old_project_attempt
        .expect_err("old writer must not reacquire its project lock")
        .to_string()
        .contains("server already running"));
    assert_eq!(journal_before, std::fs::read(&journal).unwrap());

    running.abort();
    let _ = running.await;
    std::fs::remove_dir_all(root).expect("remove startup test root");
}

#[tokio::test]
async fn daemon_exits_when_its_owned_state_root_is_removed() {
    let _startup_test_lock = startup_test_lock();
    let root = test_root("owned-state-root-removed");
    let host_paths = HostPaths::for_state_root(root.join("host-state")).unwrap();
    host_paths.ensure_root().unwrap();
    let socket = host_paths.socket_path();
    let running = tokio::spawn(run_with_host_paths(
        Scope { root: root.clone() },
        host_paths.clone(),
    ));

    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    while tokio::time::Instant::now() < deadline && !crate::client::alive(&socket) {
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    assert!(crate::client::alive(&socket), "daemon did not start");

    std::fs::remove_dir_all(host_paths.state_root()).expect("remove owned state root");
    tokio::time::timeout(Duration::from_secs(5), running)
        .await
        .expect("daemon did not exit after its state root disappeared")
        .expect("daemon task failed")
        .expect("daemon returned an error");

    assert!(!socket.exists());
    assert!(!host_paths.pid_path().exists());
    std::fs::remove_dir_all(root).expect("remove startup test root");
}

#[tokio::test]
async fn daemon_exits_when_its_owned_state_root_is_replaced() {
    let _startup_test_lock = startup_test_lock();
    let root = test_root("owned-state-root-replaced");
    let host_paths = HostPaths::for_state_root(root.join("host-state")).unwrap();
    host_paths.ensure_root().unwrap();
    let socket = host_paths.socket_path();
    let running = tokio::spawn(run_with_host_paths(
        Scope { root: root.clone() },
        host_paths.clone(),
    ));

    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    while tokio::time::Instant::now() < deadline && !crate::client::alive(&socket) {
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    assert!(crate::client::alive(&socket), "daemon did not start");

    std::fs::remove_dir_all(host_paths.state_root()).expect("remove owned state root");
    std::fs::create_dir(host_paths.state_root()).expect("recreate state root");
    tokio::time::timeout(Duration::from_secs(5), running)
        .await
        .expect("daemon did not exit after its state root was replaced")
        .expect("daemon task failed")
        .expect("daemon returned an error");

    assert!(!socket.exists());
    assert!(!host_paths.pid_path().exists());
    std::fs::remove_dir_all(root).expect("remove startup test root");
}

#[tokio::test]
async fn removing_an_unrelated_directory_does_not_stop_the_daemon() {
    let _startup_test_lock = startup_test_lock();
    let root = test_root("unrelated-root-removed");
    let host_paths = HostPaths::for_state_root(root.join("host-state")).unwrap();
    host_paths.ensure_root().unwrap();
    let socket = host_paths.socket_path();
    let unrelated = root.join("unrelated");
    std::fs::create_dir_all(&unrelated).unwrap();
    let running = tokio::spawn(run_with_host_paths(
        Scope { root: root.clone() },
        host_paths.clone(),
    ));

    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    while tokio::time::Instant::now() < deadline && !crate::client::alive(&socket) {
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    assert!(crate::client::alive(&socket), "daemon did not start");

    std::fs::remove_dir_all(&unrelated).expect("remove unrelated directory");
    tokio::time::sleep(Duration::from_millis(500)).await;
    assert!(
        !running.is_finished(),
        "daemon exited for an unrelated directory removal"
    );
    assert!(crate::client::alive(&socket));

    running.abort();
    let _ = running.await;
    std::fs::remove_dir_all(root).expect("remove startup test root");
}

#[test]
fn cleanup_preserves_a_replacement_socket_path() {
    let root = test_root("replacement");
    let host_paths = HostPaths::for_state_root(root.join("host-state")).unwrap();
    host_paths.ensure_root().unwrap();
    let socket = host_paths.socket_path();
    let original = UnixListener::bind(&socket).expect("bind original socket");
    let captured = std::fs::symlink_metadata(&socket).expect("capture socket metadata");
    drop(original);
    // Keep the original inode allocated while creating the replacement.
    let retired = socket.with_extension("retired");
    std::fs::rename(&socket, &retired).expect("retire original socket path");
    let replacement = UnixListener::bind(&socket).expect("bind replacement socket");
    let current = std::fs::symlink_metadata(&socket).expect("read replacement metadata");
    assert!(!same_inode(&captured, &current));

    remove_listener_socket(&socket, &captured).expect("replacement cleanup check");
    assert!(
        socket.exists(),
        "cleanup must not unlink a replacement socket"
    );

    drop(replacement);
    std::fs::remove_file(&socket).ok();
    std::fs::remove_file(&retired).ok();
    std::fs::remove_dir_all(root).expect("remove startup test root");
}

#[tokio::test]
async fn retry_after_pid_failure_succeeds_once_the_path_is_fixed() {
    let _startup_test_lock = startup_test_lock();
    let root = test_root("retry");
    let host_paths = HostPaths::for_state_root(root.join("host-state")).unwrap();
    host_paths.ensure_root().unwrap();
    let pid_path = host_paths.pid_path();
    std::fs::create_dir(&pid_path).expect("occupy pid path");
    let first_error = run_with_host_paths(Scope { root: root.clone() }, host_paths.clone())
        .await
        .expect_err("first startup must fail");
    assert!(first_error.to_string().contains("directory"));
    assert!(!host_paths.socket_path().exists());
    std::fs::remove_dir(&pid_path).expect("remove pid directory");

    let socket = host_paths.socket_path();
    let running = tokio::spawn(run_with_host_paths(
        Scope { root: root.clone() },
        host_paths.clone(),
    ));
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    while tokio::time::Instant::now() < deadline {
        let socket = socket.clone();
        let status = tokio::task::spawn_blocking(move || crate::client::daemon_status(&socket))
            .await
            .expect("readiness probe task must complete");
        if status == crate::client::DaemonAvailability::Alive {
            break;
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    let socket = socket.clone();
    let status = tokio::task::spawn_blocking(move || crate::client::daemon_status(&socket))
        .await
        .expect("readiness probe task must complete");
    assert_eq!(status, crate::client::DaemonAvailability::Alive);
    assert!(pid_path.is_file());

    running.abort();
    let _ = running.await;
    std::fs::remove_dir_all(root).expect("remove startup test root");
}
