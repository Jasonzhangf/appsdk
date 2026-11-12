use super::name_resolution::*;
use super::name_state::*;
use super::*;

#[test]
fn generated_peer_names_do_not_invent_an_unlinked_position() {
    assert_eq!(peer_registration_number("peer-1"), Some(1));
    assert_eq!(peer_registration_number("peer-42"), Some(42));
    assert_eq!(peer_registration_number("peer-0"), None);
    assert_eq!(peer_registration_number("peer-01"), None);
    assert_eq!(peer_registration_number("Peer1"), None);
    assert_eq!(display_slot("peer-1").unwrap(), None);
}

#[test]
fn name_and_role_takeovers_keep_source_name_work() {
    let master = parse_name_intent(Some(&serde_json::json!({
        "action": "restore_master",
        "authorization": "fixture promotes the current carrier"
    })))
    .unwrap()
    .unwrap();
    assert_eq!(master.requested_name, "Jenny");
    assert!(!master.moves_existing_names);

    let ordinary_rename = parse_name_intent(Some(&serde_json::json!({
        "action": "restore_name",
        "requested_name": "Bob",
        "authorization": "fixture explicitly renames the current carrier"
    })))
    .unwrap()
    .unwrap();
    assert!(!ordinary_rename.moves_existing_names);
}
