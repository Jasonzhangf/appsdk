use super::*;
use crate::proto::{
    FederationActor, FederationDestination, FederationPrincipal, FederationPut, FederationResponse,
    LocalNameAddress, OriginMessageRef, QualifiedNameAddress,
};

fn accept_response(delivery: &str) -> (Server, PathBuf, FederationResponse) {
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
    (server, root, response)
}

fn accept(delivery: &str) -> (Server, PathBuf, String) {
    let (server, root, response) = accept_response(delivery);
    let FederationResponse::Message { receipt } = response else {
        panic!("acceptance failed: {response:?}");
    };
    (server, root, receipt.reference.msg_id.to_string())
}

#[test]
fn queued_delivery_keeps_the_declared_mode() {
    let (server, root, id) = accept("queued");
    assert_eq!(server.state.lock().unwrap().delivery_modes[&id], "queued");
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn unread_federated_message_follows_the_stable_recipient_name() {
    let (server, root, id) = accept("immediate");
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
