use serde_json::{json, Value};
use std::io::{self, BufRead, Read, Write};
use std::process::{Command, Stdio};
use std::sync::mpsc;
use std::thread;

#[path = "collab-mcp/board_tools.rs"]
mod board_tools;

#[cfg(feature = "context-cancel-test-hooks")]
#[path = "../context_cancel_test_hooks.rs"]
mod context_cancel_test_hooks;

fn tool(name: &str, description: &str, properties: Value, required: &[&str]) -> Value {
    json!({
        "name": name,
        "description": description,
        "inputSchema": {"type":"object", "properties": properties, "required": required, "additionalProperties": false}
    })
}

fn peer_lifecycle_tool() -> Value {
    json!({
        "name": "collab_peer_lifecycle",
        "description": "Create, read, update, close, or query one exact peer lifecycle operation. Create starts a real peer and requires verified readiness. Read defaults to the caller. Update changes only the selected App Server cwd. Close retires the exact peer after responsibility checks. Create may use a stable operation_id on its first call; update/close operation_id is only for retrying a returned operation. Query reads one retained operation without a host effect. Targets and cwd must resolve to the same registered canonical project main and app scope. No token or query capability is accepted.",
        "inputSchema": {
            "type": "object",
            "properties": {
                "action": {"type":"string","enum":["create","read","update","close","query"]},
                "target_id": {"type":"string","minLength":1},
                "cwd": {"type":"string","minLength":1},
                "model": {"type":"string","minLength":1},
                "reason": {"type":"string","minLength":1},
                "operation_id": {"description":"Create: optional stable key for this first attempt or its retry. Update/close: omit on the first attempt; supply only the retained operation ID when retrying or querying an unknown result. Query: required retained operation ID.","type":"string","minLength":1}
            },
            "required": ["action"],
            "additionalProperties": false,
            "oneOf": [
                {"properties":{"action":{"const":"create"},"target_id":{"type":"string","minLength":1},"cwd":{"type":"string","minLength":1},"model":{"type":"string","minLength":1},"operation_id":{"type":"string","minLength":1}},"required":["action","target_id","cwd"],"additionalProperties":false},
                {"properties":{"action":{"const":"read"},"target_id":{"type":"string","minLength":1}},"required":["action"],"additionalProperties":false},
                {"properties":{"action":{"const":"update"},"target_id":{"type":"string","minLength":1},"cwd":{"type":"string","minLength":1},"operation_id":{"type":"string","minLength":1}},"required":["action","target_id","cwd"],"additionalProperties":false},
                {"properties":{"action":{"const":"close"},"target_id":{"type":"string","minLength":1},"reason":{"type":"string","minLength":1},"operation_id":{"type":"string","minLength":1}},"required":["action","target_id","reason"],"additionalProperties":false},
                {"properties":{"action":{"const":"query"},"operation_id":{"type":"string","minLength":1}},"required":["action","operation_id"],"additionalProperties":false}
            ]
        }
    })
}

