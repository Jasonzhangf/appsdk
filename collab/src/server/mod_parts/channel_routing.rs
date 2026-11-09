// Leading-address routing for channel-inbound (QQBot) messages.
//
// Only a bracketed address at the very start of a message is control data.
// Any bracketed text later in the body is plain user text. Resolution is
// case-insensitive for the project key, stable names and the `ceo` alias. A
// failed explicit address never falls back to the conversation default and
// never mutates it. `[ceo]` resolves only through a live typed CEO grant,
// never through a role label or the project master.

/// Result of routing one inbound channel message to a local worker.
pub(crate) struct ResolvedInbound {
    /// Local worker id the message is delivered to.
    pub to: String,
    /// The address text that produced `to` (explicit or default target).
    pub recipient_address: String,
    /// Body with any leading address stripped. Mid-body brackets stay intact.
    pub body: String,
}

#[derive(Debug)]
pub(crate) struct ChannelInboundAddressIntent {
    /// Address text selected by the leading-address, explicit, or default rule.
    pub address: String,
    /// Body after removing a leading address.
    pub body: String,
    pub explicit_to: bool,
    /// Typed classification from the crate address owner. The legacy entry
    /// point leaves this as `LocalLegacy`; a host that has machine
    /// configuration calls `channel_inbound_address_intent_with_context` so a
    /// declared remote machine is never re-read as a local `project.name`.
    pub target: crate::address::ResolvedAddress,
}

struct LeadingAddress {
    raw: String,
    body: String,
}

/// Parse a single `[address]` only when it begins the message.
fn parse_leading_address(body: &str) -> Option<LeadingAddress> {
    let trimmed = body.trim_start();
    let rest = trimmed.strip_prefix('[')?;
    let close = rest.find(']')?;
    let raw = rest[..close].trim();
    // An empty or whitespace-bearing bracket is ordinary text, not an address.
    if raw.is_empty() || raw.chars().any(char::is_whitespace) {
        return None;
    }
    let after = &rest[close + 1..];
    Some(LeadingAddress {
        raw: raw.to_owned(),
        body: after.trim_start().to_owned(),
    })
}

/// One resolved stable-name target. `name` is the canonical stored spelling,
/// never the caller's case, so a mailbox address keeps the real name.
pub(crate) struct ResolvedNamedTarget {
    pub worker_id: String,
    pub name: String,
}

/// Resolve a stable name in one project. All case-folded matches are collected
/// before selecting one, so `Bob` and `BOB` in different occupancies are an
/// explicit ambiguity instead of an arbitrary first match.
fn resolve_name_case_insensitive(
    control: &binding_slot::BindingControlSet,
    route: &RouteScope,
    requested: &str,
) -> Result<ResolvedNamedTarget, String> {
    let slot = control.current_for_name(&route.project_scope_id, &route.app_scope_id, requested)?
        .ok_or_else(|| format!("NAME_ADDRESS_UNOCCUPIED: {requested} has no current holder in this project"))?;
    let current = slot.active_binding().expect("current name query returns active binding");
    Ok(ResolvedNamedTarget { name: slot.identity_key.name.clone(),
        worker_id: current.binding.agent_id.as_str().to_owned() })
}

pub(crate) fn resolve_ceo_holder_from_grant(
    state: &crate::server::state::State,
    control: &binding_slot::BindingControlSet,
    route: &RouteScope,
    grant: &crate::ceo::CeoGrant,
) -> Result<String, String> {
    if grant.workspace != route.project_scope_id.as_str() {
        return Err(
            "CHANNEL_CEO_ROUTE_UNAVAILABLE: the active CEO grant belongs to another project".into(),
        );
    }
    let holder = resolve_name_case_insensitive(control, route, &grant.name)?;
    if !state.workers.contains_key(&holder.worker_id) {
        return Err(
            "CHANNEL_CEO_HOLDER_UNAVAILABLE: the CEO holder is not a registered worker".into(),
        );
    }
    Ok(holder.worker_id.clone())
}

fn resolve_ceo_holder(
    state: &crate::server::state::State,
    control: &binding_slot::BindingControlSet,
    route: &RouteScope,
) -> Result<String, String> {
    let grant = state.ceo_grant.as_ref().ok_or_else(|| {
        "CHANNEL_CEO_GRANT_MISSING: no typed CEO grant is active; [ceo] does not fall back to the project master"
            .to_string()
    })?;
    resolve_ceo_holder_from_grant(state, control, route, grant)
}

