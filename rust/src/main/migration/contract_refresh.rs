use super::*;

pub(crate) fn install_current_project_contract(
    root: &Path,
    relative: &str,
    canonical: &str,
    replace_legacy: bool,
) -> bool {
    let target = root.join(relative);
    assert_no_symlink_components(root, &target, "governance_contract_migration");
    let canonical: Value = serde_json::from_str(canonical)
        .unwrap_or_else(|_| fail("INVALID_CANONICAL_RECORD_CONTRACT"));
    if !replace_legacy {
        if target.is_file() {
            let current: Value = serde_json::from_str(
                &fs::read_to_string(&target)
                    .unwrap_or_else(|_| fail("SDK_RECORD_CONTRACT_MIGRATION_READ_FAILED")),
            )
            .unwrap_or_else(|_| fail("SDK_RECORD_CONTRACT_MIGRATION_READ_FAILED"));
            if current == canonical {
                return false;
            }
        }
    }
    let mut content = serde_json::to_vec_pretty(&canonical)
        .unwrap_or_else(|_| fail("SDK_RECORD_CONTRACT_MIGRATION_WRITE_FAILED"));
    content.push(b'\n');
    if let Some(parent) = target.parent() {
        fs::create_dir_all(parent)
            .unwrap_or_else(|_| fail("SDK_RECORD_CONTRACT_MIGRATION_WRITE_FAILED"));
        assert_no_symlink_components(root, parent, "governance_contract_migration");
    }
    atomic_write_bytes(
        &target,
        &content,
        "SDK_RECORD_CONTRACT_MIGRATION_WRITE_FAILED",
    );
    true
}

pub(crate) fn install_current_project_contracts(
    root: &Path,
    prefixes: &[&str],
    replace_legacy: bool,
) -> bool {
    let mut changed = false;
    for &(relative, _, canonical) in SDK_BUNDLE_RESOURCES
        .iter()
        .filter(|(path, _, _)| prefixes.iter().any(|prefix| path.starts_with(prefix)))
    {
        changed |= install_current_project_contract(root, relative, canonical, replace_legacy);
    }
    changed
}

pub(crate) fn install_current_record_contracts(root: &Path) -> bool {
    install_current_project_contracts(root, &["contracts/records/"], false)
}

const CANONICAL_ZONE_TRANSITION_CONTRACT_PATH: &str =
    "contracts/transitions/zone-transition.manifest.json";
const LEGACY_ZONE_TRANSITION_CONTRACT_PATH: &str =
    "contracts/transitions/zone-transition-manifest.json";

// Official canonical runtime blobs before ed649d9, derived from tagged source:
// v0.1.3 -> a6468f..., v0.1.4 -> 6c485a..., v0.1.5/v0.1.6 -> 456866...
const TRUSTED_LEGACY_ZONE_TRANSITION_CONTRACTS: [&str; 3] = [
    "sha256:a6468f12b64d3e0125ddd77828a4eeeee48cf3a38a0ee6d5bfe56935cd8a1957",
    "sha256:6c485a138ab5a657b760969be42b167ebd034f8a43446609505c2f5d16d5afab",
    "sha256:4568668437b4e0b44db4709d27e31c2783c8a6e4ccd828273a4675775f69ca1f",
];

pub(crate) struct TransitionContractRefreshPlan {
    targets: Vec<TransitionContractRefreshTarget>,
}

struct TransitionContractRefreshTarget {
    relative: &'static str,
    write: bool,
}

