// Daemon-owned convergence reads the host-verified current capability.
use crate::proto::TransportKind;
use crate::scope::Scope;
use crate::server::global_state::RuntimeBinding;
use std::sync::Arc;

impl ProjectRuntimeManager {
    /// Only an exact currently published Desktop binding makes AppServer
    /// registration fields inapplicable. No archive or endpoint probe selects
    /// identity, and other transport kinds retain their own requirements.
    fn has_current_desktop_binding(
        &self,
        context: &ProjectContext,
        facts: &IdentityFacts,
    ) -> anyhow::Result<bool> {
        if facts.tmux.is_some() || facts.dsh_session_id.is_some() || facts.dsh.is_some()
            || facts.desktop_bridge.is_some() || facts.endpoint.is_some()
        {
            return Ok(false);
        }
        let (Some(session), Some(thread)) = (facts.session_id.as_deref(), facts.thread_id.as_deref()) else {
            return Ok(false);
        };
        let Some(ident) = self.current_session_identity(context, facts)? else { return Ok(false) };
        if !ident.transport.as_ref().is_some_and(|transport|
            transport.kind == TransportKind::DesktopBridge
                && transport.session_id.as_deref() == Some(session)
                && transport.thread_id.as_deref() == Some(thread)) {
            return Ok(false);
        }
        let host = self.host.state.lock().unwrap();
        Ok(host.global.current_thread_routes.values().any(|binding|
            binding.project_scope == context.project_scope && binding.app_scope_id == context.app_scope_id
                && binding.agent_id.as_str() == ident.worker_id
                && binding.session_id.as_ref().map(identity::SessionId::as_str) == Some(session)
                && binding.native_thread_id.as_ref().map(identity::NativeThreadId::as_str) == Some(thread)
                && ident.runtime.as_ref().is_some_and(|runtime|
                    runtime.binding_id == binding.binding_id && runtime.endpoint_generation == binding.endpoint_generation)))
    }
    /// Reconcile one Desktop bridge binding to its real, host-verified
    /// published endpoint. Discovery is scoped to the committed project scope
    /// and session/thread; the adapter re-verifies the live thread before any
    /// generation advance, so a down socket or an unverifiable declaration
    /// stays pending rather than overwriting the current binding.
    pub(super) fn reconcile_desktop_binding(
        &self,
        runtime: &Arc<Server>,
        saved: &RuntimeBinding,
    ) -> anyhow::Result<()> {
        let context_identity = RuntimeIdentity {
            agent_id: saved.agent_id.clone(),
            runtime_id: saved.runtime_id.clone(),
            appserver_id: saved.app_scope_id.clone(),
            binding_id: saved.binding_id.clone(),
            endpoint_generation: saved.endpoint_generation,
            session_id: saved.session_id.clone(),
            native_thread_id: saved.native_thread_id.clone(),
        };
        let context = ProjectContext::for_registered_route(&runtime.root, &context_identity)?;
        let mut ident = {
            let state = runtime.state.lock().unwrap();
            let current = state
                .global
                .lookup_binding_for(&saved.route_scope(), &saved.binding_id);
            if current != Some(saved) {
                anyhow::bail!("RECOVERY_RECONCILE_REQUIRED: binding changed");
            }
            self.recovered_binding_identity(&context, saved, &state)
                .ok_or_else(|| anyhow::anyhow!("IDENTITY_WORKER_MISSING"))?
        };
        let transport = ident
            .transport
            .clone()
            .ok_or_else(|| anyhow::anyhow!("IDENTITY_TRANSPORT_MISSING"))?;
        if transport.kind != TransportKind::DesktopBridge {
            return Ok(());
        }
        let session = saved
            .session_id
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("DESKTOP_SESSION_MISSING"))?;
        let thread = saved
            .native_thread_id
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("DESKTOP_THREAD_MISSING"))?;
        let cwd = runtime.root.clone();
        // Read the publisher-owned, host-verified current declaration. The
        // discovery validates the same scope and rejects foreign or unsafe
        // control; an absent declaration simply keeps the enrolled binding.
        let candidate = match crate::desktop_capability::discover(
            &self.host.host_paths.state_root(),
            &cwd,
            Some(session.as_str()),
            Some(thread.as_str()),
        ) {
            Ok(candidate) => candidate,
            Err(error) if error.is::<crate::desktop_capability::DeclarationMissing>() =>
            {
                if !transport.capabilities.iter().any(|capability| capability == "read_thread") {
                    anyhow::bail!("DESKTOP_BRIDGE_CAPABILITY_UNVERIFIED: current Desktop binding has no verified read_thread capability; no peer update inferred");
                }
                // Read-only readiness of this exact committed channel. This
                // is not a registration challenge or a message replay.
                let committed = crate::proto::AppServerCandidate {
                    endpoint: transport.endpoint.clone().ok_or_else(|| anyhow::anyhow!("DESKTOP_ENDPOINT_MISSING"))?,
                    namespace: transport.namespace.clone().ok_or_else(|| anyhow::anyhow!("DESKTOP_NAMESPACE_MISSING"))?,
                    session_id: session.as_str().to_owned(),
                    thread_id: thread.as_str().to_owned(),
                    cwd: cwd.to_string_lossy().into_owned(),
                };
                return crate::client::adapters::desktop_bridge::verify_candidate(&committed)
                    .map(|_| ())
                    .map_err(|failure| anyhow::anyhow!("DESKTOP_BRIDGE_CAPABILITY_LOST: verified Desktop read_thread is unavailable and its publication is missing; ask this Desktop peer to refresh its bridge declaration: {failure}"));
            }
            Err(error) => return Err(error),
        };
        if transport.endpoint.as_deref() == Some(candidate.endpoint.as_str())
            && crate::desktop_capability::ping_candidate(&candidate).is_ok()
        {
            // Same owned socket and a live publisher: nothing to converge and
            // no generation churn. A matching endpoint string alone must not
            // hide a dead publisher.
            return Ok(());
        }
        // Re-verify the candidate against the live native thread before
        // advancing the generation. This reuses the exact registration
        // contract; no test-message challenge and no fabricated caller facts.
        let verified = crate::client::adapters::desktop_bridge::verify_candidate(&candidate)
            .map_err(anyhow::Error::msg)?;
        {
            let _guard = self.identity_gate.lock().unwrap();
            let state = runtime.state.lock().unwrap();
            if state
                .global
                .lookup_binding_for(&saved.route_scope(), &saved.binding_id)
                != Some(saved)
                || !state.workers.get(&ident.worker_id).is_some_and(|worker| {
                    worker.token == ident.token && worker.transport.as_ref() == Some(&transport)
                })
            {
                anyhow::bail!("RECOVERY_RECONCILE_REQUIRED: binding changed during discovery");
            }
            drop(state);
            self.refresh_session_transport(
                runtime,
                saved,
                verified,
                &mut ident,
                &Scope { root: cwd },
            )?;
        }
        Ok(())
    }
}
