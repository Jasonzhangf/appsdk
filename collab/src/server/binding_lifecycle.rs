//! Reconcile current adapter bindings on the existing maintenance cadence.
use super::{presence::append_log, ProjectRuntimeManager, TransportKind};
use std::collections::BTreeSet;
use std::sync::{Arc, Mutex};

// An endpoint-only gate would skip every peer except the first on a shared
// gateway. A binding owns one bounded job across both supported adapters.
static IN_FLIGHT: Mutex<BTreeSet<(std::path::PathBuf, String)>> = Mutex::new(BTreeSet::new());

pub(super) fn kick(manager: &Arc<ProjectRuntimeManager>) {
    for runtime in manager.runtimes() {
        for kind in [TransportKind::Dsh, TransportKind::DesktopBridge] {
            for binding in manager.current_adapter_bindings(&runtime, &kind) {
                let key = (
                    runtime.storage_root.clone(),
                    binding.binding_id.as_str().to_owned(),
                );
                if !IN_FLIGHT.lock().unwrap().insert(key.clone()) {
                    continue;
                }
                let owner = manager.clone();
                let cleanup_key = key.clone();
                let log_path = runtime.host_paths.log_path();
                let runtime = runtime.clone();
                let kind = kind.clone();
                let started = std::thread::Builder::new()
                    .name("collab-binding-reconcile".into())
                    .spawn(move || {
                        let result = match kind {
                            TransportKind::Dsh => owner.reconcile_dsh_binding(&runtime, &binding),
                            TransportKind::DesktopBridge => {
                                owner.reconcile_desktop_binding(&runtime, &binding)
                            }
                            _ => unreachable!(),
                        };
                        if let Err(error) = result {
                            append_log(
                                &log_path,
                                &format!(
                                    "BINDING_PENDING: binding={} error={error}",
                                    binding.binding_id.as_str()
                                ),
                            );
                        }
                        IN_FLIGHT.lock().unwrap().remove(&cleanup_key);
                    });
                if let Err(error) = started {
                    IN_FLIGHT.lock().unwrap().remove(&key);
                    append_log(
                        &manager.host.host_paths.log_path(),
                        &format!("BINDING_PENDING: cannot start reconciliation: {error}"),
                    );
                }
            }
        }
    }
}
