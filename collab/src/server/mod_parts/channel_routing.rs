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

pub(crate) struct ChannelInboundAddressIntent {
    /// Address text selected by the leading-address, explicit, or default rule.
    pub address: String,
    /// Body after removing a leading address.
    pub body: String,
    pub explicit_to: bool,
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
    let control = binding_store::BindingControlStore::new(server.host_paths.state_root()).load()?;

    if let Some(leading) = parse_leading_address(body) {
        let to = resolve_address(state, &control, &route, &project_key, &leading.raw)?;
        return Ok(ResolvedInbound {
            to,
            recipient_address: leading.raw,
            body: leading.body,
        });
    }

    if !explicit_to.trim().is_empty() {
        return Ok(ResolvedInbound {
            to: explicit_to.to_owned(),
            recipient_address: explicit_to.to_owned(),
            body: body.to_owned(),
        });
    }

    let default = crate::server::conversation_default::lookup_default(
        state,
        channel,
        channel_instance,
        conversation,
    )
    .cloned()
    .ok_or_else(|| {
        "CHANNEL_DEFAULT_MISSING: this conversation has no default target; send an explicit [address]"
            .to_string()
    })?;
    let to = resolve_address(state, &control, &route, &project_key, &default.target_address)?;
    Ok(ResolvedInbound {
        to,
        recipient_address: default.target_address,
        body: body.to_owned(),
    })
}

/// Resolve only the address intent before selecting a runtime. This keeps
/// `ceo` from being interpreted through the caller's project-local grant. The
/// manager uses the result to decide whether it must route through the host
/// authority owner; all other addresses continue through the existing local
/// resolver.
pub(crate) fn channel_inbound_address_intent(
    state: &crate::server::state::State,
    channel: &str,
    channel_instance: &str,
    conversation: &str,
    body: &str,
    explicit_to: &str,
) -> Result<ChannelInboundAddressIntent, String> {
    if let Some(leading) = parse_leading_address(body) {
        return Ok(ChannelInboundAddressIntent {
            address: leading.raw,
            body: leading.body,
            explicit_to: false,
        });
    }
    if !explicit_to.trim().is_empty() {
        return Ok(ChannelInboundAddressIntent {
            address: explicit_to.to_owned(),
            body: body.to_owned(),
            explicit_to: true,
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
    Ok(ChannelInboundAddressIntent {
        address: default.target_address.clone(),
        body: body.to_owned(),
        explicit_to: false,
    })
}
