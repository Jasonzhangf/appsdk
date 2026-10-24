// Daemon-owned convergence reads the host-verified current capability.
use crate::proto::TransportKind;
use crate::scope::Scope;
use crate::server::global_state::RuntimeBinding;
use std::sync::Arc;

impl ProjectRuntimeManager {
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
            Err(error)
                if error
                    .downcast_ref::<std::io::Error>()
                    .is_some_and(|error| error.kind() == std::io::ErrorKind::NotFound) =>
            {
                return Ok(())
            }
            Err(error) => return Err(error),
        };
        if transport.endpoint.as_deref() == Some(candidate.endpoint.as_str()) {
            // Same owned socket: nothing to converge, no generation churn.
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
