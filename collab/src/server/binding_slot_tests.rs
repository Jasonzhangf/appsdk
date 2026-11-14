use super::binding_slot::{
    BindingControlSet, BindingValue, CommitOutcome, IdentityBindingKey, IdentityBindingSlot,
    IdentityStatus, PendingEffect, PendingEffectKind, SlotError,
};
use crate::identity::{AgentId, AppServerId, BindingId, NativeThreadId, RuntimeId, SessionId};
use crate::proto::{IdentityFacts, RexCandidate, RexEndpoint, SelectedTransport, TransportKind};
use crate::scope::ProjectScopeId;
use crate::server::global_state::RuntimeBinding;
use serde_json::json;

fn key(name: &str) -> IdentityBindingKey {
    key_in("/tmp/collab-binding-slot", name)
}

fn key_in(project: &str, name: &str) -> IdentityBindingKey {
    let name = match name {
        "alice" => "Alice",
        "bob" => "Bob",
        name => name,
    };
    IdentityBindingKey::new(
        ProjectScopeId::new(project).unwrap(),
        AppServerId::new("appserver-cli").unwrap(),
        name,
    )
}

const REX_SERVER: &str = "unix:///tmp/rex.sock";
const REX_SESSION: &str = "session:11111111-1111-1111-1111-111111111111";
const REX_BLOCK: &str = "block:22222222-2222-2222-2222-222222222222";
const OTHER_REX_BLOCK: &str = "block:88888888-8888-8888-8888-888888888888";
const OTHER_REX_SERVER: &str = "unix:///tmp/other-rex.sock";

#[test]
fn ordinary_name_allocation_respects_project_names_across_app_scopes() {
    let alice = key("Alice");
    let mut control = BindingControlSet::new();
    control
        .commit(
            &alice,
            value(&alice, "agent-a", "runtime-a", "unix:///tmp/a.sock"),
            None,
        )
        .unwrap();
    let next_app = AppServerId::new("another-app").unwrap();
    let allocated = control
        .ordinary_name(&alice.project_scope, &next_app, "agent-b")
        .unwrap();
    assert_eq!(
        allocated, "Bob",
        "a current project name is occupied across app scopes"
    );
}

#[test]
fn cross_app_project_selector_returns_exact_source_ref_and_reports_ambiguity() {
    let app_a = AppServerId::new("app-a").unwrap();
    let app_b = AppServerId::new("app-b").unwrap();
    let alice_a = IdentityBindingKey::new(
        ProjectScopeId::new("/tmp/collab-binding-slot-cross-app").unwrap(),
        app_a.clone(),
        "Alice",
    );
    let alice_b = IdentityBindingKey::new(alice_a.project_scope.clone(), app_b.clone(), "Alice");
    let mut control = BindingControlSet::new();
    control
        .commit(
            &alice_a,
            value(&alice_a, "agent-a", "runtime-a", "unix:///tmp/a.sock"),
            None,
        )
        .unwrap();
    let facts_a = crate::proto::IdentityFacts {
        session_id: Some("runtime-a-session".into()),
        ..Default::default()
    };
    let selected = control
        .project_current_for_facts(&alice_a.project_scope, &facts_a, None)
        .unwrap()
        .expect("exact caller facts select the source current");
    assert_eq!(selected.identity_key, alice_a);
    let source = BindingControlSet::cross_scope_source_ref(selected).unwrap();
    assert_eq!(source.key.app_scope_id, app_a);
    assert_eq!(source.generation, 1);
    assert_eq!(source.binding_id.as_str(), "binding-runtime-a");

    control
        .commit(
            &alice_b,
            value(&alice_b, "agent-b", "runtime-b", "unix:///tmp/b.sock"),
            None,
        )
        .unwrap();
    let facts_b = crate::proto::IdentityFacts {
        session_id: Some("runtime-b-session".into()),
        ..Default::default()
    };
    let selected_b = control
        .project_current_for_facts_excluding_app(
            &alice_b.project_scope,
            Some(&app_a),
            &facts_b,
            None,
        )
        .unwrap()
        .expect("excluding the target app selects the other current source");
    assert_eq!(selected_b.identity_key, alice_b);
    assert_eq!(
        control
            .project_currents_for_name(&alice_a.project_scope, "Alice")
            .len(),
        2,
        "ambiguous current slots remain individually readable"
    );
    let error = control
        .project_current_for_name(&alice_a.project_scope, "Alice")
        .expect_err("historical cross-app currents remain typed ambiguity");
    assert!(error.contains("IDENTITY_MAP_AMBIGUOUS"), "{error}");
    assert!(
        error.contains("app-a") && error.contains("app-b"),
        "{error}"
    );
}

#[test]
fn ordinary_name_allocation_keeps_other_project_names_independent() {
    let project_a = ProjectScopeId::new("/tmp/collab-binding-slot-project-a").unwrap();
    let project_b = ProjectScopeId::new("/tmp/collab-binding-slot-project-b").unwrap();
    let app_a = AppServerId::new("app-a").unwrap();
    let app_b = AppServerId::new("app-b").unwrap();
    let alice_a = IdentityBindingKey::new(project_a.clone(), app_a.clone(), "Alice");
    let mut control = BindingControlSet::new();
    control
        .commit(
            &alice_a,
            value(&alice_a, "agent-a", "runtime-a", "unix:///tmp/a.sock"),
            None,
        )
        .unwrap();

    assert_eq!(
        control
            .ordinary_name(&project_a, &app_b, "agent-b")
            .unwrap(),
        "Bob",
        "another app in the same project skips the occupied project name"
    );
    assert_eq!(
        control
            .ordinary_name(&project_b, &app_b, "agent-c")
            .unwrap(),
        "Alice",
        "the same name in another project is not occupied"
    );
}

#[test]
fn retired_ordinary_name_is_reusable_across_app_scopes() {
    let project = ProjectScopeId::new("/tmp/collab-binding-slot-retired-reuse").unwrap();
    let app_a = AppServerId::new("app-a").unwrap();
    let app_b = AppServerId::new("app-b").unwrap();
    let alice_a = IdentityBindingKey::new(project.clone(), app_a, "Alice");
    let mut control = BindingControlSet::new();
    control
        .commit(
            &alice_a,
            value(&alice_a, "agent-a", "runtime-a", "unix:///tmp/a.sock"),
            None,
        )
        .unwrap();
    assert_eq!(
        control.unregister(&alice_a, Some(1)).unwrap(),
        CommitOutcome::Committed { generation: 2 }
    );
    assert_eq!(
        control.ordinary_name(&project, &app_b, "agent-b").unwrap(),
        "Alice",
        "a retired ordinary name is reusable from another app scope"
    );
}

/// A Rex binding whose current typed endpoint is `session`/`block` and whose
/// current agent session carrier is derived from `runtime`.
fn rex_value_default(key: &IdentityBindingKey, agent: &str, runtime: &str) -> BindingValue {
    let agent_session_id = SessionId::new(format!("{runtime}-agent-session")).unwrap();
    let binding = RuntimeBinding::new_with_session(
        key.project_scope.clone(),
        key.app_scope_id.clone(),
        AgentId::new(agent).unwrap(),
        RuntimeId::new(runtime).unwrap(),
        BindingId::new(format!("binding-{runtime}")).unwrap(),
        Some(agent_session_id.clone()),
        None,
    )
    .unwrap();
    BindingValue::new(
        binding,
        SelectedTransport {
            kind: TransportKind::Rex,
            endpoint: None,
            namespace: None,
            session_id: Some(agent_session_id.as_str().to_owned()),
            thread_id: None,
            tmux_endpoint: None,
            rex_endpoint: Some(crate::proto::RexEndpoint {
                server: REX_SERVER.into(),
                session_id: REX_SESSION.into(),
                block_id: REX_BLOCK.into(),
                host_id: Some("host".into()),
                instance_id: format!("instance-{runtime}"),
                cwd: key.project_scope.as_str().into(),
            }),
            capabilities: vec!["send_message_to_block".into()],
            self_check: "ok".into(),
        },
    )
}

fn rex_facts(
    agent_session: Option<&str>,
    server: &str,
    session: &str,
    block: &str,
) -> crate::proto::IdentityFacts {
    crate::proto::IdentityFacts {
        session_id: agent_session.map(str::to_owned),
        rex: Some(crate::proto::RexCandidate {
            server: server.into(),
            session_id: session.into(),
            block_id: block.into(),
            cwd: "/tmp/collab-binding-slot".into(),
        }),
        ..Default::default()
    }
}

