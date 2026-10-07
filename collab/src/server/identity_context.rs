use super::*;
use crate::identity::{self, RuntimeIdentity};
use crate::proto::{AppServerCandidate, IdentityFacts};

impl ProjectRuntimeManager {
    /// One host-local owner for bootstrap, recovery, registration and receipt.
    /// Register retains its own transaction/gate; never acquire it here.
    pub(super) fn identity_context(
        &self,
        context: ProjectContext,
        facts: IdentityFacts,
    ) -> (Arc<Server>, Resp) {
        let _guard = self.identity_gate.lock().unwrap();
        match self.reconcile_identity_context(context, facts) {
            Ok(result) => result,
            Err(error) => (self.host.clone(), Resp::err(format!("{error:#}"))),
        }
    }

    fn reconcile_identity_context(
        &self,
        context: ProjectContext,
        mut facts: IdentityFacts,
    ) -> anyhow::Result<(Arc<Server>, Resp)> {
        if context.app_scope_id.as_str() != identity::CLI_APP_SERVER_ID
            || context.runtime_context.is_some()
        {
            anyhow::bail!("IDENTITY_CONTEXT_INVALID: bootstrap requires the CLI project scope without a caller-selected identity");
        }
        let root = PathBuf::from(&context.canonical_root);
        context.validate_registered_root(&root)?;
        validate_project_registration_cwd(&context.canonical_root, &root)
            .map_err(anyhow::Error::msg)?;
        validate_facts(&facts)?;
        let scope = Scope { root };
        if !required_fields(&facts).is_empty() {
            self.complete_persisted_native_facts(&scope, &mut facts)?;
        }
        let required = required_fields(&facts);
        if !required.is_empty() {
            let descriptions = json!({
                "session_id": "Current runtime session identifier",
                "thread_id": "Current native thread identifier",
                "endpoint": "Current native AppServer unix socket endpoint",
                "namespace": "Current native endpoint namespace: codex_app or codex_tui"
            });
            let missing_descriptions: serde_json::Map<String, serde_json::Value> = required
                .iter()
                .map(|field| ((*field).to_owned(), descriptions[*field].clone()))
                .collect();
            return Ok((
                self.host.clone(),
                Resp::data(json!({
                    "snapshot": {
                        "registered": false,
                        "identity": null,
                        "requires_identity_update": {
                            "required": true,
                            "reason": "IDENTITY_INFORMATION_REQUIRED",
                            "required_fields": required,
                        "field_descriptions": missing_descriptions,
                            "action": "collab context --provide '<JSON containing required_fields>'",
                            "requires_approval": false
                        }
                    },
                    "identity_receipt": null
                })),
            ));
        }
        let pane_route = if let Some(candidate) = facts.tmux.as_ref() {
            match self.resolve_route_by_tmux_endpoint(&candidate.endpoint) {
                Ok(route) => Some(route),
                Err(error) if error.starts_with("ROUTE_RESOLVE_NOT_FOUND:") => None,
                Err(error) => return Err(anyhow::Error::msg(error)),
            }
        } else {
            None
        };
        let mut ident = identity::resolve_for_daemon_with_route_at(
            &self.host.host_paths,
            &scope,
            &facts,
            pane_route.as_ref(),
        )?;
        self.reconcile_committed_credential(&scope, &facts, &mut ident)?;
        let candidates = TransportCandidates {
            appserver: facts.endpoint.as_ref().map(|endpoint| AppServerCandidate {
                endpoint: endpoint.clone(),
                namespace: facts.namespace.clone().expect("complete native facts"),
                session_id: facts.session_id.clone().expect("complete native facts"),
                thread_id: facts.thread_id.clone().expect("complete native facts"),
                cwd: context.canonical_root.clone(),
            }),
            tmux: facts.tmux,
            dsh: None,
        };
        let provisional = RuntimeIdentity::cli_adapter(&ident.worker_id)?;
        let register_runtime = ident.runtime.as_ref().unwrap_or(&provisional);
        let register_context = ProjectContext::for_registered_route(&scope.root, register_runtime)?;
        let (runtime, receipt) = self.dispatch_sync(
            Some(register_context),
            Req::Register {
                worker_id: ident.worker_id.clone(),
                token: ident.token.clone(),
                cwd: context.canonical_root,
                candidates: Some(candidates),
            },
        );
        if !receipt.ok {
            return Ok((runtime, receipt));
        }
        let (binding, transport) =
            identity::registration_from_receipt(&receipt.data, &ident.worker_id, &scope.root)?;
        identity::persist_registration_at(
            &self.host.host_paths,
            &scope,
            &mut ident,
            binding.clone(),
            transport,
        )?;
        let registered_context = ProjectContext::for_registered_route(&scope.root, &binding)?;
        let (runtime, snapshot) = self.dispatch_sync(
            Some(registered_context),
            Req::Context {
                worker_id: ident.worker_id.clone(),
                token: ident.token.clone(),
            },
        );
        if !snapshot.ok {
            return Ok((runtime, snapshot));
        }
        Ok((
            runtime,
            Resp::data(json!({
                "snapshot": snapshot.data,
                "identity_receipt": ident
            })),
        ))
    }

