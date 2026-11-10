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
fn generated_names_have_no_fixed_display_slot() {
    assert_eq!(display_slot("peer-21").unwrap(), None);
    assert_eq!(
        physical_tmux_name("/projects/app-sdk", "peer-21").unwrap(),
        "app-sdk_peer-21",
        "a generated free name must not claim a fixed display slot"
    );
    assert!(
        display_slot("peer 21").is_err(),
        "a malformed free name is not a valid display-slot source"
    );
}
