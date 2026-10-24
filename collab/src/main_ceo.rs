//! Public CLI execution for the host-wide CEO control plane.
//!
//! The daemon owns authentication, target-master resolution and every durable
//! effect. This module only validates CLI input, carries the source project
//! context and performs the one external launch owned by the bootstrap command.

use crate::ceo::CeoCommand;
use crate::identity::Identity;
use crate::main_cli::CeoCmd;
use crate::proto::{ReplyPolicy, Req};
use crate::scope::Scope;
use serde_json::Value;
use std::os::unix::fs::FileTypeExt;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

pub(crate) fn run(command: CeoCmd) -> anyhow::Result<()> {
    match command {
        CeoCmd::Status => status(),
        CeoCmd::Promote {
            approval,
            workspace,
        } => {
            let approval = required_text("approval", approval)?;
            let workspace = workspace
                .map(|path| canonical_existing("workspace", &path))
                .transpose()?
                .map(|path| path_text(&path))
                .transpose()?;
            emit_ceo(
                authenticated(),
                CeoCommand::Promote {
                    approval,
                    workspace,
                },
            )
        }
        CeoCmd::Clear { approval } => {
            let approval = required_text("approval", approval)?;
            emit_ceo(authenticated(), CeoCommand::Clear { approval })
        }
        CeoCmd::Send {
            project,
            to,
            subject,
            in_reply_to,
            reply_required,
            reply_on_completion,
            body,
        } => {
            let project = canonical_existing("project", &project)?;
            let subject = required_text("subject", subject)?;
            let body = required_body(body)?;
            emit_ceo(
                authenticated(),
                CeoCommand::Send {
                    project: path_text(&project)?,
                    to,
                    subject,
                    body,
                    in_reply_to,
                    reply_policy: reply_policy(reply_required, reply_on_completion),
                },
            )
        }
        CeoCmd::AppendTask {
            project,
            id,
            title,
            description,
            delivery_condition,
            test_condition,
            priority,
            reply_required,
            reply_on_completion,
        } => {
            let project = canonical_existing("project", &project)?;
            emit_ceo(
                authenticated(),
                CeoCommand::AppendTask {
                    project: path_text(&project)?,
                    id,
                    title,
                    description,
                    delivery_condition,
                    test_condition,
                    priority,
                    reply_policy: Some(
                        reply_policy(reply_required, reply_on_completion)
                            .unwrap_or(ReplyPolicy::OnCompletion),
                    ),
                },
            )
        }
        CeoCmd::Reply {
            subject,
            in_reply_to,
            body,
        } => {
            let subject = required_text("subject", subject)?;
            let body = required_body(body)?;
            emit_ceo(
                authenticated(),
                CeoCommand::Reply {
                    subject,
                    body,
                    in_reply_to,
                },
            )
        }
        CeoCmd::Progress { project } => {
            let project = project
                .map(|path| canonical_existing("project", &path))
                .transpose()?
                .map(|path| path_text(&path))
                .transpose()?;
            emit_ceo(authenticated(), CeoCommand::Progress { project })
        }
        CeoCmd::BootstrapMaster {
            project,
            approval,
            profile,
            tmux_socket,
            tmux_session,
            operation_id,
        } => bootstrap_master(
            project,
            approval,
            profile,
            tmux_socket,
            tmux_session,
            operation_id,
        ),
        CeoCmd::BootstrapReady {
            operation_id,
            subject,
            body,
        } => {
            let operation_id = required_text("operation id", operation_id)?;
            let subject = required_text("subject", subject)?;
            let body = required_body(body)?;
            emit_ceo(
                authenticated(),
                CeoCommand::BootstrapReady {
                    operation_id,
                    subject,
                    body,
                },
            )
        }
    }
}

