fn endpoint_path(endpoint: &str) -> Result<PathBuf, AdapterError> {
    let path = endpoint
        .strip_prefix("unix://")
        .ok_or_else(|| AdapterError::Unknown {
            operation: "verify_candidate",
            detail: "only unix:// App Server endpoints are supported".into(),
        })?;
    if path.is_empty() {
        return Err(AdapterError::Unknown {
            operation: "verify_candidate",
            detail: "App Server endpoint has no socket path".into(),
        });
    }
    Ok(PathBuf::from(path))
}

fn thread_metadata(client: &mut Client, thread_id: &str) -> Result<Value, AdapterError> {
    let receipt = client.call("thread/read", json!({"threadId": thread_id}))?;
    let thread = receipt.get("thread").ok_or_else(|| AdapterError::Unknown {
        operation: "thread/read",
        detail: "response is missing thread".into(),
    })?;
    let observed = thread
        .get("id")
        .and_then(Value::as_str)
        .filter(|value| !value.trim().is_empty())
        .ok_or_else(|| AdapterError::Unknown {
            operation: "thread/read",
            detail: "response is missing thread.id".into(),
        })?;
    if observed != thread_id {
        return Err(AdapterError::Unknown {
            operation: "thread/read",
            detail: format!("thread identity mismatch: expected {thread_id}, observed {observed}"),
        });
    }
    Ok(thread.clone())
}

fn thread_status_from_metadata(thread: &Value) -> Result<String, AdapterError> {
    thread
        .pointer("/status/type")
        .and_then(Value::as_str)
        .filter(|value| !value.trim().is_empty())
        .map(str::to_owned)
        .ok_or_else(|| AdapterError::Unknown {
            operation: "thread/read",
            detail: "response is missing thread.status.type".into(),
        })
}