fn value(key: &IdentityBindingKey, agent: &str, runtime: &str, endpoint: &str) -> BindingValue {
    let session_id = SessionId::new(format!("{runtime}-session")).unwrap();
    let native_thread_id = NativeThreadId::new(format!("{runtime}-thread")).unwrap();
    let binding = RuntimeBinding::new_with_session(
        key.project_scope.clone(),
        key.app_scope_id.clone(),
        AgentId::new(agent).unwrap(),
        RuntimeId::new(runtime).unwrap(),
        BindingId::new(format!("binding-{runtime}")).unwrap(),
        Some(session_id.clone()),
        Some(native_thread_id.clone()),
    )
    .unwrap();
    BindingValue::new(
        binding,
        SelectedTransport {
            kind: TransportKind::AppServer,
            endpoint: Some(endpoint.to_owned()),
            namespace: Some("test".to_owned()),
            session_id: Some(session_id.as_str().to_owned()),
            thread_id: Some(native_thread_id.as_str().to_owned()),
            tmux_endpoint: None,
            rex_endpoint: None,
            capabilities: vec!["test".to_owned()],
            self_check: "ok".to_owned(),
        },
    )
}

fn rex_value(
    key: &IdentityBindingKey,
    agent: &str,
    runtime: &str,
    session: &str,
    block: &str,
) -> BindingValue {
    rex_value_on_server(
        key,
        agent,
        runtime,
        session,
        block,
        "unix:///tmp/rex-test.sock",
    )
}

fn rex_value_on_server(
    key: &IdentityBindingKey,
    agent: &str,
    runtime: &str,
    session: &str,
    block: &str,
    server: &str,
) -> BindingValue {
    let binding = RuntimeBinding::new_with_session(
        key.project_scope.clone(),
        key.app_scope_id.clone(),
        AgentId::new(agent).unwrap(),
        RuntimeId::new(runtime).unwrap(),
        BindingId::new(format!("binding-{runtime}")).unwrap(),
        Some(SessionId::new(session).unwrap()),
        None,
    )
    .unwrap();
    BindingValue::new(
        binding,
        SelectedTransport {
            kind: TransportKind::Rex,
            endpoint: None,
            namespace: None,
            session_id: Some(session.into()),
            thread_id: None,
            tmux_endpoint: None,
            rex_endpoint: Some(RexEndpoint {
                server: server.into(),
                session_id: "rex-session".into(),
                block_id: block.into(),
                host_id: Some("host".into()),
                instance_id: "instance".into(),
                cwd: key.project_scope.as_str().into(),
            }),
            capabilities: vec!["send_message_to_block".into()],
            self_check: "ok".into(),
        },
    )
}

fn tmux_value_on_socket(
    key: &IdentityBindingKey,
    agent: &str,
    runtime: &str,
    socket: &str,
    session: &str,
    pane: &str,
) -> BindingValue {
    let mut value = value(key, agent, runtime, "endpoint");
    let endpoint = crate::proto::TmuxEndpoint {
        socket_path: socket.into(),
        server_pid: 11,
        tmux_session_id: session.into(),
        pane_id: pane.into(),
        pane_pid: 22,
        codex_session_id: value
            .binding
            .session_id
            .as_ref()
            .map(|id| id.as_str().into()),
        codex_thread_id: None,
    };
    value.binding.tmux_endpoint = Some(endpoint.clone());
    value.transport = SelectedTransport {
        kind: TransportKind::Tmux,
        endpoint: Some(socket.into()),
        namespace: Some(session.into()),
        session_id: value
            .binding
            .session_id
            .as_ref()
            .map(|id| id.as_str().into()),
        thread_id: None,
        tmux_endpoint: Some(endpoint),
        rex_endpoint: None,
        capabilities: vec!["send_message_to_pane".into()],
        self_check: "ok".into(),
    };
    value
}

fn committed_slot(key: IdentityBindingKey, value: BindingValue) -> IdentityBindingSlot {
    let mut slot = IdentityBindingSlot::new(key.clone());
    slot.commit(&key, value, Some(0)).unwrap();
    slot
}

fn current_effect(
    key: &IdentityBindingKey,
    effect_id: &str,
    kind: PendingEffectKind,
    target_worker_id: &str,
    target_session_id: &str,
) -> PendingEffect {
    PendingEffect {
        kind,
        role: None,
        effect_id: effect_id.into(),
        operation_id: format!("operation-{effect_id}"),
        project_scope: key.project_scope.clone(),
        app_scope_id: key.app_scope_id.clone(),
        name: key.name.clone(),
        previous_worker_id: None,
        previous_session_id: None,
        target_worker_id: target_worker_id.into(),
        target_session_id: target_session_id.into(),
        occupancy_revision: 1,
        source_names: Vec::new(),
        moves_existing_names: false,
        pane_anchor: None,
        created_ms: 1,
    }
}

#[test]
fn first_binding_becomes_current_without_previous() {
    let key = key("alice");
    let mut slot = IdentityBindingSlot::new(key.clone());

    assert_eq!(
        slot.commit(
            &key,
            value(&key, "alice", "runtime-a", "endpoint-a"),
            Some(0)
        )
        .unwrap(),
        CommitOutcome::Committed { generation: 1 }
    );

    assert_eq!(slot.identity_status(), IdentityStatus::Active);
    assert_eq!(slot.generation(), 1);
    assert!(slot.reference_previous().is_none());
    slot.validate().unwrap();
}

fn native_fixture_start(order: u64) -> String {
    match crate::host_runtime::current_rex_execution_boundary().unwrap() {
        super::binding_slot::RexExecutionFence::UnixWall { .. } => format!("{order}:1"),
        super::binding_slot::RexExecutionFence::LinuxBoot {
            boot_id,
            ticks_per_second,
            ..
        } => format!("linux:{boot_id}:{ticks_per_second}:{order}"),
    }
}

#[test]
fn rex_human_fence_merge_is_monotonic_and_part_of_current_fact() {
    use super::binding_slot::{RexExecutionFence, VerifiedRexExecutionAssociation};
    let key = key("Alice");
    let mut target = rex_value_default(&key, "target", "target");
    target.rex_execution = Some(VerifiedRexExecutionAssociation {
        owner_pid: 42,
        owner_name: "codex".into(),
        process_start_id: native_fixture_start(10),
    });
    target.rex_execution_fence = RexExecutionFence::from_start_id(&native_fixture_start(12));
    let mut actor = rex_value_default(&key, "actor", "actor");
    actor.rex_execution_fence = RexExecutionFence::from_start_id(&native_fixture_start(11));
    actor.replace_rex_holder(&target).unwrap();
    assert_eq!(
        actor.rex_execution_fence,
        RexExecutionFence::from_start_id(&native_fixture_start(12))
    );
    target.rex_execution.as_mut().unwrap().process_start_id = native_fixture_start(13);
    actor.replace_rex_holder(&target).unwrap();
    assert_eq!(
        actor.rex_execution_fence,
        RexExecutionFence::from_start_id(&native_fixture_start(13))
    );
    let mut slot = IdentityBindingSlot::new(key.clone());
    slot.commit(&key, actor.clone(), Some(0)).unwrap();
    let mut advanced = actor.clone();
    advanced.rex_execution_fence = RexExecutionFence::from_start_id(&native_fixture_start(14));
    assert_eq!(
        slot.commit(&key, advanced.clone(), Some(1)).unwrap(),
        CommitOutcome::Committed { generation: 2 }
    );
    assert!(
        slot.commit(&key, actor, Some(1)).is_err(),
        "stale generation cannot overwrite fence"
    );
    assert_eq!(
        slot.commit(&key, advanced.clone(), Some(2)).unwrap(),
        CommitOutcome::Unchanged { generation: 2 }
    );
    advanced.rex_execution_fence = Some(RexExecutionFence::UnixWall {
        seconds: 14,
        microseconds: 1_000_000,
    });
    assert!(slot.commit(&key, advanced, Some(2)).is_err());
}