fn tools() -> Value {
    let mut tools = json!([
        tool("collab_msg", "Read a durable notification by ID.", json!({"id":{"type":"string"}}), &["id"]),
        tool(
            "collab_recv",
            "Consume the durable inbox batch. receive_id is the caller-owned receive identity; repeat the same value to replay a committed receive after a lost response.",
            json!({
                "timeout":{"type":"integer","minimum":0},
                "receive_id":{"type":"string"}
            }),
            &["receive_id"]
        ),
        tool("collab_subagent", "Parent manages children; child uses ready/working and sends results via collab_sendmessage. status includes mailbox, keepalive and notification history. snapshot is explicit screen-tail read only, not a health probe. Observers without an App Server push channel must check status/mailbox themselves. rearm requires an explicit operator request after exhaustion. start accepts optional runtime=codex to override ~/.appsdk/config.toml. dispatch retains the legacy private managed-child path and is idempotent by request_id; ordinary peers must instead use collab_board_publish/invite with delivery/test conditions and an observed revision. bind commits one completed, verified ordinary Create result as this master's managed child; supply only the managed id and the retained create_operation_id, and the daemon derives parent, child thread, binding, generation and scope.", json!({"action":{"type":"string","enum":["start","dispatch","list","status","snapshot","rearm","send","ready","working","close","bind"]},"id":{"type":"string"},"create_operation_id":{"type":"string","minLength":1},"request_id":{"type":"string"},"runtime":{"type":"string","enum":["codex"]},"lines":{"type":"integer","minimum":1,"maximum":200},"subject":{"type":"string"},"body":{"type":"string"},"feature_id":{"type":"string"},"worktree_path":{"type":"string"},"branch":{"type":"string"},"base_commit":{"type":"string"},"priority":{"type":"string","enum":["p0","p1","p2","p3","p4"]},"next_step":{"type":"string"}}), &["action"]),
        tool(
            "collab_who",
            "List registered workers and active tasks.",
            json!({}),
            &[]
        ),
        tool(
            "collab_sendmessage",
            "Persist an explicit peer notification with a required short subject and original body preview; the recipient is woken only through its own active direct-message subscription.",
            json!({"to":{"type":"string"},"subject":{"type":"string"},"body":{"type":"string"},"delivery":{"type":"string","enum":["immediate","queued"],"default":"immediate"}}),
            &["to", "subject", "body"]
        ),
        tool(
            "collab_notify_methods",
            "List supported opt-in notification methods and event types.",
            json!({}),
            &[]
        ),
        tool(
            "collab_notify_subscribe",
            "Register one owner-scoped finite subscription; deadline uses either absolute at_ms values or a periodic interval, master-idle uses a recurring 15- or 60-minute interval for the live master, and at most three active subscriptions are allowed per Agent.",
            json!({"event":{"type":"string","enum":["direct-message","resource-released","deadline","master-idle"]},"subject":{"type":"string"},"at_ms":{"type":"array","items":{"type":"integer"}},"every_ms":{"type":"integer","minimum":1},"trigger_ms":{"type":"integer"},"repeat_count":{"type":"integer","minimum":1,"maximum":100},"ttl_seconds":{"type":"integer","minimum":1}}),
            &["event", "ttl_seconds"]
        ),
        tool(
            "collab_notify_status",
            "List the calling Agent's notification subscriptions.",
            json!({}),
            &[]
        ),
        tool(
            "collab_notify_unsubscribe",
            "Cancel one calling-Agent-owned notification subscription.",
            json!({"subscription_id":{"type":"string"}}),
            &["subscription_id"]
        ),
        tool(
            "collab_task_status",
            "Read the authoritative task registry.",
            json!({"id":{"type":"string"}}),
            &[]
        ),
        tool(
            "collab_task_accept",
            "Accept a board invitation with expected_revision through the shared accept transaction, or a legal legacy scheduler assignment without it. Rechecks other owned responsibilities before starting work.",
            json!({"id":{"type":"string"},"expected_revision":{"type":"integer","minimum":1}}),
            &["id"]
        ),
        tool(
            "collab_task_register",
            "Register a task owned by the calling peer; /goal delegation is deferred.",
            json!({"id":{"type":"string"},"feature":{"type":"string"},"worktree":{"type":"string"},"branch":{"type":"string"},"base_commit":{"type":"string"},"priority":{"type":"string"},"next":{"type":"string"}}),
            &["id"]
        ),
        tool(
            "collab_task_wait",
            "Record a bounded resource wait against the blocking task owner.",
            json!({"id":{"type":"string"},"blocking_task":{"type":"string"}}),
            &["id", "blocking_task"]
        ),
        tool(
            "collab_task_deliver",
            "Deliver a claimed task through the Server.",
            json!({"id":{"type":"string"},"evidence":{"type":"string"},"worktree":{"type":"string"}}),
            &["id", "evidence", "worktree"]
        ),
        tool(
            "collab_task_review",
            "Accept a delivered task or return it for rework with durable evidence. Accept registers a daemon-owned pending merge and notifies the live master; the master must merge on refs/heads/main and record collab_task_integrated before the task can close (TASK_MERGE_PENDING).",
            json!({"id":{"type":"string"},"accept":{"type":"boolean"},"rework":{"type":"boolean"},"evidence":{"type":"string"}}),
            &["id", "evidence"]
        ),
        tool(
            "collab_task_integrated",
            "Record exact integration of an accepted task on refs/heads/main and resolve its daemon-owned pending merge. Required before close; surfaced by collab_context, collab status --all pending_merges, appsdk longhorizon show, and master idle wake.",
            json!({"id":{"type":"string"},"commit":{"type":"string"},"evidence":{"type":"string"}}),
            &["id", "commit", "evidence"]
        ),
        tool(
            "collab_task_relocate",
            "Relocate the calling peer's task to the configured project worktree path.",
            json!({"id":{"type":"string"},"worktree":{"type":"string"},"branch":{"type":"string"},"base_commit":{"type":"string"}}),
            &["id", "worktree"]
        ),
        tool(
            "collab_task_block",
            "Mark an owned task blocked without notifying unrelated peers.",
            json!({"id":{"type":"string"},"next":{"type":"string"}}),
            &["id"]
        ),
        tool(
            "collab_task_update",
            "Update an authorized task state through the Server.",
            json!({"id":{"type":"string"},"status":{"type":"string"},"next":{"type":"string"}}),
            &["id"]
        ),
        tool(
            "collab_task_close",
            "Close the owner's merged task and safely clean its declared resources. Fails with TASK_MERGE_PENDING while a pending merge is unresolved.",
            json!({"id":{"type":"string"}}),
            &["id"]
        ),
        tool(
            "collab_migrate",
            "Run peer-authorized migration inspect, plan, apply, or verify.",
            json!({"action":{"type":"string","enum":["inspect","plan","apply","verify"]}}),
            &["action"]
        ),
        tool(
            "collab_inbox",
            "Read the durable Collab inbox.",
            json!({}),
            &[]
        ),
        tool(
            "collab_context",
            "The single agent bootstrap entry. Automatically resolves the canonical project root, creates a missing baseline, starts a stopped daemon, and lets the daemon establish, restore, or update identity, binding, and the default direct-message lease. Pass `provide` only when the returned snapshot has requires_identity_update.required=true; it must be a JSON object or JSON string containing only the exact `required_fields` (session_id, thread_id, endpoint, namespace). Values are observed facts, not identity selection; unknown, duplicate, null, empty, whitespace, or conflicting fields fail explicitly. Approved recovery accepts distinct `approve_identity` and `approve_grant` JSON objects; neither is generated by the adapter. The response displays only the authoritative snapshot and never the internal identity receipt or token. Do not use init, whoami, worker recover, route resolve, or down/up for this.",
            json!({"operation_id":{"description":"Retained operation key. Required only with query=true.","type":"string","minLength":1},"project_scope":{"description":"Canonical project root to select for this operation.","type":"string","minLength":1},"app_scope_id":{"description":"App scope identifier to select for this operation.","type":"string","minLength":1},"approve_identity":{"description":"Exact user-supplied identity recovery approval object.","oneOf":[{"type":"object"},{"type":"string","minLength":1}]},"approve_grant":{"description":"Exact user-supplied master grant replacement approval object.","oneOf":[{"type":"object"},{"type":"string","minLength":1}]},"provide":{"description":"JSON object or JSON string containing only the snapshot's exact required_fields; optional when facts are already observable","oneOf":[{"type":"object","additionalProperties":false,"properties":{"session_id":{"type":"string","minLength":1},"thread_id":{"type":"string","minLength":1},"endpoint":{"type":"string","minLength":1},"namespace":{"type":"string","minLength":1}}},{"type":"string","minLength":1}]},"query":{"description":"Read the durable projection with operation_id and the locally retained capability.","type":"boolean","default":false}}),
            &[]
        ),
        tool(
            "collab_ack",
            "Acknowledge owned mailbox messages.",
            json!({"ids":{"type":"array","items":{"type":"string"}}}),
            &["ids"]
        ),
        tool(
            "collab_master",
            "Inspect the current Collab master, replace it with an explicitly approved promote, clear it with an explicitly approved clear, or delegate as the current master. Codex root is unrelated. Init and register never create master. Independent peers may decline a master board invitation; private subworkers are not exposed on the public board.",
            json!({"action":{"type":"string","enum":["status","promote","clear","delegate"]},"approval":{"type":"string"},"target":{"type":"string"}}),
            &["action"]
        ),
        peer_lifecycle_tool()
    ]);
    tools
        .as_array_mut()
        .expect("tool list")
        .extend(board_tools::tools());
    tools
}

fn collab_bin() -> std::path::PathBuf {
    if let Ok(path) = std::env::var("COLLAB_BIN") {
        return path.into();
    }
    std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(|d| d.join("collab")))
        .unwrap_or_else(|| "collab".into())
}

fn call(name: &str, args: &Value) -> Result<String, String> {
    let argv = build_argv(name, args)?;
    let mut command = Command::new(collab_bin());
    command.args(argv);
    let output = command.output().map_err(|e| e.to_string())?;
    let stdout = String::from_utf8_lossy(&output.stdout).trim().to_string();
    let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
    if name == "collab_peer_lifecycle" {
        return peer_lifecycle_cli_output(output.status.success(), &stdout, &stderr);
    }
    if !output.status.success() {
        if name == "collab_context"
            && !stdout.is_empty()
            && serde_json::from_str::<Value>(&stdout)
                .ok()
                .is_some_and(|value| {
                    value.get("ok") == Some(&Value::Bool(false)) && value.get("result").is_some()
                })
        {
            return Ok(stdout);
        }
        return Err(if stderr.is_empty() { stdout } else { stderr });
    }
    if name == "collab_subagent" {
        if stdout.trim().is_empty() {
            return Err(
                "COLLAB_MCP_EMPTY_RESULT: collab subagent returned success without a result".into(),
            );
        }
        serde_json::from_str::<Value>(&stdout).map_err(|error| {
            format!("COLLAB_MCP_INVALID_RESULT: collab subagent returned invalid JSON: {error}")
        })?;
    }
    Ok(stdout)
}

fn peer_lifecycle_cli_output(success: bool, stdout: &str, stderr: &str) -> Result<String, String> {
    if stdout.is_empty() {
        return Err(if stderr.is_empty() {
            "COLLAB_MCP_EMPTY_RESULT: peer lifecycle returned no result".into()
        } else {
            stderr.to_owned()
        });
    }
    let value: Value = serde_json::from_str(stdout).map_err(|error| {
        format!("COLLAB_MCP_INVALID_RESULT: peer lifecycle returned invalid JSON: {error}")
    })?;
    let typed = value.get("result").is_some();
    if !typed {
        return Err(if stderr.is_empty() {
            "COLLAB_MCP_INVALID_RESULT: peer lifecycle response has no typed result".into()
        } else {
            stderr.to_owned()
        });
    }
    if success || value.get("ok") == Some(&Value::Bool(false)) {
        Ok(stdout.to_owned())
    } else {
        Err(if stderr.is_empty() {
            stdout.to_owned()
        } else {
            stderr.to_owned()
        })
    }
}

