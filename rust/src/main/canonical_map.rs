use super::*;

pub(crate) const GOVERNANCE_MAP_NAMES: [&str; 4] = [
    "resource-map.json",
    "function-map.json",
    "mainline-call-map.json",
    "verification-map.json",
];
pub(crate) const CANONICAL_RECORD_CONTRACTS: [&str; 20] = [
    "contracts/records/worktree-record.schema.json",
    "contracts/records/reproduction-record.schema.json",
    "contracts/records/evidence-record.schema.json",
    "contracts/records/fix-candidate-record.schema.json",
    "contracts/records/goal-clarification-record.schema.json",
    "contracts/records/review-record.schema.json",
    "contracts/records/effectiveness-record.schema.json",
    "contracts/records/pre-review-validation-record.schema.json",
    "contracts/records/collaboration-record.schema.json",
    "contracts/records/collaboration-index.schema.json",
    "contracts/records/merge-queue-record.schema.json",
    "contracts/records/merge-queue-state.schema.json",
    "contracts/records/integration-record.schema.json",
    "contracts/records/mainline-receipt-record.schema.json",
    "contracts/records/collab-live-closure-record.schema.json",
    "contracts/records/merge-record.schema.json",
    "contracts/records/promotion-record.schema.json",
    "contracts/records/regression-report.schema.json",
    "contracts/records/freeze-record.schema.json",
    "contracts/records/record-graph.contract.json",
];

pub(crate) const SDK_MAP_MIGRATION_0_1_5_TO_0_1_6: &str =
    include_str!("../../../contracts/migrations/sdk-0.1.5-to-0.1.6.json");
pub(crate) const SDK_MAP_MIGRATION_0_1_6_TO_0_1_0007: &str =
    include_str!("../../../contracts/migrations/sdk-0.1.6-to-0.1.0007.json");
pub(crate) const SDK_MAP_MIGRATION_0_1_0007_TO_0_1_0008: &str =
    include_str!("../../../contracts/migrations/sdk-0.1.0007-to-0.1.0008.json");
pub(crate) const SDK_MAP_MIGRATION_0_1_0008_TO_0_1_0009: &str =
    include_str!("../../../contracts/migrations/sdk-0.1.0008-to-0.1.0009.json");
pub(crate) const SDK_MAP_MIGRATION_0_1_0009_TO_0_1_0010: &str =
    include_str!("../../../contracts/migrations/sdk-0.1.0009-to-0.1.0010.json");
pub(crate) const SDK_MAP_MIGRATION_0010_TO_0011: &str =
    include_str!("../../../contracts/migrations/sdk-0.1.0010-to-0.1.0011.json");
pub(crate) const SDK_MAP_MIGRATION_0011_TO_0012: &str =
    include_str!("../../../contracts/migrations/sdk-0.1.0011-to-0.1.0012.json");
pub(crate) const SDK_MAP_MIGRATION_0012_TO_0013: &str =
    include_str!("../../../contracts/migrations/sdk-0.1.0012-to-0.1.0013.json");
pub(crate) const SDK_MAP_MIGRATION_0013_TO_0014: &str =
    include_str!("../../../contracts/migrations/sdk-0.1.0013-to-0.1.0014.json");
pub(crate) const SDK_MAP_MIGRATION_0014_TO_0015: &str =
    include_str!("../../../contracts/migrations/sdk-0.1.0014-to-0.1.0015.json");
pub(crate) const SDK_MAP_MIGRATION_STEPS: [&str; 10] = [
    "0.1.5-to-0.1.6",
    "0.1.6-to-0.1.0007",
    "0.1.0007-to-0.1.0008",
    "0.1.0008-to-0.1.0009",
    "0.1.0009-to-0.1.0010",
    "0.1.0010-to-0.1.0011",
    "0.1.0011-to-0.1.0012",
    "0.1.0012-to-0.1.0013",
    "0.1.0013-to-0.1.0014",
    "0.1.0014-to-0.1.0015",
];