#[test]
fn rex_human_fence_missing_association_uses_exact_publication_clock() {
    use super::binding_slot::RexExecutionFence;
    let key = key("Alice");
    let target = rex_value_default(&key, "target", "target");
    let mut actor = rex_value_default(&key, "actor", "actor");
    let before = crate::host_runtime::current_rex_execution_boundary().unwrap();
    actor.replace_rex_holder(&target).unwrap();
    let after = crate::host_runtime::current_rex_execution_boundary().unwrap();
    let fence = actor.rex_execution_fence.clone().unwrap();
    assert!(before <= fence && fence <= after);
    let same_holder = actor.clone();
    actor.replace_rex_holder(&same_holder).unwrap();
    assert_eq!(
        actor.rex_execution_fence,
        Some(fence),
        "a same-holder retry does not advance time"
    );
    assert_eq!(RexExecutionFence::from_start_id("10:1000000"), None);
}

#[test]
fn rex_human_replacement_uses_the_current_clock_when_history_is_incomparable() {
    use super::binding_slot::{RexExecutionFence, VerifiedRexExecutionAssociation};
    let key = key("Alice");
    let before = crate::host_runtime::current_rex_execution_boundary().unwrap();
    let foreign = match before {
        RexExecutionFence::UnixWall { .. } => {
            RexExecutionFence::from_start_id("linux:b879bb0a-cf9a-47d1-b889-f63aa872d851:100:700")
        }
        RexExecutionFence::LinuxBoot { .. } => RexExecutionFence::from_start_id("12:7"),
    }
    .unwrap();
    let foreign_start_id = match &foreign {
        RexExecutionFence::UnixWall {
            seconds,
            microseconds,
        } => format!("{seconds}:{microseconds}"),
        RexExecutionFence::LinuxBoot {
            boot_id,
            ticks_per_second,
            ticks,
        } => format!("linux:{boot_id}:{ticks_per_second}:{ticks}"),
    };
    let mut target = rex_value_default(&key, "target", "target");
    target.rex_execution_fence = Some(foreign.clone());
    target.rex_execution = Some(VerifiedRexExecutionAssociation {
        owner_pid: 42,
        owner_name: "codex".into(),
        process_start_id: foreign_start_id,
    });
    let mut actor = rex_value_default(&key, "actor", "actor");
    actor.rex_execution_fence = Some(foreign.clone());

    actor.replace_rex_holder(&target).unwrap();
    let after = crate::host_runtime::current_rex_execution_boundary().unwrap();
    let fence = actor.rex_execution_fence.clone().unwrap();
    assert!(
        fence.compare(&before).is_some() && fence.compare(&after).is_some(),
        "human replacement must establish a boundary in the current clock domain: before={before:?} fence={fence:?} after={after:?}"
    );

    let mut later = actor.clone();
    later.rex_execution_fence = Some(foreign);
    actor.preserve_rex_fence(&later);
    assert_eq!(
        actor.rex_execution_fence,
        Some(fence),
        "ordinary preservation must not replace the new local boundary with an incomparable historical value"
    );
}

#[test]
fn rex_fence_round_trips_legacy_and_linux_clock_domains() {
    use super::binding_slot::RexExecutionFence;

    let legacy = json!({"seconds": 123, "microseconds": 456});
    let legacy_fence: RexExecutionFence = serde_json::from_value(legacy.clone())
        .expect("legacy Unix wall-time fence must remain readable");
    assert_eq!(
        serde_json::to_value(&legacy_fence).unwrap(),
        legacy,
        "macOS must keep its existing wall-time object shape"
    );

    let linux = json!({
        "clock": "linux_boottime",
        "boot_id": "b879bb0a-cf9a-47d1-b889-f63aa872d851",
        "ticks_per_second": 100,
        "ticks": 34771
    });
    let linux_fence: RexExecutionFence = serde_json::from_value(linux.clone())
        .expect("Linux boot-clock fence must have an explicit typed shape");
    assert_eq!(
        serde_json::to_value(&linux_fence).unwrap(),
        linux,
        "Linux fence round-trip must preserve boot ID, rate, and native ticks"
    );
    for invalid in [
        json!({
            "clock": "linux_boottime",
            "boot_id": "boot with spaces",
            "ticks_per_second": 100,
            "ticks": 1
        }),
        json!({
            "clock": "linux_boottime",
            "boot_id": "b879bb0a-cf9a-47d1-b889-f63aa872d851",
            "ticks_per_second": 0,
            "ticks": 1
        }),
        json!({
            "clock": "linux_boottime",
            "boot_id": "b879bb0a-cf9a-47d1-b889-f63aa872d851",
            "ticks_per_second": 100,
            "ticks": 1,
            "extra": true
        }),
    ] {
        assert!(
            serde_json::from_value::<RexExecutionFence>(invalid).is_err(),
            "strict Linux fence fields must reject malformed persisted facts"
        );
    }
    assert_eq!(
        RexExecutionFence::from_start_id("linux:b879bb0a-cf9a-47d1-b889-f63aa872d851:100:1:extra"),
        None
    );
    assert_eq!(RexExecutionFence::from_start_id("linux: boot:100:1"), None);
}

#[test]
fn rex_fence_comparison_does_not_rank_incomparable_executions() {
    use super::binding_slot::RexExecutionFence;
    use std::cmp::Ordering;

    let legacy = RexExecutionFence::from_start_id("12:7").unwrap();
    let linux_low =
        RexExecutionFence::from_start_id("linux:b879bb0a-cf9a-47d1-b889-f63aa872d851:100:700")
            .unwrap();
    let linux_high =
        RexExecutionFence::from_start_id("linux:b879bb0a-cf9a-47d1-b889-f63aa872d851:100:701")
            .unwrap();
    let other_boot =
        RexExecutionFence::from_start_id("linux:11111111-1111-1111-1111-111111111111:100:999")
            .unwrap();
    let other_rate =
        RexExecutionFence::from_start_id("linux:b879bb0a-cf9a-47d1-b889-f63aa872d851:1000:7000")
            .unwrap();

    assert_eq!(
        linux_low.partial_cmp(&linux_high),
        Some(Ordering::Less),
        "same boot and same rate must preserve native tick order"
    );
    assert_eq!(legacy.partial_cmp(&linux_high), None);
    assert_eq!(linux_high.partial_cmp(&other_boot), None);
    assert_eq!(linux_high.partial_cmp(&other_rate), None);
    assert_eq!(
        linux_high.partial_cmp(&RexExecutionFence::from_start_id(
            "linux:b879bb0a-cf9a-47d1-b889-f63aa872d851:100:701"
        )
        .unwrap()),
        Some(Ordering::Equal),
        "equal ticks on one boot are equal as a fence; caller code must still reject equal different executions"
    );
}

#[test]
fn rex_human_fence_survives_endpoint_adaptor_and_source_target_moves() {
    use super::binding_slot::{RexExecutionFence, VerifiedRexExecutionAssociation};
    let key = key("Alice");
    let mut current = rex_value_default(&key, "actor", "runtime");
    current.rex_execution_fence = RexExecutionFence::from_start_id(&native_fixture_start(20));
    current.rex_execution = Some(VerifiedRexExecutionAssociation {
        owner_pid: 42,
        owner_name: "codex".into(),
        process_start_id: native_fixture_start(10),
    });
    let mut same = current.clone();
    same.rex_execution = None;
    same.rex_execution_fence = None;
    same.preserve_rex_fence(&current);
    assert_eq!(
        same.rex_execution, current.rex_execution,
        "same current rematerialization retains association"
    );
    let mut endpoint_move = current.clone();
    endpoint_move
        .transport
        .rex_endpoint
        .as_mut()
        .unwrap()
        .server = OTHER_REX_SERVER.into();
    endpoint_move.rex_execution = None;
    endpoint_move.rex_execution_fence = None;
    endpoint_move.preserve_rex_fence(&current);
    assert_eq!(
        endpoint_move.rex_execution_fence,
        current.rex_execution_fence
    );
    assert!(
        endpoint_move.rex_execution.is_none(),
        "endpoint change is not execution proof"
    );
    let mut native = value(
        &key,
        "actor",
        "native-runtime",
        "unix:///tmp/native-fence.sock",
    );
    native.preserve_rex_fence(&current);
    assert_eq!(native.rex_execution_fence, current.rex_execution_fence);
    assert!(native.rex_execution.is_none());
    let mut returned = rex_value_default(&key, "actor", "returned-runtime");
    returned.preserve_rex_fence(&native);
    assert_eq!(returned.rex_execution_fence, current.rex_execution_fence);
    let mut target = current.clone();
    target.binding.app_scope_id = AppServerId::new("different-target-app").unwrap();
    target.rex_execution.as_mut().unwrap().process_start_id = native_fixture_start(30);
    returned.replace_rex_holder(&target).unwrap();
    assert_eq!(
        returned.rex_execution_fence,
        RexExecutionFence::from_start_id(&native_fixture_start(30))
    );
}