fn spawn_context_call(
    name: &str,
    args: &Value,
) -> Result<(u32, mpsc::Receiver<Result<Option<String>, String>>), String> {
    let name = name.to_owned();
    let argv = build_argv(&name, args)?;
    let mut command = Command::new(collab_bin());
    command
        .args(argv)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    #[cfg(feature = "context-cancel-test-hooks")]
    {
        // Keep SIGINT blocked across exec so the CLI installs its context
        // handler before an early cancellation can reach the child.
        use std::os::unix::process::CommandExt;
        unsafe {
            command.pre_exec(|| {
                let mut set = std::mem::MaybeUninit::<libc::sigset_t>::uninit();
                libc::sigemptyset(set.as_mut_ptr());
                libc::sigaddset(set.as_mut_ptr(), libc::SIGINT);
                if libc::sigprocmask(libc::SIG_BLOCK, set.as_ptr(), std::ptr::null_mut()) != 0 {
                    return Err(io::Error::last_os_error());
                }
                Ok(())
            });
        }
    }
    let mut child = command.spawn().map_err(|error| error.to_string())?;
    let pid = child.id();
    let mut stdout = child
        .stdout
        .take()
        .ok_or("context child stdout unavailable")?;
    let mut stderr = child
        .stderr
        .take()
        .ok_or("context child stderr unavailable")?;
    let stdout_thread = {
        thread::spawn(move || {
            let mut stdout_bytes = Vec::new();
            let _ = std::io::Read::read_to_end(&mut stdout, &mut stdout_bytes);
            String::from_utf8_lossy(&stdout_bytes).trim().to_owned()
        })
    };
    let stderr_thread = thread::spawn(move || {
        let mut stderr_bytes = Vec::new();
        let _ = std::io::Read::read_to_end(&mut stderr, &mut stderr_bytes);
        String::from_utf8_lossy(&stderr_bytes).trim().to_owned()
    });
    let (event_sender, event_receiver) = mpsc::channel();
    thread::spawn(move || {
        let status = child.wait();
        let stdout_text = stdout_thread.join().unwrap_or_default();
        let stderr_text = stderr_thread.join().unwrap_or_default();
        let result = match status {
            Ok(status) => context_child_result(&name, status.success(), stdout_text, stderr_text),
            Err(error) => Err(error.to_string()),
        };
        let _ = event_sender.send(result);
    });
    Ok((pid, event_receiver))
}

fn context_child_result(
    name: &str,
    success: bool,
    stdout: String,
    stderr: String,
) -> Result<Option<String>, String> {
    // The CLI marks a local pre-send cancellation on stderr so the adapter can
    // suppress the targeted tool result while keeping the public stdout payload
    // exactly as the typed contract describes.
    if name == "collab_context" && stderr.contains("COLLAB_CONTEXT_LOCAL_CANCELLATION") {
        return Ok(None);
    }
    if !success {
        if name == "collab_context"
            && !stdout.is_empty()
            && serde_json::from_str::<Value>(&stdout)
                .ok()
                .is_some_and(|value| {
                    value.get("ok") == Some(&Value::Bool(false)) && value.get("result").is_some()
                })
        {
            return Ok(Some(stdout));
        }
        return Err(if stderr.is_empty() { stdout } else { stderr });
    }
    Ok(Some(stdout))
}

