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
    assert_eq!(display_slot("peer-1").unwrap(), 22);
    assert_eq!(display_slot("peer-42").unwrap(), 63);
    assert_eq!(
        physical_tmux_name("/projects/app-sdk", "peer-1").unwrap(),
        "app-sdk_22_peer-1"
    );
    for invalid in ["Peer1", "peer-0", "peer-01", "peer-", "peer-one"] {
        assert!(
            display_slot(invalid).is_err(),
            "{invalid} must not be canonical"
        );
    }
}
