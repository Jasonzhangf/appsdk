pub struct CancellationHarness {
    _root: PathBuf,
    socket: PathBuf,
    received: Receiver<Barrier>,
    armed: Arc<Mutex<std::collections::HashSet<(String, String)>>>,
    stop: Arc<AtomicBool>,
    waiter: Option<JoinHandle<()>>,
}

impl CancellationHarness {
    pub fn start(root: &Path) -> Self {
        let root = root.join(format!(
            "hop-{}-{}",
            std::process::id(),
            FIXTURE_SEQUENCE.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir_all(&root).expect("create barrier root");
        let socket = root.join("barrier.sock");
        let listener = UnixListener::bind(&socket).expect("bind barrier listener");
        let (sender, received) = mpsc::channel::<Barrier>();
        let armed: Arc<Mutex<std::collections::HashSet<(String, String)>>> =
            Arc::new(Mutex::new(std::collections::HashSet::new()));
        let armed_loop = armed.clone();
        let stop = Arc::new(AtomicBool::new(false));
        let stop_loop = stop.clone();
        let waiter = std::thread::spawn(move || {
            for stream in listener.incoming() {
                let Ok(stream) = stream else { break };
                if stop_loop.load(Ordering::Relaxed) {
                    break;
                }
                let mut reader = BufReader::new(stream.try_clone().expect("clone barrier stream"));
                let mut line = String::new();
                if reader.read_line(&mut line).unwrap_or(0) == 0 {
                    continue;
                }
                let Ok(frame) = serde_json::from_str::<Value>(&line) else {
                    continue;
                };
                let barrier = Barrier {
                    operation_id: frame["operation_id"]
                        .as_str()
                        .unwrap_or_default()
                        .to_owned(),
                    boundary: frame["boundary"].as_str().unwrap_or_default().to_owned(),
                    pid: frame["pid"].as_u64().unwrap_or_default() as u32,
                    nested_command_id: frame["nested_command_id"].as_str().map(ToOwned::to_owned),
                    nested_operation_id: frame["nested_operation_id"]
                        .as_str()
                        .map(ToOwned::to_owned),
                    release: stream,
                };
                let held = armed_loop
                    .lock()
                    .unwrap()
                    .contains(&(barrier.boundary.clone(), barrier.operation_id.clone()));
                if held {
                    if sender.send(barrier).is_err() {
                        break;
                    }
                } else {
                    barrier.release();
                }
            }
        });
        Self {
            _root: root,
            socket,
            received,
            armed,
            stop,
            waiter: Some(waiter),
        }
    }

    pub fn socket(&self) -> &Path {
        &self.socket
    }

    /// Hold this exact barrier when it arrives; unrelated boundaries are
    /// released automatically so seeds and other operations never block.
    pub fn arm(&self, boundary: &str, operation_id: &str) {
        self.armed
            .lock()
            .unwrap()
            .insert((boundary.to_owned(), operation_id.to_owned()));
    }

    pub fn recv(&mut self, timeout: Duration) -> Barrier {
        self.received
            .recv_timeout(timeout)
            .expect("barrier acknowledgement within timeout")
    }

    /// Receive barriers until one matches `(boundary, operation_id)`, releasing
    /// any unrelated barrier so the sender is not left blocked.
    pub fn wait_for(&mut self, boundary: &str, operation_id: &str, timeout: Duration) -> Barrier {
        self.try_wait_for(boundary, operation_id, timeout)
            .unwrap_or_else(|error| panic!("{error}"))
    }

    pub fn try_wait_for(
        &mut self,
        boundary: &str,
        operation_id: &str,
        timeout: Duration,
    ) -> Result<Barrier, String> {
        let deadline = Instant::now() + timeout;
        loop {
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                return Err(format!(
                    "barrier {boundary} for {operation_id} did not arrive"
                ));
            }
            let barrier = self
                .received
                .recv_timeout(remaining)
                .map_err(|error| format!("barrier {boundary} for {operation_id}: {error}"))?;
            if barrier.boundary == boundary && barrier.operation_id == operation_id {
                return Ok(barrier);
            }
            barrier.release();
        }
    }
}

impl Drop for CancellationHarness {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(waiter) = self.waiter.take() {
            // Signal shutdown and wake accept() while the listening path still exists.
            let _ = UnixStream::connect(&self.socket);
            let _ = waiter.join();
        }
        let _ = std::fs::remove_file(&self.socket);
    }
}