fn build_argv(name: &str, args: &Value) -> Result<Vec<String>, String> {
    let catalog = tools();
    let spec = catalog
        .as_array()
        .unwrap()
        .iter()
        .find(|tool| tool["name"] == name)
        .ok_or_else(|| format!("unknown tool {name}"))?;
    let properties = spec["inputSchema"]["properties"].as_object().unwrap();
    let object = args
        .as_object()
        .ok_or_else(|| format!("{name} arguments must be an object"))?;
    for key in object.keys() {
        if !properties.contains_key(key) {
            return Err(format!("unknown argument {key} for {name}"));
        }
    }
    if let Some(argv) = board_tools::argv(name, args)? {
        return Ok(argv);
    }
    let mut argv = Vec::<String>::new();
    match name {
        "collab_msg" => argv.extend(["msg".into(), required(args, "id")?]),
        "collab_recv" => {
            argv.push("recv".into());
            optional_integer_flag(&mut argv, args, "timeout", "--timeout")?;
            argv.extend([
                "--receive-id".into(),
                required_receive_id(args, "receive_id")?,
            ]);
        }
        "collab_subagent" => {
            let action = required(args, "action")?;
            if ![
                "start", "dispatch", "list", "status", "snapshot", "rearm", "send", "ready",
                "working", "close", "bind",
            ]
            .contains(&action.as_str())
            {
                return Err("invalid subagent action".into());
            }
            argv.extend(["subagent".into(), action.clone()]);
            if action == "start" {
                optional_flag(&mut argv, args, "id", "--id")?;
                optional_flag(&mut argv, args, "runtime", "--runtime")?;
            } else if action == "dispatch" {
                argv.extend([
                    "--request-id".into(),
                    required(args, "request_id")?,
                    "--subject".into(),
                    required(args, "subject")?,
                    required(args, "body")?,
                ]);
                optional_flag(&mut argv, args, "feature_id", "--feature-id")?;
                optional_flag(&mut argv, args, "worktree_path", "--worktree-path")?;
                optional_flag(&mut argv, args, "branch", "--branch")?;
                optional_flag(&mut argv, args, "base_commit", "--base-commit")?;
                optional_flag(&mut argv, args, "priority", "--priority")?;
                optional_flag(&mut argv, args, "next_step", "--next-step")?;
            } else if action != "list" {
                argv.push(required(args, "id")?);
            }
            if action == "send" {
                argv.extend([
                    "--subject".into(),
                    required(args, "subject")?,
                    required(args, "body")?,
                ]);
            }
            if action == "snapshot" {
                optional_integer_flag(&mut argv, args, "lines", "--lines")?;
            }
            if action == "bind" {
                argv.extend(["--create-op".into(), required(args, "create_operation_id")?]);
            }
        }
        "collab_who" => argv.push("who".into()),
        "collab_sendmessage" => {
            argv.extend([
                "sendmessage".into(),
                "--to".into(),
                required(args, "to")?,
                "--subject".into(),
                required(args, "subject")?,
                required(args, "body")?,
            ]);
            if let Some(delivery) = args.get("delivery").and_then(Value::as_str) {
                argv.extend(["--delivery".into(), delivery.to_string()]);
            }
        }
        "collab_notify_methods" => argv.extend(["notify".into(), "methods".into()]),
        "collab_notify_subscribe" => {
            argv.extend([
                "notify".into(),
                "subscribe".into(),
                "--event".into(),
                required(args, "event")?,
            ]);
            optional_flag(&mut argv, args, "subject", "--subject")?;
            if let Some(values) = args.get("at_ms").and_then(Value::as_array) {
                for value in values {
                    argv.extend([
                        "--at-ms".into(),
                        value
                            .as_i64()
                            .ok_or("at_ms must contain integers")?
                            .to_string(),
                    ]);
                }
            }
            optional_integer_flag(&mut argv, args, "every_ms", "--every-ms")?;
            optional_integer_flag(&mut argv, args, "repeat_count", "--repeat-count")?;
            optional_integer_flag(&mut argv, args, "trigger_ms", "--trigger-ms")?;
            optional_integer_flag(&mut argv, args, "ttl_seconds", "--ttl-seconds")?;
        }
        "collab_notify_status" => argv.extend(["notify".into(), "status".into()]),
        "collab_notify_unsubscribe" => argv.extend([
            "notify".into(),
            "unsubscribe".into(),
            required(args, "subscription_id")?,
        ]),
        "collab_inbox" => argv.push("inbox".into()),
        "collab_context" => {
            argv.push("context".into());
            if let Some(operation_id) = args.get("operation_id").and_then(Value::as_str) {
                if operation_id.trim().is_empty() {
                    return Err("operation_id must not be empty".into());
                }
                argv.extend(["--op".into(), operation_id.to_string()]);
            }
            if args.get("query").and_then(Value::as_bool).unwrap_or(false) {
                argv.push("--query".into());
            }
            optional_flag(&mut argv, args, "project_scope", "--project")?;
            optional_flag(&mut argv, args, "app_scope_id", "--app-scope")?;
            if let Some(value) = context_approval_json(args, "approve_identity")? {
                argv.extend(["--approve-identity".into(), value]);
            }
            if let Some(value) = context_approval_json(args, "approve_grant")? {
                argv.extend(["--approve-grant".into(), value]);
            }
            if let Some(provide) = args.get("provide") {
                let value = match provide {
                    Value::String(value) => value.clone(),
                    Value::Object(object) => {
                        if object.is_empty() {
                            return Err("provide must contain at least one identity fact".into());
                        }
                        for (key, value) in object {
                            if !matches!(
                                key.as_str(),
                                "session_id" | "thread_id" | "endpoint" | "namespace"
                            ) {
                                return Err(format!("unknown provide field {key}"));
                            }
                            let value = value
                                .as_str()
                                .ok_or_else(|| format!("provide {key} must be a string"))?;
                            if value.trim().is_empty() {
                                return Err(format!("provide {key} must not be empty"));
                            }
                        }
                        serde_json::to_string(provide).map_err(|error| {
                            format!("provide must be JSON serializable: {error}")
                        })?
                    }
                    _ => return Err("provide must be a JSON object or string".into()),
                };
                if value.trim().is_empty() {
                    return Err("provide must not be empty".into());
                }
                argv.extend(["--provide".into(), value]);
            }
        }
        "collab_peer_lifecycle" => return peer_lifecycle_argv(args),
        "collab_ack" => {
            argv.push("ack".into());
            for id in args
                .get("ids")
                .and_then(Value::as_array)
                .ok_or("ids must be an array")?
            {
                argv.push(id.as_str().ok_or("ids must contain strings")?.into());
            }
        }
        "collab_task_status" => {
            argv.extend(["task".into(), "status".into()]);
            if let Some(id) = args.get("id").and_then(Value::as_str) {
                argv.push(id.into());
            }
        }
        "collab_task_accept" => {
            argv.extend(["task".into(), "accept".into(), required(args, "id")?]);
            optional_integer_flag(&mut argv, args, "expected_revision", "--expected-revision")?;
        }
        "collab_task_register" => {
            argv.extend(["task".into(), "register".into(), required(args, "id")?]);
            optional_flag(&mut argv, args, "feature", "--feature")?;
            optional_flag(&mut argv, args, "worktree", "--worktree")?;
            optional_flag(&mut argv, args, "branch", "--branch")?;
            optional_flag(&mut argv, args, "base_commit", "--base-commit")?;
            optional_flag(&mut argv, args, "priority", "--priority")?;
            optional_flag(&mut argv, args, "next", "--next")?;
        }
        "collab_task_wait" => {
            argv.extend(["task".into(), "wait".into(), required(args, "id")?]);
            argv.extend(["--for".into(), required(args, "blocking_task")?]);
        }
        "collab_task_deliver" => {
            argv.extend(["task".into(), "deliver".into(), required(args, "id")?]);
            argv.extend(["--evidence".into(), required(args, "evidence")?]);
            argv.extend(["--worktree".into(), required(args, "worktree")?]);
        }
        "collab_task_review" => {
            argv.extend(["task".into(), "review".into(), required(args, "id")?]);
            if args.get("accept").and_then(Value::as_bool).unwrap_or(false) {
                argv.push("--accept".into());
            }
            if args.get("rework").and_then(Value::as_bool).unwrap_or(false) {
                argv.push("--rework".into());
            }
            argv.extend(["--evidence".into(), required(args, "evidence")?]);
        }
        "collab_task_integrated" => {
            argv.extend(["task".into(), "integrated".into(), required(args, "id")?]);
            argv.extend(["--commit".into(), required(args, "commit")?]);
            argv.extend(["--evidence".into(), required(args, "evidence")?]);
        }
        "collab_task_relocate" => {
            argv.extend(["task".into(), "relocate".into(), required(args, "id")?]);
            argv.extend(["--worktree".into(), required(args, "worktree")?]);
            optional_flag(&mut argv, args, "branch", "--branch")?;
            optional_flag(&mut argv, args, "base_commit", "--base-commit")?;
        }
        "collab_task_block" => {
            argv.extend(["task".into(), "block".into(), required(args, "id")?]);
            optional_flag(&mut argv, args, "next", "--next")?;
        }
        "collab_task_update" => {
            argv.extend(["task".into(), "update".into(), required(args, "id")?]);
            optional_flag(&mut argv, args, "status", "--status")?;
            optional_flag(&mut argv, args, "next", "--next")?;
        }
        "collab_task_close" => argv.extend(["task".into(), "close".into(), required(args, "id")?]),
        "collab_migrate" => {
            argv.extend(["migrate".into(), required(args, "action")?]);
        }
        "collab_master" => {
            let action = required(args, "action")?;
            match action.as_str() {
                "status" => argv.extend(["master".into(), "status".into()]),
                "promote" => argv.extend([
                    "master".into(),
                    "promote".into(),
                    "--approval".into(),
                    required(args, "approval")?,
                ]),
                "clear" => argv.extend([
                    "master".into(),
                    "clear".into(),
                    "--approval".into(),
                    required(args, "approval")?,
                ]),
                "delegate" => argv.extend([
                    "master".into(),
                    "delegate".into(),
                    required(args, "target")?,
                ]),
                _ => return Err("invalid master action".into()),
            }
        }
        _ => return Err(format!("unknown tool {name}")),
    }
    Ok(argv)
}

fn required(args: &Value, key: &str) -> Result<String, String> {
    args.get(key)
        .and_then(Value::as_str)
        .map(str::to_owned)
        .ok_or_else(|| format!("missing required argument {key}"))
}

fn required_receive_id(args: &Value, key: &str) -> Result<String, String> {
    let value = args
        .get(key)
        .ok_or_else(|| format!("missing required argument {key}"))?
        .as_str()
        .ok_or_else(|| format!("{key} must be a string"))?;
    if value.trim().is_empty() {
        return Err(format!("{key} must not be empty"));
    }
    Ok(value.to_owned())
}

