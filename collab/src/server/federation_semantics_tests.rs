use super::*;
use crate::proto::{
    FederationActor, FederationDestination, FederationPrincipal, FederationPut, FederationResponse,
    LocalNameAddress, OriginMessageRef, QualifiedNameAddress, ReplyPolicy,
};

fn accept(delivery: &str, reply_policy: Option<ReplyPolicy>) -> (Server, PathBuf, String) {
    let (server, root) = peer_tests::test_server();
    assert!(peer_tests::register_synthetic_peer(&server, "recipient", "recipient-thread").ok);
    server
        .state
        .lock()
        .unwrap()
        .notification_subscriptions
        .clear();
    let route = server_route_scope(&server, &server.state.lock().unwrap())
        .unwrap()
        .unwrap();
    let address = LocalNameAddress {
        project: canonical_project_key(&root).unwrap(),
        name: "recipient".into(),
    };
    let recipient = federation::ResolvedRecipient {
        worker_id: "recipient".into(),
        address: address.clone(),
        principal: FederationPrincipal {
            address: QualifiedNameAddress {
                machine: "beta".into(),
                local: address.clone(),
            },
            project_scope: route.project_scope_id,
            app_scope_id: route.app_scope_id,
            stable_name: "recipient".into(),
        },
    };
    let origin = OriginMessageRef {
        machine: "alpha".into(),
        project_scope: ProjectScopeId::new("/remote/source").unwrap(),
        app_scope_id: AppServerId::new("remote-app").unwrap(),
        operation_id: crate::identity::OperationId::new(gen_msg_id()).unwrap(),
    };
    let request = FederationPut {
        sender: FederationActor::Peer(FederationPrincipal {
            address: QualifiedNameAddress {
                machine: "alpha".into(),
                local: LocalNameAddress {
                    project: "source".into(),
                    name: "Alice".into(),
                },
            },
            project_scope: origin.project_scope.clone(),
            app_scope_id: origin.app_scope_id.clone(),
            stable_name: "Alice".into(),
        }),
        origin,
        target: FederationDestination::Named(address),
        mtype: "message".into(),
        subject: "contract".into(),
        body: "original body".into(),
        delivery: delivery.into(),
        target_session: None,
        in_reply_to: None,
        original_origin: None,
        reply_policy,
        channel: None,
    };
    let response = accept_federated_message(
        &server,
        &"beta".into(),
        &federation::AuthenticatedMachine {
            source: "alpha".into(),
            target: "beta".into(),
        },
        &request,
        &recipient,
        now_ms(),
    );
    let FederationResponse::Message { receipt } = response else {
        panic!("acceptance failed: {response:?}");
    };
    (server, root, receipt.reference.msg_id.to_string())
}

#[test]
fn queued_delivery_keeps_the_declared_mode() {
    let (server, root, id) = accept("queued", None);
    assert_eq!(server.state.lock().unwrap().delivery_modes[&id], "queued");
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn unread_federated_message_follows_the_stable_recipient_name() {
    let (server, root, id) = accept("immediate", None);
    let state = server.state.lock().unwrap();
    let address = state
        .mailbox_name_addresses
        .get(&id)
        .expect("stable name must own the mailbox row");
    assert_eq!(address.name, "recipient");
    assert_eq!(
        address.project_scope,
        root.canonicalize().unwrap().to_string_lossy()
    );
    drop(state);
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn required_reply_has_an_obligation_with_the_real_remote_sender() {
    let (server, root, id) = accept("immediate", Some(ReplyPolicy::Required));
    let state = server.state.lock().unwrap();
    let obligation = state
        .reply_obligations
        .get(&id)
        .expect("declared reply policy must persist atomically");
    assert_eq!(obligation.from_worker_id, "alpha.source.Alice");
    assert_eq!(obligation.from_name, None);
    assert_eq!(obligation.recipient_name.as_deref(), Some("recipient"));
    assert_eq!(obligation.recipient_worker_id, "recipient");
    assert_eq!(state.msgs[&id].state, "pending");
    drop(state);
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn reply_obligation_clears_only_for_confirmed_acceptance_of_its_recipient_reply() {
    let (server, root, id) = accept("immediate", Some(ReplyPolicy::Required));
    let state = server.state.lock().unwrap();
    let meta = state.federation.messages[&id].clone();
    let FederationActor::Peer(original_sender) = &meta.sender else {
        unreachable!()
    };
    let record = federation::prepare_outbound(
        federation::AddressSendRequest {
            sender: meta.recipient.clone(),
            target: crate::proto::FederationEndpoint::Named(original_sender.address.clone()),
            mtype: "reply".into(),
            subject: "reply".into(),
            body: "done".into(),
            delivery: "immediate".into(),
            operation_id: None,
            in_reply_to: Some(meta.reference.clone()),
            original_origin: Some(meta.origin.clone()),
            reply_policy: None,
            target_session: None,
            channel: None,
            now_ms: now_ms(),
        },
        &"beta".into(),
    )
    .unwrap();
    let unknown = federation::FederationSendOutcome::Unknown {
        origin: record.origin.clone(),
        detail: "EOF".into(),
    };
    assert!(
        federation_runtime::accepted_reply_clear_event(&state, &record, &unknown, "beta").is_none()
    );
    let accepted = federation::FederationSendOutcome::Accepted {
        origin: record.origin.clone(),
        reference: crate::proto::MessageRef {
            machine: "alpha".into(),
            project_scope: meta.origin.project_scope,
            app_scope_id: meta.origin.app_scope_id,
            msg_id: crate::identity::MessageId::new("accepted-reply").unwrap(),
        },
    };
    let event = federation_runtime::accepted_reply_clear_event(&state, &record, &accepted, "beta")
        .expect("confirmed reply must clear the original local obligation");
    assert!(matches!(event, Event::ReplyObligationCleared { message_id } if message_id == id));
    let mut stranger = record;
    stranger.sender = meta.sender;
    assert!(
        federation_runtime::accepted_reply_clear_event(&state, &stranger, &accepted, "beta")
            .is_none()
    );
    assert_eq!(state.msgs[&id].state, "pending");
    drop(state);
    std::fs::remove_dir_all(root).unwrap();
}