fn status() -> anyhow::Result<()> {
    let paths = crate::scope::HostPaths::resolve()?;
    let scope = Scope::resolve().ok();
    let (worker_id, token) = match scope
        .as_ref()
        .and_then(|scope| crate::registered_project_identity(scope).ok())
    {
        Some(identity) => (identity.worker_id, identity.token),
        None => (String::new(), String::new()),
    };
    let request = Req::Ceo {
        worker_id,
        token,
        command: CeoCommand::Status,
    };
    let value: Value = crate::client::call_with_context(
        &paths.socket_path(),
        &request,
        scope
            .as_ref()
            .map(|scope| crate::main_context::cli_project_context(&scope.root))
            .transpose()?,
    )?;
    crate::out(&value);
    Ok(())
}

fn authenticated() -> (Scope, Identity) {
    match Scope::resolve().and_then(|scope| {
        let identity = crate::registered_project_identity(&scope)?;
        Ok((scope, identity))
    }) {
        Ok(value) => value,
        Err(error) => {
            eprintln!("collab: {}", crate::format_cli_error(&error.to_string()));
            std::process::exit(1);
        }
    }
}

fn emit_ceo((scope, identity): (Scope, Identity), command: CeoCommand) -> anyhow::Result<()> {
    let request = Req::Ceo {
        worker_id: identity.worker_id.clone(),
        token: identity.token.clone(),
        command,
    };
    let value: Value = crate::call_project(&scope, &identity, &request)?;
    emit_value(value)
}

fn emit_value(value: Value) -> anyhow::Result<()> {
    let failed = value.get("ok") == Some(&Value::Bool(false));
    crate::out(&value);
    if failed {
        std::process::exit(2);
    }
    Ok(())
}

fn reply_policy(required: bool, on_completion: bool) -> Option<ReplyPolicy> {
    if required {
        Some(ReplyPolicy::Required)
    } else if on_completion {
        Some(ReplyPolicy::OnCompletion)
    } else {
        None
    }
}

fn required_text(label: &str, value: String) -> anyhow::Result<String> {
    if value.trim().is_empty() {
        anyhow::bail!(
            "CEO_{}_REQUIRED: {label} must not be empty",
            label.to_uppercase().replace(' ', "_")
        );
    }
    Ok(value)
}

fn required_body(body: Vec<String>) -> anyhow::Result<String> {
    let body = body.join(" ");
    if body.trim().is_empty() {
        anyhow::bail!("CEO_BODY_REQUIRED: message body must not be empty");
    }
    Ok(body)
}

fn canonical_existing(label: &str, path: &Path) -> anyhow::Result<PathBuf> {
    std::fs::canonicalize(path).map_err(|error| {
        anyhow::anyhow!(
            "CEO_{}_INVALID: {}: {error}",
            label.to_uppercase().replace(' ', "_"),
            path.display()
        )
    })
}

fn path_text(path: &Path) -> anyhow::Result<String> {
    path.to_str()
        .map(str::to_owned)
        .ok_or_else(|| anyhow::anyhow!("CEO_PATH_NOT_UTF8: {}", path.display()))
}

