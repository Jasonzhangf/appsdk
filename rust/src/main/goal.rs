use super::*;

pub(super) fn handle_longhorizon_command<I>(root: &Path, mut args: I)
where
    I: Iterator<Item = String>,
{
    let sub = args.next().unwrap_or_else(|| "show".to_string());
    match sub.as_str() {
        "show" | "brief" => {}
        "--json" => {
            longhorizon_show(root, true);
            return;
        }
        other => fail(format!(
            "UNKNOWN_LONGHORIZON_SUBCOMMAND:{} (USAGE: appsdk longhorizon show [--json])",
            other
        )),
    }

    let mut format_json = false;
    for arg in args {
        match arg.as_str() {
            "--json" => format_json = true,
            other => fail(format!("UNKNOWN_LONGHORIZON_SHOW_OPTION:{}", other)),
        }
    }
    longhorizon_show(root, format_json);
}

pub(super) fn longhorizon_show(root: &Path, format_json: bool) {
    let context = goal_context(root);
    let role = context
        .as_ref()
        .map(execution_role)
        .unwrap_or(ExecutionRole::Unknown);
    let caller = context
        .as_ref()
        .ok()
        .and_then(|context| context["identity"]["worker_id"].as_str())
        .map(str::to_owned);
    let (record, record_error) = match context
        .and_then(|context| goal_scope_from_context(&context, root))
        .and_then(|scope| goal_record_read(&scope))
    {
        Ok(record) => (record, None),
        Err(error) => (None, Some(error)),
    };
    let (status, collab_status_error) = match collab_status_all(root) {
        Ok(status) => (Some(status), None),
        Err(error) => (None, Some(error)),
    };
    let (bugs, open_bugs_error) = match open_bugs_json(root) {
        Ok(bugs) => (bugs, None),
        Err(err) => (Vec::new(), Some(err)),
    };
    let role_label = role.label();
    let charter = role.charter();
    let fleet_rules = role.fleet_rules();

    let goal_path = record
        .as_ref()
        .and_then(|r| r["goal_path"].as_str())
        .map(PathBuf::from);
    let objective = goal_path
        .as_ref()
        .map(|p| goal_objective_excerpt(p, 24, 1200))
        .unwrap_or_else(|| {
            "(未注册长程目标，先运行 appsdk goal subscribe --goal <path.md>)".to_string()
        });

    let empty = Vec::new();
    let tasks = status
        .as_ref()
        .and_then(|s| s["tasks"].as_array())
        .unwrap_or(&empty);
    let pending_merges = status
        .as_ref()
        .and_then(|s| s["pending_merges"].as_array())
        .unwrap_or(&empty);
    let workers = status
        .as_ref()
        .and_then(|s| s["workers"].as_array())
        .unwrap_or(&empty);

    let is_blocked = |task: &Value| {
        matches!(
            task["status"].as_str().unwrap_or(""),
            "blocked" | "waiting" | "resource-waiting"
        )
    };
    let in_goal_scope = |task: &Value| {
        role == ExecutionRole::Master
            || (role == ExecutionRole::Worker
                && task["owner"]
                    .as_str()
                    .is_some_and(|owner| Some(owner) == caller.as_deref()))
    };
    let blocked_tasks: Vec<&Value> = tasks
        .iter()
        .filter(|t| in_goal_scope(t) && is_blocked(t))
        .collect();
    let active_tasks: Vec<&Value> = tasks
        .iter()
        .filter(|t| in_goal_scope(t) && !is_blocked(t))
        .collect();

    // Spare capacity means a live App Server route holding no task. A worker
    // whose route is
    // lost still owns its task, so it is an intervention item, not capacity.
    let is_idle = |worker: &Value| {
        worker["active_task"].is_null()
            && worker["endpoint_live"].as_bool().unwrap_or(false)
            && worker["identity_valid"].as_bool().unwrap_or(false)
            && !worker["suspected_offline"].as_bool().unwrap_or(false)
    };
    let needs_intervention = |worker: &Value| {
        !worker["identity_valid"].as_bool().unwrap_or(false)
            || !worker["endpoint_live"].as_bool().unwrap_or(false)
            || worker["suspected_offline"].as_bool().unwrap_or(false)
    };
    let idle_workers: Vec<&Value> = workers.iter().filter(|w| is_idle(w)).collect();
    let broken_workers: Vec<&Value> = workers.iter().filter(|w| needs_intervention(w)).collect();

    if format_json {
        let payload = serde_json::json!({
            "role": role.role(),
            "role_label": role_label,
            "charter": charter,
            "fleet_rules": fleet_rules,
            "notification_rules": POLICY.notification_rules(),
            "goal": {
                "registered": record.is_some(),
                "record_error": record_error.as_deref(),
                "owner": record.as_ref().and_then(|r| r["owner"].as_str()),
                "record_source": record.as_ref().and_then(|r| r["record_source"].as_str()),
                "path": goal_path.as_ref().map(|p| p.to_string_lossy().to_string()),
                "interval": record.as_ref().and_then(|r| r["interval"].as_str()),
                "active": record.as_ref().and_then(|r| r["active"].as_bool()),
                "registered_at": record.as_ref().and_then(|r| r["registered_at"].as_str()),
                "objective": objective,
            },
            "assigned": active_tasks,
            "pending_merges": pending_merges,
            "blocked": blocked_tasks,
            "idle_workers": idle_workers,
            "workers_needing_intervention": broken_workers,
            "open_bugs": bugs,
            "open_bugs_error": open_bugs_error,
            "collab_reachable": status.is_some(),
            "collab_status_error": collab_status_error.as_deref(),
        });
        println!(
            "{}",
            serde_json::to_string_pretty(&payload).unwrap_or_default()
        );
        return;
    }

    println!("{}", "=".repeat(80));
    println!("LONG-HORIZON {} BRIEFING", role_label);
    println!("{}", "=".repeat(80));

    println!("\n## 0. 你的角色\n\n{}", charter);
    if !fleet_rules.is_empty() {
        println!("\n{}", fleet_rules);
    }
    println!("\n{}", POLICY.notification_rules());

    println!("\n## 1. 长程目标\n");
    if let Some(error) = record_error.as_deref() {
        println!(
            "- 目标状态未知，记录读取失败: {}。请恢复记录后重试。",
            error
        );
    } else {
        match (&record, &goal_path) {
        (Some(rec), Some(path)) => {
            println!("- 目标文档: {}", path.display());
            println!(
                "- 唤醒方式: 一次性 deadline，延迟 {} | 活跃: {} | 注册于: {}",
                rec["interval"].as_str().unwrap_or("unknown"),
                rec["active"].as_bool().unwrap_or(false),
                rec["registered_at"].as_str().unwrap_or("unknown")
            );
        }
        _ => println!("- 未注册长程目标。先运行 `appsdk goal subscribe --goal <path.md> --interval <period>`。"),
        }
    }
    println!("\n目标摘要:\n{}", objective);

    println!("\n## 2. 工作分配\n");
    if let Some(error) = collab_status_error {
        println!(
            "- collab 状态未知，无法读取任务与 worker 状态: {}。先恢复 Collab 再重试。",
            error
        );
    }
    println!("已分配任务 ({}):", active_tasks.len());
    if active_tasks.is_empty() {
        println!("- 无");
    }
    for task in &active_tasks {
        println!(
            "- {} [{}] owner={} next={}",
            task["id"].as_str().unwrap_or("?"),
            task["status"].as_str().unwrap_or("?"),
            task["owner"].as_str().unwrap_or("?"),
            task["next_step"].as_str().unwrap_or("(未记录)")
        );
    }

    println!("\n空闲产能 ({}):", idle_workers.len());
    if idle_workers.is_empty() {
        println!("- 无空闲 worker");
    }
    for worker in &idle_workers {
        println!(
            "- {} agent_state={}",
            worker["id"].as_str().unwrap_or("?"),
            worker["agent_state"].as_str().unwrap_or("?")
        );
    }

    println!("\n待合并 ({}):", pending_merges.len());
    if pending_merges.is_empty() {
        println!("- 无");
    }
    for merge in pending_merges {
        println!(
            "- {} [{}] owner={} requested_by={} branch={} worktree={}",
            merge["task_id"].as_str().unwrap_or("?"),
            merge["status"].as_str().unwrap_or("?"),
            merge["owner"].as_str().unwrap_or("?"),
            merge["requested_by"].as_str().unwrap_or("?"),
            merge["branch"].as_str().unwrap_or("?"),
            merge["worktree"].as_str().unwrap_or("?"),
        );
    }

    println!("\n## 3. 阻塞与缺陷\n");
    println!("Blocked / 等待中的任务 ({}):", blocked_tasks.len());
    if blocked_tasks.is_empty() {
        println!("- 无");
    }
    for task in &blocked_tasks {
        println!(
            "- {} [{}] owner={} next={}",
            task["id"].as_str().unwrap_or("?"),
            task["status"].as_str().unwrap_or("?"),
            task["owner"].as_str().unwrap_or("?"),
            task["next_step"].as_str().unwrap_or("(未记录)")
        );
    }

    println!("\n需要介入的 worker ({}):", broken_workers.len());
    if broken_workers.is_empty() {
        println!("- 无");
    }
    for worker in &broken_workers {
        println!(
            "- {} status={} diagnostic={}",
            worker["id"].as_str().unwrap_or("?"),
            worker["status"].as_str().unwrap_or("?"),
            worker["diagnostic"].as_str().unwrap_or("(无)")
        );
    }

    if let Some(err) = open_bugs_error {
        println!("\n开放缺陷读取失败: {}", err);
    } else {
        println!("\n开放缺陷 ({}，P0 优先):", bugs.len());
        if bugs.is_empty() {
            println!("- 无");
        }
        for bug in bugs.iter().take(10) {
            let labels: Vec<&str> = bug["labels"]
                .as_array()
                .map(|arr| arr.iter().filter_map(Value::as_str).collect())
                .unwrap_or_default();
            println!(
                "- {} [{}] {}",
                bug["human_id"].as_str().unwrap_or("?"),
                labels.join(","),
                bug["title"].as_str().unwrap_or("?")
            );
        }
    }

    println!("\n## 4. 本轮下一步\n");
    match role {
        ExecutionRole::Master => {
            println!("从以下四项中选一项并立即执行，不要以 ACK 或\"已读\"结束本轮：");
            println!("1. 派发 ready 长程工作给获授权的合适 peer；一次性执行或观察直接派独立 GCM worker；");
            println!("2. 收敛一个 worktree、resource、P0/P1 或 canonical issue blocker；受保护资源显式保留；");
            println!("3. 合并一个 pending merge（accepted 任务）并记录 collab task integrated，再关闭 worktree；");
            println!("4. 用行为证据关闭阶段；仍需调度时在结束前重新武装一次性 wake。");
            println!("\n若确为外部门禁（需人类批准的不可逆操作、发布、成本、新范围）：");
            println!("  collab master wake hold --reason \"<门禁与解除条件>\" --ttl-seconds <n>");
        }
        ExecutionRole::Worker => {
            println!("继续本人 goal 的编排：拆解、派发子任务、核验证据、解阻、集成与资源收口。");
            println!("自行管理本人的 goal，不让 master 代登记；不接管他人的目标或项目仲裁权。");
        }
        ExecutionRole::ManagedSubagent => {
            println!("这是旧 managed-subagent 兼容记录，没有生产执行授权；不接新任务、不递归派发，向 peer/master 报告兼容限制后停止。");
        }
        ExecutionRole::Unknown => {
            println!("Collab 身份未验证；继续独立完成用户已授权的工作，可派独立 `codex exec --profile gcm` worker。");
            println!("仅在用户明确要求时注册；当前不获得 Collab 管理权限，不关闭其他 peer。");
        }
    }
}