/// The single case-insensitive stable-address resolver shared by `sendmessage`,
/// `default` and channel-inbound routing. `ceo` and a bare current project key
/// both resolve through the typed CEO grant. Unknown non-name text returns
/// `None` so each caller can apply its own raw-target rule.
pub(crate) fn resolve_named_target(
    state: &crate::server::state::State,
    control: &binding_slot::BindingControlSet,
    route: &RouteScope,
    project_key: &str,
    address: &str,
) -> Result<Option<ResolvedNamedTarget>, String> {
    if address.eq_ignore_ascii_case("ceo") || address.eq_ignore_ascii_case(project_key) {
        let worker_id = resolve_ceo_holder(state, control, route)?;
        let name = state
            .ceo_grant
            .as_ref()
            .map(|grant| grant.name.clone())
            .unwrap_or_else(|| "ceo".to_owned());
        return Ok(Some(ResolvedNamedTarget { worker_id, name }));
    }
    // Project-qualified addresses are structural: split from the right so a
    // project key may itself contain dots. The name is resolved only against
    // current occupancies; no display-name or fixed name list participates.
    if let Some((project, name)) = address.rsplit_once('.') {
        if project.is_empty() || name.is_empty() {
            return Ok(None);
        }
        if !project.eq_ignore_ascii_case(project_key) {
            return Err(format!(
                "CROSS_PROJECT_ADDRESS_UNSUPPORTED: {project} is not the current project key {project_key}"
            ));
        }
        return resolve_name_case_insensitive(control, route, name).map(Some);
    }
    // A raw worker ID keeps its legacy direct-send meaning even for the typed
    // not-registered failure path. A bare stable name is resolved only when it
    // names a current holder; other bare values remain raw targets for the
    // existing admission and handoff checks.
    if state.workers.contains_key(address) {
        return Ok(None);
    }
    let name_like = control.current_for_name(&route.project_scope_id, &route.app_scope_id, address)?.is_some();
    if name_like {
        resolve_name_case_insensitive(control, route, address).map(Some)
    } else {
        Ok(None)
    }
}

/// Resolve one address string (explicit or default) to a deliverable local
/// worker. Project scope, name and `ceo` matching are all case-insensitive.
fn resolve_address(
    state: &crate::server::state::State,
    control: &binding_slot::BindingControlSet,
    route: &RouteScope,
    project_key: &str,
    address: &str,
) -> Result<String, String> {
    let worker_id = match resolve_named_target(state, control, route, project_key, address)? {
        Some(target) => target.worker_id,
        None => resolve_name_case_insensitive(control, route, address)?.worker_id,
    };
    // The resolved target must be reachable on this route scope; an unresolved
    // binding is an explicit non-deliverable result, not a silent success.
    authoritative_send_binding(state, route, &worker_id).map_err(str::to_owned)?;
    Ok(worker_id)
}

/// The selected address, body, and how it was selected. A leading address or
/// host-supplied explicit target wins; with neither, the conversation default
/// decides. A missing default is an explicit failure, never a CEO fallback.
struct ChannelAddressSelection {
    address: String,
    body: String,
    explicit_to: bool,
    from_default: bool,
    resolved_target: Option<crate::address::ResolvedAddress>,
}

fn channel_address_selection(
    state: &crate::server::state::State,
    channel: &str,
    channel_instance: &str,
    conversation: &str,
    body: &str,
    explicit_to: &str,
) -> Result<ChannelAddressSelection, String> {
    if let Some(leading) = parse_leading_address(body) {
        return Ok(ChannelAddressSelection {
            address: leading.raw,
            body: leading.body,
            explicit_to: false,
            from_default: false,
            resolved_target: None,
        });
    }
    if !explicit_to.trim().is_empty() {
        return Ok(ChannelAddressSelection {
            address: explicit_to.to_owned(),
            body: body.to_owned(),
            explicit_to: true,
            from_default: false,
            resolved_target: None,
        });
    }
    let default = crate::server::conversation_default::lookup_default(
        state,
        channel,
        channel_instance,
        conversation,
    )
    .ok_or_else(|| {
        "CHANNEL_DEFAULT_MISSING: this conversation has no default target; send an explicit [address]"
            .to_string()
    })?;
    Ok(ChannelAddressSelection {
        address: default.target_address.clone(),
        body: body.to_owned(),
        explicit_to: false,
        from_default: true,
        resolved_target: default.resolved_target.clone(),
    })
}

