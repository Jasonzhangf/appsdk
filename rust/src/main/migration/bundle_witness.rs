use super::*;

/// Independently proven historical SDK bundle transitions.
///
/// Each entry binds the `bundle_digest` that authored a legacy migration record
/// to the complete canonical governance-map target tuple of the SDK bundle the
/// record transitioned into. Both digests are recovered from SDK source history
/// with the real bundle algorithm; they are never derived from a consumer's
/// record or lock, so a record's own `bundle_digest` can never authorize itself.
///
/// 4813363d is the bundle of SDK 99ca3788b4fe3ca4008600f14378d418864513bd. The
/// target tuple is the `contracts/maps/*` content of SDK
/// 916cf9d88e32304f52aa1cb3ecbfa86b222b1805 (bundle b3bd7c8e), which is the
/// immediate successor the legacy record transitioned into.
struct HistoricalBundleTransition {
    step: &'static str,
    record_bundle: &'static str,
    target_digests: [&'static str; 4],
}

const HISTORICAL_BUNDLE_TRANSITIONS: &[HistoricalBundleTransition] =
    &[HistoricalBundleTransition {
        step: "0.1.5-to-0.1.6",
        record_bundle: "sha256:4813363da77e9eec678bb7e4bbc8664beec920479aa61677c5991d4e45f82018",
        target_digests: [
            "sha256:373f7121a351f87c7126c7b163784190a0c5393edc9ddccb5c776a9c1224246b",
            "sha256:c0fbf20f6e697e3ea71d424f9d550eb22821eefeee2d7568447f2689a215c0d6",
            "sha256:d8964f67c3d7e51e5131a0a5fffc011462198231328f7e5644de5740095ca8f6",
            "sha256:f873ccad5a2590a1cec2a516a1ef9ebda36edab4023f18e4d69e211fe3ce7668",
        ],
    }];

fn historical_bundle_transition(
    step: &str,
    record_bundle: &str,
) -> Option<&'static HistoricalBundleTransition> {
    HISTORICAL_BUNDLE_TRANSITIONS
        .iter()
        .find(|transition| transition.step == step && transition.record_bundle == record_bundle)
}

pub(super) fn is_historical_bundle_transition(step: &str, record_bundle: &str) -> bool {
    historical_bundle_transition(step, record_bundle).is_some()
}

/// Legacy `0.1.5-to-0.1.6` records predate canonical map bindings, so their
/// `canonical_source_digest`/`canonical_target_digest` are null. Accept their
/// complete target tuple only when it equals an independently proven
/// historical tuple and the recorded source is the canonical migration source.
pub(super) fn historical_target_matches(
    step: &str,
    record_bundle: &str,
    declared: &Value,
    name: &str,
    source_digest: &str,
    target_digest: &str,
) -> bool {
    let Some(transition) = historical_bundle_transition(step, record_bundle) else {
        return false;
    };
    let Some(index) = GOVERNANCE_MAP_NAMES.iter().position(|entry| *entry == name) else {
        return false;
    };
    declared.get("source_digest").and_then(Value::as_str) == Some(source_digest)
        && transition.target_digests[index] == target_digest
}

/// Whether an unanchored legacy record's complete four-map tuple equals the one
/// independently proven historical transition.
///
/// This is the atomic trust boundary for a historical record: every map's null
/// canonical binding, source digest and target digest must agree with the
/// proven transition together, so a mixture of historical and current targets
/// or an explicit custom binding is never accepted.
pub(super) fn historical_legacy_tuple_matches(
    step: &str,
    record_bundle: &str,
    manifest: &Value,
    maps: &[Value],
) -> bool {
    let Some(transition) = historical_bundle_transition(step, record_bundle) else {
        return false;
    };
    let Some(declared_maps) = manifest.get("maps").and_then(Value::as_array) else {
        return false;
    };
    GOVERNANCE_MAP_NAMES
        .iter()
        .enumerate()
        .all(|(index, name)| {
            let declared = declared_maps
                .iter()
                .find(|entry| entry.get("name").and_then(Value::as_str) == Some(*name));
            let entry = maps
                .iter()
                .find(|entry| entry.get("name").and_then(Value::as_str) == Some(*name));
            let (Some(declared), Some(entry)) = (declared, entry) else {
                return false;
            };
            let custom_source = entry
                .get("canonical_source_digest")
                .is_some_and(|value| !value.is_null());
            let custom_target = entry
                .get("canonical_target_digest")
                .is_some_and(|value| !value.is_null());
            let source_matches = entry.get("source_digest").and_then(Value::as_str)
                == declared.get("source_digest").and_then(Value::as_str);
            let target_matches = entry.get("target_digest").and_then(Value::as_str)
                == Some(transition.target_digests[index]);
            !custom_source && !custom_target && source_matches && target_matches
        })
}