pub(super) fn handle_goal_command<I>(root: &Path, mut args: I)
where
    I: Iterator<Item = String>,
{
    let sub = args
        .next()
        .unwrap_or_else(|| fail("USAGE: appsdk goal <subscribe|status|cancel|prompt> [options]"));

    match sub.as_str() {
        "subscribe" | "register" => {
            let mut goal_file: Option<String> = None;
            let mut interval_str = "10m".to_string();
            let mut repeat_count: u32 = 100;
            let mut ttl_seconds: u64 = 604800;
            let mut format_json = false;

            while let Some(arg) = args.next() {
                match arg.as_str() {
                    "-g" | "--goal" => {
                        goal_file = Some(args.next().unwrap_or_else(|| fail("MISSING_GOAL_ARG")));
                    }
                    "-i" | "--interval" | "--every" | "--period" => {
                        interval_str = args.next().unwrap_or_else(|| fail("MISSING_INTERVAL_ARG"));
                    }
                    "-r" | "--repeat" | "--repeat-count" => {
                        let r = args.next().unwrap_or_else(|| fail("MISSING_REPEAT_ARG"));
                        repeat_count = r.parse::<u32>().unwrap_or_else(|error| {
                            fail(format!(
                                "GOAL_REPEAT_COUNT_INVALID: '{}' is not an integer: {}",
                                r, error
                            ))
                        });
                    }
                    "--ttl" | "--ttl-seconds" => {
                        let t = args.next().unwrap_or_else(|| fail("MISSING_TTL_ARG"));
                        ttl_seconds = t.parse::<u64>().unwrap_or_else(|error| {
                            fail(format!(
                                "GOAL_TTL_INVALID: '{}' is not an unsigned integer: {}",
                                t, error
                            ))
                        });
                    }
                    "--json" => format_json = true,
                    _ => fail(format!("UNKNOWN_GOAL_SUBSCRIBE_OPTION:{}", arg)),
                }
            }

            let raw_goal = goal_file.unwrap_or_else(|| {
                fail("USAGE: appsdk goal subscribe --goal <path.md> [--interval <duration>]")
            });
            if !(1..=100).contains(&repeat_count) {
                fail("GOAL_REPEAT_COUNT_INVALID: --repeat must be from 1 through 100");
            }
            if ttl_seconds == 0 {
                fail("GOAL_TTL_INVALID: '0' must be greater than zero");
            }
            if !raw_goal.to_lowercase().ends_with(".md") {
                fail(format!(
                    "GOAL_PATH_MUST_BE_MD_FILE: '{}' is not a markdown file (.md)",
                    raw_goal
                ));
            }

            let goal_path = if Path::new(&raw_goal).is_absolute() {
                PathBuf::from(&raw_goal)
            } else {
                root.join(&raw_goal)
            };

            if !goal_path.exists() || !goal_path.is_file() {
                fail(format!(
                    "GOAL_FILE_NOT_FOUND: '{}' does not exist or is not a file",
                    goal_path.display()
                ));
            }

            let every_ms = parse_duration_to_ms(&interval_str).unwrap_or_else(|e| fail(e));
            // A goal deadline is armed with an absolute at-ms that Collab
            // validates after the subscribe request has queued behind the
            // daemon's batch budget. An interval shorter than that budget can
            // already be past by then, so reject it here, before the goal
            // record is mutated, instead of arming a subscription that Collab
            // will refuse and leaving subscription_id stranded at null.
            if every_ms < GOAL_MIN_INTERVAL_MS {
                fail(format!(
                    "GOAL_INTERVAL_TOO_SHORT:{}: a goal deadline trigger is validated after the subscribe request can queue for up to {} seconds, so the interval must be at least {} seconds; rerun with --interval {}m",
                    interval_str,
                    GOAL_MIN_INTERVAL_MS / 1000,
                    GOAL_MIN_INTERVAL_MS / 1000,
                    GOAL_MIN_INTERVAL_MS / 60_000
                ));
            }
            let goal_prompt = generate_long_horizon_goal_prompt(&goal_path, &interval_str);
            let canonical_goal_path = goal_path
                .canonicalize()
                .unwrap_or_else(|_| goal_path.clone());
            let goal_id = sha256(&canonical_goal_path.to_string_lossy());
            let goal_revision = fs::read(&goal_path)
                .map(|content| sha256(&String::from_utf8_lossy(&content)))
                .unwrap_or_else(|error| {
                    goal_fail(
                        format_json,
                        &format!("GOAL_FILE_READ_FAILED:{}", error),
                        None,
                    )
                });

            let scope = match resolve_goal_scope(root) {
                Ok(scope) => scope,
                Err(error) => goal_fail(format_json, &error, None),
            };
            let owner = scope.owner.clone();
            let _goal_lock = match GoalLock::acquire(&scope) {
                Ok(lock) => lock,
                Err(error) => goal_fail(format_json, &error, None),
            };
            let existing = match goal_record_read(&scope) {
                Ok(existing) => existing,
                Err(error) => {
                    drop(_goal_lock);
                    goal_fail(format_json, &error, None)
                }
            };
            let goal_subject = format!("goal:{}", goal_id);
            let mut terminal_previous = None;
            if let Some(existing) = existing.as_ref() {
                if existing["desired"].as_str() == Some("cancel_pending") {
                    if let Some(retained_id) = goal_record_subscription_id(existing) {
                        match goal_subscription_status(root, &scope, &retained_id) {
                            Ok((remote_status, remote_record)) => {
                                if goal_subscription_is_terminal(&remote_status) {
                                    terminal_previous = Some((remote_status, remote_record));
                                } else {
                                    let matched_subject =
                                        remote_record["subject"].as_str().unwrap_or("unknown");
                                    let error = format!(
                                        "GOAL_CANCEL_PENDING_RECONCILIATION_REQUIRED: subscription {} remains {} under subject {}; rerun goal cancel before subscribing",
                                        retained_id, remote_status, matched_subject
                                    );
                                    drop(_goal_lock);
                                    goal_fail(format_json, &error, Some(existing));
                                }
                            }
                            Err(error)
                                if !error.starts_with("GOAL_STATUS_SUBSCRIPTION_NOT_FOUND:") =>
                            {
                                let error =
                                    format!("GOAL_CANCEL_PENDING_RECONCILIATION_FAILED:{}", error);
                                drop(_goal_lock);
                                goal_fail(format_json, &error, Some(existing));
                            }
                            Err(_) => {}
                        }
                    }
                    match goal_subscription_by_subject_candidates(
                        root,
                        &scope,
                        Some(existing),
                        &goal_subject,
                    ) {
                        Ok(Some((subscription_id, _, _, matched_subject))) => {
                            let error = format!(
                                "GOAL_CANCEL_PENDING_RECONCILIATION_REQUIRED: armed subscription {} remains under subject {}; rerun goal cancel before subscribing",
                                subscription_id, matched_subject
                            );
                            drop(_goal_lock);
                            goal_fail(format_json, &error, Some(existing));
                        }
                        Ok(None) => {}
                        Err(error) => {
                            let error =
                                format!("GOAL_CANCEL_PENDING_RECONCILIATION_FAILED:{}", error);
                            drop(_goal_lock);
                            goal_fail(format_json, &error, Some(existing));
                        }
                    }
                }
                let mut due_deadline_rearm: Option<String> = None;
                if matches!(
                    existing["desired"].as_str(),
                    Some("subscribed" | "recovery_required")
                ) {
                    let recovering = existing["desired"].as_str() == Some("recovery_required");
                    match goal_subscription_by_subject_candidates(
                        root,
                        &scope,
                        Some(existing),
                        &goal_subject,
                    ) {
                        Ok(Some((
                            subscription_id,
                            remote_status,
                            remote_record,
                            matched_subject,
                        ))) => {
                            if existing["goal_id"].as_str() != Some(goal_id.as_str()) {
                                drop(_goal_lock);
                                goal_fail(
                                    format_json,
                                    "GOAL_ALREADY_SUBSCRIBED: cancel the existing goal before subscribing another",
                                    Some(existing),
                                );
                            }
                            if goal_deadline_trigger_is_due(&remote_record, goal_now_ms()) {
                                // Retaining an already-due one-shot deadline
                                // would report `subscribed` while the patrol
                                // loop stays stopped. Cancel it and fall
                                // through to arm a fresh future trigger.
                                due_deadline_rearm = Some(subscription_id);
                            } else {
                                let mut response = existing.clone();
                                let retained_subject =
                                    existing["subject"].as_str().map(str::to_owned);
                                response["subscription_id"] = Value::String(subscription_id);
                                response["collab_subscription"] = remote_record;
                                response["remote_state"] = Value::String(remote_status);
                                response["desired"] = Value::String("subscribed".into());
                                response["observed"] = Value::String("subscribed".into());
                                response["active"] = Value::Bool(true);
                                response["error"] = Value::Null;
                                response["revision"] = Value::Number(
                                    (existing["revision"].as_u64().unwrap_or(0) + 1).into(),
                                );
                                response["idempotent"] = Value::Bool(true);
                                response["goal_prompt"] = Value::String(goal_prompt);
                                if recovering {
                                    response["recovered_at"] =
                                        Value::String(chrono::Utc::now().to_rfc3339());
                                }
                                if retained_subject.as_deref() != Some(matched_subject.as_str()) {
                                    response["subject_migration"] = serde_json::json!({
                                        "from": retained_subject,
                                        "to": matched_subject,
                                        "status": "canonical_subject_migrated",
                                        "migrated_at": chrono::Utc::now().to_rfc3339()
                                    });
                                    response["subject"] = Value::String(matched_subject);
                                }
                                if let Err(error) = goal_record_write(&scope, &response) {
                                    drop(_goal_lock);
                                    goal_fail(format_json, &error, Some(&response));
                                }
                                if format_json {
                                    println!(
                                        "{}",
                                        serde_json::to_string_pretty(&response).unwrap()
                                    );
                                } else {
                                    println!("Long-horizon goal already registered:");
                                    println!("- Goal file: {}", canonical_goal_path.display());
                                    println!("- Existing Collab subscription retained");
                                }
                                return;
                            }
                        }
                        Ok(None) if recovering => {}
                        Ok(None) => {
                            let retained = goal_record_subscription_id(existing).map(|id| {
                                goal_subscription_status(root, &scope, &id)
                                    .map(|(status, remote_record)| (status, remote_record))
                            });
                            match retained {
                                Some(Ok((remote_status, remote_record)))
                                    if existing["goal_id"].as_str() == Some(goal_id.as_str())
                                        && goal_subscription_is_terminal(&remote_status) =>
                                {
                                    terminal_previous = Some((remote_status, remote_record));
                                }
                                _ => {
                                    let mut recovery = existing.clone();
                                    goal_mark_recovery_required(
                                        &mut recovery,
                                        "GOAL_EXISTING_SUBSCRIPTION_NOT_RECONCILED: no armed deadline subscription matches the retained or canonical subject".into(),
                                    );
                                    if let Err(error) = goal_record_write(&scope, &recovery) {
                                        drop(_goal_lock);
                                        goal_fail(format_json, &error, Some(&recovery));
                                    }
                                    drop(_goal_lock);
                                    goal_fail(
                                        format_json,
                                        recovery["error"].as_str().unwrap(),
                                        Some(&recovery),
                                    );
                                }
                            }
                        }
                        Err(error) => {
                            let mut recovery = existing.clone();
                            goal_mark_recovery_required(&mut recovery, error.clone());
                            if let Err(write_error) = goal_record_write(&scope, &recovery) {
                                drop(_goal_lock);
                                goal_fail(format_json, &write_error, Some(&recovery));
                            }
                            drop(_goal_lock);
                            goal_fail(format_json, &error, Some(&recovery));
                        }
                    }
                }
                if due_deadline_rearm.is_some() {
                    if let Err(error) = goal_reconcile_ended(
                        root,
                        &goal_id,
                        &owner,
                        Some("goal-due-deadline-rearm"),
                    ) {
                        let mut recovery = (*existing).clone();
                        goal_mark_recovery_required(
                            &mut recovery,
                            format!("GOAL_DUE_DEADLINE_REARM_CANCEL_FAILED:{}", error),
                        );
                        if let Err(write_error) = goal_record_write(&scope, &recovery) {
                            drop(_goal_lock);
                            goal_fail(format_json, &write_error, Some(&recovery));
                        }
                        drop(_goal_lock);
                        goal_fail(
                            format_json,
                            recovery["error"].as_str().unwrap(),
                            Some(&recovery),
                        );
                    }
                }
            }

            let mut trigger_ms = goal_now_ms().saturating_add(every_ms.min(i64::MAX as u64) as i64);
            let mut record = serde_json::json!({
                "schema_version": 1,
                "goal_id": goal_id,
                "goal_revision": goal_revision,
                "revision": 1,
                "desired": "subscribed",
                "observed": "pending",
                "goal_path": canonical_goal_path.to_string_lossy(),
                "interval": interval_str,
                "every_ms": every_ms,
                "trigger_ms": trigger_ms,
                "repeat_count": 1,
                "requested_repeat_count": repeat_count,
                "schedule": "one-shot",
                "local_schedule": "periodic-rearm-intent",
                "rearm_interval_ms": every_ms,
                "ttl_seconds": ttl_seconds,
                "owner": owner,
                "subject": goal_subject,
                "collab_subscribed": false,
                "collab_subscription": Value::Null,
                "subscription_id": Value::Null,
                "remote_state": "pending",
                "error": Value::Null,
                "registered_at": chrono::Utc::now().to_rfc3339(),
                "active": false,
                "recovery": "When the one-shot deadline is consumed, expires, or Collab restarts, rerun appsdk goal subscribe with this goal to create a fresh one-shot deadline; renewal is explicit and is not automatic."
            });
            if let Some(previous) = existing.as_ref().filter(|previous| {
                previous["desired"].as_str() == Some("recovery_required")
                    || previous["desired"].as_str() == Some("cancel_pending")
                    || terminal_previous.is_some()
            }) {
                let mut previous = previous.clone();
                if let Some((remote_status, remote_record)) = terminal_previous {
                    previous["remote_state"] = Value::String(remote_status.clone());
                    previous["observed"] = Value::String(remote_status);
                    previous["active"] = Value::Bool(false);
                    previous["collab_subscribed"] = Value::Bool(false);
                    previous["collab_subscription"] = remote_record;
                }
                if let Some(previous_object) = previous.as_object_mut() {
                    previous_object.remove("recovery_history");
                }
                record["recovery_history"] = serde_json::json!([{
                    "previous_record": previous,
                    "recovered_at": chrono::Utc::now().to_rfc3339()
                }]);
            }
            if let Err(error) = goal_record_write(&scope, &record) {
                record["error"] = Value::String(error.clone());
                drop(_goal_lock);
                goal_fail(format_json, &error, Some(&record));
            }
            // Recompute the absolute trigger at the last possible moment before
            // the subscribe call. The interval floor keeps Collab's validation
            // inside the future for a slow subscribe path; this keeps the
            // actual at-ms from carrying the record-write delay as well.
            trigger_ms = goal_now_ms().saturating_add(every_ms.min(i64::MAX as u64) as i64);
            record["trigger_ms"] = Value::Number(trigger_ms.into());
            let collab_sub = goal_reconcile_active(root, &goal_id, &owner, trigger_ms, ttl_seconds);

            let (collab_subscribed, sub_details, subscription_id, sub_error) = match collab_sub {
                Ok((response, id)) => {
                    let subscription = response.get("subscription").unwrap_or(&response);
                    if let Err(error) = goal_check_subscription_owner(&scope, subscription) {
                        goal_mark_recovery_required(&mut record, error.clone());
                        if let Err(write_error) = goal_record_write(&scope, &record) {
                            goal_fail(format_json, &write_error, Some(&record));
                        }
                        goal_fail(format_json, &error, Some(&record));
                    }
                    (true, Some(response), Some(id), None)
                }
                Err(error) => (
                    false,
                    None,
                    None,
                    Some(
                        if matches!(
                            error.as_str(),
                            "GOAL_COLLAB_COMMAND_TIMEOUT" | "GOAL_COLLAB_OUTPUT_DRAIN_TIMEOUT"
                        ) {
                            error
                        } else {
                            format!("GOAL_RECONCILE_ACTIVE_FAILED:{}", error)
                        },
                    ),
                ),
            };

            record["collab_subscribed"] = Value::Bool(collab_subscribed);
            record["collab_subscription"] = sub_details.unwrap_or(Value::Null);
            record["subscription_id"] = subscription_id.map(Value::String).unwrap_or(Value::Null);
            record["remote_state"] = Value::String(if collab_subscribed {
                "armed".into()
            } else {
                "unknown".into()
            });
            record["observed"] = Value::String(if collab_subscribed {
                "subscribed".into()
            } else {
                "unknown".into()
            });
            record["active"] = Value::Bool(collab_subscribed);
            record["error"] = sub_error.map(Value::String).unwrap_or(Value::Null);
            let subscription_not_armed_error = record["error"].as_str().and_then(|error| {
                error
                    .starts_with("GOAL_ONE_SHOT_SUBSCRIPTION_NOT_ARMED:")
                    .then_some(error.to_string())
            });
            if !collab_subscribed {
                if let Some(error) = subscription_not_armed_error {
                    goal_mark_recovery_required(&mut record, error);
                } else if let Some(error) = record["error"]
                    .as_str()
                    .filter(|_| record["subscription_id"].as_str().is_none())
                    .filter(|error| goal_subscribe_failure_allows_no_subscription_recovery(error))
                    .map(str::to_owned)
                {
                    goal_mark_recovery_required(&mut record, error);
                }
            }
            record["revision"] = Value::Number(2.into());
            if let Err(error) = goal_record_write(&scope, &record) {
                record["active"] = Value::Bool(false);
                record["observed"] = Value::String("unknown".into());
                record["error"] = Value::String(error.clone());
                record["recovery"] = Value::String(
                    "retain the returned subscription_id and cancel it after restoring local storage".into(),
                );
                drop(_goal_lock);
                goal_fail(format_json, &error, Some(&record));
            }

            if !collab_subscribed {
                let error = record["error"].as_str().unwrap_or("GOAL_SUBSCRIBE_UNKNOWN");
                drop(_goal_lock);
                goal_fail(format_json, error, Some(&record));
            } else if format_json {
                let mut resp = record.clone();
                resp["goal_prompt"] = Value::String(goal_prompt);
                println!("{}", serde_json::to_string_pretty(&resp).unwrap());
            } else {
                println!("Long-horizon goal successfully registered:");
                println!("- Goal file: {}", goal_path.display());
                println!(
                    "- One-shot deadline: first trigger after {} (local rearm intent: every {}, up to {} deliveries; TTL {} seconds)",
                    interval_str, interval_str, repeat_count, ttl_seconds
                );
                println!("- Collab notification status: one-shot armed; rearm is explicit and verifiable");
                println!("\n{}", goal_prompt);
            }
        }
        "status" => {
            let mut format_json = false;
            while let Some(arg) = args.next() {
                match arg.as_str() {
                    "--json" => format_json = true,
                    other => fail(format!("UNKNOWN_GOAL_STATUS_OPTION:{}", other)),
                }
            }
            let scope = match resolve_goal_scope(root) {
                Ok(scope) => scope,
                Err(error) => goal_fail(format_json, &error, None),
            };
            let _goal_lock = match GoalLock::acquire(&scope) {
                Ok(lock) => lock,
                Err(error) => goal_fail(format_json, &error, None),
            };
            let existing = match goal_record_read(&scope) {
                Ok(existing) => existing,
                Err(error) => {
                    drop(_goal_lock);
                    goal_fail(format_json, &error, None)
                }
            };
            let Some(mut record) = existing else {
                if format_json {
                    println!(
                        "{}",
                        serde_json::json!({
                            "active": false,
                            "desired": "unknown",
                            "observed": "unknown",
                            "error": "GOAL_RECORD_NOT_FOUND",
                            "recovery": "subscribe a goal before requesting status"
                        })
                    );
                } else {
                    println!("Goal status unknown: no local goal record; subscribe a goal before requesting status.");
                }
                return;
            };
            if !record.is_object() || record["goal_id"].as_str().is_none() {
                drop(_goal_lock);
                goal_fail(
                    format_json,
                    "GOAL_RECORD_INVALID: goal_id is missing",
                    Some(&record),
                );
            }
            let old_record = record.clone();
            let subscription_id = goal_record_subscription_id(&record);
            let mut reconcile_error = None;
            if matches!(
                record["desired"].as_str(),
                Some("subscribed" | "recovery_required")
            ) {
                let by_id = subscription_id
                    .as_deref()
                    .map(|id| goal_subscription_status(root, &scope, id));
                let by_id =
                    by_id.unwrap_or_else(|| Err("GOAL_STATUS_SUBSCRIPTION_ID_MISSING".into()));
                match by_id {
                    Ok((remote_status, remote_record)) => {
                        record["remote_state"] = Value::String(remote_status.clone());
                        record["collab_subscription"] = remote_record;
                        if remote_status == "armed" {
                            record["desired"] = Value::String("subscribed".into());
                            record["observed"] = Value::String("subscribed".into());
                            record["active"] = Value::Bool(true);
                            record["collab_subscribed"] = Value::Bool(true);
                            record["error"] = Value::Null;
                        } else {
                            let error = format!(
                                "GOAL_ONE_SHOT_SUBSCRIPTION_NOT_ARMED:{}: deadline may be consumed or expired, or Collab may have restarted",
                                remote_status
                            );
                            record["desired"] = Value::String("recovery_required".into());
                            record["observed"] = Value::String(remote_status);
                            record["active"] = Value::Bool(false);
                            record["error"] = Value::String(error.clone());
                            record["recovery"] = Value::String(
                                "Rerun appsdk goal subscribe --goal <path.md> to rearm a fresh one-shot deadline; no automatic renewal is attempted".into(),
                            );
                            reconcile_error = Some(error);
                        }
                    }
                    Err(primary_error) => {
                        let canonical_subject = record["goal_id"]
                            .as_str()
                            .map(|goal_id| format!("goal:{}", goal_id))
                            .unwrap_or_default();
                        match goal_subscription_by_subject_candidates(
                            root,
                            &scope,
                            Some(&record),
                            &canonical_subject,
                        ) {
                            Ok(Some((
                                resolved_id,
                                remote_status,
                                remote_record,
                                matched_subject,
                            ))) => {
                                let retained_subject =
                                    record["subject"].as_str().map(str::to_owned);
                                record["subscription_id"] = Value::String(resolved_id);
                                record["remote_state"] = Value::String(remote_status);
                                record["desired"] = Value::String("subscribed".into());
                                record["observed"] = Value::String("subscribed".into());
                                record["active"] = Value::Bool(true);
                                record["collab_subscribed"] = Value::Bool(true);
                                record["collab_subscription"] = remote_record;
                                record["error"] = Value::Null;
                                if retained_subject.as_deref() != Some(matched_subject.as_str()) {
                                    record["subject_migration"] = serde_json::json!({
                                        "from": retained_subject,
                                        "to": matched_subject,
                                        "status": "canonical_subject_migrated",
                                        "migrated_at": chrono::Utc::now().to_rfc3339()
                                    });
                                    record["subject"] = Value::String(matched_subject);
                                }
                                reconcile_error = Some(format!(
                                    "{}; reconciled by retained-compatible subject",
                                    primary_error
                                ));
                            }
                            Ok(None) => {
                                let error = format!(
                                    "{}; GOAL_STATUS_SUBSCRIPTION_LOST: no armed deadline subscription matches the retained or canonical subject",
                                    primary_error
                                );
                                reconcile_error = Some(error.clone());
                                record["desired"] = Value::String("recovery_required".into());
                                record["active"] = Value::Bool(false);
                                record["observed"] = Value::String("unknown".into());
                                record["remote_state"] = Value::String("unknown".into());
                                record["error"] = Value::String(error);
                            }
                            Err(subject_error) => {
                                let error = format!(
                                    "{}; GOAL_STATUS_SUBJECT_RECONCILIATION_FAILED:{}",
                                    primary_error, subject_error
                                );
                                reconcile_error = Some(error.clone());
                                record["desired"] = Value::String("recovery_required".into());
                                record["active"] = Value::Bool(false);
                                record["observed"] = Value::String("unknown".into());
                                record["remote_state"] = Value::String("unknown".into());
                                record["error"] = Value::String(error);
                            }
                        }
                        record["recovery"] = Value::String(
                            "restore Collab, then rerun appsdk goal status; if the one-shot deadline expired, rerun appsdk goal subscribe --goal <path.md> to rearm it".into(),
                        );
                    }
                }
            }
            if (record != old_record || record["session_id"] != serde_json::json!(scope.session_id))
                && old_record["record_source"] != "legacy-read-only"
            {
                record["session_id"] = serde_json::json!(scope.session_id);
                let revision = old_record["revision"].as_u64().unwrap_or(0);
                record["revision"] = Value::Number((revision + 1).into());
                if let Err(error) = goal_record_write(&scope, &record) {
                    record["active"] = Value::Bool(false);
                    record["observed"] = Value::String("unknown".into());
                    record["error"] = Value::String(error.clone());
                    drop(_goal_lock);
                    goal_fail(format_json, &error, Some(&record));
                }
            }
            if format_json {
                let payload = serde_json::json!({
                    "active": record["active"].as_bool().unwrap_or(false),
                    "desired": record["desired"].as_str().unwrap_or("unknown"),
                    "observed": record["observed"].as_str().unwrap_or("unknown"),
                    "goal_id": record["goal_id"].as_str(),
                    "goal_path": record["goal_path"].as_str(),
                    "interval": record["interval"].as_str(),
                    "subscription_id": goal_record_subscription_id(&record),
                    "collab_subscribed": record["collab_subscribed"].as_bool(),
                    "error": record["error"],
                    "reconciliation_error": reconcile_error,
                    "record": record,
                });
                println!("{}", serde_json::to_string_pretty(&payload).unwrap());
            } else {
                println!("Active Long-Horizon Goal:");
                println!(
                    "- Goal: {}",
                    record["goal_path"].as_str().unwrap_or("unknown")
                );
                println!(
                    "- Interval: {}",
                    record["interval"].as_str().unwrap_or("unknown")
                );
                println!(
                    "- Registered at: {}",
                    record["registered_at"].as_str().unwrap_or("unknown")
                );
                println!("- Active: {}", record["active"].as_bool().unwrap_or(false));
                println!(
                    "- Desired: {} | Observed: {}",
                    record["desired"].as_str().unwrap_or("unknown"),
                    record["observed"].as_str().unwrap_or("unknown")
                );
                if let Some(error) = record["error"].as_str() {
                    println!("- Error: {}", error);
                }
            }
        }
        "cancel" => {
            let mut format_json = false;
            while let Some(arg) = args.next() {
                match arg.as_str() {
                    "--json" => format_json = true,
                    other => fail(format!("UNKNOWN_GOAL_CANCEL_OPTION:{}", other)),
                }
            }
            let scope = match resolve_goal_scope(root) {
                Ok(scope) => scope,
                Err(error) => goal_fail(format_json, &error, None),
            };
            let owner = scope.owner.clone();
            let _goal_lock = match GoalLock::acquire(&scope) {
                Ok(lock) => lock,
                Err(error) => goal_fail(format_json, &error, None),
            };
            let Some(mut record) = (match goal_record_read(&scope) {
                Ok(record) => record,
                Err(error) => {
                    drop(_goal_lock);
                    goal_fail(format_json, &error, None)
                }
            }) else {
                drop(_goal_lock);
                goal_fail(format_json, "GOAL_CANCEL_SUBSCRIPTION_MISSING", None);
            };
            if record["owner"].as_str() != Some(owner.as_str()) {
                drop(_goal_lock);
                goal_fail(
                    format_json,
                    "GOAL_CANCEL_OWNER_MISMATCH: only the recorded goal owner may cancel this subscription",
                    Some(&record),
                );
            }
            if record["desired"].as_str() == Some("unsubscribed")
                && record["observed"].as_str() == Some("cancelled")
            {
                if format_json {
                    println!(
                        "{}",
                        serde_json::to_string_pretty(&serde_json::json!({
                            "ok": true,
                            "status": "cancelled",
                            "idempotent": true,
                            "subscription_id": goal_record_subscription_id(&record),
                            "revision": record["revision"],
                            "cancel_receipt": record["cancel_receipt"],
                            "record": record,
                        }))
                        .unwrap()
                    );
                } else {
                    println!("Goal subscription already cancelled.");
                }
                return;
            }
            let subject = record["subject"]
                .as_str()
                .filter(|value| !value.trim().is_empty())
                .map(str::to_owned);
            let canonical_subject = record["goal_id"]
                .as_str()
                .map(|goal_id| format!("goal:{}", goal_id))
                .unwrap_or_default();
            let Some(cancel_goal_id) = record["goal_id"]
                .as_str()
                .filter(|goal_id| !goal_id.trim().is_empty())
                .map(str::to_owned)
            else {
                drop(_goal_lock);
                goal_fail(
                    format_json,
                    "GOAL_CANCEL_GOAL_ID_MISSING: local goal record has no goal identity",
                    Some(&record),
                );
            };
            let subscription_id = goal_record_subscription_id(&record);
            if subscription_id.is_none() {
                let subject_result = goal_subscription_by_subject_candidates(
                    root,
                    &scope,
                    Some(&record),
                    &canonical_subject,
                );
                match subject_result {
                    Ok(Some((resolved_id, remote_status, remote_record, matched_subject))) => {
                        if subject.as_deref() != Some(matched_subject.as_str()) {
                            record["subject_migration"] = serde_json::json!({
                                "from": subject.clone(),
                                "to": matched_subject,
                                "status": "canonical_subject_migrated",
                                "migrated_at": chrono::Utc::now().to_rfc3339()
                            });
                            record["subject"] = Value::String(matched_subject.clone());
                        }
                        record["subscription_id"] = Value::String(resolved_id.clone());
                        record["collab_subscription"] = remote_record;
                        record["remote_state"] = Value::String(remote_status);
                        record["observed"] = Value::String("subscribed".into());
                        record["active"] = Value::Bool(true);
                        record["revision"] =
                            Value::Number((record["revision"].as_u64().unwrap_or(0) + 1).into());
                        if let Err(write_error) = goal_record_write(&scope, &record) {
                            record["error"] = Value::String(format!(
                                "GOAL_CANCEL_SUBJECT_BIND_RECORD_WRITE_FAILED:{}",
                                write_error
                            ));
                            drop(_goal_lock);
                            goal_fail(
                                format_json,
                                record["error"].as_str().unwrap(),
                                Some(&record),
                            );
                        }
                    }
                    Ok(None) => {
                        // The semantic reconcile can retire a canonical
                        // occurrence even when legacy subject lookup found
                        // none. Keep the record pending until the daemon
                        // returns its receipt.
                        record["remote_state"] = Value::String("no_matching_armed_subject".into());
                    }
                    Err(error) => {
                        record["desired"] = Value::String("cancel_pending".into());
                        record["observed"] = Value::String("unknown".into());
                        record["active"] = Value::Bool(false);
                        record["error"] = Value::String(error.clone());
                        record["remote_state"] = Value::String("unknown".into());
                        record["revision"] =
                            Value::Number((record["revision"].as_u64().unwrap_or(0) + 1).into());
                        if let Err(write_error) = goal_record_write(&scope, &record) {
                            record["error"] = Value::String(format!(
                                "GOAL_CANCEL_PENDING_RECORD_WRITE_FAILED:{}; original={}",
                                write_error, error
                            ));
                            drop(_goal_lock);
                            goal_fail(
                                format_json,
                                record["error"].as_str().unwrap(),
                                Some(&record),
                            );
                        }
                        drop(_goal_lock);
                        goal_fail(format_json, &error, Some(&record));
                    }
                }
            }
            record["desired"] = Value::String("cancel_pending".into());
            record["observed"] = Value::String("pending".into());
            record["active"] = Value::Bool(false);
            record["error"] = Value::Null;
            record["revision"] =
                Value::Number((record["revision"].as_u64().unwrap_or(0) + 1).into());
            if let Err(error) = goal_record_write(&scope, &record) {
                drop(_goal_lock);
                goal_fail(
                    format_json,
                    &format!("GOAL_CANCEL_PENDING_RECORD_WRITE_FAILED:{}", error),
                    Some(&record),
                );
            }
            let cancel_result =
                goal_reconcile_ended(root, &cancel_goal_id, &owner, Some("appsdk-goal-cancel"));
            let cancel_receipt = match cancel_result {
                Ok(receipt) => receipt,
                Err(error) => {
                    record["desired"] = Value::String("cancel_pending".into());
                    record["observed"] = Value::String("unknown".into());
                    record["active"] = Value::Bool(false);
                    record["error"] = Value::String(error.clone());
                    record["remote_state"] = Value::String("unknown".into());
                    record["revision"] =
                        Value::Number((record["revision"].as_u64().unwrap_or(0) + 1).into());
                    if let Err(write_error) = goal_record_write(&scope, &record) {
                        record["error"] = Value::String(format!(
                            "GOAL_CANCEL_PENDING_RECORD_WRITE_FAILED:{}; original={}",
                            write_error, error
                        ));
                        drop(_goal_lock);
                        goal_fail(
                            format_json,
                            record["error"].as_str().unwrap(),
                            Some(&record),
                        );
                    }
                    drop(_goal_lock);
                    goal_fail(format_json, &error, Some(&record));
                }
            };
            record["desired"] = Value::String("unsubscribed".into());
            record["observed"] = Value::String("cancelled".into());
            record["active"] = Value::Bool(false);
            record["remote_state"] = Value::String("cancelled".into());
            record["error"] = Value::Null;
            record["cancel_receipt"] = cancel_receipt;
            record["cancelled_at"] = Value::String(chrono::Utc::now().to_rfc3339());
            record["revision"] =
                Value::Number((record["revision"].as_u64().unwrap_or(0) + 1).into());
            if let Err(error) = goal_record_write(&scope, &record) {
                record["desired"] = Value::String("cancel_pending".into());
                record["observed"] = Value::String("unknown".into());
                record["active"] = Value::Bool(false);
                record["error"] =
                    Value::String(format!("GOAL_CANCEL_RECORD_WRITE_FAILED:{}", error));
                record["revision"] =
                    Value::Number((record["revision"].as_u64().unwrap_or(0) + 1).into());
                drop(_goal_lock);
                goal_fail(
                    format_json,
                    record["error"].as_str().unwrap(),
                    Some(&record),
                );
            }
            if format_json {
                println!(
                    "{}",
                    serde_json::json!({
                        "ok": true,
                        "status": "cancelled",
                        "subscription_id": goal_record_subscription_id(&record),
                        "revision": record["revision"],
                        "cancel_receipt": record["cancel_receipt"],
                        "record": record,
                    })
                );
            } else {
                let final_id =
                    goal_record_subscription_id(&record).unwrap_or_else(|| cancel_goal_id.clone());
                println!("Goal schedule cancelled: {}", final_id);
            }
        }
        "prompt" => {
            let mut goal_file: Option<String> = None;
            let mut interval_str = "10m".to_string();

            while let Some(arg) = args.next() {
                match arg.as_str() {
                    "-g" | "--goal" => {
                        goal_file = Some(args.next().unwrap_or_else(|| fail("MISSING_GOAL_ARG")));
                    }
                    "-i" | "--interval" | "--every" => {
                        interval_str = args.next().unwrap_or_else(|| fail("MISSING_INTERVAL_ARG"));
                    }
                    _ => {}
                }
            }

            let raw_goal = goal_file.unwrap_or_else(|| {
                fail("USAGE: appsdk goal prompt --goal <path.md> [--interval <duration>]")
            });
            if !raw_goal.to_lowercase().ends_with(".md") {
                fail(format!(
                    "GOAL_PATH_MUST_BE_MD_FILE: '{}' is not a markdown file (.md)",
                    raw_goal
                ));
            }
            let goal_path = if Path::new(&raw_goal).is_absolute() {
                PathBuf::from(&raw_goal)
            } else {
                root.join(&raw_goal)
            };
            if !goal_path.exists() || !goal_path.is_file() {
                fail(format!(
                    "GOAL_FILE_NOT_FOUND: '{}' does not exist or is not a file",
                    goal_path.display()
                ));
            }
            if let Err(error) = resolve_goal_scope(root) {
                goal_fail(false, &error, None);
            }

            let prompt = generate_long_horizon_goal_prompt(&goal_path, &interval_str);
            println!("{}", prompt);
        }
        _ => fail(format!("UNKNOWN_GOAL_SUBCOMMAND:{}", sub)),
    }
}

