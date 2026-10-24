// Durable bootstrap phases; the CLI owns process launch and the real master
// owns registration. No agent identity is fabricated by this owner.
impl ProjectRuntimeManager {
    #[allow(clippy::too_many_arguments)]
    fn ceo_bootstrap_prepare(
        &self,
        actor: &str,
        project: String,
        approval: String,
        profile: String,
        tmux_socket: String,
        tmux_session: String,
        operation_id: String,
    ) -> Result<Resp, Resp> {
        if approval.trim().is_empty() {
            return Err(Resp::err("MASTER_BOOTSTRAP_APPROVAL_REQUIRED"));
        }
        if [&profile, &tmux_socket, &tmux_session, &operation_id]
            .iter()
            .any(|s| s.trim().is_empty())
        {
            return Err(Resp::err("CEO_BOOTSTRAP_ARGUMENT_REQUIRED"));
        }
        let project = std::fs::canonicalize(project)
            .map_err(|e| Resp::err(format!("CEO_PROJECT_INVALID: {e}")))?;
        if !project.is_dir() {
            return Err(Resp::err("CEO_PROJECT_DIRECTORY_REQUIRED"));
        }
        let project = project.to_string_lossy().into_owned();
        {
            let state = self.host.state.lock().unwrap();
            if let Some(record) = state.ceo_bootstraps.get(&operation_id) {
                if record.project != project
                    || record.approval != approval
                    || record.profile != profile
                    || record.tmux_socket != tmux_socket
                    || record.tmux_session != tmux_session
                    || record.requested_by != actor
                {
                    return Err(Resp::err("CEO_BOOTSTRAP_OPERATION_CONFLICT"));
                }
                return Ok(Resp::data(
                    json!({"should_launch":false,"phase":record.phase,"operation":record}),
                ));
            }
        }
        match self.ceo_target_master(&project) {
            Ok(_) => return Err(Resp::err("CEO_BOOTSTRAP_MASTER_ALREADY_EXISTS")),
            Err(error)
                if error
                    .error
                    .as_deref()
                    .is_some_and(|e| e.starts_with("MASTER_BOOTSTRAP_APPROVAL_REQUIRED")) => {}
            Err(error) => return Err(error),
        }
        let sessions = Command::new("tmux")
            .args([
                "-S",
                &tmux_socket,
                "list-sessions",
                "-F",
                "#{session_id}\t#{session_name}",
            ])
            .output()
            .map_err(|e| Resp::err(format!("TMUX_ENDPOINT_UNAVAILABLE: {e}")))?;
        if !sessions.status.success()
            || !String::from_utf8_lossy(&sessions.stdout)
                .lines()
                .any(|line| line.split('\t').any(|part| part == tmux_session))
        {
            return Err(Resp::err("TMUX_ENDPOINT_OR_SESSION_UNAVAILABLE"));
        }
        let record = CeoBootstrapRecord {
            operation_id: operation_id.clone(),
            project: project.clone(),
            approval,
            profile,
            tmux_socket,
            tmux_session,
            requested_by: actor.into(),
            phase: "prepared".into(),
            pane_id: None,
            error: None,
            handshake_message_id: None,
            updated_ms: now_ms(),
        };
        let mut state = self.host.state.lock().unwrap();
        if state.ceo_bootstraps.contains_key(&operation_id) {
            return Err(Resp::err(
                "CEO_BOOTSTRAP_OPERATION_CHANGED: read the existing operation; do not relaunch",
            ));
        }
        if state
            .ceo_bootstraps
            .values()
            .any(|r| r.project == project && !matches!(r.phase.as_str(), "failed" | "ready"))
        {
            return Err(Resp::err(
                "CEO_BOOTSTRAP_ALREADY_PENDING: inspect the existing operation",
            ));
        }
        self.host
            .commit_locked_checked(
                &mut state,
                &[Event::CeoBootstrapSet {
                    record: record.clone(),
                }],
            )
            .map_err(|e| Resp::err(format!("CEO_BOOTSTRAP_DURABILITY_FAILED: {e}")))?;
        Ok(Resp::data(
            json!({"should_launch":true,"phase":"prepared","operation":record}),
        ))
    }

    fn ceo_bootstrap_record(
        &self,
        actor: &str,
        id: String,
        pane_id: Option<String>,
        outcome: String,
        error: Option<String>,
    ) -> Result<Resp, Resp> {
        if !["launched", "failed", "unknown"].contains(&outcome.as_str()) {
            return Err(Resp::err("CEO_BOOTSTRAP_OUTCOME_INVALID"));
        }
        let mut state = self.host.state.lock().unwrap();
        let mut record = state
            .ceo_bootstraps
            .get(&id)
            .cloned()
            .ok_or_else(|| Resp::err("CEO_BOOTSTRAP_OPERATION_NOT_FOUND"))?;
        if record.requested_by != actor {
            return Err(Resp::err("CEO_BOOTSTRAP_OWNER_REQUIRED"));
        }
        if record.phase == "ready" {
            return Ok(Resp::data(
                json!({"should_launch":false,"phase":"ready","operation":record}),
            ));
        }
        if record.phase != "prepared" {
            if record.phase == outcome && record.pane_id == pane_id && record.error == error {
                return Ok(Resp::data(
                    json!({"should_launch":false,"phase":record.phase,"operation":record}),
                ));
            }
            return Err(Resp::err("CEO_BOOTSTRAP_PHASE_CONFLICT"));
        }
        if outcome == "launched" && pane_id.as_ref().is_none_or(|id| id.trim().is_empty()) {
            return Err(Resp::err("CEO_BOOTSTRAP_PANE_RECEIPT_REQUIRED"));
        }
        record.phase = outcome;
        record.pane_id = pane_id;
        record.error = error;
        record.updated_ms = now_ms();
        self.host
            .commit_locked_checked(
                &mut state,
                &[Event::CeoBootstrapSet {
                    record: record.clone(),
                }],
            )
            .map_err(|e| Resp::err(format!("CEO_BOOTSTRAP_RECORD_FAILED: {e}")))?;
        Ok(Resp::data(
            json!({"should_launch":false,"phase":record.phase,"operation":record}),
        ))
    }