#[test]
fn ordinary_rex_lookup_rejects_non_rex_session_holder_against_rex_target() {
    let alice = key("alice");
    let bob = key("bob");
    let mut control = BindingControlSet::new();
    control
        .commit(
            &alice,
            value(&alice, "alice", "runtime-alice", "endpoint-alice"),
            None,
        )
        .unwrap();
    control
        .commit(
            &bob,
            rex_value(&bob, "bob", "runtime-bob", "bob-session", "block-bob"),
            None,
        )
        .unwrap();
    let facts = IdentityFacts {
        session_id: Some("runtime-alice-session".into()),
        rex: Some(RexCandidate {
            server: "unix:///tmp/rex-test.sock".into(),
            session_id: "rex-session".into(),
            block_id: "block-bob".into(),
            cwd: "/tmp/collab-binding-slot".into(),
        }),
        ..Default::default()
    };

    let error = control
        .lookup(&alice.project_scope, &alice.app_scope_id, &facts, None)
        .unwrap_err();
    assert!(error.starts_with("IDENTITY_ANCHOR_CONFLICT:"), "{error}");
}

#[test]
fn current_rex_anchor_disambiguates_a_reused_agent_session() {
    let alice = key("alice");
    let bob = key("bob");
    let mut control = BindingControlSet::new();
    control
        .commit(
            &alice,
            rex_value(
                &alice,
                "alice",
                "runtime-alice",
                "reused-agent-session",
                "block-alice",
            ),
            None,
        )
        .unwrap();
    control
        .commit(
            &bob,
            rex_value(
                &bob,
                "bob",
                "runtime-bob",
                "reused-agent-session",
                "block-bob",
            ),
            None,
        )
        .unwrap();

    let facts = IdentityFacts {
        session_id: Some("reused-agent-session".into()),
        rex: Some(RexCandidate {
            server: "unix:///tmp/rex-test.sock".into(),
            session_id: "rex-session".into(),
            block_id: "block-bob".into(),
            cwd: "/tmp/collab-binding-slot".into(),
        }),
        ..Default::default()
    };

    let selected = control
        .lookup(&bob.project_scope, &bob.app_scope_id, &facts, None)
        .unwrap()
        .expect("the exact current Rex block disambiguates duplicate session history");
    assert_eq!(selected.identity_key, bob);
}

#[test]
fn current_lookup_rejects_reused_agent_session_without_matching_anchor() {
    let alice = key("alice");
    let bob = key("bob");
    let mut control = BindingControlSet::new();
    control
        .commit(
            &alice,
            rex_value(
                &alice,
                "alice",
                "runtime-alice",
                "reused-agent-session",
                "block-alice",
            ),
            None,
        )
        .unwrap();
    control
        .commit(
            &bob,
            rex_value(
                &bob,
                "bob",
                "runtime-bob",
                "reused-agent-session",
                "block-bob",
            ),
            None,
        )
        .unwrap();
    let before = serde_json::to_value(&control).unwrap();
    let facts = IdentityFacts {
        session_id: Some("reused-agent-session".into()),
        ..Default::default()
    };

    let error = control
        .lookup(&alice.project_scope, &alice.app_scope_id, &facts, None)
        .unwrap_err();
    assert!(error.starts_with("IDENTITY_MAP_AMBIGUOUS:"), "{error}");
    assert_eq!(serde_json::to_value(&control).unwrap(), before);
}

#[test]
fn current_lookup_keeps_anchor_outside_reused_session_matches_ambiguous() {
    let alice = key("alice");
    let bob = key("bob");
    let carol = key("Carol");
    let mut control = BindingControlSet::new();
    for (key, agent, runtime, block) in [
        (&alice, "alice", "runtime-alice", "block-alice"),
        (&bob, "bob", "runtime-bob", "block-bob"),
    ] {
        control
            .commit(
                key,
                rex_value(key, agent, runtime, "reused-agent-session", block),
                None,
            )
            .unwrap();
    }
    control
        .commit(
            &carol,
            rex_value(
                &carol,
                "carol",
                "runtime-carol",
                "carol-session",
                "block-carol",
            ),
            None,
        )
        .unwrap();
    let before = serde_json::to_value(&control).unwrap();
    let facts = IdentityFacts {
        session_id: Some("reused-agent-session".into()),
        rex: Some(RexCandidate {
            server: "unix:///tmp/rex-test.sock".into(),
            session_id: "rex-session".into(),
            block_id: "block-carol".into(),
            cwd: "/tmp/collab-binding-slot".into(),
        }),
        ..Default::default()
    };

    let error = control
        .lookup(&alice.project_scope, &alice.app_scope_id, &facts, None)
        .unwrap_err();
    assert!(error.starts_with("IDENTITY_MAP_AMBIGUOUS:"), "{error}");
    assert_eq!(serde_json::to_value(&control).unwrap(), before);
}

#[test]
fn current_lookup_uses_unique_session_when_a_rex_block_has_seeded_duplicate_currents() {
    let alice = key("alice");
    let bob = key("bob");
    let alice_value = rex_value(
        &alice,
        "alice",
        "runtime-alice",
        "alice-session",
        "shared-block",
    );
    let bob_value = rex_value(&bob, "bob", "runtime-bob", "bob-session", "shared-block");
    let control = BindingControlSet::from_parts(
        vec![
            committed_slot(alice.clone(), alice_value),
            committed_slot(bob, bob_value),
        ],
        Vec::new(),
    );
    let facts = IdentityFacts {
        session_id: Some("alice-session".into()),
        rex: Some(RexCandidate {
            server: "unix:///tmp/rex-test.sock".into(),
            session_id: "rex-session".into(),
            block_id: "shared-block".into(),
            cwd: "/tmp/collab-binding-slot".into(),
        }),
        ..Default::default()
    };

    let selected = control
        .lookup(&alice.project_scope, &alice.app_scope_id, &facts, None)
        .unwrap()
        .expect(
            "the known session repairs the seeded duplicate without choosing an arbitrary owner",
        );
    assert_eq!(selected.identity_key, alice);
}

#[test]
fn current_lookup_rejects_conflicting_unique_session_when_rex_block_is_authoritative() {
    let alice = key("alice");
    let bob = key("bob");
    let mut control = BindingControlSet::new();
    control
        .commit(
            &alice,
            rex_value(
                &alice,
                "alice",
                "runtime-alice",
                "alice-session",
                "block-alice",
            ),
            None,
        )
        .unwrap();
    control
        .commit(
            &bob,
            rex_value(&bob, "bob", "runtime-bob", "bob-session", "block-bob"),
            None,
        )
        .unwrap();
    let before = serde_json::to_value(&control).unwrap();
    let facts = IdentityFacts {
        session_id: Some("bob-session".into()),
        rex: Some(RexCandidate {
            server: "unix:///tmp/rex-test.sock".into(),
            session_id: "rex-session".into(),
            block_id: "block-alice".into(),
            cwd: "/tmp/collab-binding-slot".into(),
        }),
        ..Default::default()
    };

    let error = control
        .lookup(&alice.project_scope, &alice.app_scope_id, &facts, None)
        .unwrap_err();
    assert!(error.starts_with("IDENTITY_ANCHOR_CONFLICT:"), "{error}");
    assert_eq!(serde_json::to_value(&control).unwrap(), before);
}

#[test]
fn current_lookup_matches_the_full_rex_server_and_block() {
    let alice = key("alice");
    let bob = key("bob");
    let mut control = BindingControlSet::new();
    control
        .commit(
            &alice,
            rex_value_on_server(
                &alice,
                "alice",
                "runtime-alice",
                "alice-session",
                "shared-block",
                "unix:///tmp/rex-a.sock",
            ),
            None,
        )
        .unwrap();
    control
        .commit(
            &bob,
            rex_value_on_server(
                &bob,
                "bob",
                "runtime-bob",
                "bob-session",
                "shared-block",
                "unix:///tmp/rex-b.sock",
            ),
            None,
        )
        .unwrap();
    let facts = IdentityFacts {
        session_id: Some("alice-session".into()),
        rex: Some(RexCandidate {
            server: "unix:///tmp/rex-a.sock".into(),
            session_id: "rex-session".into(),
            block_id: "shared-block".into(),
            cwd: "/tmp/collab-binding-slot".into(),
        }),
        ..Default::default()
    };

    let selected = control
        .lookup(&alice.project_scope, &alice.app_scope_id, &facts, None)
        .unwrap()
        .expect("the full server scope selects only the matching principal");
    assert_eq!(selected.identity_key, alice);
}