pub(super) fn handle_task_command<I>(root: &Path, mut args: I)
where
    I: Iterator<Item = String>,
{
    let sub = match args.next() {
        Some(s) => s,
        None => {
            let status = Command::new("collab")
                .arg("task")
                .current_dir(root)
                .status()
                .unwrap_or_else(|e| fail(format!("COLLAB_UNAVAILABLE:{}", e)));
            std::process::exit(status.code().unwrap_or(1));
        }
    };

    if sub == "block" {
        let task_id = args
            .next()
            .unwrap_or_else(|| fail("USAGE: appsdk task block <id> [--reason <text>] [--json]"));
        let mut reason: Option<String> = None;
        let mut format_json = false;

        while let Some(arg) = args.next() {
            match arg.as_str() {
                "--reason" | "-m" | "--next" => {
                    reason = Some(args.next().unwrap_or_else(|| fail("MISSING_REASON_ARG")));
                }
                "--json" => format_json = true,
                _ => {}
            }
        }

        // 1. Invoke collab task block to update durable state
        let mut block_cmd = Command::new("collab");
        block_cmd.args(["task", "block", &task_id]);
        if let Some(r) = &reason {
            block_cmd.args(["--next", r]);
        }
        block_cmd.current_dir(root);
        let block_out = match block_cmd.output() {
            Ok(out) => out,
            Err(e) => fail(format!("COLLAB_UNAVAILABLE:{}", e)),
        };
        if !block_out.status.success() {
            let code = block_out.status.code().unwrap_or(1);
            let stderr = String::from_utf8_lossy(&block_out.stderr)
                .trim()
                .to_string();
            let stdout = String::from_utf8_lossy(&block_out.stdout)
                .trim()
                .to_string();
            eprintln!(
                "COLLAB_TASK_BLOCK_FAILED:exit={}{}{}",
                code,
                if stderr.is_empty() {
                    String::new()
                } else {
                    format!(":{}", stderr)
                },
                if stdout.is_empty() {
                    String::new()
                } else {
                    format!(":{}", stdout)
                }
            );
            std::process::exit(code);
        }
        let block_result = String::from_utf8_lossy(&block_out.stdout)
            .trim()
            .to_string();

        // 2. Construct the mandatory governance reminder. Blocking a task does
        // not implicitly close all notifications; only the underlying Collab
        // command may change precise subscription policy.
        let notice_title = format!("【AppSDK 任务阻塞门禁提醒】任务 '{}' 已调用 collab task block，通知策略以 Collab 响应为准。", task_id);
        let notice_body = r#"================================================================================
【重要门禁与合规约束】
1. AppSDK 的问题可以报 bug：若阻断由 AppSDK 框架缺陷导致（CLI 异常、verify 误报、准入阻断），
   必须立即上报 upstream 缺陷系统：
   appsdk bug new --upstream -t "[SDK Bug] <简述>" -m "<复现与上下文>" -l "P0,cli"
2. 合法等待必须写清原因、责任人、解除条件和恢复触发：
   外部依赖、资源占用、凭证/批准缺失、跨 owner 决策等可以进入 waiting/blocked，
   但严禁只写“blocked”而不带恢复方案，也严禁因任务难就空等。
3. peer 保留受托范围的唯一交付责任。向 master 提交真实失败入口、原错/证据、已尝试解法及结果、授权内替代路径为何不可行、所需决定和恢复条件；求助不等于停工批准。
4. master 独立检查或复现证据，核对权限、替代路径和影响范围后才认可阻塞；证据不足退回调查。不得为停止提醒强制关闭未完成任务。
5. 巡检只是兜底唤醒，不是执行依赖。有可执行动作立即推进；等待决定时继续未受影响工作，不等下一次唤醒或别人代为解阻。
================================================================================"#;

        if format_json {
            let resp = serde_json::json!({
                "ok": true,
                "task_id": task_id,
                "status": "blocked",
                "reminders_stopped": false,
                "reason": reason,
                "collab_result": block_result,
                "rule": "blocked/waiting needs original failure evidence, attempted remedies, unavailable authorized alternatives and a decision/resume condition; master independently verifies before accepting; peer retains delivery ownership unless explicitly reassigned; continue unaffected work without waiting for a patrol wake",
                "notice": format!("{}\n{}", notice_title, notice_body)
            });
            println!("{}", serde_json::to_string_pretty(&resp).unwrap());
        } else {
            println!(
                "{}\n{}\n{}{}",
                notice_title,
                notice_body,
                if block_result.is_empty() {
                    String::new()
                } else {
                    "\nCollab result:\n".to_string()
                },
                block_result
            );
        }
    } else {
        let mut rest_args = vec!["task".to_string(), sub];
        rest_args.extend(args);
        let status = Command::new("collab")
            .args(&rest_args)
            .current_dir(root)
            .status()
            .unwrap_or_else(|e| fail(format!("COLLAB_UNAVAILABLE:{}", e)));
        std::process::exit(status.code().unwrap_or(1));
    }
}