fn peer_lifecycle_required(args: &Value, key: &str) -> Result<String, String> {
    required(args, key).and_then(|value| {
        if value.trim().is_empty() {
            Err(format!("{key} must not be empty"))
        } else {
            Ok(value)
        }
    })
}

fn peer_lifecycle_argv(args: &Value) -> Result<Vec<String>, String> {
    let action = required(args, "action")?.to_ascii_lowercase();
    let allowed = match action.as_str() {
        "create" => &["action", "target_id", "cwd", "model", "operation_id"][..],
        "read" => &["action", "target_id"][..],
        "update" => &["action", "target_id", "cwd", "operation_id"][..],
        "close" => &["action", "target_id", "reason", "operation_id"][..],
        "query" => &["action", "operation_id"][..],
        _ => return Err(format!("unsupported peer lifecycle action {action}")),
    };
    for key in args
        .as_object()
        .ok_or("peer lifecycle arguments must be an object")?
        .keys()
    {
        if !allowed.contains(&key.as_str()) {
            return Err(format!("invalid {action} argument {key}"));
        }
    }
    let mut argv = vec!["worker".to_string(), action.clone()];
    match action.as_str() {
        "read" => {
            if args.get("target_id").is_some() {
                argv.push(peer_lifecycle_required(args, "target_id")?);
            }
        }
        "create" | "update" => {
            argv.extend([
                peer_lifecycle_required(args, "target_id")?,
                "--cwd".to_string(),
                peer_lifecycle_required(args, "cwd")?,
            ]);
            if args.get("operation_id").is_some() {
                argv.extend([
                    "--op".into(),
                    peer_lifecycle_required(args, "operation_id")?,
                ]);
            }
            if action == "create" && args.get("model").is_some() {
                argv.extend(["--model".into(), peer_lifecycle_required(args, "model")?]);
            }
        }
        "close" => {
            argv.extend([
                peer_lifecycle_required(args, "target_id")?,
                "--reason".to_string(),
                peer_lifecycle_required(args, "reason")?,
            ]);
            if args.get("operation_id").is_some() {
                argv.extend([
                    "--op".into(),
                    peer_lifecycle_required(args, "operation_id")?,
                ]);
            }
        }
        "query" => {
            argv.extend([
                "--op".to_string(),
                peer_lifecycle_required(args, "operation_id")?,
            ]);
        }
        _ => unreachable!("unsupported peer lifecycle action rejected above"),
    }
    Ok(argv)
}

fn context_approval_json(args: &Value, key: &str) -> Result<Option<String>, String> {
    let Some(value) = args.get(key) else {
        return Ok(None);
    };
    let encoded = match value {
        Value::Object(_) => serde_json::to_string(value)
            .map_err(|error| format!("{key} must be JSON serializable: {error}"))?,
        Value::String(raw) => {
            let parsed: Value = serde_json::from_str(raw)
                .map_err(|error| format!("{key} must contain a JSON object: {error}"))?;
            if !parsed.is_object() {
                return Err(format!("{key} must contain a JSON object"));
            }
            raw.clone()
        }
        _ => return Err(format!("{key} must be a JSON object or string")),
    };
    Ok(Some(encoded))
}

fn optional_flag(
    argv: &mut Vec<String>,
    args: &Value,
    key: &str,
    flag: &str,
) -> Result<(), String> {
    if let Some(value) = args.get(key) {
        argv.extend([
            flag.into(),
            value
                .as_str()
                .ok_or_else(|| format!("{key} must be a string"))?
                .into(),
        ]);
    }
    Ok(())
}

fn optional_integer_flag(
    argv: &mut Vec<String>,
    args: &Value,
    key: &str,
    flag: &str,
) -> Result<(), String> {
    if let Some(value) = args.get(key) {
        argv.extend([
            flag.into(),
            value
                .as_i64()
                .ok_or_else(|| format!("{key} must be an integer"))?
                .to_string(),
        ]);
    }
    Ok(())
}

fn response(id: &Value, result: Value) -> Value {
    json!({"jsonrpc":"2.0", "id":id, "result":result})
}

#[derive(Clone, Copy)]
enum Frame {
    Line,
    ContentLength,
}

fn read_message(stdin: &mut impl BufRead) -> io::Result<Option<(Frame, Value)>> {
    let mut first = String::new();
    if stdin.read_line(&mut first)? == 0 {
        return Ok(None);
    }
    let header = first.trim_end_matches(['\r', '\n']);
    if header.is_empty() {
        return read_message(stdin);
    }
    if header.to_ascii_lowercase().starts_with("content-length:") {
        let Some(length) = header
            .split_once(':')
            .and_then(|(_, value)| value.trim().parse::<usize>().ok())
        else {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "invalid Content-Length",
            ));
        };
        loop {
            let mut next = String::new();
            if stdin.read_line(&mut next)? == 0 {
                break;
            }
            if next == "\n" || next == "\r\n" {
                break;
            }
        }
        let mut body = vec![0; length];
        Read::read_exact(stdin, &mut body)?;
        let req = serde_json::from_slice(&body).map_err(io::Error::other)?;
        return Ok(Some((Frame::ContentLength, req)));
    }
    let req = serde_json::from_str(header).map_err(io::Error::other)?;
    Ok(Some((Frame::Line, req)))
}

fn write_message(out: &mut impl Write, frame: Frame, message: &Value) -> io::Result<()> {
    let body = serde_json::to_string(message)?;
    match frame {
        Frame::Line => writeln!(out, "{body}")?,
        Frame::ContentLength => write!(out, "Content-Length: {}\r\n\r\n{body}", body.len())?,
    }
    out.flush()
}

fn handle(req: &Value) -> Option<Value> {
    let method = req.get("method").and_then(Value::as_str).unwrap_or("");
    if method.starts_with("notifications/") {
        return None;
    }
    let id = req.get("id").cloned().unwrap_or(Value::Null);
    Some(match method {
        "initialize" => {
            let version = req
                .pointer("/params/protocolVersion")
                .and_then(Value::as_str)
                .unwrap_or("2024-11-05");
            response(
                &id,
                json!({"protocolVersion":version,"capabilities":{"tools":{"listChanged":false}},"serverInfo":{"name":"collab","version":env!("COLLAB_VERSION")}}),
            )
        }
        "ping" => response(&id, json!({})),
        "tools/list" => response(&id, json!({"tools":tools()})),
        "resources/list" => response(&id, json!({"resources":[]})),
        "prompts/list" => response(&id, json!({"prompts":[]})),
        "tools/call" => {
            let params = req.get("params").cloned().unwrap_or_default();
            let name = params.get("name").and_then(Value::as_str).unwrap_or("");
            let args = params
                .get("arguments")
                .cloned()
                .unwrap_or_else(|| json!({}));
            match call(name, &args) {
                Ok(text) => response(
                    &id,
                    json!({"content":[{"type":"text","text":text}],"isError":false}),
                ),
                Err(error) => response(
                    &id,
                    json!({"content":[{"type":"text","text":error}],"isError":true}),
                ),
            }
        }
        _ => {
            json!({"jsonrpc":"2.0","id":id,"error":{"code":-32601,"message":format!("method not found: {method}")}})
        }
    })
}

enum LoopEvent {
    Message(Frame, Value),
    ContextDone(Value, Result<Option<String>, String>),
    InputClosed,
}