#[test]
fn current_lookup_does_not_match_same_tmux_session_id_on_a_different_socket() {
    let alice = key("alice");
    let mut control = BindingControlSet::new();
    control
        .commit(
            &alice,
            tmux_value_on_socket(
                &alice,
                "alice",
                "runtime-alice",
                "/tmp/tmux-a.sock",
                "$1",
                "%1",
            ),
            None,
        )
        .unwrap();
    let facts = IdentityFacts {
        tmux: Some(crate::proto::TmuxCandidate {
            endpoint: crate::proto::TmuxEndpoint {
                socket_path: "/tmp/tmux-b.sock".into(),
                server_pid: 12,
                tmux_session_id: "$1".into(),
                pane_id: "%1".into(),
                pane_pid: 23,
                codex_session_id: None,
                codex_thread_id: None,
            },
            cwd: alice.project_scope.as_str().into(),
        }),
        ..Default::default()
    };

    assert!(
        control
            .lookup(&alice.project_scope, &alice.app_scope_id, &facts, None)
            .unwrap()
            .is_none(),
        "a tmux session id is local to its socket and cannot recover another socket's identity"
    );
}

#[test]
fn current_lookup_keeps_same_tmux_session_id_on_the_same_socket() {
    let alice = key("alice");
    let mut control = BindingControlSet::new();
    control
        .commit(
            &alice,
            tmux_value_on_socket(
                &alice,
                "alice",
                "runtime-alice",
                "/tmp/tmux-a.sock",
                "$1",
                "%1",
            ),
            None,
        )
        .unwrap();
    let facts = IdentityFacts {
        tmux: Some(crate::proto::TmuxCandidate {
            endpoint: crate::proto::TmuxEndpoint {
                socket_path: "/tmp/tmux-a.sock".into(),
                server_pid: 12,
                tmux_session_id: "$1".into(),
                pane_id: "%9".into(),
                pane_pid: 23,
                codex_session_id: None,
                codex_thread_id: None,
            },
            cwd: alice.project_scope.as_str().into(),
        }),
        ..Default::default()
    };

    let selected = control
        .lookup(&alice.project_scope, &alice.app_scope_id, &facts, None)
        .unwrap()
        .expect("the same socket and tmux session preserve the identity anchor");
    assert_eq!(selected.identity_key, alice);
}

#[test]
fn current_lookup_allows_same_subject_to_move_to_a_new_rex_block() {
    let alice = key("alice");
    let mut control = BindingControlSet::new();
    control
        .commit(
            &alice,
            rex_value(
                &alice,
                "alice",
                "runtime-alice",
                "agent-session",
                "old-block",
            ),
            None,
        )
        .unwrap();
    let facts = IdentityFacts {
        session_id: Some("agent-session".into()),
        rex: Some(RexCandidate {
            server: "unix:///tmp/rex-test.sock".into(),
            session_id: "rex-session".into(),
            block_id: "new-block".into(),
            cwd: "/tmp/collab-binding-slot".into(),
        }),
        ..Default::default()
    };

    assert_eq!(
        control
            .lookup(&alice.project_scope, &alice.app_scope_id, &facts, None)
            .unwrap()
            .unwrap()
            .identity_key,
        alice
    );
}

#[test]
fn current_lookup_rejects_two_matching_names_without_mutation_or_cross_scope_match() {
    let alice = key("alice");
    let bob = key("bob");
    let mut control = BindingControlSet::new();
    control
        .commit(&alice, value(&alice, "carrier", "shared", "endpoint"), None)
        .unwrap();
    let facts = crate::proto::IdentityFacts {
        session_id: Some("shared-session".into()),
        ..Default::default()
    };
    assert_eq!(
        control
            .lookup(&alice.project_scope, &alice.app_scope_id, &facts, None)
            .unwrap()
            .unwrap()
            .identity_key,
        alice,
    );
    control
        .commit(&bob, value(&bob, "carrier", "shared", "endpoint"), None)
        .unwrap();
    let before = serde_json::to_value(&control).unwrap();
    let error = control
        .lookup(&alice.project_scope, &alice.app_scope_id, &facts, None)
        .unwrap_err();
    assert!(error.starts_with("IDENTITY_MAP_AMBIGUOUS:"), "{error}");
    assert_eq!(serde_json::to_value(&control).unwrap(), before);
    assert!(control
        .lookup(
            &ProjectScopeId::new("/tmp/unrelated-project").unwrap(),
            &alice.app_scope_id,
            &facts,
            None,
        )
        .unwrap()
        .is_none());
    assert!(control
        .lookup(
            &alice.project_scope,
            &AppServerId::new("other-app").unwrap(),
            &facts,
            None,
        )
        .unwrap()
        .is_none());
}

#[test]
fn rex_lookup_continues_a_unique_current_block_with_a_new_agent_session() {
    let key = key("alice");
    let mut control = BindingControlSet::new();
    control
        .commit(&key, rex_value_default(&key, "alice-agent", "old"), None)
        .unwrap();

    let facts = rex_facts(
        Some("new-agent-session"),
        REX_SERVER,
        REX_SESSION,
        REX_BLOCK,
    );
    let found = control
        .lookup(&key.project_scope, &key.app_scope_id, &facts, None)
        .unwrap()
        .expect("the unique current Rex block should continue Alice");
    assert_eq!(found.identity_key, key);
}

#[test]
fn rex_lookup_continues_the_same_agent_session_on_a_new_target() {
    let key = key("alice");
    let mut control = BindingControlSet::new();
    control
        .commit(&key, rex_value_default(&key, "alice-agent", "old"), None)
        .unwrap();
    let saved_agent_session = "old-agent-session";

    // The agent session is the worker carrier. A new server, Rex session or
    // block is an endpoint/anchor move for the same worker, so the name must
    // continue instead of being rejected by a blanket endpoint mismatch.
    for (server, session, block) in [
        (OTHER_REX_SERVER, REX_SESSION, REX_BLOCK),
        (
            REX_SERVER,
            "session:88888888-8888-8888-8888-888888888888",
            REX_BLOCK,
        ),
        (REX_SERVER, REX_SESSION, OTHER_REX_BLOCK),
    ] {
        let facts = rex_facts(Some(saved_agent_session), server, session, block);
        let found = control
            .lookup(&key.project_scope, &key.app_scope_id, &facts, None)
            .unwrap()
            .unwrap_or_else(|| panic!("{server}/{session}/{block} should continue Alice"));
        assert_eq!(found.identity_key, key);
    }

    // Without the saved agent session there is no conflicting current fact, so
    // the caller is treated as a new registration target.
    let unregistered = rex_facts(None, REX_SERVER, REX_SESSION, OTHER_REX_BLOCK);
    assert!(control
        .lookup(&key.project_scope, &key.app_scope_id, &unregistered, None)
        .unwrap()
        .is_none());
}

#[test]
fn rex_lookup_continues_the_same_agent_session_across_adaptors() {
    let bob = key("bob");
    let mut control = BindingControlSet::new();
    control
        .commit(
            &bob,
            value(&bob, "bob-agent", "bob-runtime", "bob-endpoint"),
            None,
        )
        .unwrap();

    // The same worker session moves from a non-Rex adaptor into a new Rex
    // block. The current map still owns that session, so B2 continues Bob.
    let facts = rex_facts(
        Some("bob-runtime-session"),
        REX_SERVER,
        REX_SESSION,
        REX_BLOCK,
    );
    let found = control
        .lookup(&bob.project_scope, &bob.app_scope_id, &facts, None)
        .unwrap()
        .expect("the session holder continues across adaptors");
    assert_eq!(found.identity_key, bob);
}

