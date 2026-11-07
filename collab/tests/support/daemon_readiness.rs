use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixStream;
use std::path::Path;
use std::process::Child;
use std::time::{Duration, Instant};

pub fn wait_for_ping(child: &mut Child, socket: &Path, timeout: Duration) {
    let deadline = Instant::now() + timeout;
    loop {
        if let Some(status) = child.try_wait().expect("check test daemon") {
            panic!("test daemon exited before Ping: {status}");
        }
        if let Ok(mut stream) = UnixStream::connect(socket) {
            let probe_timeout = Duration::from_millis(200);
            let _ = stream.set_read_timeout(Some(probe_timeout));
            let _ = stream.set_write_timeout(Some(probe_timeout));
            if stream.write_all(b"{\"op\":\"Ping\"}\n").is_ok() {
                let mut line = String::new();
                if BufReader::new(stream).read_line(&mut line).is_ok()
                    && serde_json::from_str::<serde_json::Value>(&line)
                        .is_ok_and(|value| value["ok"] == true)
                {
                    return;
                }
            }
        }
        assert!(
            Instant::now() < deadline,
            "test daemon did not answer Ping at {}",
            socket.display()
        );
        std::thread::sleep(Duration::from_millis(25));
    }
}