pub(crate) fn preflight_current_transition_contracts(
    root: &Path,
    project: &Value,
) -> TransitionContractRefreshPlan {
    let declared = project
        .pointer("/governance/zone_transition_contract")
        .and_then(Value::as_str)
        .unwrap_or_else(|| {
            fail("INVALID_GOVERNANCE_CONTRACT:/governance/zone_transition_contract")
        });
    let relatives = match declared {
        CANONICAL_ZONE_TRANSITION_CONTRACT_PATH => {
            vec![CANONICAL_ZONE_TRANSITION_CONTRACT_PATH]
        }
        LEGACY_ZONE_TRANSITION_CONTRACT_PATH => vec![
            CANONICAL_ZONE_TRANSITION_CONTRACT_PATH,
            LEGACY_ZONE_TRANSITION_CONTRACT_PATH,
        ],
        _ => fail(format!("UNSUPPORTED_ZONE_TRANSITION_CONTRACT:{declared}")),
    };
    let mut targets = Vec::with_capacity(relatives.len());
    for relative in relatives {
        let target = safe_owned_path(root, relative, "zone_transition_contract");
        let write = match fs::symlink_metadata(&target) {
            Ok(metadata) if !metadata.is_file() => {
                fail(format!("GOVERNANCE_CONTRACT_NOT_FILE:{relative}"));
            }
            Ok(_) => {
                let bytes = fs::read(&target).unwrap_or_else(|_| {
                    fail(format!("SDK_TRANSITION_CONTRACT_READ_FAILED:{relative}"))
                });
                if bytes == CANONICAL_ZONE_TRANSITION_CONTRACT.as_bytes() {
                    false
                } else if trusted_legacy_zone_transition_contract(&bytes) {
                    true
                } else {
                    fail(format!(
                        "SDK_TRANSITION_CONTRACT_UNKNOWN_CONTENT:{relative}"
                    ));
                }
            }
            Err(error) if error.kind() == ErrorKind::NotFound => true,
            Err(_) => fail(format!(
                "SDK_TRANSITION_CONTRACT_METADATA_FAILED:{relative}"
            )),
        };
        targets.push(TransitionContractRefreshTarget { relative, write });
    }
    TransitionContractRefreshPlan { targets }
}

pub(crate) fn install_current_transition_contracts(
    root: &Path,
    plan: &TransitionContractRefreshPlan,
) -> bool {
    let mut changed = false;
    for target in &plan.targets {
        if !target.write {
            continue;
        }
        let path = root.join(target.relative);
        assert_no_symlink_components(root, &path, "zone_transition_contract");
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)
                .unwrap_or_else(|_| fail("SDK_TRANSITION_CONTRACT_WRITE_FAILED"));
            assert_no_symlink_components(root, parent, "zone_transition_contract");
        }
        atomic_write_bytes(
            &path,
            CANONICAL_ZONE_TRANSITION_CONTRACT.as_bytes(),
            "SDK_TRANSITION_CONTRACT_WRITE_FAILED",
        );
        changed = true;
    }
    changed
}

fn trusted_legacy_zone_transition_contract(bytes: &[u8]) -> bool {
    let digest = digest_bytes(bytes);
    TRUSTED_LEGACY_ZONE_TRANSITION_CONTRACTS
        .iter()
        .any(|known| *known == digest.as_str())
}

pub(crate) fn assert_fresh_project_contract_target(root: &Path, relative: &str) {
    let target = root.join(relative);
    assert_no_symlink_components(root, &target, "governance_contract_migration");
    match fs::symlink_metadata(&target) {
        Ok(metadata) if !metadata.is_file() => {
            fail(format!("GOVERNANCE_CONTRACT_NOT_FILE:{}", relative));
        }
        Ok(_) => {}
        Err(error) if error.kind() != ErrorKind::NotFound => {
            fail(format!("GOVERNANCE_CONTRACT_METADATA_FAILED:{}", relative));
        }
        Err(_) => {}
    }
}

pub(crate) fn assert_fresh_project_contract_targets(root: &Path) {
    for &(relative, _, _) in SDK_BUNDLE_RESOURCES.iter().filter(|(path, _, _)| {
        path.starts_with("contracts/records/") || path.starts_with("contracts/transitions/")
    }) {
        assert_fresh_project_contract_target(root, relative);
    }
    assert_fresh_project_contract_target(
        root,
        "contracts/transitions/zone-transition-manifest.json",
    );
}