#[test]
fn rex_lookup_reports_ambiguity_for_multiple_current_slots_on_one_block() {
    let alice = key("alice");
    let bob = key("bob");
    let control = BindingControlSet::from_parts(
        vec![
            committed_slot(
                alice.clone(),
                rex_value_default(&alice, "alice-agent", "alice"),
            ),
            committed_slot(bob.clone(), rex_value_default(&bob, "bob-agent", "bob")),
        ],
        Vec::new(),
    );

    let facts = rex_facts(
        Some("new-agent-session"),
        REX_SERVER,
        REX_SESSION,
        REX_BLOCK,
    );
    let error = control
        .lookup(&alice.project_scope, &alice.app_scope_id, &facts, None)
        .unwrap_err();
    assert!(error.starts_with("IDENTITY_MAP_AMBIGUOUS:"), "{error}");
}

#[test]
fn rex_lookup_uses_the_exact_current_session_when_names_share_one_block() {
    let alice = key("alice");
    let bob = key("bob");
    let mut control = BindingControlSet::new();
    control
        .commit(
            &alice,
            rex_value_default(&alice, "alice-agent", "alice"),
            None,
        )
        .unwrap();
    let bob_value = rex_value_default(&bob, "bob-agent", "bob");
    let bob_session = bob_value.binding.session_id.as_ref().unwrap().to_string();
    control.commit(&bob, bob_value, None).unwrap();
    let before = serde_json::to_value(&control).unwrap();
    let facts = rex_facts(Some(&bob_session), REX_SERVER, REX_SESSION, REX_BLOCK);
    let found = control
        .lookup(&bob.project_scope, &bob.app_scope_id, &facts, None)
        .unwrap()
        .expect("the exact current session disambiguates its name");
    assert_eq!(found.identity_key, bob);
    assert_eq!(serde_json::to_value(&control).unwrap(), before);
}

#[test]
fn rex_lookup_rejects_session_holder_against_current_block_anchor() {
    let alice = key("alice");
    let bob = key("bob");
    let mut control = BindingControlSet::new();
    control
        .commit(
            &alice,
            rex_value_default(&alice, "alice-agent", "alice"),
            None,
        )
        .unwrap();
    control
        .commit(
            &bob,
            value(&bob, "bob-agent", "bob-runtime", "bob-endpoint"),
            None,
        )
        .unwrap();

    // The caller's Rex block selects Alice, while the agent session selects Bob.
    // A normal lookup must reject this cross-subject conflict before publication.
    let facts = rex_facts(
        Some("bob-runtime-session"),
        REX_SERVER,
        REX_SESSION,
        REX_BLOCK,
    );
    let error = control
        .lookup(&alice.project_scope, &alice.app_scope_id, &facts, None)
        .unwrap_err();
    assert!(error.starts_with("IDENTITY_ANCHOR_CONFLICT:"), "{error}");
}

#[test]
fn rex_lookup_rejects_session_holder_when_the_target_block_belongs_to_another_identity() {
    let alice = key("alice");
    let bob = key("bob");
    let mut control = BindingControlSet::new();
    control
        .commit(
            &alice,
            rex_value_default(&alice, "alice-agent", "alice"),
            None,
        )
        .unwrap();
    control
        .commit(
            &bob,
            value(&bob, "bob-agent", "bob-runtime", "bob-endpoint"),
            None,
        )
        .unwrap();

    let facts = rex_facts(
        Some("bob-runtime-session"),
        REX_SERVER,
        REX_SESSION,
        REX_BLOCK,
    );
    let error = control
        .lookup(&alice.project_scope, &alice.app_scope_id, &facts, None)
        .unwrap_err();
    assert!(error.starts_with("IDENTITY_ANCHOR_CONFLICT:"), "{error}");
}

#[test]
fn three_distinct_bindings_keep_only_current_and_previous() {
    let key = key("alice");
    let mut slot = IdentityBindingSlot::new(key.clone());
    slot.commit(
        &key,
        value(&key, "alice", "runtime-a", "endpoint-a"),
        Some(0),
    )
    .unwrap();
    slot.commit(
        &key,
        value(&key, "alice", "runtime-b", "endpoint-b"),
        Some(1),
    )
    .unwrap();
    slot.commit(
        &key,
        value(&key, "alice", "runtime-c", "endpoint-c"),
        Some(2),
    )
    .unwrap();

    let current = slot.active_binding().expect("current binding");
    let previous = slot.reference_previous().expect("previous binding");
    assert_eq!(current.binding.runtime_id.as_str(), "runtime-c");
    assert_eq!(previous.binding.runtime_id.as_str(), "runtime-b");
    assert_eq!(slot.generation(), 3);
    slot.validate().unwrap();
}

#[test]
fn repeated_binding_fact_does_not_roll_or_advance_generation() {
    let key = key("alice");
    let mut slot = IdentityBindingSlot::new(key.clone());
    slot.commit(
        &key,
        value(&key, "alice", "runtime-a", "endpoint-a"),
        Some(0),
    )
    .unwrap();

    let mut repeated = value(&key, "alice", "runtime-a", "endpoint-a");
    repeated.transport.capabilities = vec!["changed".to_owned()];
    repeated.transport.self_check = "changed self check".to_owned();

    assert_eq!(
        slot.commit(&key, repeated, Some(1)).unwrap(),
        CommitOutcome::Unchanged { generation: 1 }
    );
    assert_eq!(slot.generation(), 1);
    assert!(slot.reference_previous().is_none());
    slot.validate().unwrap();
}

#[test]
fn stable_name_keeps_one_slot_across_session_derived_agents() {
    let key = key("alice");
    let mut set = BindingControlSet::new();

    // S1 and S2 carry different session-derived agent ids and transports but
    // the same stable name. They must collapse into one logical slot with the
    // newest carrier as current and the previous carrier retained once.
    set.commit(
        &key,
        value(&key, "alice", "runtime-a", "endpoint-a"),
        Some(0),
    )
    .unwrap();
    assert_eq!(
        set.commit(
            &key,
            value(&key, "carrier-s2", "runtime-b", "endpoint-b"),
            Some(1),
        )
        .unwrap(),
        CommitOutcome::Committed { generation: 2 }
    );

    assert_eq!(set.slots().count(), 1);
    let slot = set.slot(&key).unwrap();
    assert_eq!(slot.generation(), 2);
    assert_eq!(
        slot.active_binding().unwrap().binding.agent_id.as_str(),
        "carrier-s2"
    );
    assert_eq!(
        slot.reference_previous().unwrap().binding.agent_id.as_str(),
        "alice"
    );
    set.validate().unwrap();
}

#[test]
fn new_session_replaces_the_current_carrier_without_an_endpoint_counter() {
    let key = key("alice");
    let mut slot = IdentityBindingSlot::new(key.clone());
    slot.commit(
        &key,
        value(&key, "carrier-s1", "runtime-a", "endpoint-a"),
        Some(0),
    )
    .unwrap();
    slot.commit(
        &key,
        value(&key, "carrier-s2", "runtime-b", "endpoint-b"),
        Some(1),
    )
    .unwrap();
    assert_eq!(slot.generation(), 2);
    assert_eq!(
        slot.active_binding().unwrap().binding.agent_id.as_str(),
        "carrier-s2"
    );
    assert_eq!(
        slot.reference_previous().unwrap().binding.agent_id.as_str(),
        "carrier-s1"
    );
    slot.validate().unwrap();
}

#[test]
fn returning_to_a_previous_address_rolls_instead_of_swapping() {
    let key = key("alice");
    let mut slot = IdentityBindingSlot::new(key.clone());
    slot.commit(
        &key,
        value(&key, "alice", "runtime-a", "endpoint-a"),
        Some(0),
    )
    .unwrap();
    slot.commit(
        &key,
        value(&key, "alice", "runtime-b", "endpoint-b"),
        Some(1),
    )
    .unwrap();
    slot.commit(
        &key,
        value(&key, "alice", "runtime-c", "endpoint-a"),
        Some(2),
    )
    .unwrap();

    assert_eq!(
        slot.active_binding().unwrap().binding.runtime_id.as_str(),
        "runtime-c"
    );
    assert_eq!(
        slot.reference_previous()
            .unwrap()
            .binding
            .runtime_id
            .as_str(),
        "runtime-b"
    );
    assert_eq!(slot.generation(), 3);
    slot.validate().unwrap();
}

#[test]
fn stale_expected_generation_is_rejected_without_mutation() {
    let key = key("alice");
    let mut slot = IdentityBindingSlot::new(key.clone());
    slot.commit(
        &key,
        value(&key, "alice", "runtime-a", "endpoint-a"),
        Some(0),
    )
    .unwrap();

    assert_eq!(
        slot.commit(
            &key,
            value(&key, "alice", "runtime-b", "endpoint-b"),
            Some(0)
        ),
        Err(SlotError::GenerationConflict {
            expected: 1,
            observed: 0
        })
    );
    assert_eq!(slot.generation(), 1);
    assert_eq!(
        slot.active_binding().unwrap().binding.runtime_id.as_str(),
        "runtime-a"
    );
    assert!(slot.reference_previous().is_none());
}

