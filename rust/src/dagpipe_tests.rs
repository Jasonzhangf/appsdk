use super::*;

#[test]
fn fix_and_notification_graphs_are_single_source_single_sink() {
    for source in [FIX_LIFECYCLE_GRAPH, NOTIFICATION_LIFECYCLE_GRAPH] {
        let graph = parse_graph_json(source).unwrap();
        ensure_single_source_single_sink(&graph).unwrap();
    }
}

/// `validate_graph_contracts()` is short-circuited by a pre-existing manifest
/// count mismatch (the manifest declares appsdk-collab-identity-adjudication,
/// which `embedded_graph_paths()` does not embed). The collab context graph
/// still has to satisfy the contract it will be judged by once that mismatch is
/// resolved, so this checks it directly: one entry, one exit, exactly one input
/// per node, exactly one sink per arc, and only registered design operators.
#[test]
fn collab_context_design_graph_is_single_source_single_sink_and_registered() {
    let (_, source) = embedded_graph_paths()
        .into_iter()
        .find(|(path, _)| *path == "docs/dagpipe/collab-context.graph.json")
        .expect("the collab context graph is embedded");
    let graph = parse_graph_json(source).unwrap();
    ensure_single_source_single_sink(&graph).unwrap();
    validate_graph_registry(&graph).unwrap();
}

/// Shape alone is not enough: `context_snapshot` fails into the classified
/// identity terminal from three call sites — `load_or_create_for_context`,
/// `ensure_registration_with_outcome`, and the first authenticated
/// `call_project(&Req::Context)` — every one of them before `find_master`. The
/// graph therefore must not re-introduce those boundaries as pre-master branch
/// nodes; it carries them as the single `identity_gate` stage, and the walk from
/// the one source to the one sink must be the order the implementation runs.
#[test]
fn collab_context_design_graph_matches_the_implementation_order() {
    let (_, source) = embedded_graph_paths()
        .into_iter()
        .find(|(path, _)| *path == "docs/dagpipe/collab-context.graph.json")
        .expect("the collab context graph is embedded");
    let graph = parse_graph_json(source).unwrap();

    // The three real identity-failure boundaries and the terminal they enter are
    // one stage. A node for any of them would be a pre-master branch the
    // implementation does not have.
    for retired in [
        "load_identity",
        "verify_token",
        "ensure_registration",
        "identity_update",
    ] {
        assert!(
            !graph.nodes.iter().any(|node| node.id == retired),
            "`{retired}` is not a separate stage: every identity failure enters the \
             same terminal from `identity_gate`"
        );
    }

    // Walk the single path and compare it with `context_snapshot`'s real order.
    let sink = graph.outputs[0].clone();
    let mut reached = vec![graph.inputs[0].id.clone()];
    let mut current = graph.inputs[0].id.clone();
    loop {
        let node = graph
            .nodes
            .iter()
            .find(|node| node.inputs.first() == Some(&current))
            .expect("every arc on the path has exactly one sink");
        reached.push(node.output.id.clone());
        if node.output.id == sink {
            break;
        }
        current = node.output.id.clone();
    }

    assert_eq!(
        reached,
        [
            "context_request",
            "resolved_scope",
            "baseline_ready",
            "daemon_ready",
            "identity_verified",
            "notify_state",
            "master_grant",
            "read_only_projection",
            "env_projection",
            "state_snapshot",
        ],
        "the graph path must be the order `context_snapshot` runs"
    );
}

#[test]
fn graph_manifest_covers_every_embedded_graph_and_validate_reports_contracted_designs() {
    let result = validate_graph_contracts().unwrap();
    let graph_ids = result["graphs"]
        .as_array()
        .unwrap()
        .iter()
        .map(|graph| graph["id"].as_str().unwrap())
        .collect::<Vec<_>>();
    assert!(graph_ids.contains(&"appsdk-fix-lifecycle"));
    assert!(graph_ids.contains(&"appsdk-notification-object"));
    assert!(graph_ids.contains(&"appsdk-collab-appserver-route-repair"));
    assert!(graph_ids.contains(&"appsdk-collab-subscription-lifecycle"));
    assert!(graph_ids.contains(&"appsdk-collab-notification-consumption"));
    assert!(result["compiled_registry"].as_bool().unwrap());
    assert!(result["manifest"]["graphs"].as_array().unwrap().len() == embedded_graph_paths().len());
}