fn bootstrap_master(
    project: PathBuf,
    approval: String,
    profile: String,
    tmux_socket: PathBuf,
    tmux_session: String,
    operation_id: Option<String>,
) -> anyhow::Result<()> {
    let approval = required_text("approval", approval)?;
    let profile = required_text("profile", profile)?;
    let tmux_session = required_text("tmux session", tmux_session)?;
    if tmux_session.chars().any(char::is_control) || tmux_session.contains(':') {
        anyhow::bail!(
            "CEO_BOOTSTRAP_SESSION_INVALID: tmux session must be one exact existing name"
        );
    }
    let project = canonical_existing("project", &project)?;
    let tmux_socket = canonical_existing("tmux socket", &tmux_socket)?;
    verify_tmux_endpoint(&tmux_socket, &tmux_session)?;
    let codex = codex_for_tmux_session(&tmux_socket, &tmux_session)?;

    let operation_id = match operation_id {
        Some(value) => required_text("operation id", value)?,
        None => new_operation_id(),
    };
    let (scope, identity) = authenticated();
    let prepare = call_ceo(
        &scope,
        &identity,
        CeoCommand::BootstrapPrepare {
            project: path_text(&project)?,
            approval: approval.clone(),
            profile: profile.clone(),
            tmux_socket: path_text(&tmux_socket)?,
            tmux_session: tmux_session.clone(),
            operation_id: operation_id.clone(),
        },
    )?;
    let should_launch = prepare
        .get("should_launch")
        .and_then(Value::as_bool)
        .ok_or_else(|| {
            anyhow::anyhow!(
                "CEO_BOOTSTRAP_PREPARE_INVALID: daemon response has no boolean should_launch"
            )
        })?;
    if !should_launch {
        return emit_value(prepare);
    }

    let shell_command =
        bootstrap_shell_command(&project, &approval, &profile, &operation_id, &codex)?;
    let output = Command::new("tmux")
        .arg("-S")
        .arg(&tmux_socket)
        .args(["new-window", "-d", "-P", "-F", "#{pane_id}", "-t"])
        .arg(format!("{tmux_session}:"))
        .arg("-c")
        .arg(&project)
        .arg(&shell_command)
        .output();
    match output {
        Ok(output) if output.status.success() => {
            let pane_id = String::from_utf8_lossy(&output.stdout).trim().to_owned();
            let record = call_ceo(
                &scope,
                &identity,
                CeoCommand::BootstrapRecord {
                    operation_id,
                    pane_id: if pane_id.is_empty() {
                        None
                    } else {
                        Some(pane_id)
                    },
                    outcome: "launched".to_owned(),
                    error: None,
                },
            )?;
            emit_value(with_phase(record, "launched"))
        }
        Ok(output) => {
            let error = format!(
                "tmux new-window failed with {}: {}",
                output.status,
                String::from_utf8_lossy(&output.stderr).trim()
            );
            let record = call_ceo(
                &scope,
                &identity,
                CeoCommand::BootstrapRecord {
                    operation_id,
                    pane_id: None,
                    outcome: "failed".to_owned(),
                    error: Some(error.clone()),
                },
            );
            if let Ok(record) = record {
                crate::out(&with_phase(record, "failed"));
            }
            anyhow::bail!("CEO_BOOTSTRAP_LAUNCH_FAILED: {error}")
        }
        Err(error) => {
            let error = format!("failed to run tmux: {error}");
            let record = call_ceo(
                &scope,
                &identity,
                CeoCommand::BootstrapRecord {
                    operation_id,
                    pane_id: None,
                    outcome: "failed".to_owned(),
                    error: Some(error.clone()),
                },
            );
            if let Ok(record) = record {
                crate::out(&with_phase(record, "failed"));
            }
            anyhow::bail!("CEO_BOOTSTRAP_LAUNCH_FAILED: {error}")
        }
    }
}

fn call_ceo(scope: &Scope, identity: &Identity, command: CeoCommand) -> anyhow::Result<Value> {
    let request = Req::Ceo {
        worker_id: identity.worker_id.clone(),
        token: identity.token.clone(),
        command,
    };
    crate::call_project(scope, identity, &request)
}

fn with_phase(mut value: Value, phase: &str) -> Value {
    if value.get("ok") != Some(&Value::Bool(false)) {
        if let Some(object) = value.as_object_mut() {
            object
                .entry("phase")
                .or_insert_with(|| Value::String(phase.to_owned()));
        }
    }
    value
}