    fn ceo_bootstrap_ready(
        &self,
        source: &Arc<Server>,
        actor: &str,
        name: &str,
        id: String,
        subject: String,
        body: String,
    ) -> Result<Resp, Resp> {
        let mut record = self
            .host
            .state
            .lock()
            .unwrap()
            .ceo_bootstraps
            .get(&id)
            .cloned()
            .ok_or_else(|| Resp::err("CEO_BOOTSTRAP_OPERATION_NOT_FOUND"))?;
        if record.project != source.root.to_string_lossy()
            || current_master_holder(source, &source.state.lock().unwrap())
                .map_err(Resp::err)?
                .as_deref()
                != Some(actor)
        {
            return Err(Resp::err("CEO_BOOTSTRAP_TARGET_MASTER_REQUIRED"));
        }
        let expected_pane=record.pane_id.as_deref().ok_or_else(||Resp::err("CEO_BOOTSTRAP_LAUNCH_RECEIPT_REQUIRED: retry this same ready handshake after launch is recorded; do not launch another window"))?;
        let endpoint = {
            let state = source.state.lock().unwrap();
            state
                .workers
                .get(actor)
                .and_then(|worker| worker.transport.as_ref())
                .and_then(|transport| transport.tmux_endpoint.clone())
                .ok_or_else(|| Resp::err("CEO_BOOTSTRAP_TMUX_BINDING_REQUIRED"))?
        };
        let approved_socket = std::fs::canonicalize(&record.tmux_socket)
            .map_err(|e| Resp::err(format!("CEO_BOOTSTRAP_ENDPOINT_UNAVAILABLE: {e}")))?;
        let actual_socket = std::fs::canonicalize(&endpoint.socket_path)
            .map_err(|e| Resp::err(format!("CEO_BOOTSTRAP_ENDPOINT_UNAVAILABLE: {e}")))?;
        if endpoint.pane_id != expected_pane || actual_socket != approved_socket {
            return Err(Resp::err("CEO_BOOTSTRAP_LAUNCHED_PANE_REQUIRED"));
        }
        let probe = Command::new("tmux")
            .arg("-S")
            .arg(&approved_socket)
            .args([
                "display-message",
                "-p",
                "-t",
                expected_pane,
                "#{pid}\t#{pane_id}\t#{session_name}",
            ])
            .output()
            .map_err(|e| Resp::err(format!("CEO_BOOTSTRAP_ENDPOINT_UNAVAILABLE: {e}")))?;
        let line = String::from_utf8_lossy(&probe.stdout);
        let fields = line.trim().split('\t').collect::<Vec<_>>();
        if !probe.status.success()
            || fields.len() != 3
            || fields[0] != endpoint.server_pid.to_string()
            || fields[1] != expected_pane
            || (fields[2] != record.tmux_session && endpoint.tmux_session_id != record.tmux_session)
        {
            return Err(Resp::err("CEO_BOOTSTRAP_LAUNCHED_ENDPOINT_NOT_CURRENT"));
        }
        if record.phase == "ready" {
            return Ok(Resp::data(
                json!({"phase":"ready","operation":record,"msg_id":record.handshake_message_id}),
            ));
        }
        if !matches!(record.phase.as_str(), "prepared" | "launched" | "unknown") {
            return Err(Resp::err("CEO_BOOTSTRAP_NOT_LAUNCHED"));
        }
        let grant = self.ceo_grant()?;
        let (target, to) = self.ceo_holder(&grant)?;
        let response = self.ceo_deliver(
            source,
            actor,
            name,
            &target,
            &to,
            subject,
            body,
            None,
            None,
            None,
            Some(id),
        )?;
        let mid = response.data["msg_id"]
            .as_str()
            .ok_or_else(|| Resp::err("CEO_BOOTSTRAP_HANDSHAKE_RECEIPT_MISSING"))?
            .to_owned();
        record.phase = "ready".into();
        record.handshake_message_id = Some(mid.clone());
        record.updated_ms = now_ms();
        self.host
            .commit_checked(&[Event::CeoBootstrapSet {
                record: record.clone(),
            }])
            .map_err(|e| {
                let mut response =
                    Resp::data(json!({"msg_id":mid,"durable":true,"phase":"launched"}));
                response.ok = false;
                response.error = Some(format!(
                    "CEO_BOOTSTRAP_READY_RECORD_FAILED: handshake is durable; do not resend: {e}"
                ));
                response
            })?;
        Ok(Resp::data(
            json!({"phase":"ready","operation":record,"msg_id":mid}),
        ))
    }
}