#[test]
fn validate_graph_registry_rejects_design_operator_on_non_design_graph() {
    let source = include_str!("../../docs/dagpipe/collab-context.graph.json");
    let mut graph = parse_graph_json(source).unwrap();
    graph.id = "appsdk-collab-unknown-design".to_owned();

    let error = validate_graph_registry(&graph).unwrap_err();

    assert!(
        error.starts_with(
            "DAGPIPE_DESIGN_OPERATOR_GRAPH_SCOPE_INVALID:appsdk-collab-unknown-design"
        ),
        "{error}"
    );
}

#[test]
fn validate_graph_contracts_detects_duplicate_manifest_entries() {
    let mut manifest: Value = serde_json::from_str(DAGPIPE_GRAPH_MANIFEST).unwrap();
    manifest["graphs"][1]["path"] = manifest["graphs"][0]["path"].clone();
    let error = validate_graph_contracts_with_manifest(&manifest.to_string()).unwrap_err();
    assert!(
        error.starts_with("DAGPIPE_GRAPH_MANIFEST_DUPLICATE_PATH:"),
        "{error}"
    );

    let mut manifest: Value = serde_json::from_str(DAGPIPE_GRAPH_MANIFEST).unwrap();
    manifest["graphs"][1]["id"] = manifest["graphs"][0]["id"].clone();
    let error = validate_graph_contracts_with_manifest(&manifest.to_string()).unwrap_err();
    assert!(
        error.starts_with("DAGPIPE_GRAPH_MANIFEST_DUPLICATE_ID:"),
        "{error}"
    );
}

#[test]
fn validate_graph_contracts_rejects_embedded_path_mismatch() {
    let mut manifest: Value = serde_json::from_str(DAGPIPE_GRAPH_MANIFEST).unwrap();
    manifest["graphs"][0]["path"] = json!("docs/dagpipe/not-embedded.graph.json");
    let error = validate_graph_contracts_with_manifest(&manifest.to_string()).unwrap_err();
    assert!(
        error.starts_with("DAGPIPE_GRAPH_MANIFEST_MISSING_SOURCE:"),
        "{error}"
    );
}

#[test]
fn validate_graph_registry_rejects_design_operator_on_executable_graph() {
    let mut graph = parse_graph_json(FIX_LIFECYCLE_GRAPH).unwrap();
    graph.nodes[0].operator = design_graph_operator_names()[0].to_owned();
    let error = validate_graph_registry(&graph).unwrap_err();
    assert!(
        error.starts_with("DAGPIPE_DESIGN_OPERATOR_GRAPH_SCOPE_INVALID:appsdk-fix-lifecycle"),
        "{error}"
    );
}

#[test]
fn single_source_single_sink_rejects_fan_out() {
    let mut graph = parse_graph_json(NOTIFICATION_LIFECYCLE_GRAPH).unwrap();
    graph.nodes[0].inputs = vec![
        graph.nodes[0].inputs[0].clone(),
        "notification_draft".to_owned(),
    ];
    let error = ensure_single_source_single_sink(&graph).unwrap_err();
    assert!(error.starts_with("DAGPIPE_NODE_MUST_HAVE_SINGLE_INPUT"));
}

#[test]
fn single_source_single_sink_rejects_multiple_graph_outputs() {
    let mut graph = parse_graph_json(FIX_LIFECYCLE_GRAPH).unwrap();
    graph.outputs.push("candidate_state".to_owned());
    let error = ensure_single_source_single_sink(&graph).unwrap_err();
    assert!(error.starts_with("DAGPIPE_GRAPH_MUST_BE_SINGLE_SOURCE_SINGLE_SINK"));
}