pub(super) fn historical_0_1_0010_map(name: &str) -> &'static str {
    match name {
        "resource-map.json" => {
            include_str!("../../../contracts/migrations/0.1.0010/governance-maps/resource-map.json")
        }
        "function-map.json" => {
            include_str!("../../../contracts/migrations/0.1.0010/governance-maps/function-map.json")
        }
        "mainline-call-map.json" => include_str!(
            "../../../contracts/migrations/0.1.0010/governance-maps/mainline-call-map.json"
        ),
        "verification-map.json" => include_str!(
            "../../../contracts/migrations/0.1.0010/governance-maps/verification-map.json"
        ),
        _ => fail("UNKNOWN_GOVERNANCE_MAP"),
    }
}

pub(super) fn historical_0_1_0011_map(name: &str) -> &'static str {
    match name {
        "resource-map.json" => {
            include_str!("../../../contracts/migrations/0.1.0011/governance-maps/resource-map.json")
        }
        "function-map.json" => {
            include_str!("../../../contracts/migrations/0.1.0011/governance-maps/function-map.json")
        }
        "mainline-call-map.json" => include_str!(
            "../../../contracts/migrations/0.1.0011/governance-maps/mainline-call-map.json"
        ),
        "verification-map.json" => include_str!(
            "../../../contracts/migrations/0.1.0011/governance-maps/verification-map.json"
        ),
        _ => fail("UNKNOWN_GOVERNANCE_MAP"),
    }
}

pub(super) fn historical_0_1_0012_map(name: &str) -> &'static str {
    match name {
        "resource-map.json" => {
            include_str!("../../../contracts/migrations/0.1.0012/governance-maps/resource-map.json")
        }
        "function-map.json" => {
            include_str!("../../../contracts/migrations/0.1.0012/governance-maps/function-map.json")
        }
        "mainline-call-map.json" => include_str!(
            "../../../contracts/migrations/0.1.0012/governance-maps/mainline-call-map.json"
        ),
        "verification-map.json" => include_str!(
            "../../../contracts/migrations/0.1.0012/governance-maps/verification-map.json"
        ),
        _ => fail("UNKNOWN_GOVERNANCE_MAP"),
    }
}

pub(super) fn historical_0_1_0013_map(name: &str) -> &'static str {
    match name {
        "resource-map.json" => {
            include_str!("../../../contracts/migrations/0.1.0013/governance-maps/resource-map.json")
        }
        "function-map.json" => {
            include_str!("../../../contracts/migrations/0.1.0013/governance-maps/function-map.json")
        }
        "mainline-call-map.json" => include_str!(
            "../../../contracts/migrations/0.1.0013/governance-maps/mainline-call-map.json"
        ),
        "verification-map.json" => include_str!(
            "../../../contracts/migrations/0.1.0013/governance-maps/verification-map.json"
        ),
        _ => fail("UNKNOWN_GOVERNANCE_MAP"),
    }
}

pub(super) fn historical_0_1_0014_map(name: &str) -> &'static str {
    match name {
        "resource-map.json" => {
            include_str!("../../../contracts/migrations/0.1.0014/governance-maps/resource-map.json")
        }
        "function-map.json" => {
            include_str!("../../../contracts/migrations/0.1.0014/governance-maps/function-map.json")
        }
        "mainline-call-map.json" => include_str!(
            "../../../contracts/migrations/0.1.0014/governance-maps/mainline-call-map.json"
        ),
        "verification-map.json" => include_str!(
            "../../../contracts/migrations/0.1.0014/governance-maps/verification-map.json"
        ),
        _ => fail("UNKNOWN_GOVERNANCE_MAP"),
    }
}

pub(crate) fn canonical_governance_map(name: &str) -> &'static str {
    match name {
        "resource-map.json" => include_str!("../../../contracts/maps/resource-map.json"),
        "function-map.json" => include_str!("../../../contracts/maps/function-map.json"),
        "mainline-call-map.json" => {
            include_str!("../../../contracts/maps/mainline-call-map.json")
        }
        "verification-map.json" => {
            include_str!("../../../contracts/maps/verification-map.json")
        }
        _ => fail("UNKNOWN_GOVERNANCE_MAP"),
    }
}

