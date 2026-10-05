use super::{optional_flag, required, tool};
use serde_json::{json, Value};

pub fn tools() -> Vec<Value> {
    let revision = json!({"type":"integer","minimum":1});
    vec![
        tool("collab_board_show", "Read public master/peer tasks and observed members. Does not register peers, start the daemon, consume messages, or expose private subworkers.", json!({}), &[]),
        tool("collab_board_publish", "Live project master publishes a pending task with delivery/test conditions. This does not start execution or transfer ownership.", json!({"id":{"type":"string"},"title":{"type":"string"},"description":{"type":"string"},"delivery_condition":{"type":"string"},"test_condition":{"type":"string"},"priority":{"type":"string","enum":["p0","p1","p2","p3","p4"]}}), &["id","title","description","delivery_condition","test_condition"]),
        tool("collab_board_invite", "Live master invites one idle ordinary peer. Invitation reserves public dispatch capacity, not task ownership. A peer may decline or independently register its own work.", json!({"id":{"type":"string"},"to":{"type":"string"},"expected_revision":revision}), &["id","to","expected_revision"]),
        tool("collab_board_respond", "Only the invited peer accepts or declines an observed invitation. Accept rechecks other responsibilities and binding before transferring owner; decline requires reason and preserves publisher ownership.", json!({"id":{"type":"string"},"accept":{"type":"boolean"},"decline":{"type":"boolean"},"expected_revision":revision,"reason":{"type":"string"}}), &["id","expected_revision"]),
        tool("collab_board_withdraw", "Live master withdraws an unaccepted invitation with its observed revision and reason. Does not reclaim peer-owned execution.", json!({"id":{"type":"string"},"expected_revision":revision,"reason":{"type":"string"}}), &["id","expected_revision","reason"]),
        tool("collab_board_update", "Authenticated task owner updates its own progress using a current revision. Old revisions fail explicitly; delivery/review/integration/cleanup still use evidence-bearing task tools.", json!({"id":{"type":"string"},"expected_revision":revision,"status":{"type":"string"},"next":{"type":"string"}}), &["id","expected_revision"]),
        tool("collab_board_describe", "Authenticated task owner describes an existing independent task. Does not modify lifecycle. Withdraw invitations before changing their contract.", json!({"id":{"type":"string"},"expected_revision":revision,"title":{"type":"string"},"description":{"type":"string"},"delivery_condition":{"type":"string"},"test_condition":{"type":"string"}}), &["id","expected_revision","title","description","delivery_condition","test_condition"]),
        tool("collab_task_decline", "Reject a board invitation, or an unstarted legacy assignment when legacy_assignment=true. Only the invited peer/legacy owner may act; reason and current revision are required.", json!({"id":{"type":"string"},"expected_revision":revision,"reason":{"type":"string"},"legacy_assignment":{"type":"boolean"}}), &["id","expected_revision","reason"]),
    ]
}

fn revision(argv: &mut Vec<String>, args: &Value) -> Result<(), String> {
    let value = args.get("expected_revision").and_then(Value::as_u64).filter(|value| *value > 0)
        .ok_or("expected_revision must be a positive integer")?;
    argv.extend(["--expected-revision".into(), value.to_string()]);
    Ok(())
}

fn boolean(args: &Value, key: &str) -> Result<bool, String> {
    match args.get(key) {
        None => Ok(false),
        Some(value) => value.as_bool().ok_or_else(|| format!("{key} must be a boolean")),
    }
}