#[test]
fn single_source_single_sink_rejects_unbound_or_unconsumed_outputs() {
    let mut graph = parse_graph_json(FIX_LIFECYCLE_GRAPH).unwrap();
    graph.outputs = vec!["missing_state".to_owned()];
    let error = ensure_single_source_single_sink(&graph).unwrap_err();
    assert!(error.starts_with("DAGPIPE_GRAPH_OUTPUT_UNBOUND"));

    let mut graph = parse_graph_json(FIX_LIFECYCLE_GRAPH).unwrap();
    graph.nodes.last_mut().unwrap().inputs[0] = "orphan_state".to_owned();
    let error = ensure_single_source_single_sink(&graph).unwrap_err();
    assert!(error.starts_with("DAGPIPE_ARC_UNCONSUMED"));
}

#[test]
fn single_source_single_sink_rejects_cycle_that_passes_local_shape_checks() {
    let graph = parse_graph_json(
        r#"{
          "id": "cycle",
          "version": "0.1.0",
          "inputs": [{"id": "source_arc", "schema": "Any"}],
          "nodes": [
            {"id": "a", "operator": "x", "operator_version": "1", "inputs": ["arc_b"],
             "output": {"id": "arc_a", "schema": "Any"},
             "input_selector": {"include": [], "exclude": [], "predicate": null},
             "output_selector": {"include": [], "exclude": [], "predicate": null},
             "iterator": "Whole"},
            {"id": "b", "operator": "y", "operator_version": "1", "inputs": ["arc_a"],
             "output": {"id": "arc_b", "schema": "Any"},
             "input_selector": {"include": [], "exclude": [], "predicate": null},
             "output_selector": {"include": [], "exclude": [], "predicate": null},
             "iterator": "Whole"}
          ],
          "edges": [
            {"from": "a", "to": "b", "arc_id": "arc_a"},
            {"from": "b", "to": "a", "arc_id": "arc_b"}
          ],
          "outputs": ["arc_a"]
        }"#,
    )
    .unwrap();
    let error = ensure_single_source_single_sink(&graph).unwrap_err();
    assert!(
        error.starts_with("DAGPIPE_GRAPH_TOPOLOGY_INVALID"),
        "{error}"
    );
    assert!(error.contains("cycle"), "{error}");
}

#[test]
fn single_source_single_sink_rejects_edge_arc_endpoint_mismatch() {
    // Passes the local per-node/per-ARC shape checks: every node has one
    // input, every produced ARC is consumed, and the declared output is
    // bound. Only the shared topology validator rejects the edge whose
    // `arc_id` is not the source node's output.
    let graph = parse_graph_json(
        r#"{
          "id": "endpoint_mismatch",
          "version": "0.1.0",
          "inputs": [{"id": "source_arc", "schema": "Any"}],
          "nodes": [
            {"id": "a", "operator": "x", "operator_version": "1", "inputs": ["source_arc"],
             "output": {"id": "arc_a", "schema": "Any"},
             "input_selector": {"include": [], "exclude": [], "predicate": null},
             "output_selector": {"include": [], "exclude": [], "predicate": null},
             "iterator": "Whole"},
            {"id": "b", "operator": "y", "operator_version": "1", "inputs": ["arc_a"],
             "output": {"id": "arc_b", "schema": "Any"},
             "input_selector": {"include": [], "exclude": [], "predicate": null},
             "output_selector": {"include": [], "exclude": [], "predicate": null},
             "iterator": "Whole"},
            {"id": "c", "operator": "z", "operator_version": "1", "inputs": ["arc_b"],
             "output": {"id": "arc_c", "schema": "Any"},
             "input_selector": {"include": [], "exclude": [], "predicate": null},
             "output_selector": {"include": [], "exclude": [], "predicate": null},
             "iterator": "Whole"}
          ],
          "edges": [
            {"from": "a", "to": "b", "arc_id": "arc_b"}
          ],
          "outputs": ["arc_c"]
        }"#,
    )
    .unwrap();
    let error = ensure_single_source_single_sink(&graph).unwrap_err();
    assert!(
        error.starts_with("DAGPIPE_GRAPH_TOPOLOGY_INVALID"),
        "{error}"
    );
}