pub(crate) fn escape_delegated_text(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

fn delegated_prompt(
    source_thread_id: Option<&str>,
    client_user_message_id: &str,
    body: &str,
) -> String {
    let source = source_thread_id
        .map(|source_thread_id| {
            format!(
                "  <source_thread_id>{}</source_thread_id>\n",
                escape_delegated_text(source_thread_id)
            )
        })
        .unwrap_or_default();
    format!(
        "<codex_delegation>\n{}  <client_message_id>{}</client_message_id>\n  <input>{}</input>\n</codex_delegation>",
        source,
        escape_delegated_text(client_user_message_id),
        escape_delegated_text(body)
    )
}

fn active_turn_id(client: &mut Client, thread_id: &str) -> Result<Option<String>, AdapterError> {
    let page = client.call(
        "thread/turns/list",
        json!({
            "threadId": thread_id,
            "limit": 100,
            "sortDirection": "desc",
        }),
    )?;
    active_turn_id_from_page(&page)
}

fn active_turn_id_from_page(page: &Value) -> Result<Option<String>, AdapterError> {
    let turns =
        page.get("data")
            .and_then(Value::as_array)
            .ok_or_else(|| AdapterError::Unknown {
                operation: "thread/turns/list",
                detail: "response is missing data array".into(),
            })?;
    let mut active = Vec::new();
    for (index, turn) in turns.iter().enumerate() {
        match turn.get("status").and_then(Value::as_str) {
            Some("inProgress") => {}
            Some("completed" | "interrupted" | "failed") => continue,
            Some(status) => {
                return Err(AdapterError::Unknown {
                    operation: "turn/steer",
                    detail: format!(
                        "AUTO_NOTIFY_UNSUPPORTED_TURN_STATUS: turn at data[{index}] has status {status}"
                    ),
                })
            }
            None => {
                return Err(AdapterError::Unknown {
                    operation: "turn/steer",
                    detail: format!("turn at data[{index}] is missing status"),
                })
            }
        }
        let turn_id = turn
            .get("id")
            .ok_or_else(|| AdapterError::Unknown {
                operation: "turn/steer",
                detail: format!("inProgress turn at data[{index}] is missing id"),
            })?
            .as_str()
            .ok_or_else(|| AdapterError::Unknown {
                operation: "turn/steer",
                detail: format!("inProgress turn at data[{index}] id must be a JSON string"),
            })?;
        if turn_id.trim().is_empty() {
            return Err(AdapterError::Unknown {
                operation: "turn/steer",
                detail: format!("inProgress turn at data[{index}] id must be non-empty after trim"),
            });
        }
        if turn_id.chars().any(char::is_whitespace) {
            return Err(AdapterError::Unknown {
                operation: "turn/steer",
                detail: format!("inProgress turn at data[{index}] id must not contain whitespace"),
            });
        }
        active.push(turn_id);
    }
    match active.len() {
        0 => Ok(None),
        1 => {
            let turn_id = active.remove(0);
            Ok(Some(turn_id.to_owned()))
        }
        _ => Err(AdapterError::Unknown {
            operation: "turn/steer",
            detail: format!(
                "STEER_ACTIVE_TURN_AMBIGUOUS: recipient has {} inProgress turns",
                active.len()
            ),
        }),
    }
}

#[derive(Debug, PartialEq, Eq)]
enum NotificationAction {
    Start,
    Steer(String),
    Queue,
}

fn notification_action(
    thread_status: &str,
    active_turn_id: Option<String>,
) -> Result<NotificationAction, AdapterError> {
    match thread_status {
        "active" => Ok(match active_turn_id {
            Some(turn_id) => NotificationAction::Steer(turn_id),
            None => NotificationAction::Start,
        }),
        "idle" => Ok(NotificationAction::Start),
        // `thread/read` reports notLoaded for a persisted thread that is cold
        // on this endpoint.  `turn/start` is the native load-and-start call,
        // so an immediate notification loads it instead of refusing.
        "notLoaded" => Ok(NotificationAction::Start),
        status => Err(AdapterError::Unknown {
            operation: "thread/read",
            detail: format!(
                "AUTO_NOTIFY_UNSUPPORTED_THREAD_STATUS: cannot deliver to thread status {status}"
            ),
        }),
    }
}

fn queued_notification_action(
    thread_status: &str,
    active_turn_id: Option<String>,
) -> Result<NotificationAction, AdapterError> {
    match thread_status {
        "active" => Ok(match active_turn_id {
            Some(_) => NotificationAction::Queue,
            None => NotificationAction::Start,
        }),
        "idle" => Ok(NotificationAction::Start),
        "notLoaded" => Ok(NotificationAction::Start),
        status => Err(AdapterError::Unknown {
            operation: "thread/read",
            detail: format!(
                "AUTO_NOTIFY_UNSUPPORTED_THREAD_STATUS: cannot queue to thread status {status}"
            ),
        }),
    }
}

fn validate_queue_receipt(receipt: &Value) -> Result<(), AdapterError> {
    let id = receipt
        .pointer("/queuedSubmission/id")
        .and_then(Value::as_str)
        .filter(|value| !value.trim().is_empty())
        .ok_or_else(|| AdapterError::Unknown {
            operation: "thread/queue/add",
            detail: "response is missing queuedSubmission.id".into(),
        })?;
    if id.chars().any(char::is_whitespace) {
        return Err(AdapterError::Unknown {
            operation: "thread/queue/add",
            detail: "response returned an invalid queuedSubmission.id".into(),
        });
    }
    Ok(())
}

fn validate_steer_receipt(receipt: &Value, expected_turn_id: &str) -> Result<(), AdapterError> {
    let turn_id = receipt
        .get("turnId")
        .and_then(Value::as_str)
        .filter(|value| !value.trim().is_empty())
        .ok_or_else(|| AdapterError::Unknown {
            operation: "turn/steer",
            detail: "response is missing turnId".into(),
        })?;
    if turn_id != expected_turn_id {
        return Err(AdapterError::Unknown {
            operation: "turn/steer",
            detail: format!(
                "turn identity mismatch: expected {expected_turn_id}, observed {turn_id}"
            ),
        });
    }
    Ok(())
}

fn validate_immediate_receipt(receipt: &Value) -> Result<(), AdapterError> {
    let turn_id = receipt
        .pointer("/turn/id")
        .and_then(Value::as_str)
        .filter(|value| !value.trim().is_empty())
        .ok_or_else(|| AdapterError::Unknown {
            operation: "turn/start",
            detail: "response is missing turn.id".into(),
        })?;
    let status = receipt
        .pointer("/turn/status")
        .and_then(Value::as_str)
        .ok_or_else(|| AdapterError::Unknown {
            operation: "turn/start",
            detail: "response is missing turn.status".into(),
        })?;
    if !matches!(
        status,
        "inProgress" | "completed" | "interrupted" | "failed"
    ) {
        return Err(AdapterError::Unknown {
            operation: "turn/start",
            detail: format!("response returned unsupported turn status {status}"),
        });
    }
    if turn_id.chars().any(char::is_whitespace) {
        return Err(AdapterError::Unknown {
            operation: "turn/start",
            detail: "response returned an invalid turn.id".into(),
        });
    }
    Ok(())
}

fn method_exists(client: &mut Client, method: &str, params: Value) -> Result<bool, AdapterError> {
    match client.call_raw(method, params)? {
        Ok(_) => Ok(true),
        Err(error) => Ok(error.code != -32601),
    }
}

fn socket_candidate() -> Option<PathBuf> {
    if let Some(value) = std::env::var_os(APPSERVER_SOCKET_ENV).filter(|value| !value.is_empty()) {
        return Some(PathBuf::from(value));
    }
    if let Some(value) =
        std::env::var_os("CODEX_APP_SERVER_SOCKET").filter(|value| !value.is_empty())
    {
        return Some(PathBuf::from(value));
    }
    None
}

struct Client {
    stream: UnixStream,
    timeout: Duration,
    next_id: u64,
}

#[derive(Debug)]
struct RpcError {
    code: i64,
    message: String,
}

impl Client {
    fn connect(path: &Path, timeout: Duration) -> Result<Self, AdapterError> {
        let stream = UnixStream::connect(path).map_err(|error| {
            if matches!(
                error.kind(),
                std::io::ErrorKind::NotFound
                    | std::io::ErrorKind::ConnectionRefused
                    | std::io::ErrorKind::ConnectionReset
                    | std::io::ErrorKind::ConnectionAborted
            ) {
                AdapterError::RouteUnavailable {
                    detail: format!("{}: {error}", path.display()),
                }
            } else {
                AdapterError::Unknown {
                    operation: "connect",
                    detail: format!("{}: {error}", path.display()),
                }
            }
        })?;
        stream
            .set_read_timeout(Some(timeout))
            .map_err(|error| AdapterError::Unknown {
                operation: "connect",
                detail: format!("set read timeout: {error}"),
            })?;
        stream
            .set_write_timeout(Some(timeout))
            .map_err(|error| AdapterError::Unknown {
                operation: "connect",
                detail: format!("set write timeout: {error}"),
            })?;
        let mut client = Self {
            stream,
            timeout,
            next_id: 1,
        };
        client.handshake()?;
        Ok(client)
    }

    fn handshake(&mut self) -> Result<(), AdapterError> {
        let key = websocket_key();
        let request = format!(
            "GET / HTTP/1.1\r\nHost: localhost\r\nUpgrade: websocket\r\nConnection: Upgrade\r\nSec-WebSocket-Key: {key}\r\nSec-WebSocket-Version: 13\r\n\r\n"
        );
        self.stream
            .write_all(request.as_bytes())
            .map_err(|error| transport("websocket handshake write", error))?;
        let mut reader = BufReader::new(
            self.stream
                .try_clone()
                .map_err(|error| transport("websocket handshake clone", error))?,
        );
        let mut status = String::new();
        reader
            .read_line(&mut status)
            .map_err(|error| transport("websocket handshake status", error))?;
        if !status.starts_with("HTTP/1.1 101") && !status.starts_with("HTTP/1.0 101") {
            return Err(AdapterError::Unknown {
                operation: "websocket handshake",
                detail: format!("upgrade rejected: {}", status.trim()),
            });
        }
        loop {
            let mut header = String::new();
            reader
                .read_line(&mut header)
                .map_err(|error| transport("websocket handshake header", error))?;
            if header == "\r\n" || header == "\n" || header.is_empty() {
                break;
            }
        }
        Ok(())
    }

    fn initialize(&mut self) -> Result<Value, AdapterError> {
        let result = self.call(
            "initialize",
            json!({
                "clientInfo": {
                    "name": "collab",
                    "title": "Collab",
                    "version": env!("COLLAB_VERSION")
                },
                "capabilities": {"experimentalApi": true}
            }),
        )?;
        self.notify("initialized", json!({}))?;
        Ok(result)
    }

    fn call(&mut self, method: &str, params: Value) -> Result<Value, AdapterError> {
        match self.call_raw(method, params)? {
            Ok(value) => Ok(value),
            // One thread admits one writer.  The refusal means a different
            // live App Server process owns the rollout, so this endpoint can
            // never take the thread over; report it as its own terminal class
            // instead of an opaque rpc failure.
            Err(error) if is_thread_writer_conflict(&error.message) => {
                Err(AdapterError::ThreadWriterConflict {
                    detail: error.message,
                })
            }
            Err(error) => Err(AdapterError::Unknown {
                operation: "rpc",
                detail: error.message,
            }),
        }
    }

    fn call_raw(
        &mut self,
        method: &str,
        params: Value,
    ) -> Result<Result<Value, RpcError>, AdapterError> {
        let id = self.next_id;
        self.next_id = self.next_id.saturating_add(1);
        self.write_json(&json!({"method": method, "id": id, "params": params}))?;
        loop {
            let value = self.read_json()?;
            if value.get("id").and_then(Value::as_u64) != Some(id) {
                continue;
            }
            if let Some(error) = value.get("error") {
                return Ok(Err(RpcError {
                    code: error.get("code").and_then(Value::as_i64).unwrap_or(0),
                    message: error
                        .get("message")
                        .and_then(Value::as_str)
                        .unwrap_or("native JSON-RPC error")
                        .to_owned(),
                }));
            }
            return Ok(Ok(value.get("result").cloned().ok_or_else(|| {
                AdapterError::Unknown {
                    operation: "rpc",
                    detail: "native response is missing result".into(),
                }
            })?));
        }
    }

    fn notify(&mut self, method: &str, params: Value) -> Result<(), AdapterError> {
        self.write_json(&json!({"method": method, "params": params}))
    }

    fn write_json(&mut self, value: &Value) -> Result<(), AdapterError> {
        let payload = serde_json::to_vec(value).map_err(|error| AdapterError::Unknown {
            operation: "encode",
            detail: error.to_string(),
        })?;
        if payload.len() > MAX_OUTGOING_FRAME_BYTES {
            return Err(AdapterError::Unknown {
                operation: "encode",
                detail: "native frame exceeds maximum size".into(),
            });
        }
        self.stream
            .write_all(&encode_frame(0x1, &payload))
            .map_err(|error| transport("websocket write", error))?;
        self.stream
            .flush()
            .map_err(|error| transport("websocket flush", error))
    }

    fn read_json(&mut self) -> Result<Value, AdapterError> {
        loop {
            let payload = self.read_frame()?;
            let value: Value =
                serde_json::from_slice(&payload).map_err(|error| AdapterError::Unknown {
                    operation: "decode",
                    detail: error.to_string(),
                })?;
            if value.get("id").is_some() {
                return Ok(value);
            }
        }
    }

    fn read_frame(&mut self) -> Result<Vec<u8>, AdapterError> {
        let mut header = [0_u8; 2];
        self.stream
            .read_exact(&mut header)
            .map_err(|error| transport("websocket header", error))?;
        let opcode = header[0] & 0x0f;
        let masked = header[1] & 0x80 != 0;
        let mut length = (header[1] & 0x7f) as u64;
        if length == 126 {
            let mut bytes = [0_u8; 2];
            self.stream
                .read_exact(&mut bytes)
                .map_err(|error| transport("websocket length", error))?;
            length = u16::from_be_bytes(bytes) as u64;
        } else if length == 127 {
            let mut bytes = [0_u8; 8];
            self.stream
                .read_exact(&mut bytes)
                .map_err(|error| transport("websocket length", error))?;
            length = u64::from_be_bytes(bytes);
        }
        if length > MAX_INCOMING_FRAME_BYTES as u64 {
            return Err(AdapterError::Unknown {
                operation: "websocket frame",
                detail: "native frame exceeds maximum size".into(),
            });
        }
        let length = usize::try_from(length).map_err(|_| AdapterError::Unknown {
            operation: "websocket frame",
            detail: "native frame exceeds maximum size".into(),
        })?;
        let mut mask = [0_u8; 4];
        if masked {
            self.stream
                .read_exact(&mut mask)
                .map_err(|error| transport("websocket mask", error))?;
        }
        let mut payload = vec![0_u8; length];
        self.stream
            .read_exact(&mut payload)
            .map_err(|error| transport("websocket payload", error))?;
        if masked {
            for (index, byte) in payload.iter_mut().enumerate() {
                *byte ^= mask[index % 4];
            }
        }
        match opcode {
            0x1 => Ok(payload),
            0x8 => Err(AdapterError::Unknown {
                operation: "websocket",
                detail: "native App Server closed the connection".into(),
            }),
            0x9 => {
                self.stream
                    .write_all(&encode_frame(0xA, &payload))
                    .map_err(|error| transport("websocket pong", error))?;
                self.read_frame()
            }
            _ => self.read_frame(),
        }
    }
}

fn transport(operation: &'static str, error: std::io::Error) -> AdapterError {
    if matches!(
        error.kind(),
        std::io::ErrorKind::TimedOut | std::io::ErrorKind::WouldBlock
    ) {
        AdapterError::Timeout { operation }
    } else {
        AdapterError::Unknown {
            operation,
            detail: error.to_string(),
        }
    }
}

fn encode_frame(opcode: u8, payload: &[u8]) -> Vec<u8> {
    let mut frame = Vec::with_capacity(payload.len() + 14);
    frame.push(0x80 | opcode);
    let mask = [
        rand::random::<u8>(),
        rand::random::<u8>(),
        rand::random::<u8>(),
        rand::random::<u8>(),
    ];
    match payload.len() {
        length if length < 126 => frame.push(0x80 | length as u8),
        length if length <= u16::MAX as usize => {
            frame.push(0x80 | 126);
            frame.extend_from_slice(&(length as u16).to_be_bytes());
        }
        length => {
            frame.push(0x80 | 127);
            frame.extend_from_slice(&(length as u64).to_be_bytes());
        }
    }
    frame.extend_from_slice(&mask);
    frame.extend(
        payload
            .iter()
            .enumerate()
            .map(|(index, byte)| byte ^ mask[index % 4]),
    );
    frame
}

fn websocket_key() -> String {
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let bytes: [u8; 16] = rand::random();
    let mut output = String::with_capacity(24);
    for chunk in bytes.chunks(3) {
        let b0 = chunk[0] as u32;
        let b1 = chunk.get(1).copied().unwrap_or(0) as u32;
        let b2 = chunk.get(2).copied().unwrap_or(0) as u32;
        let value = (b0 << 16) | (b1 << 8) | b2;
        output.push(ALPHABET[((value >> 18) & 0x3f) as usize] as char);
        output.push(ALPHABET[((value >> 12) & 0x3f) as usize] as char);
        output.push(if chunk.len() > 1 {
            ALPHABET[((value >> 6) & 0x3f) as usize] as char
        } else {
            '='
        });
        output.push(if chunk.len() > 2 {
            ALPHABET[(value & 0x3f) as usize] as char
        } else {
            '='
        });
    }
    output
}