/// Resolve only the address intent before selecting a runtime. This keeps
/// `ceo` from being interpreted through the caller's project-local grant. The
/// manager uses the result to decide whether it must route through the host
/// authority owner; all other addresses continue through the existing local
/// resolver.
///
/// The legacy entry point keeps local `short` and `project.name` syntaxes
/// exactly as before, including project keys that contain dots. Hosts with
/// machine configuration must call the `_with_context` variant so a declared
/// machine address is classified as remote instead of being re-read as a
/// local project key.
pub(crate) fn channel_inbound_address_intent(
    state: &crate::server::state::State,
    channel: &str,
    channel_instance: &str,
    conversation: &str,
    body: &str,
    explicit_to: &str,
) -> Result<ChannelInboundAddressIntent, String> {
    let selection =
        channel_address_selection(state, channel, channel_instance, conversation, body, explicit_to)?;
    let target = crate::address::ResolvedAddress::LocalLegacy(selection.address.clone());
    Ok(ChannelInboundAddressIntent {
        address: selection.address,
        body: selection.body,
        explicit_to: selection.explicit_to,
        target,
    })
}

/// Typed-address variant for hosts that have loaded federation configuration.
/// It uses `address.rs` as the single grammar owner. An explicit address is
/// always classified through the current grammar. A stored typed default is
/// consumed without reinterpretation; an old untyped default stays local.
pub(crate) fn channel_inbound_address_intent_with_context(
    state: &crate::server::state::State,
    channel: &str,
    channel_instance: &str,
    conversation: &str,
    body: &str,
    explicit_to: &str,
    context: &crate::address::AddressContext<'_>,
) -> Result<ChannelInboundAddressIntent, String> {
    let selection =
        channel_address_selection(state, channel, channel_instance, conversation, body, explicit_to)?;
    let target = if selection.from_default {
        match selection.resolved_target {
            Some(crate::address::ResolvedAddress::Remote { machine, target }) => {
                let revalidated = crate::address::resolve_address(
                    &crate::proto::AddressInput::Qualified(crate::proto::QualifiedNameAddress {
                        machine,
                        local: target,
                    }),
                    context,
                )
                .map_err(|error| error.to_string())?;
                match revalidated {
                    crate::address::ResolvedAddress::Remote { .. } => revalidated,
                    _ => {
                        return Err(format!(
                            "REMOTE_DEFAULT_DECLARATION_INVALID: stored remote target {selection_address} no longer resolves through a declared remote machine",
                            selection_address = selection.address
                        ))
                    }
                }
            }
            Some(target) => target,
            None => crate::address::ResolvedAddress::LocalLegacy(selection.address.clone()),
        }
    } else {
        crate::address::resolve_address(
            &crate::proto::AddressInput::Legacy(selection.address.clone()),
            context,
        )
        .map_err(|error| error.to_string())?
    };
    Ok(ChannelInboundAddressIntent {
        address: selection.address,
        body: selection.body,
        explicit_to: selection.explicit_to,
        target,
    })
}

/// Build the address classification context from the same daemon-owned config
/// and canonical project key used by the local send owner. The channel layer
/// never reads environment, hostname, or a remote directory entry.
pub(crate) fn channel_address_context(
    server: &Server,
    project_key: &str,
) -> (Option<String>, Vec<String>, Vec<String>) {
    let federation = server.config.federation.as_ref();
    let local_machine = federation.map(|configuration| configuration.machine.clone());
    let peers = federation
        .map(|configuration| configuration.peers.keys().cloned().collect::<Vec<_>>())
        .unwrap_or_default();
    let local_projects = vec![project_key.to_owned()];
    (local_machine, peers, local_projects)
}

