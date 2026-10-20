use super::*;

#[test]
fn role_patrol_peer_deadline_is_finite_owned_and_consumable() {
    let fixture = AppFixture::new();
    fixture.run_public(&["context"], THREAD_A);
    let peer = fixture.run_public(&["context"], THREAD_B);
    let peer_id = peer["identity"]["worker_id"].as_str().unwrap();
    fixture.run_public(
        &["master", "promote", "--approval", "private patrol test"],
        THREAD_A,
    );
    let subscribed = fixture.run_public(
        &[
            "notify",
            "subscribe",
            "--event",
            "deadline",
            "--subject",
            "patrol:assigned-work",
            "--every-ms",
            "100",
            "--repeat-count",
            "2",
            "--ttl-seconds",
            "30",
        ],
        THREAD_B,
    );
    let subscription_id = subscribed["subscription"]["id"].as_str().unwrap();
    assert_eq!(subscribed["subscription"]["worker_id"], peer_id);
    assert_eq!(subscribed["subscription"]["repeat_count"], 2);
    let master_status = fixture.run_public(&["notify", "status"], THREAD_A);
    assert!(!master_status.to_string().contains(subscription_id));
    for _ in 0..2 {
        let received = fixture.run_public(&["recv", "--timeout", "5"], THREAD_B);
        let messages = received["messages"].as_array().unwrap();
        assert_eq!(messages.len(), 1, "{received}");
        assert_eq!(messages[0]["subject"], "deadline:patrol:assigned-work");
        assert_eq!(messages[0]["to"], peer_id);
    }
    let status = fixture.run_public(&["notify", "status"], THREAD_B);
    let subscription = status["subscriptions"]
        .as_array()
        .unwrap()
        .iter()
        .find(|s| s["id"] == subscription_id)
        .unwrap();
    assert_eq!(subscription["fired_count"], 2);
    assert_eq!(subscription["status"], "consumed");
    let no_more = fixture.run_public(&["recv", "--timeout", "2"], THREAD_B);
    assert_eq!(no_more["count"], 0, "{no_more}");

    for args in [
        vec![
            "notify",
            "subscribe",
            "--event",
            "master-idle",
            "--subject",
            "patrol",
            "--ttl-seconds",
            "30",
        ],
        vec![
            "notify",
            "subscribe",
            "--event",
            "deadline",
            "--subject",
            "goal:project",
            "--every-ms",
            "100",
            "--ttl-seconds",
            "30",
        ],
        vec![
            "notify",
            "subscribe",
            "--event",
            "deadline",
            "--subject",
            "patrol:invalid",
            "--every-ms",
            "0",
            "--ttl-seconds",
            "30",
        ],
    ] {
        let output = fixture.command_public(&args, THREAD_B);
        assert!(
            !output.status.success(),
            "unexpected success: {}",
            String::from_utf8_lossy(&output.stdout)
        );
    }
}