/// A reusable MCP reader that preserves numeric versus string JSON-RPC ids so a
/// cancellation notification can route to exactly the active context call.
pub struct McpReader {
    pub child: Child,
    stdin: Option<ChildStdin>,
    replies: Mutex<Receiver<Result<Value, String>>>,
    reader: Option<JoinHandle<()>>,
}

impl McpReader {
    pub fn start(fixture: &Fixture, pane: Option<&Pane>) -> Self {
        let Mcp {
            child,
            stdin,
            replies,
            reader,
        } = Mcp::start(fixture, pane);
        Self {
            child,
            stdin,
            replies: Mutex::new(replies),
            reader,
        }
    }

    pub fn send_raw(&mut self, value: &Value) -> u64 {
        let stdin = self.stdin.as_mut().expect("MCP stdin is open");
        serde_json::to_writer(&mut *stdin, value).expect("serialize raw MCP message");
        writeln!(stdin).expect("write raw MCP message");
        stdin.flush().expect("flush raw MCP message");
        value["id"].as_u64().unwrap_or_default()
    }

    pub fn cancel(&mut self, request_id: u64) {
        self.send_raw(&json!({
            "jsonrpc":"2.0",
            "method":"notifications/cancelled",
            "params":{"requestId":request_id,"reason":"fixture cancellation boundary"}
        }));
    }

    pub fn recv(&mut self, timeout: Duration) -> Result<Value, String> {
        self.replies
            .lock()
            .unwrap()
            .recv_timeout(timeout)
            .map_err(|error| format!("MCP reply timeout: {error}"))?
    }

    pub fn finish(mut self) {
        self.stdin.take();
        let status = self.child.wait().expect("wait for MCP process");
        assert!(status.success(), "MCP process must exit cleanly: {status}");
        if let Some(reader) = self.reader.take() {
            reader.join().expect("join MCP stdout reader");
        }
    }
}

pub fn read_lines(path: &Path) -> Vec<Value> {
    let Ok(bytes) = std::fs::read(path) else {
        return Vec::new();
    };
    bytes
        .split(|byte| *byte == b'\n')
        .filter(|line| !line.is_empty())
        .map(|line| serde_json::from_slice(line).expect("journal line is JSON"))
        .collect()
}

pub fn journal_operations(path: &Path, operation_id: &str) -> Vec<Value> {
    read_lines(path)
        .into_iter()
        .filter(|record| record["operation_id"] == operation_id)
        .collect()
}

pub fn journal_count(path: &Path, operation_id: &str) -> usize {
    journal_operations(path, operation_id).len()
}

pub fn phases(record: &Value) -> Vec<String> {
    record["committed_phases"]
        .as_array()
        .map(|values| {
            values
                .iter()
                .filter_map(Value::as_str)
                .map(ToOwned::to_owned)
                .collect()
        })
        .unwrap_or_default()
}

pub fn hash_records(records: &[Value]) -> String {
    let mut digest = Sha256::new();
    for record in records {
        digest.update(record.to_string().as_bytes());
    }
    format!("sha256:{:x}", digest.finalize())
}

pub fn assert_no_secret_text(label: &str, text: &str, secret: &str) {
    assert!(
        !text.contains(secret),
        "{label} leaked the query capability"
    );
}

pub fn assert_no_public_secret_surface(label: &str, value: &Value) {
    match value {
        Value::Object(map) => {
            for (key, child) in map {
                assert_ne!(key, "token", "{label} leaked a token key");
                assert_ne!(
                    key, "query_capability",
                    "{label} leaked a query capability key"
                );
                assert_no_public_secret_surface(label, child);
            }
        }
        Value::Array(items) => {
            for item in items {
                assert_no_public_secret_surface(label, item);
            }
        }
        Value::String(value) => {
            assert!(
                !value.starts_with("base64url:"),
                "{label} leaked a raw capability value"
            );
        }
        _ => {}
    }
}