/// Resolve a typed classification to the one local worker that may receive
/// the message. A remote target is an explicit refusal here: the host
/// federation dispatch owns cross-machine transport. This keeps the local
/// mailbox path from ever consuming a `machine.project.name` as if it were
/// local `project.name`.
pub(crate) fn local_inbound_worker_for_target(
    state: &crate::server::state::State,
    control: &binding_slot::BindingControlSet,
    route: &RouteScope,
    project_key: &str,
    target: &crate::address::ResolvedAddress,
) -> Result<String, String> {
    match target {
        crate::address::ResolvedAddress::LocalLegacy(address) => {
            resolve_address(state, control, route, project_key, address)
        }
        crate::address::ResolvedAddress::LocalName(local) => {
            if !local.project.eq_ignore_ascii_case(project_key) {
                return Err(format!(
                    "CROSS_PROJECT_ADDRESS_UNSUPPORTED: {} is not the current project key {project_key}",
                    local.project
                ));
            }
            let resolved = resolve_name_case_insensitive(control, route, &local.name)?;
            authoritative_send_binding(state, route, &resolved.worker_id).map_err(str::to_owned)?;
            Ok(resolved.worker_id)
        }
        crate::address::ResolvedAddress::Remote { machine, target } => {
            Err(format!(
                "REMOTE_CHANNEL_TARGET_REQUIRES_FEDERATION: {machine}.{}.{}",
                target.project, target.name
            ))
        }
    }
}

/// Route one inbound channel message. A leading explicit address wins; with no
/// leading address the conversation default is used. Resolution failures are
/// returned as-is and never fall back to the default.
pub(crate) fn resolve_channel_inbound_target(
    server: &Server,
    state: &crate::server::state::State,
    channel: &str,
    channel_instance: &str,
    conversation: &str,
    body: &str,
    explicit_to: &str,
) -> Result<ResolvedInbound, String> {
    let route = server_route_scope(server, state)?.ok_or_else(|| {
        "CHANNEL_ROUTE_SCOPE_MISSING: the current project route is unavailable".to_string()
    })?;
    let project_key = canonical_project_key(&server.root).ok_or_else(|| {
        "CHANNEL_ROUTE_SCOPE_MISSING: the current project key is unavailable".to_string()
    })?;
    let (local_machine, peers, local_projects) = channel_address_context(server, &project_key);
    let context = crate::address::AddressContext {
        local_machine: local_machine.as_deref(),
        peers: &peers,
        local_projects: &local_projects,
    };
    let intent = channel_inbound_address_intent_with_context(
        state,
        channel,
        channel_instance,
        conversation,
        body,
        explicit_to,
        &context,
    )?;
    let control = binding_store::BindingControlStore::new(server.host_paths.state_root()).load()?;
    let to = local_inbound_worker_for_target(state, &control, &route, &project_key, &intent.target)?;
    Ok(ResolvedInbound {
        to,
        recipient_address: intent.address,
        body: intent.body,
    })
}

#[cfg(test)]
mod channel_routing_federation_tests {
    use super::*;
    use crate::address::{AddressContext, ResolvedAddress};
    use crate::proto::LocalNameAddress;