    /// A successful supplement belongs to its committed identity. A unique
    /// current anchor may reuse that typed receipt on later partial calls;
    /// anonymous callers and conflicting observations never inherit it.
    fn complete_persisted_native_facts(
        &self,
        scope: &Scope,
        facts: &mut IdentityFacts,
    ) -> anyhow::Result<()> {
        let Some(existing) = identity::existing_for_daemon_at(&self.host.host_paths, scope, facts)?
        else {
            return Ok(());
        };
        let (Some(runtime), Some(transport)) = (existing.runtime, existing.transport) else {
            return Ok(());
        };
        // A recovered dsh identity needs no App Server completion: its anchor is
        // the gateway session id, and the gateway-owned address is supplied by
        // the caller once through the `required_fields` request instead.
        if transport.kind == TransportKind::Dsh {
            return Ok(());
        }
        if transport.kind != TransportKind::AppServer
            || facts.session_id.as_deref().is_some_and(|value| {
                runtime.session_id.as_ref().map(identity::SessionId::as_str) != Some(value)
            })
            || facts.thread_id.as_deref().is_some_and(|value| {
                runtime
                    .native_thread_id
                    .as_ref()
                    .map(NativeThreadId::as_str)
                    != Some(value)
            })
        {
            return Ok(());
        }
        facts.session_id = facts
            .session_id
            .take()
            .or_else(|| runtime.session_id.map(|value| value.to_string()));
        facts.thread_id = facts
            .thread_id
            .take()
            .or_else(|| runtime.native_thread_id.map(|value| value.to_string()));
        let address_matches = facts
            .endpoint
            .as_ref()
            .is_none_or(|value| Some(value) == transport.endpoint.as_ref())
            && facts
                .namespace
                .as_ref()
                .is_none_or(|value| Some(value) == transport.namespace.as_ref());
        if address_matches {
            facts.endpoint = facts.endpoint.take().or(transport.endpoint);
            facts.namespace = facts.namespace.take().or(transport.namespace);
        }
        Ok(())
    }