fn verify_tmux_endpoint(socket: &Path, session: &str) -> anyhow::Result<()> {
    let metadata = std::fs::metadata(socket).map_err(|error| {
        anyhow::anyhow!(
            "CEO_BOOTSTRAP_SOCKET_MISSING: {}: {error}",
            socket.display()
        )
    })?;
    if !metadata.file_type().is_socket() {
        anyhow::bail!(
            "CEO_BOOTSTRAP_SOCKET_INVALID: {} is not a Unix socket",
            socket.display()
        );
    }
    let output = Command::new("tmux")
        .arg("-S")
        .arg(socket)
        .args(["list-sessions", "-F", "#{session_name}"])
        .output()
        .map_err(|error| anyhow::anyhow!("CEO_BOOTSTRAP_TMUX_UNAVAILABLE: {error}"))?;
    if !output.status.success() {
        anyhow::bail!(
            "CEO_BOOTSTRAP_SESSION_MISSING: tmux cannot list sessions on {}: {}",
            socket.display(),
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }
    let names = String::from_utf8_lossy(&output.stdout);
    if names.lines().any(|name| name == session) {
        Ok(())
    } else {
        anyhow::bail!(
            "CEO_BOOTSTRAP_SESSION_MISSING: session {session:?} does not exist on {}",
            socket.display()
        );
    }
}

fn codex_for_tmux_session(socket: &Path, session: &str) -> anyhow::Result<PathBuf> {
    let output = Command::new("tmux")
        .arg("-S")
        .arg(socket)
        .args(["show-environment", "-t", session, "PATH"])
        .output()?;
    if !output.status.success() {
        anyhow::bail!(
            "CEO_BOOTSTRAP_CODEX_PATH_UNAVAILABLE: existing session has no readable PATH"
        );
    }
    let text = String::from_utf8(output.stdout)?;
    let path = text
        .trim_end()
        .strip_prefix("PATH=")
        .ok_or_else(|| anyhow::anyhow!("CEO_BOOTSTRAP_CODEX_PATH_UNAVAILABLE"))?;
    use std::os::unix::fs::PermissionsExt;
    for directory in std::env::split_paths(path) {
        let candidate = directory.join("codex");
        if std::fs::metadata(&candidate)
            .is_ok_and(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
        {
            return Ok(std::fs::canonicalize(candidate)?);
        }
    }
    anyhow::bail!("CEO_BOOTSTRAP_CODEX_NOT_FOUND: no executable codex in the existing session PATH")
}

fn bootstrap_shell_command(
    project: &Path,
    approval: &str,
    profile: &str,
    operation_id: &str,
    codex: &Path,
) -> anyhow::Result<String> {
    let collab = std::env::current_exe()
        .and_then(|path| std::fs::canonicalize(path))
        .map_err(|error| anyhow::anyhow!("CEO_BOOTSTRAP_BINARY_INVALID: {error}"))?;
    let collab = shell_quote(&path_text(&collab)?);
    let project_text = path_text(project)?;
    let approval_quoted = shell_quote(approval);
    let operation_quoted = shell_quote(operation_id);
    let prompt = format!(
        "Register this real Codex agent for the project before claiming any role.\n\
From {project_text}, run exactly:\n\
{collab} context\n\
Do not invent, inject, or reuse a parent session id; use only host-provided facts.\n\
The user explicitly approved this exact text for master promotion:\n\
{approval_quoted}\n\
Run exactly:\n\
{collab} master promote --approval {approval_quoted}\n\
Only after registration and promotion are real, send the CEO handshake with a factual subject and body:\n\
{collab} ceo bootstrap-ready --op {operation_quoted} --subject <factual-subject> <factual-body>\n\
If ready reports CEO_BOOTSTRAP_LAUNCH_RECEIPT_REQUIRED, retry only this same ready handshake after launch is recorded; never create another window.\n\
Report the real context, promotion and handshake receipts. Do not claim ready before the handshake succeeds."
    );
    let argv = [
        "env".to_owned(),
        "-u".to_owned(),
        "CODEX_SESSION_ID".to_owned(),
        "-u".to_owned(),
        "CODEX_THREAD_ID".to_owned(),
        "-u".to_owned(),
        "CODEX_INTERNAL_ORIGINATOR_OVERRIDE".to_owned(),
        "-u".to_owned(),
        "CODEX_APP_TOOLS_PIPE_PATH".to_owned(),
        path_text(codex)?,
        "--dangerously-bypass-approvals-and-sandbox".to_owned(),
        "--profile".to_owned(),
        profile.to_owned(),
        "--cd".to_owned(),
        project_text,
        prompt,
    ];
    Ok(argv
        .iter()
        .map(|value| shell_quote(value))
        .collect::<Vec<_>>()
        .join(" "))
}

fn shell_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\\''"))
}

fn new_operation_id() -> String {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_nanos())
        .unwrap_or_default();
    format!("ceo-bootstrap-{}-{nanos}", std::process::id())
}
