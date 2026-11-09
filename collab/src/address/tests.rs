use super::*;

fn qualified(machine: &str, project: &str, name: &str) -> QualifiedNameAddress {
    QualifiedNameAddress {
        machine: machine.into(),
        local: LocalNameAddress {
            project: project.into(),
            name: name.into(),
        },
    }
}

#[test]
fn legacy_local_addresses_work_without_machine_configuration() {
    for address in ["Alice", "appsdk.Alice", "worker-123", "ceo"] {
        assert_eq!(
            resolve_address(
                &AddressInput::Legacy(address.into()),
                &AddressContext::default()
            )
            .unwrap(),
            ResolvedAddress::LocalLegacy(address.into())
        );
    }
}

#[test]
fn explicit_machine_routes_and_local_machine_are_distinct() {
    let peers = vec!["MacBookAir".into(), "coder1".into()];
    let context = AddressContext {
        local_machine: Some("studio"),
        peers: &peers,
        local_projects: &[],
    };
    assert_eq!(
        resolve_address(
            &AddressInput::Legacy("macbookair.appsdk.Alice".into()),
            &context
        )
        .unwrap(),
        ResolvedAddress::Remote {
            machine: "MacBookAir".into(),
            target: qualified("", "appsdk", "Alice").local
        }
    );
    assert_eq!(
        resolve_address(
            &AddressInput::Legacy("STUDIO.appsdk.Alice".into()),
            &context
        )
        .unwrap(),
        ResolvedAddress::LocalName(qualified("", "appsdk", "Alice").local)
    );
    assert_eq!(
        resolve_address(
            &AddressInput::Legacy("unknown.appsdk.Alice".into()),
            &context
        )
        .unwrap_err()
        .code,
        "MACHINE_UNKNOWN"
    );
}

#[test]
fn dotted_project_legacy_alias_is_preserved_and_conflicts_are_explicit() {
    let projects = vec!["a.b".into()];
    let peers = vec!["a".into()];
    let local = AddressContext {
        local_machine: Some("studio"),
        peers: &[],
        local_projects: &projects,
    };
    let target = qualified("", "a.b", "Alice").local;
    assert_eq!(
        resolve_address(&AddressInput::Legacy("a.b.Alice".into()), &local).unwrap(),
        ResolvedAddress::LocalName(target.clone())
    );
    let conflict = AddressContext {
        peers: &peers,
        ..local
    };
    assert_eq!(
        resolve_address(&AddressInput::Legacy("a.b.Alice".into()), &conflict)
            .unwrap_err()
            .code,
        "ADDRESS_AMBIGUOUS"
    );
    assert_eq!(
        resolve_address(
            &AddressInput::Legacy("studio.a%2Eb.Alice".into()),
            &conflict
        )
        .unwrap(),
        ResolvedAddress::LocalName(target)
    );
    assert_eq!(
        resolve_address(&AddressInput::Legacy("a.%62.Alice".into()), &conflict).unwrap(),
        ResolvedAddress::Remote {
            machine: "a".into(),
            target: qualified("", "b", "Alice").local
        }
    );
}

#[test]
fn canonical_addresses_roundtrip_dots_percent_and_unicode() {
    let peers = vec!["air".into()];
    let context = AddressContext {
        local_machine: Some("studio"),
        peers: &peers,
        local_projects: &[],
    };
    let target = qualified("air", "a.b%", "探索者.name%");
    let encoded = format_qualified_address(&target, &context).unwrap();
    assert_eq!(encoded, "air.a%2Eb%25.探索者%2Ename%25");
    assert_eq!(
        resolve_address(&AddressInput::Legacy(encoded), &context).unwrap(),
        ResolvedAddress::Remote {
            machine: "air".into(),
            target: target.local
        }
    );
}

#[test]
fn formatter_disambiguates_configured_machine_from_legacy_project() {
    let peers = vec!["a".into()];
    let projects = vec!["a.b".into()];
    let context = AddressContext {
        local_machine: Some("studio"),
        peers: &peers,
        local_projects: &projects,
    };
    let target = qualified("a", "b", "Alice");
    let address = format_qualified_address(&target, &context).unwrap();
    assert_eq!(address, "a.%62.Alice");
    assert_eq!(
        resolve_address(&AddressInput::Legacy(address), &context).unwrap(),
        ResolvedAddress::Remote {
            machine: "a".into(),
            target: target.local
        }
    );
}

#[test]
fn malformed_qualified_input_is_rejected_without_local_fallback() {
    let peers = vec!["air".into()];
    let context = AddressContext {
        local_machine: Some("studio"),
        peers: &peers,
        local_projects: &[],
    };
    for address in [
        "air..Alice",
        "air.appsdk.",
        "air.appsdk.A%",
        "air.appsdk.A%GG",
        "air.appsdk.A%FF",
        "air.appsdk.A%0A",
        "air.appsdk.A lice",
    ] {
        assert_eq!(
            resolve_address(&AddressInput::Legacy(address.into()), &context)
                .unwrap_err()
                .code,
            "ADDRESS_INVALID",
            "{address}"
        );
    }
    assert_eq!(
        resolve_address(
            &AddressInput::Qualified(qualified("other", "appsdk", "Alice")),
            &context
        )
        .unwrap_err()
        .code,
        "MACHINE_UNKNOWN"
    );
}

#[test]
fn unconfigured_explicit_machine_has_an_explicit_error() {
    assert_eq!(
        resolve_address(
            &AddressInput::Legacy("air.appsdk.Alice".into()),
            &AddressContext::default()
        )
        .unwrap_err()
        .code,
        "MACHINE_NOT_CONFIGURED"
    );
}