pub(crate) fn historical_governance_map(version: &str, name: &str) -> &'static str {
    match (version, name) {
        ("0.1.5", "resource-map.json") => {
            include_str!("../../../contracts/migrations/0.1.5/governance-maps/resource-map.json")
        }
        ("0.1.5", "function-map.json") => {
            include_str!("../../../contracts/migrations/0.1.5/governance-maps/function-map.json")
        }
        ("0.1.5", "mainline-call-map.json") => {
            include_str!(
                "../../../contracts/migrations/0.1.5/governance-maps/mainline-call-map.json"
            )
        }
        ("0.1.5", "verification-map.json") => {
            include_str!(
                "../../../contracts/migrations/0.1.5/governance-maps/verification-map.json"
            )
        }
        ("0.1.6", "resource-map.json") => {
            include_str!("../../../contracts/migrations/0.1.6/governance-maps/resource-map.json")
        }
        ("0.1.6", "function-map.json") => {
            include_str!("../../../contracts/migrations/0.1.6/governance-maps/function-map.json")
        }
        ("0.1.6", "mainline-call-map.json") => {
            include_str!(
                "../../../contracts/migrations/0.1.6/governance-maps/mainline-call-map.json"
            )
        }
        ("0.1.6", "verification-map.json") => {
            include_str!(
                "../../../contracts/migrations/0.1.6/governance-maps/verification-map.json"
            )
        }
        ("0.1.0007", "resource-map.json") => {
            include_str!("../../../contracts/migrations/0.1.0007/governance-maps/resource-map.json")
        }
        ("0.1.0007", "function-map.json") => {
            include_str!("../../../contracts/migrations/0.1.0007/governance-maps/function-map.json")
        }
        ("0.1.0007", "mainline-call-map.json") => include_str!(
            "../../../contracts/migrations/0.1.0007/governance-maps/mainline-call-map.json"
        ),
        ("0.1.0007", "verification-map.json") => include_str!(
            "../../../contracts/migrations/0.1.0007/governance-maps/verification-map.json"
        ),
        ("0.1.0008", "resource-map.json") => {
            include_str!("../../../contracts/migrations/0.1.0008/governance-maps/resource-map.json")
        }
        ("0.1.0008", "function-map.json") => {
            include_str!("../../../contracts/migrations/0.1.0008/governance-maps/function-map.json")
        }
        ("0.1.0008", "mainline-call-map.json") => include_str!(
            "../../../contracts/migrations/0.1.0008/governance-maps/mainline-call-map.json"
        ),
        ("0.1.0008", "verification-map.json") => include_str!(
            "../../../contracts/migrations/0.1.0008/governance-maps/verification-map.json"
        ),
        ("0.1.0009", "resource-map.json") => {
            include_str!("../../../contracts/migrations/0.1.0009/governance-maps/resource-map.json")
        }
        ("0.1.0009", "function-map.json") => {
            include_str!("../../../contracts/migrations/0.1.0009/governance-maps/function-map.json")
        }
        ("0.1.0009", "mainline-call-map.json") => include_str!(
            "../../../contracts/migrations/0.1.0009/governance-maps/mainline-call-map.json"
        ),
        ("0.1.0009", "verification-map.json") => include_str!(
            "../../../contracts/migrations/0.1.0009/governance-maps/verification-map.json"
        ),
        ("0.1.0010", name) => historical_0_1_0010_map(name),
        ("0.1.0011", name) => historical_0_1_0011_map(name),
        ("0.1.0012", name) => historical_0_1_0012_map(name),
        ("0.1.0013", name) => historical_0_1_0013_map(name),
        ("0.1.0014", name) => historical_0_1_0014_map(name),
        _ => fail("UNKNOWN_GOVERNANCE_MAP"),
    }
}
