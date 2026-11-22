use super::name_resolution::*;
use super::name_state::*;
use super::*;

#[test]
fn physical_names_preserve_folder_and_include_the_display_slot() {
    assert_eq!(
        physical_tmux_name("/projects/app-sdk", "Jenny").unwrap(),
        "app-sdk_1_Jenny"
    );
    assert_eq!(
        physical_tmux_name("/projects/app-sdk", "Alice").unwrap(),
        "app-sdk_2_Alice"
    );
    assert!(physical_tmux_name("/projects/app-sdk", "unknown").is_err());
}

#[test]
fn generated_registration_names_have_no_fixed_display_slot() {
    assert_eq!(
        physical_tmux_name("/projects/app-sdk", "peer-17").unwrap(),
        "app-sdk_peer-17"
    );
}

#[test]
fn malformed_generated_registration_names_are_rejected() {
    for name in [
        "peer-0",
        "peer-01",
        "peer-18446744073709551616",
        "peer-x",
        "peer- 1",
        "unknown",
    ] {
        assert!(display_slot(name).is_err(), "{name} must be rejected");
    }
}

#[test]
fn generated_registration_name_cannot_claim_a_fixed_display_slot() {
    assert_eq!(display_slot("peer-17").unwrap(), None);
}
