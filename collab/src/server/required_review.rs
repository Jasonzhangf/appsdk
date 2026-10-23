fn required_work_reviewer(state: &State, task_id: &str) -> Option<String> {
    state
        .effective_work_policy(task_id)
        .reviewer_name
        .or_else(|| {
            state
                .task_lifecycle
                .get(task_id)
                .and_then(|record| record.required_reviewer_name.clone())
        })
}

fn work_review_valid(state: &State, task_id: &str) -> bool {
    let Some(task) = state.tasks.get(task_id) else {
        return false;
    };
    let Some(record) = state.task_lifecycle.get(task_id) else {
        return false;
    };
    if !matches!(task.status.as_str(), "accepted" | "merged" | "closed")
        || record.reviewer.is_none()
        || record.reviewed_ms.is_none()
        || !record
            .review_evidence
            .as_deref()
            .is_some_and(|text| !text.trim().is_empty())
    {
        return false;
    }
    let required = required_work_reviewer(state, task_id);
    if required
        .as_deref()
        .is_some_and(|name| record.reviewer_name.as_deref() != Some(name))
    {
        return false;
    }
    if record.review_subject_revision > 0 || required.is_some() {
        return record.reviewed_subject_revision == Some(record.review_subject_revision);
    }
    true
}

fn invalidate_work_review(
    state: &State,
    task: &TaskRec,
    record: &mut state::TaskLifecycleRecord,
    reviewer: Option<String>,
) -> Vec<Event> {
    record.required_reviewer_name = reviewer;
    record.review_subject_revision = record.review_subject_revision.saturating_add(1);
    record.reviewed_subject_revision = None;
    record.review_evidence = None;
    record.reviewer = None;
    record.reviewer_name = None;
    record.reviewed_ms = None;
    let mut events = Vec::new();
    if state.pending_merges.contains_key(&task.id) {
        events.push(Event::MergeResolved {
            task_id: task.id.clone(),
            resolved_by: task.owner.clone(),
            reason: Some("delivery or review contract changed".into()),
            at_ms: now_ms(),
        });
        let ids = pending_merge_notice_ids(state, &task.id);
        if !ids.is_empty() {
            events.push(Event::Superseded { ids });
        }
    }
    let ids = work_review_notice_ids(state, &task.id);
    if !ids.is_empty() {
        events.push(Event::Superseded { ids });
    }
    events
}

fn work_review_notice_ids(state: &State, task_id: &str) -> Vec<String> {
    let subject = format!("review-required:{task_id}");
    state
        .msgs
        .values()
        .filter(|message| {
            message.subject.as_deref() == Some(subject.as_str())
                && matches!(message.state.as_str(), "pending" | "delivered")
        })
        .map(|message| message.id.clone())
        .collect()
}

fn work_review_gate(state: &State, task_id: &str) -> Result<(), Resp> {
    if required_work_reviewer(state, task_id).is_some() && !work_review_valid(state, task_id) {
        return Err(Resp::err_data(
            "TASK_REQUIRED_REVIEW_MISSING",
            json!({
                "task_id":task_id,"required_reviewer":required_work_reviewer(state,task_id),
                "delivery_complete":false,"next_action":"the named reviewer must accept the current delivery",
            }),
        ));
    }
    Ok(())
}

#[cfg(test)]
mod required_review_state_tests {
    use super::*;

    fn reviewed_state() -> State {
        let mut state = State::default();
        state.tasks.insert(
            "task".into(),
            TaskRec {
                id: "task".into(),
                owner: "author".into(),
                created_by: "master".into(),
                feature_id: None,
                worktree_path: None,
                branch: None,
                base_commit: None,
                priority: "p2".into(),
                status: "accepted".into(),
                next_step: None,
                wait: None,
                created_ms: 1,
                updated_ms: 1,
            },
        );
        state.task_lifecycle.insert(
            "task".into(),
            state::TaskLifecycleRecord {
                required_reviewer_name: Some("Bob".into()),
                review_subject_revision: 2,
                reviewed_subject_revision: Some(2),
                reviewer_name: Some("Bob".into()),
                reviewer: Some("bob-session".into()),
                reviewed_ms: Some(3),
                review_evidence: Some("review passed".into()),
                delivery_kind: state::TaskDeliveryKind::ReadOnly,
                delivery_evidence: Some("report".into()),
                delivered_ms: Some(2),
                ..Default::default()
            },
        );
        state
    }

    #[test]
    fn review_requires_named_acceptance_of_current_subject_and_not_rework() {
        let mut state = reviewed_state();
        assert!(work_review_valid(&state, "task"));
        state.tasks.get_mut("task").unwrap().status = "rework".into();
        assert!(!work_review_valid(&state, "task"));
        state.tasks.get_mut("task").unwrap().status = "accepted".into();
        state.task_lifecycle.get_mut("task").unwrap().reviewer_name = Some("Alice".into());
        assert!(!work_review_valid(&state, "task"));
        state.task_lifecycle.get_mut("task").unwrap().reviewer_name = Some("Bob".into());
        state
            .task_lifecycle
            .get_mut("task")
            .unwrap()
            .reviewed_subject_revision = Some(1);
        assert!(!work_review_valid(&state, "task"));
    }

    #[test]
    fn changed_contract_invalidates_acceptance_and_compaction_preserves_subject() {
        let mut state = reviewed_state();
        let task = state.tasks["task"].clone();
        let mut record = state.task_lifecycle["task"].clone();
        invalidate_work_review(&state, &task, &mut record, Some("Bob".into()));
        assert_eq!(record.review_subject_revision, 3);
        state.task_lifecycle.insert("task".into(), record);
        assert!(!work_review_valid(&state, "task"));
        let mut replay = State::default();
        for event in state.snapshot_events() {
            replay.apply_checked(&event).unwrap();
        }
        assert_eq!(
            replay.task_lifecycle["task"]
                .required_reviewer_name
                .as_deref(),
            Some("Bob")
        );
        assert!(!work_review_valid(&replay, "task"));
    }
}
