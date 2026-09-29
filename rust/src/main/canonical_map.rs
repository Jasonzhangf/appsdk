use super::*;

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