    fn federated_context<'a>(
        peers: &'a [String],
        local_projects: &'a [String],
    ) -> AddressContext<'a> {
        AddressContext {
            local_machine: Some("studio"),
            peers,
            local_projects,
        }
    }

    #[test]
    fn mid_body_bracket_is_not_control_data() {
        assert!(parse_leading_address("please ask [appsdk.Bob] later").is_none());
        let leading = parse_leading_address("  [macbookair.appsdk.Alice] body").unwrap();
        assert_eq!(leading.raw, "macbookair.appsdk.Alice");
        assert_eq!(leading.body, "body");
    }

    fn typed_default(state: &mut crate::server::state::State, target: &str) {
        typed_default_with_target(state, target, None);
    }

    fn typed_default_with_target(
        state: &mut crate::server::state::State,
        target: &str,
        resolved_target: Option<ResolvedAddress>,
    ) {
        let default = crate::server::conversation_default::ConversationDefault {
            channel: "qqbot".into(),
            channel_instance: "qqbot@host".into(),
            conversation: "u1:session".into(),
            project_scope: "/project".into(),
            target_address: target.into(),
            resolved_target,
            revision: 1,
            updated_ms: 0,
        };
        state.conversation_defaults.insert(default.key(), default);
    }

    #[test]
    fn old_untyped_dotted_default_stays_local_when_peer_alias_appears() {
        let mut state = crate::server::state::State::default();
        typed_default(&mut state, "macbookair.appsdk.Alice");
        let peers = vec!["macbookair".to_owned()];
        let local_projects = vec!["macbookair.appsdk".to_owned()];
        let context = federated_context(&peers, &local_projects);
        let intent = channel_inbound_address_intent_with_context(
            &state,
            "qqbot",
            "qqbot@host",
            "u1:session",
            "unaddressed body",
            "",
            &context,
        )
        .unwrap();
        assert_eq!(intent.address, "macbookair.appsdk.Alice");
        assert_eq!(
            intent.target,
            ResolvedAddress::LocalLegacy("macbookair.appsdk.Alice".into()),
            "a default created before federation keeps its local meaning",
        );
    }

    #[test]
    fn typed_remote_default_is_revalidated_when_declared() {
        let mut state = crate::server::state::State::default();
        typed_default_with_target(
            &mut state,
            "macbookair.appsdk.Alice",
            Some(ResolvedAddress::Remote {
                machine: "macbookair".into(),
                target: LocalNameAddress {
                    project: "appsdk".into(),
                    name: "Alice".into(),
                },
            }),
        );
        let peers = vec!["macbookair".to_owned()];
        let local_projects = vec!["appsdk".to_owned()];
        let context = federated_context(&peers, &local_projects);
        let intent = channel_inbound_address_intent_with_context(
            &state,
            "qqbot",
            "qqbot@host",
            "u1:session",
            "unaddressed body",
            "",
            &context,
        )
        .unwrap();
        assert_eq!(intent.address, "macbookair.appsdk.Alice");
        assert_eq!(
            intent.target,
            ResolvedAddress::Remote {
                machine: "macbookair".into(),
                target: LocalNameAddress {
                    project: "appsdk".into(),
                    name: "Alice".into(),
                },
            }
        );
    }

    #[test]
    fn typed_remote_default_fails_when_peer_is_undeclared() {
        let mut state = crate::server::state::State::default();
        typed_default_with_target(
            &mut state,
            "macbookair.appsdk.Alice",
            Some(ResolvedAddress::Remote {
                machine: "macbookair".into(),
                target: LocalNameAddress {
                    project: "appsdk".into(),
                    name: "Alice".into(),
                },
            }),
        );
        let peers = Vec::new();
        let local_projects = vec!["appsdk".to_owned()];
        let context = federated_context(&peers, &local_projects);
        let error = channel_inbound_address_intent_with_context(
            &state,
            "qqbot",
            "qqbot@host",
            "u1:session",
            "unaddressed body",
            "",
            &context,
        )
        .unwrap_err();
        assert!(
            error.contains("MACHINE_UNKNOWN") || error.contains("MACHINE_NOT_CONFIGURED"),
            "{error}"
        );
    }

    #[test]
    fn local_short_and_project_name_syntax_stay_legacy() {
        let state = crate::server::state::State::default();
        let peers = Vec::new();
        let local_projects = vec!["appsdk".to_owned()];
        let context = federated_context(&peers, &local_projects);
        for address in ["Alice", "appsdk.Alice", "worker-123", "ceo"] {
            let intent = channel_inbound_address_intent_with_context(
                &state,
                "qqbot",
                "qqbot@host",
                "u1:session",
                &format!("[{address}] body"),
                "",
                &context,
            )
            .unwrap();
            assert_eq!(intent.address, address);
            assert_eq!(intent.body, "body");
            assert_eq!(intent.target, ResolvedAddress::LocalLegacy(address.to_owned()));
        }
    }

    #[test]
    fn explicit_local_machine_address_is_typed_not_local_project_name() {
        let state = crate::server::state::State::default();
        let peers = Vec::new();
        let local_projects = vec!["appsdk".to_owned()];
        let context = federated_context(&peers, &local_projects);
        let intent = channel_inbound_address_intent_with_context(
            &state,
            "qqbot",
            "qqbot@host",
            "u1:session",
            "[STUDIO.appsdk.Alice] body",
            "",
            &context,
        )
        .unwrap();
        assert_eq!(
            intent.target,
            ResolvedAddress::LocalName(LocalNameAddress {
                project: "appsdk".into(),
                name: "Alice".into(),
            })
        );
    }

    #[test]
    fn declared_remote_machine_is_classified_not_split_as_project_name() {
        let state = crate::server::state::State::default();
        let peers = vec!["macbookair".to_owned()];
        let local_projects = vec!["appsdk".to_owned()];
        let context = federated_context(&peers, &local_projects);
        let intent = channel_inbound_address_intent_with_context(
            &state,
            "qqbot",
            "qqbot@host",
            "u1:session",
            "[macbookair.appsdk.Alice] body",
            "",
            &context,
        )
        .unwrap();
        assert_eq!(
            intent.target,
            ResolvedAddress::Remote {
                machine: "macbookair".into(),
                target: LocalNameAddress {
                    project: "appsdk".into(),
                    name: "Alice".into(),
                },
            }
        );
    }

    #[test]
    fn typed_local_default_uses_the_stored_canonical_name() {
        let mut state = crate::server::state::State::default();
        typed_default_with_target(
            &mut state,
            "legacy-display-text",
            Some(ResolvedAddress::LocalName(LocalNameAddress {
                project: "appsdk".into(),
                name: "Alice".into(),
            })),
        );
        let peers = Vec::new();
        let local_projects = vec!["appsdk".to_owned()];
        let context = federated_context(&peers, &local_projects);
        let intent = channel_inbound_address_intent_with_context(
            &state,
            "qqbot",
            "qqbot@host",
            "u1:session",
            "unaddressed body",
            "",
            &context,
        )
        .unwrap();
        assert_eq!(intent.address, "legacy-display-text");
        assert_eq!(
            intent.target,
            ResolvedAddress::LocalName(LocalNameAddress {
                project: "appsdk".into(),
                name: "Alice".into(),
            }),
            "the typed decision wins while display text remains unchanged",
        );
    }

    #[test]
    fn an_unknown_machine_fails_before_local_resolution_and_without_fallback() {
        let mut state = crate::server::state::State::default();
        typed_default(&mut state, "macbookair.appsdk.Alice");
        let peers = vec!["macbookair".to_owned()];
        let local_projects = vec!["appsdk".to_owned()];
        let context = federated_context(&peers, &local_projects);
        let error = channel_inbound_address_intent_with_context(
            &state,
            "qqbot",
            "qqbot@host",
            "u1:session",
            "[ghost.appsdk.Alice] body",
            "",
            &context,
        )
        .unwrap_err();
        assert!(error.contains("MACHINE_UNKNOWN"), "{error}");
        let default =
            crate::server::conversation_default::lookup_default(&state, "qqbot", "qqbot@host", "u1:session")
                .unwrap();
        assert_eq!(default.target_address, "macbookair.appsdk.Alice");
        assert_eq!(default.resolved_target, None);
    }

    #[test]
    fn explicit_bad_address_does_not_consume_a_typed_default() {
        let mut state = crate::server::state::State::default();
        typed_default_with_target(
            &mut state,
            "macbookair.appsdk.Alice",
            Some(ResolvedAddress::Remote {
                machine: "macbookair".into(),
                target: LocalNameAddress {
                    project: "appsdk".into(),
                    name: "Alice".into(),
                },
            }),
        );
        let peers = vec!["macbookair".to_owned()];
        let local_projects = vec!["appsdk".to_owned()];
        let context = federated_context(&peers, &local_projects);
        let error = channel_inbound_address_intent_with_context(
            &state,
            "qqbot",
            "qqbot@host",
            "u1:session",
            "[ghost.appsdk.Alice] body",
            "",
            &context,
        )
        .unwrap_err();
        assert!(error.contains("MACHINE_UNKNOWN"), "{error}");
        let default =
            crate::server::conversation_default::lookup_default(&state, "qqbot", "qqbot@host", "u1:session")
                .unwrap();
        assert_eq!(default.resolved_target.as_ref().and_then(|target| match target {
            ResolvedAddress::Remote { machine, .. } => Some(machine.as_str()),
            _ => None,
        }), Some("macbookair"));
    }

    #[test]
    fn a_legacy_intent_never_reclassifies_a_dotted_project_key() {
        let state = crate::server::state::State::default();
        let intent = channel_inbound_address_intent(
            &state,
            "qqbot",
            "qqbot@host",
            "u1:session",
            "[a.b.Alice] body",
            "",
        )
        .unwrap();
        // The legacy variant keeps the dotted project key as local text; only
        // the typed-context variant consults the machine grammar.
        assert_eq!(intent.address, "a.b.Alice");
        assert_eq!(intent.body, "body");
        assert_eq!(
            intent.target,
            ResolvedAddress::LocalLegacy("a.b.Alice".to_owned())
        );
    }
}
