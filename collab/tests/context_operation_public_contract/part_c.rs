#[test]
fn pane_control_requests_survive_notification_input() {
    use std::os::unix::fs::PermissionsExt;
    let _fixture_lock = TMUX_FIXTURE_LOCK.lock().unwrap();
    let real_tmux = Command::new("/bin/sh").args(["-c", "command -v tmux"])
        .output().unwrap();
    assert!(real_tmux.status.success());
    let real_tmux = String::from_utf8(real_tmux.stdout).unwrap().trim().to_owned();
    for mode in ["after", "before-long"] {
        let (fixture, pane) = Fixture::isolated(&format!("pane-control-{mode}"));
        fixture.registered_context(&format!("pane-control-seed-{mode}"), &pane);
        let wrapper = fixture.root.join("control-wrapper");
        fs::create_dir(&wrapper).unwrap();
        let notify = format!("{} -S {} send-keys -t {} -l {}",
            shell_quote(&real_tmux), shell_quote(&fixture.tmux_socket.to_string_lossy()),
            shell_quote(&pane.pane_id),
            shell_quote(&format!("COLLAB_NOTIFY {}", "x".repeat(if mode == "after" { 1 } else { 8192 }))));
        let script = if mode == "after" {
            format!("#!/bin/sh\n{} \"$@\" || exit $?\ncase \"$*\" in *COLLAB_TEST_PANE_REQUEST:*) {notify} ;; esac\n", shell_quote(&real_tmux))
        } else {
            format!("#!/bin/sh\ncase \"$*\" in *COLLAB_TEST_PANE_REQUEST:*) {notify} ;; esac\nexec {} \"$@\"\n", shell_quote(&real_tmux))
        };
        let wrapper_file = wrapper.join("tmux");
        fs::write(&wrapper_file, script).unwrap();
        fs::set_permissions(&wrapper_file, fs::Permissions::from_mode(0o755)).unwrap();
        let mut child = fixture.configured_command(&["who"], Some(&pane))
            .env("PANE_CONTROL_PATH", &wrapper)
            .stdout(Stdio::piped()).stderr(Stdio::piped()).spawn().unwrap();
        let output = wait_child_output(&mut child, Duration::from_secs(5));
        assert!(output.status.success(), "{mode}: {}", text(&output));
        let who: Value = serde_json::from_slice(&output.stdout).unwrap();
        assert!(who["workers"].as_array().is_some_and(|workers| !workers.is_empty()), "{mode}: {who}");
    }
}

fn wait_child_output(child: &mut std::process::Child, timeout: Duration) -> std::process::Output {
    let stdout = child.stdout.take().expect("fixture CLI child stdout is piped");
    let stderr = child.stderr.take().expect("fixture CLI child stderr is piped");
    let stdout_reader = std::thread::spawn(move || {
        let mut bytes = Vec::new();
        std::io::Read::read_to_end(&mut std::io::BufReader::new(stdout), &mut bytes).unwrap();
        bytes
    });
    let stderr_reader = std::thread::spawn(move || {
        let mut bytes = Vec::new();
        std::io::Read::read_to_end(&mut std::io::BufReader::new(stderr), &mut bytes).unwrap();
        bytes
    });
    let exited = wait_until(timeout, || child.try_wait().expect("poll exact fixture CLI child").is_some());
    if !exited {
        child.kill().expect("stop only this owned fixture CLI child after its deadline");
    }
    let status = child.wait().expect("wait for exact fixture CLI child");
    let stdout_bytes = stdout_reader.join().expect("join owned stdout reader");
    let stderr_bytes = stderr_reader.join().expect("join owned stderr reader");
    assert!(exited, "fixture CLI child {} did not exit within {timeout:?}; stdout={} stderr={}",
        child.id(), String::from_utf8_lossy(&stdout_bytes), String::from_utf8_lossy(&stderr_bytes));
    std::process::Output { status, stdout: stdout_bytes, stderr: stderr_bytes }
}