    /// A committed registration can outlive its local receipt file. Recover
    /// that same credential from the reducer only for the proven anchor.
    /// A persisted credential is never replaced, even when it is rejected.
    fn reconcile_committed_credential(
        &self,
        scope: &Scope,
        facts: &IdentityFacts,
        ident: &mut identity::Identity,
    ) -> anyhow::Result<()> {
        if ident.runtime.is_some()
            || identity::read_persisted(&self.host.host_paths, &ident.worker_id)?.is_some()
        {
            return Ok(());
        }
        let host_binding = {
            let state = self.host.state.lock().unwrap();
            if let Some(tmux) = &facts.tmux {
                state
                    .global
                    .lookup_unique_tmux_pane_route(&tmux.endpoint)
                    .cloned()
            } else if let (Some(session), Some(thread)) = (&facts.session_id, &facts.thread_id) {
                state
                    .global
                    .lookup_current_thread_route(
                        &identity::SessionId::new(session.clone())?,
                        &NativeThreadId::new(thread.clone())?,
                    )
                    .cloned()
            } else {
                None
            }
        };
        if let Some(binding) = host_binding {
            if binding.project_scope.as_str() == scope.root.to_string_lossy()
                && binding.agent_id.as_str() == ident.worker_id
            {
                let context = ProjectContext::for_registered_root_with_app(
                    &scope.root,
                    binding.app_scope_id,
                )?;
                self.select_runtime(&context).map_err(anyhow::Error::msg)?;
            }
        }
        let mut recovered = None;
        for runtime in self.runtimes() {
            let state = runtime.state.lock().unwrap();
            let Some(project) = state.global.lookup_project(&ProjectScopeId::new(
                scope.root.to_string_lossy().into_owned(),
            )?) else {
                continue;
            };
            for binding in project.runtime_bindings.values() {
                let anchor_matches = if let Some(tmux) = &facts.tmux {
                    binding.tmux_endpoint.as_ref().is_some_and(|bound| {
                        // Ownership follows the pane address, never the concrete
                        // endpoint. A reissued shell pid in the same owned pane
                        // is still this anchor, so this must agree with the
                        // resolver's `same_owned_pane` arm.
                        crate::client::adapters::tmux::same_owned_pane(bound, &tmux.endpoint)
                    })
                } else {
                    binding.session_id.as_ref().map(identity::SessionId::as_str)
                        == facts.session_id.as_deref()
                        && binding
                            .native_thread_id
                            .as_ref()
                            .map(NativeThreadId::as_str)
                            == facts.thread_id.as_deref()
                };
                if binding.agent_id.as_str() != ident.worker_id || !anchor_matches {
                    continue;
                }
                let Some(worker) = state.workers.get(&ident.worker_id) else {
                    continue;
                };
                let Some(transport) = worker.transport.clone() else {
                    anyhow::bail!("IDENTITY_RESTORE_CONFLICT: committed worker has no transport");
                };
                let candidate = identity::Identity {
                    worker_id: ident.worker_id.clone(),
                    token: worker.token.clone(),
                    project_scope: Some(binding.project_scope.clone()),
                    runtime: Some(RuntimeIdentity {
                        agent_id: binding.agent_id.clone(),
                        runtime_id: binding.runtime_id.clone(),
                        appserver_id: binding.app_scope_id.clone(),
                        endpoint_generation: binding.endpoint_generation,
                        binding_id: binding.binding_id.clone(),
                        session_id: binding.session_id.clone(),
                        native_thread_id: binding.native_thread_id.clone(),
                    }),
                    transport: Some(transport),
                };
                if recovered
                    .as_ref()
                    .is_some_and(|prior: &identity::Identity| {
                        prior.token != candidate.token || prior.runtime != candidate.runtime
                    })
                {
                    anyhow::bail!("IDENTITY_RESTORE_CONFLICT: current anchor has conflicting committed credentials");
                }
                recovered = Some(candidate);
            }
        }
        if let Some(recovered) = recovered {
            *ident = recovered;
        }
        Ok(())
    }
}

fn validate_facts(facts: &IdentityFacts) -> anyhow::Result<()> {
    for (name, value) in [
        ("session_id", facts.session_id.as_deref()),
        ("thread_id", facts.thread_id.as_deref()),
        ("endpoint", facts.endpoint.as_deref()),
        ("namespace", facts.namespace.as_deref()),
    ] {
        if let Some(value) = value {
            if value.trim().is_empty() {
                anyhow::bail!("IDENTITY_FACT_INVALID: {name} must not be empty");
            }
            if matches!(name, "session_id" | "thread_id") {
                identity::validate_id_for_protocol(value)?;
            } else if value.chars().any(char::is_control) {
                anyhow::bail!("IDENTITY_FACT_INVALID: {name} contains control characters");
            }
        }
    }
    if let Some(namespace) = facts.namespace.as_deref() {
        if !matches!(namespace, "codex_tui" | "codex_app") {
            anyhow::bail!("IDENTITY_FACT_INVALID: unsupported namespace {namespace}");
        }
    }
    if let Some(endpoint) = facts.endpoint.as_deref() {
        let path = endpoint
            .strip_prefix("unix://")
            .ok_or_else(|| anyhow::anyhow!("IDENTITY_FACT_INVALID: endpoint must be unix://"))?;
        if !Path::new(path).is_absolute() {
            anyhow::bail!("IDENTITY_FACT_INVALID: endpoint must contain an absolute socket path");
        }
    }
    Ok(())
}