struct ActiveContext {
    id: Value,
    frame: Frame,
    pid: u32,
}

fn main() {
    let (event_tx, event_rx) = mpsc::channel::<LoopEvent>();
    let input_tx = event_tx.clone();
    thread::spawn(move || {
        let stdin = io::stdin();
        let mut locked = stdin.lock();
        while let Ok(Some((frame, req))) = read_message(&mut locked) {
            if input_tx.send(LoopEvent::Message(frame, req)).is_err() {
                break;
            }
        }
        let _ = input_tx.send(LoopEvent::InputClosed);
    });

    let mut out = io::stdout();
    let mut active: Option<ActiveContext> = None;
    let mut input_closed = false;
    let mut pending: std::collections::VecDeque<(Frame, Value)> = std::collections::VecDeque::new();
    loop {
        let event = if active.is_none() {
            if let Some((frame, req)) = pending.pop_front() {
                LoopEvent::Message(frame, req)
            } else if input_closed {
                break;
            } else {
                match event_rx.recv() {
                    Ok(event) => event,
                    Err(_) => break,
                }
            }
        } else {
            match event_rx.recv() {
                Ok(event) => event,
                Err(_) => break,
            }
        };
        match event {
            LoopEvent::InputClosed => input_closed = true,
            LoopEvent::ContextDone(id, result) => {
                let Some(current) = active.as_ref() else {
                    continue;
                };
                if current.id != id {
                    continue;
                }
                let current = active.take().expect("active context is present");
                let reply = match result {
                    Ok(Some(text)) => response(
                        &current.id,
                        json!({"content":[{"type":"text","text":text}],"isError":false}),
                    ),
                    // A local pre-send cancellation has no daemon operation and
                    // suppresses the targeted tool result.
                    Ok(None) => continue,
                    Err(error) => response(
                        &current.id,
                        json!({"content":[{"type":"text","text":error}],"isError":true}),
                    ),
                };
                let _ = write_message(&mut out, current.frame, &reply);
            }
            LoopEvent::Message(frame, req) => {
                let method = req.get("method").and_then(Value::as_str).unwrap_or("");
                if method == "notifications/cancelled" {
                    let request_id = req.pointer("/params/requestId");
                    if let (Some(current), Some(request_id)) = (active.as_ref(), request_id) {
                        if &current.id == request_id {
                            unsafe {
                                libc::kill(current.pid as i32, libc::SIGINT);
                            }
                        }
                    }
                    continue;
                }
                if method == "tools/call" {
                    let params = req.get("params").cloned().unwrap_or_default();
                    let name = params.get("name").and_then(Value::as_str).unwrap_or("");
                    if name == "collab_context" && active.is_none() {
                        let id = req.get("id").cloned().unwrap_or(Value::Null);
                        let args = params
                            .get("arguments")
                            .cloned()
                            .unwrap_or_else(|| json!({}));
                        match spawn_context_call(name, &args) {
                            Ok((pid, receiver)) => {
                                let id_for_thread = id.clone();
                                let tx = event_tx.clone();
                                thread::spawn(move || {
                                    let result = receiver.recv().unwrap_or_else(|_| {
                                        Err("COLLAB_MCP_CONTEXT_CHILD: child channel closed".into())
                                    });
                                    let _ = tx.send(LoopEvent::ContextDone(id_for_thread, result));
                                });
                                active = Some(ActiveContext { id, frame, pid });
                                continue;
                            }
                            Err(error) => {
                                let reply = response(
                                    &id,
                                    json!({"content":[{"type":"text","text":error}],"isError":true}),
                                );
                                let _ = write_message(&mut out, frame, &reply);
                                continue;
                            }
                        }
                    }
                }
                if active.is_some() {
                    pending.push_back((frame, req));
                } else if let Some(reply) = handle(&req) {
                    let _ = write_message(&mut out, frame, &reply);
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn content_length_and_newline_frames_round_trip() {
        let body = json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-03-26"}});
        let encoded = serde_json::to_string(&body).unwrap();
        let framed = format!("Content-Length: {}\r\n\r\n{encoded}", encoded.len());
        let (frame, req) = read_message(&mut framed.as_bytes()).unwrap().unwrap();
        assert!(matches!(frame, Frame::ContentLength));
        assert_eq!(
            handle(&req).unwrap()["result"]["protocolVersion"],
            "2025-03-26"
        );
        let line = format!("{encoded}\n");
        let (frame, req) = read_message(&mut line.as_bytes()).unwrap().unwrap();
        assert!(matches!(frame, Frame::Line));
        assert_eq!(
            handle(&req).unwrap()["result"]["serverInfo"]["name"],
            "collab"
        );
        assert_eq!(
            handle(&json!({"method":"resources/list","id":2})).unwrap()["result"]["resources"],
            json!([])
        );
    }

    #[test]
    fn peer_lifecycle_mcp_schema_and_argv_are_closed_and_typed() {
        let definitions = tools();
        let lifecycle = definitions
            .as_array()
            .unwrap()
            .iter()
            .find(|tool| tool["name"] == "collab_peer_lifecycle")
            .expect("peer lifecycle tool");
        assert_eq!(lifecycle["inputSchema"]["additionalProperties"], false);
        assert_eq!(
            lifecycle["inputSchema"]["oneOf"].as_array().unwrap().len(),
            5
        );
        let properties = lifecycle["inputSchema"]["properties"].as_object().unwrap();
        assert!(!properties.contains_key("token"));
        assert!(!properties.contains_key("query_capability"));
        assert_eq!(
            peer_lifecycle_argv(&json!({"action":"create","target_id":"ordinary-peer","cwd":"/tmp/project","model":"test-model","operation_id":"op-create"})).unwrap(),
            ["worker","create","ordinary-peer","--cwd","/tmp/project","--op","op-create","--model","test-model"]
        );
        assert!(peer_lifecycle_argv(&json!({"action":"create","target_id":"ordinary-peer","cwd":"/tmp","query_capability":"secret"})).is_err());

        assert_eq!(
            peer_lifecycle_argv(&json!({"action":"read","target_id":"peer-1"})).unwrap(),
            ["worker", "read", "peer-1"]
        );
        assert_eq!(
            peer_lifecycle_argv(&json!({"action":"update","target_id":"peer-1","cwd":"/tmp/project","operation_id":"op-1"})).unwrap(),
            ["worker", "update", "peer-1", "--cwd", "/tmp/project", "--op", "op-1"]
        );
        assert_eq!(
            peer_lifecycle_argv(
                &json!({"action":"close","target_id":"peer-1","reason":"finished"})
            )
            .unwrap(),
            ["worker", "close", "peer-1", "--reason", "finished"]
        );
        assert_eq!(
            peer_lifecycle_argv(&json!({"action":"query","operation_id":"op-1"})).unwrap(),
            ["worker", "query", "--op", "op-1"]
        );
        assert!(peer_lifecycle_argv(&json!({"action":"read","cwd":"/tmp"})).is_err());
        assert!(peer_lifecycle_argv(&json!({"action":"read","target_id":17})).is_err());
        assert!(peer_lifecycle_argv(
            &json!({"action":"update","target_id":"peer-1","cwd":"/tmp","token":"secret"})
        )
        .is_err());
        assert!(peer_lifecycle_argv(&json!({"action":"query","operation_id":true})).is_err());
    }

    #[test]
    fn peer_lifecycle_mcp_preserves_typed_failures_and_rejects_empty_success() {
        let refused = r#"{"ok":false,"error":"PEER_LIFECYCLE_RESPONSIBILITY_CONFLICT","result":{"outcome":"refused"}}"#;
        assert_eq!(
            peer_lifecycle_cli_output(false, refused, "collab response on stderr"),
            Ok(refused.to_owned())
        );
        assert!(peer_lifecycle_cli_output(true, "", "")
            .unwrap_err()
            .contains("EMPTY_RESULT"));
        assert!(peer_lifecycle_cli_output(true, "not json", "")
            .unwrap_err()
            .contains("INVALID_RESULT"));
        assert!(peer_lifecycle_cli_output(true, r#"{"ok":true}"#, "")
            .unwrap_err()
            .contains("typed result"));
        assert!(
            peer_lifecycle_cli_output(false, r#"{"ok":true,"result":{}}"#, "daemon failed")
                .is_err()
        );
    }

    #[test]
    fn sendmessage_schema_requires_subject_and_body() {
        let definitions = tools();
        let send = definitions
            .as_array()
            .unwrap()
            .iter()
            .find(|tool| tool["name"] == "collab_sendmessage")
            .unwrap();
        assert_eq!(
            send["inputSchema"]["required"],
            json!(["to", "subject", "body"])
        );
        assert!(send["inputSchema"]["properties"]["subject"].is_object());
    }

    #[test]
    fn collab_init_is_not_exposed_by_mcp() {
        let definitions = tools();
        assert!(
            definitions
                .as_array()
                .unwrap()
                .iter()
                .all(|tool| tool["name"] != "collab_init"),
            "the removed CLI compatibility entry must not remain an MCP tool"
        );
        assert!(build_argv("collab_init", &json!({})).is_err());
    }

    #[test]
    fn context_schema_exposes_only_optional_fact_supplement() {
        let definitions = tools();
        let context = definitions
            .as_array()
            .unwrap()
            .iter()
            .find(|tool| tool["name"] == "collab_context")
            .expect("collab_context tool");
        assert_eq!(context["inputSchema"]["required"], json!([]));
        let provide = &context["inputSchema"]["properties"]["provide"];
        assert!(provide["oneOf"].is_array(), "{provide}");
        assert!(context["description"]
            .as_str()
            .unwrap()
            .contains("required_fields"));
        assert!(
            definitions
                .as_array()
                .unwrap()
                .iter()
                .all(|tool| tool["name"] != "collab_whoami"),
            "removed identity command must not remain in the MCP surface"
        );
    }

    #[test]
    fn context_schema_exposes_operation_id_and_query_without_leaking_capability() {
        let definitions = tools();
        let context = definitions
            .as_array()
            .unwrap()
            .iter()
            .find(|tool| tool["name"] == "collab_context")
            .expect("collab_context tool");
        let properties = &context["inputSchema"]["properties"];
        assert_eq!(properties["operation_id"]["type"], json!("string"));
        assert_eq!(properties["query"]["type"], json!("boolean"));
        assert!(
            properties.get("query_capability").is_none(),
            "the raw query capability must never be an MCP argument"
        );

        let query_argv = build_argv(
            "collab_context",
            &json!({"operation_id": "ctxop-query", "query": true}),
        )
        .expect("query must build argv");
        assert_eq!(
            query_argv,
            vec!["context", "--op", "ctxop-query", "--query"]
        );
        let supplement_argv = build_argv(
            "collab_context",
            &json!({
                "operation_id": "ctxop-supplement",
                "provide": {"session_id": "session-1"}
            }),
        )
        .expect("supplement must build argv");
        assert_eq!(
            supplement_argv,
            vec![
                "context",
                "--op",
                "ctxop-supplement",
                "--provide",
                r#"{"session_id":"session-1"}"#
            ]
        );
        assert!(build_argv("collab_context", &json!({"operation_id": ""})).is_err());
    }

    #[test]
    fn context_schema_exposes_distinct_scope_and_approval_arguments() {
        let definitions = tools();
        let context = definitions
            .as_array()
            .unwrap()
            .iter()
            .find(|tool| tool["name"] == "collab_context")
            .expect("collab_context tool");
        let properties = &context["inputSchema"]["properties"];
        for field in [
            "project_scope",
            "app_scope_id",
            "approve_identity",
            "approve_grant",
        ] {
            assert!(properties[field].is_object(), "{properties}");
        }

        let identity = json!({
            "decision": "approved",
            "target_identity": "worker-a",
            "expected_incumbent": {
                "binding_id": "binding-a",
                "endpoint_generation": 3
            }
        });
        let grant = json!({
            "decision": "approved",
            "target_identity": "worker-a",
            "expected_grant": {"grant_id": "grant-a", "generation": 4}
        });
        let argv = build_argv(
            "collab_context",
            &json!({
                "operation_id": "ctxop-approved",
                "project_scope": "/canonical/project",
                "app_scope_id": "appserver-cli",
                "approve_identity": identity.clone(),
                "approve_grant": grant.clone()
            }),
        )
        .expect("approved recovery must build argv");
        assert_eq!(argv[0], "context");
        assert_eq!(argv[1], "--op");
        assert_eq!(argv[2], "ctxop-approved");
        assert_eq!(argv[3], "--project");
        assert_eq!(argv[4], "/canonical/project");
        assert_eq!(argv[5], "--app-scope");
        assert_eq!(argv[6], "appserver-cli");
        assert_eq!(argv[7], "--approve-identity");
        assert_eq!(serde_json::from_str::<Value>(&argv[8]).unwrap(), identity);
        assert_eq!(argv[9], "--approve-grant");
        assert_eq!(serde_json::from_str::<Value>(&argv[10]).unwrap(), grant);

        let raw = r#"{"decision":"approved","target_identity":"worker-a"}"#;
        assert_eq!(
            build_argv("collab_context", &json!({"approve_identity": raw})).unwrap(),
            vec!["context", "--approve-identity", raw]
        );
        assert!(build_argv("collab_context", &json!({"approve_identity": 7})).is_err());
        assert!(build_argv("collab_context", &json!({"approve_grant": "not-json"})).is_err());
        assert!(build_argv(
            "collab_context",
            &json!({"query": true, "operation_id": "ctxop-q", "approve_identity": identity})
        )
        .is_ok_and(|argv| argv.contains(&"--approve-identity".to_owned())));
    }

    #[test]
    fn context_typed_incomplete_result_is_a_tool_result_but_transport_failure_is_not() {
        static COLLAB_BIN_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
        let _guard = COLLAB_BIN_LOCK.lock().unwrap();
        let dir = std::env::temp_dir().join(format!(
            "collab-mcp-context-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        use std::os::unix::fs::PermissionsExt;
        let run = |body: &str| {
            let script = dir.join("collab");
            std::fs::write(&script, format!("#!/bin/sh\n{body}\n")).unwrap();
            std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
            std::env::set_var("COLLAB_BIN", &script);
            let result = call("collab_context", &json!({"operation_id": "ctxop-typed"}));
            std::env::remove_var("COLLAB_BIN");
            result
        };

        // A typed incomplete result is a valid payload: the CLI prints the
        // envelope to stdout and exits 2, and the adapter must surface it as a
        // normal tool result rather than a transport error.
        let typed = run(
            r#"printf '%s' '{"ok":false,"result":{"operation_id":"ctxop-typed","outcome":"missing_facts"}}'; exit 2"#,
        )
        .expect("typed incomplete result must be returned as a tool result");
        let payload: Value = serde_json::from_str(&typed).unwrap();
        assert_eq!(payload["ok"], false);
        assert_eq!(payload["result"]["outcome"], "missing_facts");

        // A transport failure carries no typed envelope and stays an error.
        let failed = run("printf 'DAEMON_UNKNOWN: no socket' 1>&2; exit 1");
        assert!(failed.is_err(), "{failed:?}");

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn context_forwards_provide_object_and_string_unchanged() {
        let object = json!({
            "session_id": "session-1",
            "thread_id": "thread-1",
            "endpoint": "unix:///tmp/codex.sock",
            "namespace": "codex_tui"
        });
        let argv = build_argv("collab_context", &json!({"provide": object.clone()}))
            .expect("object supplement must build argv");
        assert_eq!(argv[0], "context");
        assert_eq!(argv[1], "--provide");
        assert_eq!(serde_json::from_str::<Value>(&argv[2]).unwrap(), object);

        let raw = r#"{"session_id":"session-1"}"#;
        assert_eq!(
            build_argv("collab_context", &json!({"provide": raw})).unwrap(),
            vec!["context", "--provide", raw]
        );
        assert!(build_argv("collab_context", &json!({"provide": 42})).is_err());
        assert!(build_argv("collab_context", &json!({"provide": ""})).is_err());
        assert!(build_argv(
            "collab_context",
            &json!({"provide": {"worker_id": "worker-1"}})
        )
        .is_err());
        assert!(build_argv("collab_context", &json!({"provide": {}})).is_err());
        assert!(build_argv("collab_context", &json!({"provide": {"session_id": null}})).is_err());
        assert!(build_argv("collab_context", &json!({"worker_id": "worker-1"})).is_err());
        assert!(build_argv(
            "collab_sendmessage",
            &json!({"to":"peer", "subject":"s", "body":"b", "from":"guessed-peer"})
        )
        .is_err());
        assert!(build_argv(
            "collab_recv",
            &json!({"receive_id":"r", "worker":"guessed-peer"})
        )
        .is_err());
    }

    #[test]
    fn subagent_dispatch_schema_exposes_stable_request_and_task_fields() {
        let definitions = tools();
        let subagent = definitions
            .as_array()
            .unwrap()
            .iter()
            .find(|tool| tool["name"] == "collab_subagent")
            .unwrap();
        assert!(subagent["inputSchema"]["properties"]["action"]["enum"]
            .as_array()
            .unwrap()
            .contains(&json!("dispatch")));
        let properties = subagent["inputSchema"]["properties"].as_object().unwrap();
        for field in ["request_id", "subject", "body", "feature_id", "priority"] {
            assert!(properties.contains_key(field), "missing MCP field {field}");
        }
    }

    #[test]
    fn task_accept_schema_requires_owner_task_id() {
        let definitions = tools();
        let accept = definitions
            .as_array()
            .unwrap()
            .iter()
            .find(|tool| tool["name"] == "collab_task_accept")
            .unwrap();
        assert_eq!(accept["inputSchema"]["required"], json!(["id"]));
    }

    #[test]
    fn recv_schema_requires_caller_supplied_receive_id() {
        let definitions = tools();
        let recv = definitions
            .as_array()
            .unwrap()
            .iter()
            .find(|tool| tool["name"] == "collab_recv")
            .expect("collab_recv tool");
        let properties = recv["inputSchema"]["properties"].as_object().unwrap();
        assert!(
            properties.contains_key("receive_id"),
            "MCP recv must expose the durable receive identity for replay"
        );
        assert_eq!(
            recv["inputSchema"]["required"],
            json!(["receive_id"]),
            "MCP recv must require a caller-owned receive identity"
        );
    }

    #[test]
    fn recv_rejects_absent_or_invalid_receive_id_before_spawn() {
        for args in [
            json!({}),
            json!({"timeout": 30}),
            json!({"receive_id": null}),
            json!({"receive_id": 42}),
            json!({"receive_id": ""}),
            json!({"receive_id": "   "}),
        ] {
            let error = build_argv("collab_recv", &args)
                .expect_err(&format!("collab_recv must reject {args} before spawn"));
            assert!(
                error.contains("receive_id"),
                "unexpected error for {args}: {error}"
            );
        }
    }

    #[test]
    fn recv_forwards_caller_receive_id_unchanged() {
        let argv = build_argv(
            "collab_recv",
            &json!({"timeout": 30, "receive_id": "recv-0922-abc"}),
        )
        .expect("valid receive_id must build argv");
        assert_eq!(
            argv,
            vec!["recv", "--timeout", "30", "--receive-id", "recv-0922-abc"]
        );
        let repeated = build_argv("collab_recv", &json!({"receive_id": "recv-0922-abc"}))
            .expect("repeat call must build argv");
        assert_eq!(repeated, vec!["recv", "--receive-id", "recv-0922-abc"]);
    }

    #[test]
    fn master_idle_event_is_public_in_mcp_schema() {
        let definitions = tools();
        let subscribe = definitions
            .as_array()
            .unwrap()
            .iter()
            .find(|tool| tool["name"] == "collab_notify_subscribe")
            .unwrap();
        let events = subscribe["inputSchema"]["properties"]["event"]["enum"]
            .as_array()
            .unwrap();
        assert!(events.contains(&json!("master-idle")));
        assert!(events.contains(&json!("direct-message")));
        assert!(events.contains(&json!("resource-released")));
        assert!(events.contains(&json!("deadline")));
        assert!(!events.contains(&json!("async-result")));
    }

    #[test]
    fn notification_schedule_fields_match_mcp_call_arguments() {
        let definitions = tools();
        let subscribe = definitions
            .as_array()
            .unwrap()
            .iter()
            .find(|tool| tool["name"] == "collab_notify_subscribe")
            .unwrap();
        let properties = subscribe["inputSchema"]["properties"].as_object().unwrap();
        for field in [
            "at_ms",
            "every_ms",
            "trigger_ms",
            "repeat_count",
            "ttl_seconds",
        ] {
            assert!(properties.contains_key(field), "missing MCP field {field}");
        }
    }

    #[test]
    fn master_tool_exposes_status_promote_clear_and_delegate() {
        let definitions = tools();
        let master = definitions
            .as_array()
            .unwrap()
            .iter()
            .find(|tool| tool["name"] == "collab_master")
            .unwrap();
        assert_eq!(master["inputSchema"]["required"], json!(["action"]));
        assert_eq!(
            master["inputSchema"]["properties"]["action"]["enum"],
            json!(["status", "promote", "clear", "delegate"])
        );
    }

    #[test]
    fn lifecycle_review_and_integration_tools_are_exposed() {
        let definitions = tools();
        for (name, required) in [
            ("collab_task_review", json!(["id", "evidence"])),
            (
                "collab_task_integrated",
                json!(["id", "commit", "evidence"]),
            ),
        ] {
            let tool = definitions
                .as_array()
                .unwrap()
                .iter()
                .find(|tool| tool["name"] == name)
                .unwrap_or_else(|| panic!("missing MCP tool {name}"));
            assert_eq!(tool["inputSchema"]["required"], required);
        }
    }
}
