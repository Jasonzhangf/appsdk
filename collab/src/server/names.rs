#[path = "name_resolution.rs"]
pub(crate) mod name_resolution;

#[path = "name_mailbox.rs"]
pub(crate) mod name_mailbox;

#[cfg(test)]
#[path = "name_mailbox_tests.rs"]
mod name_mailbox_tests;

#[cfg(test)]
#[path = "name_resolution_tests.rs"]
mod name_resolution_tests;

#[cfg(test)]
#[path = "name_presence_lock_tests.rs"]
mod name_presence_lock_tests;

/// A missing current map row is shown only as its raw diagnostic worker ID.
fn worker_display_name(control: &binding_slot::BindingControlSet, route: Option<&RouteScope>, id: &str) -> Result<String, String> {
    let Some(route) = route else { return Ok(id.into()); };
    Ok(control.current_for_worker(&route.project_scope_id, &route.app_scope_id, id)?
        .map_or_else(|| id.into(), |slot| slot.identity_key.name.clone()))
}