/// Whether a migration record's SDK bundle is authorized by the on-disk lock or
/// only by the independently proven historical transition table.
pub(super) fn bundle_authority(root: &Path, step: &str, record: &Value) -> Option<BundleAuthority> {
    let record_bundle = record
        .get("bundle_digest")
        .and_then(Value::as_str)
        .filter(|digest| valid_bundle_digest(digest))?;
    let lock_path = root.join(".appsdk/sdk.lock");
    if !lock_path.is_file() {
        return None;
    }
    if fs::symlink_metadata(&lock_path)
        .map(|metadata| metadata.file_type().is_symlink())
        .unwrap_or(false)
    {
        fail("GOVERNANCE_PATH_SYMLINK:sdk_lock");
    }
    let lock: Value = serde_json::from_str(
        &fs::read_to_string(&lock_path).unwrap_or_else(|_| fail("INVALID_SDK_LOCK")),
    )
    .unwrap_or_else(|_| fail("INVALID_SDK_LOCK"));
    let lock_bundle = lock
        .get("bundle_digest")
        .and_then(Value::as_str)
        .filter(|digest| valid_bundle_digest(digest))?;
    let lock_previous_bundles = lock
        .get("previous_bundle_digests")
        .and_then(Value::as_array)
        .map(|digests| {
            digests
                .iter()
                .filter_map(Value::as_str)
                .map(str::to_owned)
                .collect::<Vec<_>>()
        })
        .unwrap_or_else(|| {
            lock.get("previous_bundle_digest")
                .and_then(Value::as_str)
                .into_iter()
                .map(str::to_owned)
                .collect()
        });
    let current_bundle = sdk_bundle_digest();
    if record_bundle == current_bundle {
        return None;
    }
    // A bundle the lock still remembers keeps the existing anchored behavior.
    if lock_bundle == record_bundle
        || lock_previous_bundles
            .iter()
            .any(|known| known == record_bundle)
    {
        return Some(BundleAuthority::LockAnchored);
    }
    // A record authored by an SDK bundle the lock no longer remembers is only
    // accepted when the SDK independently proves that historical transition.
    if is_historical_bundle_transition(step, record_bundle) {
        return Some(BundleAuthority::Historical);
    }
    None
}

pub(super) fn sdk_migration_bundle_witnesses(root: &Path) -> Vec<String> {
    let lock_path = root.join(".appsdk/sdk.lock");
    let lock_witnesses = if lock_path.is_file() {
        let lock: Value = serde_json::from_str(
            &fs::read_to_string(&lock_path).unwrap_or_else(|_| fail("INVALID_SDK_LOCK")),
        )
        .unwrap_or_else(|_| fail("INVALID_SDK_LOCK"));
        lock_migration_bundle_witnesses(&lock)
    } else {
        Vec::new()
    };
    // A record's own `bundle_digest` may only become a durable, bundle-wide lock
    // witness when the whole record (maps, snapshots, metadata) validates. This
    // collector runs before any consumer write, so a malformed historical record
    // cannot seed a witness for an unrelated migration step.
    let mut records = Vec::new();
    for step in SDK_MAP_MIGRATION_STEPS {
        let record_path = sdk_map_migration_root(root, step).join("record.json");
        if !record_path.is_file() {
            continue;
        }
        let record = super::assert_sdk_migration_record(root, step, false)
            .unwrap_or_else(|| fail("INVALID_SDK_MIGRATION_RECORD"));
        if let Some(digest) = record
            .get("bundle_digest")
            .and_then(Value::as_str)
            .filter(|digest| valid_bundle_digest(digest))
        {
            if digest != sdk_bundle_digest() {
                records.push(digest.to_string());
            }
        }
    }
    if records.is_empty() {
        return Vec::new();
    }
    let mut witnesses = lock_witnesses;
    for digest in records {
        if !witnesses.iter().any(|known| known == &digest) {
            witnesses.push(digest);
        }
    }
    witnesses
}