#[test]
fn unregister_moves_current_to_previous_and_is_idempotent() {
    let key = key("alice");
    let mut slot = IdentityBindingSlot::new(key.clone());
    slot.commit(
        &key,
        value(&key, "alice", "runtime-a", "endpoint-a"),
        Some(0),
    )
    .unwrap();

    assert_eq!(
        slot.unregister(Some(1)).unwrap(),
        CommitOutcome::Committed { generation: 2 }
    );
    assert_eq!(slot.identity_status(), IdentityStatus::Unregistered);
    assert!(slot.active_binding().is_none());
    assert_eq!(
        slot.reference_previous()
            .unwrap()
            .binding
            .runtime_id
            .as_str(),
        "runtime-a"
    );

    assert_eq!(
        slot.unregister(Some(2)).unwrap(),
        CommitOutcome::Unchanged { generation: 2 }
    );
    assert_eq!(slot.generation(), 2);
    assert!(slot.reference_previous().is_some());
    slot.validate().unwrap();
}

#[test]
fn rebuild_keeps_the_registration_before_close_as_previous() {
    let key = key("alice");
    let mut slot = IdentityBindingSlot::new(key.clone());
    slot.commit(
        &key,
        value(&key, "alice", "runtime-a", "endpoint-a"),
        Some(0),
    )
    .unwrap();
    slot.unregister(Some(1)).unwrap();
    slot.commit(
        &key,
        value(&key, "carrier-s2", "runtime-b", "endpoint-b"),
        Some(2),
    )
    .unwrap();

    assert_eq!(slot.identity_status(), IdentityStatus::Active);
    assert_eq!(
        slot.active_binding().unwrap().binding.runtime_id.as_str(),
        "runtime-b"
    );
    assert_eq!(
        slot.reference_previous()
            .unwrap()
            .binding
            .runtime_id
            .as_str(),
        "runtime-a"
    );
    assert_eq!(slot.generation(), 3);
    slot.validate().unwrap();
}

#[test]
fn rebuild_new_carrier_after_retirement_keeps_last_binding() {
    let key = key("alice");
    let mut slot = IdentityBindingSlot::new(key.clone());
    slot.commit(
        &key,
        value(&key, "alice", "runtime-a", "endpoint-a"),
        Some(0),
    )
    .unwrap();
    slot.unregister(Some(1)).unwrap();
    assert_eq!(
        slot.commit(
            &key,
            value(&key, "carrier-s2", "runtime-b", "endpoint-b"),
            Some(2)
        )
        .unwrap(),
        CommitOutcome::Committed { generation: 3 }
    );
    assert_eq!(
        slot.reference_previous().unwrap().binding.agent_id.as_str(),
        "alice"
    );
    slot.validate().unwrap();
}

#[test]
fn a_value_from_a_different_scope_cannot_be_committed_into_the_slot() {
    let key = key_in("/tmp/collab-binding-slot", "alice");
    let other = key_in("/tmp/collab-binding-other", "alice");
    let mut slot = IdentityBindingSlot::new(key.clone());
    let before = slot.clone();

    assert!(matches!(
        slot.commit(
            &key,
            value(&other, "alice", "runtime-b", "endpoint-b"),
            Some(0)
        ),
        Err(SlotError::KeyMismatch { .. })
    ));
    assert_eq!(slot, before);
}

#[test]
fn slot_round_trips_through_json() {
    let key = key("alice");
    let mut slot = IdentityBindingSlot::new(key.clone());
    slot.commit(
        &key,
        value(&key, "alice", "runtime-a", "endpoint-a"),
        Some(0),
    )
    .unwrap();
    let encoded = serde_json::to_string(&slot).unwrap();
    let decoded: IdentityBindingSlot = serde_json::from_str(&encoded).unwrap();
    assert_eq!(decoded, slot);
}

#[test]
fn control_set_keeps_identity_slots_independent() {
    let alice = key("alice");
    let bob = key("bob");
    let mut set = BindingControlSet::new();
    set.commit(
        &alice,
        value(&alice, "alice", "runtime-a", "endpoint-a"),
        Some(0),
    )
    .unwrap();
    set.commit(&bob, value(&bob, "bob", "runtime-b", "endpoint-b"), Some(0))
        .unwrap();

    let active = set.active_values().collect::<Vec<_>>();
    assert_eq!(active.len(), 2);
    set.unregister(&alice, Some(1)).unwrap();

    let active = set.active_values().collect::<Vec<_>>();
    assert_eq!(active.len(), 1);
    assert_eq!(active[0].binding.agent_id.as_str(), "bob");
    assert_eq!(
        set.slot(&alice)
            .unwrap()
            .reference_previous()
            .unwrap()
            .binding
            .agent_id
            .as_str(),
        "alice"
    );
    set.validate().unwrap();
}

#[test]
fn control_set_stale_update_does_not_clobber_a_sibling_identity() {
    let alice = key("alice");
    let bob = key("bob");
    let mut set = BindingControlSet::new();
    set.commit(
        &alice,
        value(&alice, "alice", "runtime-a", "endpoint-a"),
        Some(0),
    )
    .unwrap();
    set.commit(&bob, value(&bob, "bob", "runtime-b", "endpoint-b"), Some(0))
        .unwrap();

    assert!(matches!(
        set.commit(
            &alice,
            value(&alice, "alice", "runtime-a2", "endpoint-a2"),
            Some(0)
        ),
        Err(SlotError::GenerationConflict { .. })
    ));

    assert_eq!(
        set.slot(&alice)
            .unwrap()
            .active_binding()
            .unwrap()
            .binding
            .runtime_id
            .as_str(),
        "runtime-a"
    );
    assert_eq!(
        set.slot(&bob)
            .unwrap()
            .active_binding()
            .unwrap()
            .binding
            .runtime_id
            .as_str(),
        "runtime-b"
    );
    set.validate().unwrap();
}

#[test]
fn remove_scope_drops_only_the_requested_project() {
    let alice = key_in("/tmp/collab-binding-scope-a", "alice");
    let bob = key_in("/tmp/collab-binding-scope-b", "bob");
    let mut set = BindingControlSet::new();
    set.commit(
        &alice,
        value(&alice, "alice", "runtime-a", "endpoint-a"),
        Some(0),
    )
    .unwrap();
    set.commit(&bob, value(&bob, "bob", "runtime-b", "endpoint-b"), Some(0))
        .unwrap();

    let removed = set.remove_scope("/tmp/collab-binding-scope-a");
    assert_eq!(removed, 1);
    assert!(set.slot(&alice).is_none());
    assert!(set.slot(&bob).is_some());
    set.validate().unwrap();
}

#[test]
fn move_name_retires_the_source_slot_and_keeps_the_displaced_holder_as_previous() {
    let alice = key("alice");
    let bob = key("bob");
    let mut set = BindingControlSet::new();
    set.commit(
        &alice,
        value(&alice, "alice-s1", "runtime-a", "endpoint-a"),
        Some(0),
    )
    .unwrap();
    set.commit(
        &bob,
        value(&bob, "bob-s1", "runtime-b", "endpoint-b"),
        Some(0),
    )
    .unwrap();

    let moved = set
        .move_name(
            &alice,
            value(&alice, "bob-s2", "runtime-c", "endpoint-c"),
            &[bob.name.clone()],
            Some(1),
        )
        .unwrap();
    assert_eq!(moved.sources_retired, 1);
    assert_eq!(moved.target, CommitOutcome::Committed { generation: 2 });

    let alice_slot = set.slot(&alice).unwrap();
    assert_eq!(
        alice_slot
            .active_binding()
            .unwrap()
            .binding
            .agent_id
            .as_str(),
        "bob-s2"
    );
    assert_eq!(
        alice_slot
            .reference_previous()
            .unwrap()
            .binding
            .agent_id
            .as_str(),
        "alice-s1"
    );
    let bob_slot = set.slot(&bob).unwrap();
    assert_eq!(bob_slot.identity_status(), IdentityStatus::Unregistered);
    assert!(
        bob_slot.active_binding().is_none(),
        "the retired source slot must not expose a current binding"
    );
    assert_eq!(
        bob_slot
            .reference_previous()
            .unwrap()
            .binding
            .agent_id
            .as_str(),
        "bob-s1"
    );
    set.validate().unwrap();
}

