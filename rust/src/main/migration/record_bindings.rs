use super::*;

pub(super) fn preflight_record_map_bindings(
    manifest: &Value,
    maps: &[Value],
    step: &str,
    record_bundle: &str,
    bundle_transition: bool,
) {
    for name in GOVERNANCE_MAP_NAMES {
        let declared = sdk_map_migration_entry(manifest, name);
        let entry = maps
            .iter()
            .find(|entry| entry.get("name").and_then(Value::as_str) == Some(name))
            .unwrap_or_else(|| fail("INVALID_SDK_MIGRATION_RECORD"));
        let canonical_source = entry
            .get("canonical_source_digest")
            .or_else(|| entry.get("source_digest"))
            .unwrap_or_else(|| fail("INVALID_SDK_MIGRATION_RECORD"));
        let canonical_target = entry
            .get("canonical_target_digest")
            .or_else(|| entry.get("target_digest"))
            .unwrap_or_else(|| fail("INVALID_SDK_MIGRATION_RECORD"));
        let explicit_custom_source = entry
            .get("canonical_source_digest")
            .is_some_and(|value| !value.is_null());
        let explicit_custom_target = entry
            .get("canonical_target_digest")
            .is_some_and(|value| !value.is_null());
        if explicit_custom_source && Some(canonical_source) != declared.get("source_digest") {
            fail("INVALID_SDK_MIGRATION_RECORD");
        }
        if explicit_custom_target && Some(canonical_target) != declared.get("target_digest") {
            let historical_target_authorized = bundle_transition
                && !sdk_map_migration_checks_live_target(step)
                && explicit_custom_source
                && Some(canonical_source) == declared.get("source_digest")
                && canonical_target.as_str().is_some_and(|target| {
                    sdk_map_migration_historical_target_authorized(declared, record_bundle, target)
                });
            let custom_target_bound = bundle_transition
                && explicit_custom_source
                && Some(canonical_source) == declared.get("source_digest")
                && canonical_target.as_str().is_some_and(valid_bundle_digest)
                && (sdk_map_migration_checks_live_target(step)
                    || Some(canonical_target) == entry.get("target_digest"));
            if !historical_target_authorized && !custom_target_bound {
                fail("INVALID_SDK_MIGRATION_RECORD");
            }
        }
    }
}
