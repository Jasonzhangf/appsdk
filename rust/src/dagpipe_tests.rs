use super::*;

#[test]
fn fix_and_notification_graphs_are_single_source_single_sink() {
    for source in [FIX_LIFECYCLE_GRAPH, NOTIFICATION_LIFECYCLE_GRAPH] {
        let graph = parse_graph_json(source).unwrap();
        ensure_single_source_single_sink(&graph).unwrap();
    }
}

#[test]
fn graph_manifest_covers_every_embedded_graph_and_validate_reports_both() {
    let result = validate_graph_contracts().unwrap();
    let graph_ids = result["graphs"]
        .as_array()
        .unwrap()
        .iter()
        .map(|graph| graph["id"].as_str().unwrap())
        .collect::<Vec<_>>();
    assert!(graph_ids.contains(&"appsdk-fix-lifecycle"));
    assert!(graph_ids.contains(&"appsdk-notification-object"));
    assert!(result["manifest"]["graphs"].as_array().unwrap().len() == 2);
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