#[test]
fn move_name_does_not_retire_the_target_when_source_equals_target() {
    let alice = key("alice");
    let mut set = BindingControlSet::new();
    set.commit(
        &alice,
        value(&alice, "alice-s1", "runtime-a", "endpoint-a"),
        Some(0),
    )
    .unwrap();

    let moved = set
        .move_name(
            &alice,
            value(&alice, "alice-s2", "runtime-b", "endpoint-b"),
            &[alice.name.clone()],
            Some(1),
        )
        .unwrap();
    assert_eq!(moved.sources_retired, 0);
    assert!(set.slot(&alice).is_some());
    set.validate().unwrap();
}

#[test]
fn rex_block_commit_retires_the_previous_current_and_persists_cleanup() {
    let alice = key("alice");
    let bob = key("bob");
    let mut set = BindingControlSet::new();
    set.commit(
        &alice,
        rex_value(&alice, "alice", "runtime-a", "alice-session", "block-a"),
        None,
    )
    .unwrap();
    set.commit(
        &bob,
        rex_value(&bob, "bob", "runtime-b", "bob-session", "block-b"),
        None,
    )
    .unwrap();

    let winner = rex_value(&alice, "alice", "runtime-c", "alice-session", "block-b");
    set.commit(&alice, winner.clone(), Some(1)).unwrap();

    assert_eq!(set.slot(&alice).unwrap().active_binding(), Some(&winner));
    let bob_slot = set.slot(&bob).unwrap();
    assert_eq!(bob_slot.identity_status(), IdentityStatus::Unregistered);
    assert_eq!(
        bob_slot
            .reference_previous()
            .unwrap()
            .transport
            .rex_endpoint
            .as_ref()
            .unwrap()
            .block_id,
        "block-b"
    );
    assert_eq!(
        set.active_values()
            .filter(|value| value.transport.kind == TransportKind::Rex
                && value
                    .transport
                    .rex_endpoint
                    .as_ref()
                    .is_some_and(|endpoint| endpoint.block_id == "block-b"))
            .count(),
        1
    );
    set.validate().unwrap();
}

#[test]
fn unchanged_rex_winner_still_retires_a_seeded_duplicate_current() {
    let alice = key("alice");
    let bob = key("bob");
    let winner = rex_value(
        &alice,
        "alice",
        "runtime-a",
        "alice-session",
        "shared-block",
    );
    let duplicate = rex_value(&bob, "bob", "runtime-b", "bob-session", "shared-block");
    let mut set = BindingControlSet::from_parts(
        vec![
            committed_slot(alice.clone(), winner.clone()),
            committed_slot(bob.clone(), duplicate),
        ],
        vec![
            current_effect(
                &alice,
                "alice-name",
                PendingEffectKind::Name,
                "alice",
                "alice-session",
            ),
            current_effect(
                &bob,
                "bob-register",
                PendingEffectKind::Register,
                "bob",
                "bob-session",
            ),
            current_effect(
                &bob,
                "bob-unrelated-name",
                PendingEffectKind::Name,
                "other-worker",
                "other-session",
            ),
        ],
    );

    assert_eq!(
        set.commit(&alice, winner.clone(), None).unwrap(),
        CommitOutcome::Unchanged { generation: 1 }
    );
    assert_eq!(set.slot(&alice).unwrap().active_binding(), Some(&winner));
    assert_eq!(
        set.slot(&bob).unwrap().identity_status(),
        IdentityStatus::Unregistered
    );
    assert!(set.pending_effect("alice-name").is_some());
    assert!(set.pending_effect("bob-register").is_none());
    assert!(set.pending_effect("bob-unrelated-name").is_some());
    set.validate().unwrap();
}

#[test]
fn stale_rex_generation_cannot_restore_an_old_block() {
    let alice = key("alice");
    let mut set = BindingControlSet::new();
    let old = rex_value(&alice, "alice", "runtime-a", "alice-session", "old-block");
    let new = rex_value(&alice, "alice", "runtime-b", "alice-session", "new-block");
    set.commit(&alice, old.clone(), Some(0)).unwrap();
    set.commit(&alice, new.clone(), Some(1)).unwrap();

    let error = set.commit(&alice, old, Some(1)).unwrap_err();
    assert!(
        matches!(
            error,
            SlotError::GenerationConflict {
                expected: 2,
                observed: 1
            }
        ),
        "{error}"
    );
    assert_eq!(set.slot(&alice).unwrap().active_binding(), Some(&new));
}

#[test]
fn rex_publication_rejects_an_incomplete_current_without_mutation() {
    let alice = key("alice");
    let mut set = BindingControlSet::new();
    let mut incomplete = rex_value(&alice, "alice", "runtime-a", "alice-session", "block-a");
    incomplete
        .transport
        .rex_endpoint
        .as_mut()
        .unwrap()
        .block_id
        .clear();

    let error = set.commit(&alice, incomplete, Some(0)).unwrap_err();
    assert!(
        matches!(error, SlotError::Invalid(ref message) if message.contains("requires a complete server, session and block")),
        "{error}"
    );
    assert_eq!(set, BindingControlSet::new());
}

#[test]
fn control_set_round_trips_and_rejects_duplicate_keys() {
    let alice = key("alice");
    let mut set = BindingControlSet::new();
    set.commit(
        &alice,
        value(&alice, "alice", "runtime-a", "endpoint-a"),
        Some(0),
    )
    .unwrap();
    let encoded = serde_json::to_string(&set).unwrap();
    let decoded: BindingControlSet = serde_json::from_str(&encoded).unwrap();
    assert_eq!(decoded, set);

    let single = serde_json::to_value(&IdentityBindingSlot::new(alice.clone())).unwrap();
    let duplicate: BindingControlSet = serde_json::from_value(serde_json::json!({
        "slots": [single, single],
    }))
    .unwrap();
    assert!(matches!(
        duplicate.validate(),
        Err(SlotError::Invariant(message)) if message.contains("duplicate identity key")
    ));
}

#[test]
fn pool_overflow_skips_occupied_peer_numbers_and_reuses_the_current_one() {
    let project = ProjectScopeId::new("/tmp/collab-binding-slot").unwrap();
    let app = AppServerId::new("appserver-cli").unwrap();
    let other_app = AppServerId::new("other-app").unwrap();
    let mut set = BindingControlSet::new();
    for (index, name) in super::name_resolution::ORDINARY_NAME_POOL
        .iter()
        .enumerate()
    {
        let key = IdentityBindingKey::new(project.clone(), app.clone(), *name);
        set.commit(
            &key,
            value(
                &key,
                &format!("ordinary-{index}"),
                &format!("runtime-{index}"),
                &format!("endpoint-{index}"),
            ),
            Some(0),
        )
        .unwrap();
    }
    let occupied = IdentityBindingKey::new(project.clone(), other_app, "peer-1");
    set.commit(
        &occupied,
        value(
            &occupied,
            "other-app-participant",
            "other-runtime",
            "other-endpoint",
        ),
        Some(0),
    )
    .unwrap();

    assert_eq!(
        set.ordinary_name(&project, &app, "generated-peer").unwrap(),
        "peer-2"
    );

    let current = IdentityBindingKey::new(project.clone(), app, "peer-2");
    set.commit(
        &current,
        value(
            &current,
            "generated-peer",
            "generated-runtime",
            "generated-endpoint",
        ),
        Some(0),
    )
    .unwrap();
    assert_eq!(
        set.ordinary_name(
            &current.project_scope,
            &current.app_scope_id,
            "generated-peer"
        )
        .unwrap(),
        "peer-2"
    );
}

#[test]
fn generated_peer_slots_load_with_or_without_a_legacy_display_number() {
    let key = key("peer-1");
    let slot = IdentityBindingSlot::new(key.clone());
    assert_eq!(slot.display_slot, None);
    slot.validate().unwrap();

    let mut encoded = serde_json::to_value(slot).unwrap();
    encoded["display_slot"] = json!(22);
    let legacy: IdentityBindingSlot = serde_json::from_value(encoded).unwrap();
    assert_eq!(legacy.display_slot, Some(22));
    legacy.validate().unwrap();
}