/// Which facts the caller must still supply before an identity can be resolved.
///
/// Only the App Server path has fields the caller may have to add: its
/// endpoint, namespace, session and thread are four separate observations that
/// can arrive incomplete. The tmux and dsh paths each present one designed
/// anchor that is complete on its own, so they require nothing further.
///
/// A tmux or dsh anchor only short-circuits the request while no App Server
/// endpoint was observed. If an endpoint *is* present, the four App Server
/// facts are still demanded: the endpoint selects the App Server candidate
/// below, which reads all four fields unconditionally.
fn required_fields(facts: &IdentityFacts) -> Vec<&'static str> {
    if (facts.tmux.is_some() || facts.dsh_session_id.is_some()) && facts.endpoint.is_none() {
        return Vec::new();
    }
    [
        ("session_id", &facts.session_id),
        ("thread_id", &facts.thread_id),
        ("endpoint", &facts.endpoint),
        ("namespace", &facts.namespace),
    ]
    .into_iter()
    .filter_map(|(name, value)| value.is_none().then_some(name))
    .collect()
}

#[cfg(test)]
mod required_fields_tests {
    use super::*;
    use crate::proto::TmuxCandidate;

    fn tmux_anchor() -> TmuxCandidate {
        TmuxCandidate {
            endpoint: crate::proto::TmuxEndpoint {
                socket_path: "/tmp/tmux-test.sock".into(),
                server_pid: 42,
                tmux_session_id: "$7".into(),
                pane_id: "%3".into(),
                pane_pid: 99,
                codex_session_id: None,
                codex_thread_id: None,
            },
            cwd: "/tmp/project".into(),
        }
    }

    /// A dsh anchor with no observed App Server endpoint is complete on its own.
    #[test]
    fn dsh_anchor_alone_requires_no_appserver_fields() {
        let facts = IdentityFacts {
            dsh_session_id: Some("session-1".into()),
            ..IdentityFacts::default()
        };
        assert!(required_fields(&facts).is_empty());
    }

    /// A tmux anchor with no observed endpoint is complete on its own.
    #[test]
    fn tmux_anchor_alone_requires_no_appserver_fields() {
        let facts = IdentityFacts {
            tmux: Some(tmux_anchor()),
            ..IdentityFacts::default()
        };
        assert!(required_fields(&facts).is_empty());
    }

    /// An observed App Server endpoint selects the App Server candidate, which
    /// reads all four fields unconditionally. The anchor must not short-circuit
    /// that request: doing so reaches `expect("complete native facts")` and
    /// panics the daemon handler instead of asking the caller.
    #[test]
    fn an_observed_endpoint_still_requires_the_four_appserver_fields() {
        let facts = IdentityFacts {
            dsh_session_id: Some("session-1".into()),
            endpoint: Some("unix:///tmp/appserver.sock".into()),
            ..IdentityFacts::default()
        };
        assert_eq!(
            required_fields(&facts),
            vec!["session_id", "thread_id", "namespace"]
        );
        let facts = IdentityFacts {
            tmux: Some(tmux_anchor()),
            endpoint: Some("unix:///tmp/appserver.sock".into()),
            ..IdentityFacts::default()
        };
        assert_eq!(
            required_fields(&facts),
            vec!["session_id", "thread_id", "namespace"]
        );
    }

    /// With no anchor and no endpoint all four facts are requested.
    #[test]
    fn no_anchor_requests_all_four_fields() {
        let facts = IdentityFacts::default();
        assert_eq!(
            required_fields(&facts),
            vec!["session_id", "thread_id", "endpoint", "namespace"]
        );
    }
}
