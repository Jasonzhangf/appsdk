use super::*;

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