pub fn argv(name: &str, args: &Value) -> Result<Option<Vec<String>>, String> {
    let action = match name {
        "collab_board_show" => "show",
        "collab_board_publish" => "publish",
        "collab_board_invite" => "invite",
        "collab_board_respond" => "respond",
        "collab_board_withdraw" => "withdraw",
        "collab_board_update" => "update",
        "collab_board_describe" => "describe",
        "collab_task_decline" => "decline",
        _ => return Ok(None),
    };
    let mut argv = vec![if name == "collab_task_decline" { "task" } else { "board" }.into(), action.into()];
    if action == "show" { return Ok(Some(argv)); }
    argv.push(required(args, "id")?);
    if action != "publish" { revision(&mut argv, args)?; }
    match action {
        "publish" | "describe" => {
            for (key, flag) in [("title","--title"),("description","--description"),("delivery_condition","--delivery-condition"),("test_condition","--test-condition")] {
                argv.extend([flag.into(), required(args, key)?]);
            }
            if action == "publish" { optional_flag(&mut argv, args, "priority", "--priority")?; }
        },
        "invite" => argv.extend(["--to".into(), required(args, "to")?]),
        "respond" => {
            let accept = boolean(args, "accept")?;
            let decline = boolean(args, "decline")?;
            if accept == decline { return Err("choose exactly one of accept or decline".into()); }
            argv.push(if accept { "--accept" } else { "--decline" }.into());
            if decline { argv.extend(["--reason".into(), required(args, "reason")?]); }
        },
        "withdraw" | "decline" => {
            argv.extend(["--reason".into(), required(args, "reason")?]);
            if action == "decline" && boolean(args, "legacy_assignment")? { argv.push("--legacy-assignment".into()); }
        },
        "update" => {
            optional_flag(&mut argv, args, "status", "--status")?;
            optional_flag(&mut argv, args, "next", "--next")?;
        },
        _ => unreachable!("known board action"),
    }
    Ok(Some(argv))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn public_tools_have_unique_names_and_typed_revision_constraints() {
        let tools = tools();
        let names: std::collections::HashSet<_> = tools.iter().map(|tool| tool["name"].as_str().unwrap()).collect();
        assert_eq!(names.len(), 8);
        for tool in tools.iter().filter(|tool| !matches!(tool["name"].as_str(), Some("collab_board_show" | "collab_board_publish"))) {
            assert_eq!(tool["inputSchema"]["properties"]["expected_revision"]["type"], "integer");
            assert_eq!(tool["inputSchema"]["properties"]["expected_revision"]["minimum"], 1);
        }
    }

    #[test]
    fn publish_preserves_multiline_business_text_as_single_cli_arguments() {
        let text = "first line\n第二行 'quoted' ; not a shell command";
        let arguments = argv("collab_board_publish", &json!({"id":"t","title":"标题","description":text,"delivery_condition":"交付","test_condition":"验证","priority":"p4"})).unwrap().unwrap();
        let index = arguments.iter().position(|argument| argument == "--description").unwrap();
        assert_eq!(arguments[index + 1], text);
        assert!(arguments.windows(2).any(|pair| pair == ["--priority", "p4"]));
    }

    #[test]
    fn responding_requires_one_action_and_an_observed_positive_revision() {
        for args in [json!({"id":"t","expected_revision":2}), json!({"id":"t","expected_revision":2,"accept":true,"decline":true}), json!({"id":"t","expected_revision":2,"accept":"true"}), json!({"id":"t","expected_revision":0,"accept":true}), json!({"id":"t","expected_revision":2,"decline":true})] {
            assert!(argv("collab_board_respond", &args).is_err(), "invalid input was accepted: {args}");
        }
        assert_eq!(argv("collab_board_respond", &json!({"id":"t","expected_revision":2,"decline":true,"reason":"保护已有工作"})).unwrap().unwrap(), ["board", "respond", "t", "--expected-revision", "2", "--decline", "--reason", "保护已有工作"]);
    }

    #[test]
    fn legacy_decline_is_explicit_and_unknown_tools_do_not_select_a_board_action() {
        assert_eq!(argv("collab_task_decline", &json!({"id":"old","expected_revision":3,"reason":"独立peer拒绝","legacy_assignment":true})).unwrap().unwrap(), ["task", "decline", "old", "--expected-revision", "3", "--reason", "独立peer拒绝", "--legacy-assignment"]);
        assert!(argv("not-a-board-tool", &json!({})).unwrap().is_none());
    }
}
