//! DAGpipe-backed fix lifecycle execution.
//!
//! The graph contract is the topology owner; the Operators below validate the
//! AppSDK record chain before the lifecycle state machine advances.

use pipeline_runtime::{
    compile, graph_topology, parse_graph_json, Cancellation, EffectReplay, Graph, Identity,
    Operator, OperatorContext, Registry, Runtime, StateMachine, Transition, ValueType,
};
use serde_json::{json, Value};
use std::collections::{BTreeSet, HashMap};
use std::fs;
use std::path::Path;

use crate::{
    assert_identifier, assert_no_symlink_components, assert_project_root_safe, fail,
    module_record_name,
};

pub(super) const FIX_LIFECYCLE_GRAPH: &str =
    include_str!("../../contracts/dagpipe/fix-lifecycle.graph.json");
pub(super) const NOTIFICATION_LIFECYCLE_GRAPH: &str =
    include_str!("../../contracts/dagpipe/notification.graph.json");
const DAGPIPE_GRAPH_MANIFEST: &str = include_str!("../../docs/dagpipe/manifest.json");
pub(super) const COMMUNICATION_EVENT_SCHEMA: &str =
    include_str!("../../contracts/communication/communication-event.schema.json");

pub(crate) fn run_cli(args: &mut std::iter::Peekable<std::vec::IntoIter<String>>) {
    let subcommand = args
        .next()
        .unwrap_or_else(|| {
            fail("USAGE: appsdk dagpipe fix [project] [--module <id>] | validate | validate-notifications <project>")
        });
    if subcommand == "validate" {
        let result = validate_graph_contracts()
            .unwrap_or_else(|error| fail(format!("DAGPIPE_GRAPH_VALIDATION_FAILED:{error}")));
        println!(
            "{}",
            serde_json::to_string_pretty(&result).expect("dagpipe result is serializable")
        );
        return;
    }
    if subcommand == "validate-notifications" {
        let root = std::path::PathBuf::from(
            args.next()
                .unwrap_or_else(|| fail("USAGE: appsdk dagpipe validate-notifications <project>")),
        );
        if args.next().is_some() {
            fail("USAGE: appsdk dagpipe validate-notifications <project>");
        }
        let result = validate_notification_objects(&root)
            .unwrap_or_else(|error| fail(format!("DAGPIPE_NOTIFICATION_OBJECT_INVALID:{error}")));
        println!(
            "{}",
            serde_json::to_string_pretty(&result).expect("dagpipe result is serializable")
        );
        return;
    }
    if subcommand != "fix" {
        fail("USAGE: appsdk dagpipe fix [project] [--module <id>] | validate | validate-notifications <project>");
    }

    let mut root = std::path::PathBuf::from(".");
    if let Some(first) = args.peek() {
        if first != "--module" {
            root = std::path::PathBuf::from(args.next().expect("peeked"));
        }
    }
    let mut module_id = None;
    while let Some(option) = args.next() {
        match option.as_str() {
            "--module" => {
                if module_id.is_some() {
                    fail("USAGE: appsdk dagpipe fix [project] [--module <id>]");
                }
                module_id = Some(args.next().unwrap_or_else(|| {
                    fail("USAGE: appsdk dagpipe fix [project] [--module <id>]")
                }));
            }
            _ => fail("USAGE: appsdk dagpipe fix [project] [--module <id>]"),
        }
    }

    let module_id = module_id.unwrap_or_else(|| fail("DAGPIPE_MODULE_REQUIRED"));
    let result = run_fix_lifecycle(&root, &module_id)
        .unwrap_or_else(|error| fail(format!("DAGPIPE_FIX_LIFECYCLE_FAILED:{error}")));
    println!(
        "{}",
        serde_json::to_string_pretty(&result).expect("dagpipe result is serializable")
    );
}

fn run_fix_lifecycle(root: &Path, module_id: &str) -> Result<Value, String> {
    assert_project_root_safe(root);
    assert_identifier(module_id, "DAGPIPE_MODULE_IDENTIFIER_INVALID");
    let graph = parse_graph_json(FIX_LIFECYCLE_GRAPH)
        .map_err(|error| format!("DAGPIPE_GRAPH_INVALID:{error}"))?;
    ensure_single_source_single_sink(&graph)?;
    let mut registry = Registry::default();
    register_fix_operators(&mut registry)?;
    let capabilities = BTreeSet::new();
    let compiled = compile(graph, &registry, &capabilities)
        .map_err(|error| format!("DAGPIPE_COMPILE_FAILED:{error}"))?;

    let input = lifecycle_input(root, module_id)?;
    let mut inputs = HashMap::new();
    inputs.insert("lifecycle_state".to_owned(), input.clone());
    let identity = Identity {
        project_id: "appsdk".to_owned(),
        graph_id: compiled.id().to_owned(),
        graph_version: compiled.version().to_owned(),
        execution_id: format!("fix-lifecycle-{}", UtcStamp::now()),
        attempt_id: "1".to_owned(),
    };
    let runtime = Runtime::new(capabilities);
    let result = runtime
        .run(&compiled, identity, inputs, &Cancellation::default())
        .map_err(|failure| format!("DAGPIPE_EXECUTION_FAILED:{failure}"))?;

    let mut state = "open".to_owned();
    let mut journal = Vec::new();
    let machine = lifecycle_state_machine()
        .map_err(|error| format!("DAGPIPE_STATE_MACHINE_INVALID:{error}"))?;
    for (node, event) in [
        ("admission_claim", "claim"),
        ("admission_candidate", "candidate"),
        ("admission_review", "review_pass"),
        ("admission_effectiveness", "effectiveness_replay"),
        ("admission_remote", "remote_receipt"),
        ("emit_promotion", "promote"),
    ] {
        ensure_node_completed(&result.journal, node)?;
        state = machine
            .apply(&state, event, &mut journal)
            .map_err(|error| format!("DAGPIPE_TRANSITION_FAILED:{error}"))?;
    }
    if state != "promoted" {
        return Err(format!("DAGPIPE_TERMINAL_STATE_INVALID:{state}"));
    }
    let last = result
        .outputs
        .get("promotion_state")
        .map(|value| value.payload.clone())
        .ok_or_else(|| "DAGPIPE_OUTPUT_MISSING:promotion_state".to_owned())?;
    Ok(json!({
        "graph": {"id": compiled.id(), "version": compiled.version()},
        "state": state,
        "authority": "advisory_projection",
        "authoritative_gate": "appsdk verify <project>",
        "single_source_single_sink": {"inputs": 1, "outputs": 1},
        "node_order": compiled.node_ids().collect::<Vec<_>>(),
        "runtime_journal": result.journal,
        "state_journal": journal,
        "result": last,
    }))
}

fn validate_graph_contracts() -> Result<Value, String> {
    validate_graph_contracts_with_manifest(DAGPIPE_GRAPH_MANIFEST)
}

fn validate_graph_contracts_with_manifest(raw_manifest: &str) -> Result<Value, String> {
    let manifest: Value = serde_json::from_str(raw_manifest)
        .map_err(|error| format!("DAGPIPE_GRAPH_MANIFEST_INVALID:{error}"))?;
    if manifest.get("schema_version").and_then(Value::as_u64) != Some(1) {
        return Err("DAGPIPE_GRAPH_MANIFEST_INVALID:schema_version".to_owned());
    }
    let entries = manifest
        .get("graphs")
        .and_then(Value::as_array)
        .ok_or_else(|| "DAGPIPE_GRAPH_MANIFEST_INVALID:graphs".to_owned())?;
    let embedded_paths = embedded_graph_paths();
    let mut seen_ids = std::collections::HashSet::new();
    let mut seen_paths = std::collections::HashSet::new();
    for entry in entries {
        let id = entry
            .get("id")
            .and_then(Value::as_str)
            .ok_or_else(|| "DAGPIPE_GRAPH_MANIFEST_INVALID:id".to_owned())?;
        let path = entry
            .get("path")
            .and_then(Value::as_str)
            .ok_or_else(|| "DAGPIPE_GRAPH_MANIFEST_INVALID:path".to_owned())?;
        if !seen_ids.insert(id) {
            return Err(format!("DAGPIPE_GRAPH_MANIFEST_DUPLICATE_ID:{id}"));
        }
        if !seen_paths.insert(path) {
            return Err(format!("DAGPIPE_GRAPH_MANIFEST_DUPLICATE_PATH:{path}"));
        }
    }
    if entries.len() != embedded_paths.len() {
        return Err(format!(
            "DAGPIPE_GRAPH_MANIFEST_MISMATCH:manifest={} embedded={}",
            entries.len(),
            embedded_paths.len()
        ));
    }
    let mut graphs = Vec::new();
    for entry in entries {
        let id = entry
            .get("id")
            .and_then(Value::as_str)
            .ok_or_else(|| "DAGPIPE_GRAPH_MANIFEST_INVALID:id".to_owned())?;
        let path = entry
            .get("path")
            .and_then(Value::as_str)
            .ok_or_else(|| "DAGPIPE_GRAPH_MANIFEST_INVALID:path".to_owned())?;
        let Some((_, source)) = embedded_paths
            .iter()
            .find(|(embedded_path, _)| *embedded_path == path)
        else {
            return Err(format!("DAGPIPE_GRAPH_MANIFEST_MISSING_SOURCE:{path}"));
        };
        let graph = parse_graph_json(source)
            .map_err(|error| format!("DAGPIPE_GRAPH_INVALID:{id}:{error}"))?;
        if graph.id != id {
            return Err(format!("DAGPIPE_GRAPH_ID_MISMATCH:{id}:{}", graph.id));
        }
        ensure_single_source_single_sink(&graph).map_err(|error| format!("{id}:{error}"))?;
        validate_graph_registry(&graph)?;
        graphs.push(json!({
            "id": id,
            "version": graph.version,
            "single_source_single_sink": {"inputs": graph.inputs.len(), "outputs": graph.outputs.len()}
        }));
    }
    for (path, _) in embedded_paths {
        if !seen_paths.contains(path) {
            return Err(format!("DAGPIPE_GRAPH_MANIFEST_MISSING_PATH:{path}"));
        }
    }
    Ok(json!({
        "single_source_single_sink": true,
        "compiled_registry": true,
        "graphs": graphs,
        "manifest": manifest,
    }))
}

fn validate_graph_registry(graph: &Graph) -> Result<(), String> {
    let design_graph_ids = design_graph_ids();
    let design_operators: Vec<&'static str> = if design_graph_ids.contains(&graph.id.as_str()) {
        design_graph_operator_names().into()
    } else {
        Vec::new()
    };
    let mut registry = Registry::default();
    register_fix_operators(&mut registry)?;
    register_notification_operator(&mut registry)?;
    for name in &design_operators {
        registry
            .register(DesignGraphOperator { name })
            .map_err(|error| format!("DAGPIPE_OPERATOR_REGISTER_FAILED:{error}"))?;
    }
    ensure_graph_nodes_use_registered_operators(
        graph,
        &design_graph_ids,
        design_operators.as_slice(),
    )?;
    let capabilities = BTreeSet::new();
    compile(graph.clone(), &registry, &capabilities)
        .map_err(|error| format!("DAGPIPE_COMPILE_FAILED:{error}"))?;
    Ok(())
}

fn ensure_graph_nodes_use_registered_operators(
    graph: &Graph,
    design_graph_ids: &[&str],
    registered_design_operators: &[&'static str],
) -> Result<(), String> {
    let requires_design_operators = design_graph_ids.contains(&graph.id.as_str());
    let known_design_operators = design_graph_operator_names();
    for node in &graph.nodes {
        let is_design_operator = known_design_operators.contains(&node.operator.as_str());
        if is_design_operator && !requires_design_operators {
            return Err(format!(
                "DAGPIPE_DESIGN_OPERATOR_GRAPH_SCOPE_INVALID:{}:{}:{}",
                graph.id, node.id, node.operator
            ));
        }
        if requires_design_operators
            && !registered_design_operators.contains(&node.operator.as_str())
        {
            return Err(format!(
                "DAGPIPE_DESIGN_GRAPH_OPERATOR_SCOPE_INVALID:{}:{}:{}",
                graph.id, node.id, node.operator
            ));
        }
    }
    if graph.nodes.is_empty() {
        return Err(format!("DAGPIPE_GRAPH_MUST_HAVE_NODES:{}", graph.id));
    }
    Ok(())
}

#[path = "dagpipe_notification.rs"]
mod notification;
use notification::{register_notification_operator, validate_notification_objects};

fn embedded_graph_paths() -> [(&'static str, &'static str); 13] {
    [
        (
            "contracts/dagpipe/fix-lifecycle.graph.json",
            FIX_LIFECYCLE_GRAPH,
        ),
        (
            "contracts/dagpipe/notification.graph.json",
            NOTIFICATION_LIFECYCLE_GRAPH,
        ),
        (
            "docs/dagpipe/collab-context.graph.json",
            include_str!("../../docs/dagpipe/collab-context.graph.json"),
        ),
        (
            "docs/dagpipe/appserver-route-repair.graph.json",
            include_str!("../../docs/dagpipe/appserver-route-repair.graph.json"),
        ),
        (
            "docs/dagpipe/collab-subscription-lifecycle.graph.json",
            include_str!("../../docs/dagpipe/collab-subscription-lifecycle.graph.json"),
        ),
        (
            "docs/dagpipe/collab-notification-consumption.graph.json",
            include_str!("../../docs/dagpipe/collab-notification-consumption.graph.json"),
        ),
        (
            "docs/dagpipe/merge-pending.graph.json",
            include_str!("../../docs/dagpipe/merge-pending.graph.json"),
        ),
        (
            "docs/dagpipe/collab-identity-adjudication.graph.json",
            include_str!("../../docs/dagpipe/collab-identity-adjudication.graph.json"),
        ),
        (
            "docs/dagpipe/collab-dsh-channel.graph.json",
            include_str!("../../docs/dagpipe/collab-dsh-channel.graph.json"),
        ),
        (
            "docs/dagpipe/sdk-pin-history.graph.json",
            include_str!("../../docs/dagpipe/sdk-pin-history.graph.json"),
        ),
        (
            "docs/dagpipe/collab-dashboard.graph.json",
            include_str!("../../docs/dagpipe/collab-dashboard.graph.json"),
        ),
        (
            "docs/dagpipe/collab-control-plane-reset.graph.json",
            include_str!("../../docs/dagpipe/collab-control-plane-reset.graph.json"),
        ),
        (
            "docs/dagpipe/collab-pane-route-reconcile.graph.json",
            include_str!("../../docs/dagpipe/collab-pane-route-reconcile.graph.json"),
        ),
    ]
}

fn design_graph_ids() -> [&'static str; 11] {
    [
        "appsdk-collab-context",
        "appsdk-collab-appserver-route-repair",
        "appsdk-collab-subscription-lifecycle",
        "appsdk-collab-notification-consumption",
        "appsdk-collab-merge-pending",
        "appsdk-collab-identity-adjudication",
        "appsdk-collab-dsh-channel",
        "appsdk-sdk-pin-history",
        "appsdk-collab-dashboard-operation",
        "appsdk-collab-control-plane-reset",
        "appsdk-collab-pane-route-reconcile",
    ]
}

fn design_graph_operator_names() -> [&'static str; 64] {
    [
        "appsdk.collab_context.resolve_root",
        "appsdk.collab_context.ensure_baseline",
        "appsdk.collab_context.ensure_daemon",
        "appsdk.collab_context.identity_gate",
        "appsdk.collab_context.restore_default_lease",
        "appsdk.collab_context.find_master",
        "appsdk.collab_context.read_only_state",
        "appsdk.collab_context.env_view",
        "appsdk.collab_context.emit_snapshot",
        "appsdk.collab_appserver_route.discover_live_thread",
        "appsdk.collab_appserver_route.rebind_current_route",
        "appsdk.collab_appserver_route.refresh_worker_transport_lease",
        "appsdk.collab_appserver_route.retry_notification_once",
        "appsdk.collab_appserver_route.emit_notification_result_receipt",
        "appsdk.collab_subscription.select_transport",
        "appsdk.collab_subscription.upsert_default_lease",
        "appsdk.collab_subscription.verify_transport_alignment",
        "appsdk.collab_subscription.persist_active_state",
        "appsdk.collab_notification_consume.verify_owner",
        "appsdk.collab_notification_consume.read_payloads",
        "appsdk.collab_notification_consume.commit_receipt",
        "appsdk.collab_merge.register_pending",
        "appsdk.collab_merge.publish_obligation",
        "appsdk.collab_merge.remind_master",
        "appsdk.collab_merge.integrate_main",
        "appsdk.collab_merge.resolve_pending",
        "appsdk.collab_merge.close_task",
        "appsdk.collab_adjudication.parse_declaration",
        "appsdk.collab_adjudication.load_target",
        "appsdk.collab_adjudication.collect_inherited",
        "appsdk.collab_adjudication.write_receipt",
        "appsdk.collab_adjudication.rebind_identity",
        "appsdk.collab_adjudication.emit_outcome",
        "appsdk.collab_dsh_channel.build_dsh_candidate",
        "appsdk.collab_dsh_channel.admit_dsh_transport",
        "appsdk.collab_dsh_channel.bind_and_receipt",
        "appsdk.collab_dsh_channel.publish_route",
        "appsdk.collab_dsh_channel.arm_default_lease",
        "appsdk.collab_dsh_channel.project_presence",
        "appsdk.collab_dsh_channel.deliver_wake",
        "appsdk.collab_dsh_channel.send_to_peer",
        "appsdk.sdk_pin.authenticate_request",
        "appsdk.sdk_pin.authenticate_historical_record",
        "appsdk.sdk_pin.reconcile_history",
        "appsdk.sdk_pin.materialize_current_step",
        "appsdk.sdk_pin.verify_retention",
        "appsdk.sdk_pin.publish_pin_outcome",
        "appsdk.collab.board.admit_request",
        "appsdk.collab.board.authorize",
        "appsdk.collab.board.apply",
        "appsdk.collab.board.project_result",
        "appsdk.collab_control_plane.authorize_reset",
        "appsdk.collab_control_plane.prove_exclusivity",
        "appsdk.collab_control_plane.inventory_control_plane",
        "appsdk.collab_control_plane.archive_inventory",
        "appsdk.collab_control_plane.retire_selected_state",
        "appsdk.collab_control_plane.verify_retirement",
        "appsdk.collab_control_plane.rebuild_baseline",
        "appsdk.collab_control_plane.record_reset_receipt",
        "appsdk.collab_pane_route.classify_claimants",
        "appsdk.collab_pane_route.resolve_scope_pane_owner",
        "appsdk.collab_pane_route.publish_owner_route",
        "appsdk.collab_pane_route.verify_scope_uniqueness",
        "appsdk.collab_pane_route.return_named_outcome",
    ]
}

struct DesignGraphOperator {
    name: &'static str,
}

impl Operator for DesignGraphOperator {
    fn name(&self) -> &'static str {
        self.name
    }

    fn version(&self) -> &'static str {
        "1"
    }

    fn input_type(&self) -> ValueType {
        ValueType::Any
    }

    fn output_type(&self) -> ValueType {
        ValueType::Any
    }

    fn replay(&self) -> EffectReplay {
        EffectReplay::NonReplayable
    }

    fn execute(&self, _input: Value, _context: &OperatorContext) -> Result<Value, String> {
        Err("DAGPIPE_DESIGN_OPERATOR_NOT_EXECUTABLE".to_owned())
    }
}

fn ensure_node_completed(
    journal: &[pipeline_runtime::Event],
    expected_node: &str,
) -> Result<(), String> {
    if journal.iter().any(
        |event| matches!(event, pipeline_runtime::Event::NodeCompleted { node_id, .. } if node_id == expected_node),
    ) {
        return Ok(());
    }
    Err(format!("DAGPIPE_NODE_NOT_COMPLETED:{expected_node}"))
}

pub(super) fn ensure_single_source_single_sink(
    graph: &pipeline_runtime::Graph,
) -> Result<(), String> {
    if graph.inputs.len() != 1 || graph.outputs.len() != 1 {
        return Err(format!(
            "DAGPIPE_GRAPH_MUST_BE_SINGLE_SOURCE_SINGLE_SINK:inputs={} outputs={}",
            graph.inputs.len(),
            graph.outputs.len()
        ));
    }
    let mut consumers = std::collections::HashMap::<&str, Vec<&str>>::new();
    for node in &graph.nodes {
        if node.inputs.len() != 1 {
            return Err(format!(
                "DAGPIPE_NODE_MUST_HAVE_SINGLE_INPUT:{} inputs={}",
                node.id,
                node.inputs.len()
            ));
        }
        consumers
            .entry(node.inputs[0].as_str())
            .or_default()
            .push(node.id.as_str());
    }
    for (arc, nodes) in &consumers {
        if nodes.len() != 1 {
            return Err(format!(
                "DAGPIPE_ARC_MUST_HAVE_SINGLE_SINK:{} sinks={}",
                arc,
                nodes.len()
            ));
        }
    }
    let mut producers = std::collections::HashMap::<&str, &str>::new();
    for node in &graph.nodes {
        if producers
            .insert(node.output.id.as_str(), node.id.as_str())
            .is_some()
        {
            return Err(format!(
                "DAGPIPE_ARC_MUST_HAVE_SINGLE_SOURCE:{}",
                node.output.id
            ));
        }
    }
    if !producers.contains_key(graph.outputs[0].as_str()) {
        return Err(format!("DAGPIPE_GRAPH_OUTPUT_UNBOUND:{}", graph.outputs[0]));
    }
    let declared_output = graph.outputs[0].as_str();
    for (arc, _) in &producers {
        if !consumers.contains_key(arc) && declared_output != *arc {
            return Err(format!("DAGPIPE_ARC_UNCONSUMED:{arc}"));
        }
    }
    let mut edge_arcs = std::collections::HashSet::new();
    for edge in &graph.edges {
        if !edge_arcs.insert(edge.arc_id.as_str()) {
            return Err(format!(
                "DAGPIPE_ARC_MUST_HAVE_SINGLE_SOURCE:{}",
                edge.arc_id
            ));
        }
        if consumers.get(edge.arc_id.as_str()).map(Vec::len) != Some(1) {
            return Err(format!("DAGPIPE_ARC_MUST_HAVE_SINGLE_SINK:{}", edge.arc_id));
        }
    }
    // Single-input/single-output per node is necessary but not sufficient for
    // SESE: only the shared topology validator rejects cycles, edge endpoint
    // mismatches, dead nodes, and undeclared input arcs.
    graph_topology(graph).map_err(|error| format!("DAGPIPE_GRAPH_TOPOLOGY_INVALID:{error}"))?;
    Ok(())
}

fn register_fix_operators(registry: &mut Registry) -> Result<(), String> {
    for operator in [
        FixOperator::new("appsdk.lifecycle.claim", LifecycleStep::Claim),
        FixOperator::new("appsdk.lifecycle.candidate", LifecycleStep::Candidate),
        FixOperator::new("appsdk.lifecycle.review", LifecycleStep::Review),
        FixOperator::new(
            "appsdk.lifecycle.effectiveness",
            LifecycleStep::Effectiveness,
        ),
        FixOperator::new("appsdk.lifecycle.remote", LifecycleStep::Remote),
        FixOperator::new("appsdk.lifecycle.promotion", LifecycleStep::Promotion),
    ] {
        registry
            .register(operator)
            .map_err(|error| format!("DAGPIPE_OPERATOR_REGISTER_FAILED:{error}"))?;
    }
    Ok(())
}

#[derive(Clone, Copy)]
enum LifecycleStep {
    Claim,
    Candidate,
    Review,
    Effectiveness,
    Remote,
    Promotion,
}

struct FixOperator {
    name: &'static str,
    step: LifecycleStep,
}

impl FixOperator {
    fn new(name: &'static str, step: LifecycleStep) -> Self {
        Self { name, step }
    }
}

impl Operator for FixOperator {
    fn name(&self) -> &'static str {
        self.name
    }

    fn version(&self) -> &'static str {
        "1"
    }

    fn input_type(&self) -> ValueType {
        ValueType::Any
    }

    fn output_type(&self) -> ValueType {
        ValueType::Any
    }

    fn replay(&self) -> EffectReplay {
        EffectReplay::NonReplayable
    }

    fn execute(&self, input: Value, context: &OperatorContext) -> Result<Value, String> {
        let root = input
            .get("_root")
            .and_then(Value::as_str)
            .ok_or_else(|| "DAGPIPE_ROOT_MISSING".to_owned())?;
        let module_id = input
            .get("_module_id")
            .and_then(Value::as_str)
            .ok_or_else(|| "DAGPIPE_MODULE_MISSING".to_owned())?;
        let mut output = input.clone();
        let step = match self.step {
            LifecycleStep::Claim => validate_claim(Path::new(root), module_id, &mut output),
            LifecycleStep::Candidate => validate_candidate(Path::new(root), module_id, &mut output),
            LifecycleStep::Review => validate_review(Path::new(root), module_id, &mut output),
            LifecycleStep::Effectiveness => {
                validate_effectiveness(Path::new(root), module_id, &mut output)
            }
            LifecycleStep::Remote => validate_remote(Path::new(root), module_id, &mut output),
            LifecycleStep::Promotion => validate_promotion(Path::new(root), module_id, &mut output),
        };
        step.map_err(|error| format!("{}:{error}", context.node_id))?;
        Ok(output)
    }
}

fn lifecycle_input(root: &Path, module_id: &str) -> Result<Value, String> {
    let worktree_name = module_record_name("worktree-record", module_id);
    let worktree = read_json_record(root, &worktree_name)?;
    let issue_id = required_str(&worktree, "/issue_id", &worktree_name)?;
    Ok(json!({
        "_root": root.to_string_lossy(),
        "_module_id": module_id,
        "issue_id": issue_id,
    }))
}

fn validate_claim(root: &Path, module_id: &str, output: &mut Value) -> Result<(), String> {
    let name = module_record_name("worktree-record", module_id);
    let record = read_json_record(root, &name)?;
    require_str(&record, "/worktree_id", &name)?;
    require_str(&record, "/issue_id", &name)?;
    require_str(&record, "/module_id", &name)?;
    require_str(&record, "/base_commit", &name)?;
    require_str(&record, "/branch", &name)?;
    require_str(&record, "/head_commit", &name)?;
    require_str(&record, "/scope_hash", &name)?;
    if record.get("module_id").and_then(Value::as_str) != Some(module_id) {
        return Err("FIX_WORKTREE_MODULE_MISMATCH".to_owned());
    }
    if record.get("initial_clean") != Some(&Value::Bool(true))
        || record.get("final_clean") != Some(&Value::Bool(true))
        || record.get("isolation_mode").and_then(Value::as_str) != Some("isolated_worktree")
    {
        return Err("FIX_WORKTREE_NOT_CLEAN_ISOLATED".to_owned());
    }
    output["worktree"] = record;
    Ok(())
}

fn validate_candidate(root: &Path, module_id: &str, output: &mut Value) -> Result<(), String> {
    let worktree_name = module_record_name("worktree-record", module_id);
    let candidate_name = module_record_name("fix-candidate-record", module_id);
    let worktree = output
        .get("worktree")
        .cloned()
        .map(Ok)
        .unwrap_or_else(|| read_json_record(root, &worktree_name))?;
    let candidate = read_json_record(root, &candidate_name)?;
    for path in [
        "/fix_candidate_id",
        "/issue_id",
        "/module_id",
        "/worktree_id",
        "/base_commit",
        "/head_commit",
        "/tree_hash",
        "/diff_hash",
        "/design_id",
        "/owner",
        "/scope_hash",
    ] {
        require_str(&candidate, path, &candidate_name)?;
    }
    require_array_or_empty(&candidate, "/changed_paths", &candidate_name)?;
    require_array(&candidate, "/verification_evidence_ids", &candidate_name)?;
    if candidate.get("module_id").and_then(Value::as_str) != Some(module_id) {
        return Err("FIX_CANDIDATE_MODULE_MISMATCH".to_owned());
    }
    if candidate.get("worktree_id") != worktree.get("worktree_id")
        || candidate.get("issue_id") != worktree.get("issue_id")
        || candidate.get("base_commit") != worktree.get("base_commit")
        || candidate.get("scope_hash") != worktree.get("scope_hash")
    {
        return Err("FIX_CANDIDATE_WORKTREE_MISMATCH".to_owned());
    }
    output["candidate"] = candidate;
    Ok(())
}

fn validate_review(root: &Path, module_id: &str, output: &mut Value) -> Result<(), String> {
    let candidate_name = module_record_name("fix-candidate-record", module_id);
    let validation_name = module_record_name("pre-review-validation-record", module_id);
    let review_name = module_record_name("review-record", module_id);
    let candidate = output
        .get("candidate")
        .cloned()
        .map(Ok)
        .unwrap_or_else(|| read_json_record(root, &candidate_name))?;
    let validation = read_json_record(root, &validation_name)?;
    for path in [
        "/validation_id",
        "/issue_id",
        "/module_id",
        "/fix_candidate_id",
        "/candidate_commit",
        "/candidate_tree_hash",
        "/artifact_hash",
    ] {
        require_str(&validation, path, &validation_name)?;
    }
    require_object(&validation, "/whitebox_producer", &validation_name)?;
    require_array(&validation, "/whitebox_evidence_ids", &validation_name)?;
    require_array(&validation, "/blackbox_evidence_ids", &validation_name)?;
    if validation.get("result").and_then(Value::as_str) != Some("pass")
        || validation.get("source_unchanged") != Some(&Value::Bool(true))
        || validation.get("fix_candidate_id") != candidate.get("fix_candidate_id")
        || validation.get("candidate_commit") != candidate.get("head_commit")
        || validation.get("candidate_tree_hash") != candidate.get("tree_hash")
    {
        return Err("PRE_REVIEW_VALIDATION_MISMATCH".to_owned());
    }
    let review = read_json_record(root, &review_name)?;
    for path in [
        "/review_id",
        "/issue_id",
        "/promotion_id",
        "/fix_candidate_id",
        "/pre_review_validation_id",
        "/reviewed_commit",
        "/reviewed_tree_hash",
        "/reviewed_diff_hash",
        "/reviewed_artifact_hash",
        "/reviewed_scope_hash",
        "/resource_map_hash",
        "/function_map_hash",
        "/mainline_call_map_hash",
        "/verification_map_hash",
    ] {
        require_str(&review, path, &review_name)?;
    }
    require_object(&review, "/reviewer", &review_name)?;
    require_array(&review, "/evidence_ids", &review_name)?;
    if review.get("review_kind").and_then(Value::as_str) != Some("architecture")
        || review.get("verdict").and_then(Value::as_str) != Some("pass")
        || review.get("fix_candidate_id") != candidate.get("fix_candidate_id")
        || review.get("pre_review_validation_id") != validation.get("validation_id")
        || review.get("reviewed_commit") != candidate.get("head_commit")
        || review.get("reviewed_tree_hash") != candidate.get("tree_hash")
        || review.get("reviewed_scope_hash") != candidate.get("scope_hash")
    {
        return Err("ARCHITECTURE_REVIEW_INPUT_MISMATCH".to_owned());
    }
    output["review"] = review;
    output["validation"] = validation;
    Ok(())
}

fn validate_effectiveness(root: &Path, module_id: &str, output: &mut Value) -> Result<(), String> {
    let candidate_name = module_record_name("fix-candidate-record", module_id);
    let review_name = module_record_name("review-record", module_id);
    let effectiveness_name = module_record_name("effectiveness-record", module_id);
    let candidate = output
        .get("candidate")
        .cloned()
        .map(Ok)
        .unwrap_or_else(|| read_json_record(root, &candidate_name))?;
    let review = output
        .get("review")
        .cloned()
        .map(Ok)
        .unwrap_or_else(|| read_json_record(root, &review_name))?;
    let effectiveness = read_json_record(root, &effectiveness_name)?;
    for path in [
        "/effectiveness_id",
        "/issue_id",
        "/module_id",
        "/fix_candidate_id",
        "/architecture_review_id",
        "/reviewed_commit",
        "/reviewed_tree_hash",
        "/baseline_evidence_id",
        "/fixed_replay_evidence_id",
    ] {
        require_str(&effectiveness, path, &effectiveness_name)?;
    }
    for path in [
        "/reproduction_input_hashes",
        "/positive_evidence_ids",
        "/negative_evidence_ids",
        "/blackbox_evidence_ids",
    ] {
        require_array(&effectiveness, path, &effectiveness_name)?;
    }
    if effectiveness.get("result").and_then(Value::as_str) != Some("pass")
        || effectiveness.get("source_unchanged_since_review") != Some(&Value::Bool(true))
        || effectiveness.get("fix_candidate_id") != candidate.get("fix_candidate_id")
        || effectiveness.get("architecture_review_id") != review.get("review_id")
        || effectiveness.get("reviewed_commit") != candidate.get("head_commit")
        || effectiveness.get("reviewed_tree_hash") != candidate.get("tree_hash")
    {
        return Err("POST_ARCHITECTURE_EFFECTIVENESS_MISMATCH".to_owned());
    }
    output["effectiveness"] = effectiveness;
    Ok(())
}

fn validate_remote(root: &Path, module_id: &str, output: &mut Value) -> Result<(), String> {
    let candidate_name = module_record_name("fix-candidate-record", module_id);
    let effectiveness_name = module_record_name("effectiveness-record", module_id);
    let merge_name = module_record_name("merge-record", module_id);
    let candidate = output
        .get("candidate")
        .cloned()
        .map(Ok)
        .unwrap_or_else(|| read_json_record(root, &candidate_name))?;
    let effectiveness = output
        .get("effectiveness")
        .cloned()
        .map(Ok)
        .unwrap_or_else(|| read_json_record(root, &effectiveness_name))?;
    let merge = read_json_record(root, &merge_name)?;
    for path in [
        "/merge_id",
        "/issue_id",
        "/module_id",
        "/fix_candidate_id",
        "/effectiveness_id",
        "/mainline_ref",
        "/candidate_commit",
        "/merge_commit",
        "/candidate_tree_hash",
        "/merged_tree_hash",
        "/change_identity",
    ] {
        require_str(&merge, path, &merge_name)?;
    }
    if merge.get("result").and_then(Value::as_str) != Some("pass")
        || merge.get("fix_candidate_id") != candidate.get("fix_candidate_id")
        || merge.get("candidate_commit") != candidate.get("head_commit")
        || merge.get("candidate_tree_hash") != candidate.get("tree_hash")
        || merge.get("effectiveness_id") != effectiveness.get("effectiveness_id")
    {
        return Err("MERGE_CANDIDATE_IDENTITY_MISMATCH".to_owned());
    }
    output["merge"] = merge;
    Ok(())
}

fn validate_promotion(root: &Path, module_id: &str, output: &mut Value) -> Result<(), String> {
    let worktree_name = module_record_name("worktree-record", module_id);
    let candidate_name = module_record_name("fix-candidate-record", module_id);
    let review_name = module_record_name("review-record", module_id);
    let effectiveness_name = module_record_name("effectiveness-record", module_id);
    let merge_name = module_record_name("merge-record", module_id);
    let promotion_name = module_record_name("promotion-record", module_id);
    let worktree = output
        .get("worktree")
        .cloned()
        .map(Ok)
        .unwrap_or_else(|| read_json_record(root, &worktree_name))?;
    let candidate = output
        .get("candidate")
        .cloned()
        .map(Ok)
        .unwrap_or_else(|| read_json_record(root, &candidate_name))?;
    let review = output
        .get("review")
        .cloned()
        .map(Ok)
        .unwrap_or_else(|| read_json_record(root, &review_name))?;
    let effectiveness = output
        .get("effectiveness")
        .cloned()
        .map(Ok)
        .unwrap_or_else(|| read_json_record(root, &effectiveness_name))?;
    let merge = output
        .get("merge")
        .cloned()
        .map(Ok)
        .unwrap_or_else(|| read_json_record(root, &merge_name))?;
    let promotion = read_json_record(root, &promotion_name)?;
    for path in [
        "/promotion_id",
        "/issue_id",
        "/module_id",
        "/worktree_record_id",
        "/reproduction_record_id",
        "/fix_candidate_id",
        "/architecture_review_id",
        "/effectiveness_record_id",
        "/merge_record_id",
        "/candidate_commit",
        "/merged_commit",
        "/source_commit",
        "/new_active_version",
        "/review_id",
        "/change_set_id",
        "/compatibility_level",
        "/root_cause",
        "/design_id",
        "/change_reason_comment",
        "/playground_cleanup_record_id",
    ] {
        require_str(&promotion, path, &promotion_name)?;
    }
    require_array(&promotion, "/evidence_ids", &promotion_name)?;
    require_array(&promotion, "/required_gate_results", &promotion_name)?;
    if promotion
        .get("required_gate_results")
        .and_then(Value::as_array)
        .map_or(true, |gates| {
            gates
                .iter()
                .any(|gate| gate.get("result").and_then(Value::as_str) != Some("pass"))
        })
    {
        return Err("PROMOTION_GATE_RESULT_NOT_PASS".to_owned());
    }
    if promotion.get("bug_closure_verified") != Some(&Value::Bool(true))
        || promotion.get("worktree_record_id") != worktree.get("worktree_id")
        || promotion.get("fix_candidate_id") != candidate.get("fix_candidate_id")
        || promotion.get("architecture_review_id") != review.get("review_id")
        || promotion.get("effectiveness_record_id") != effectiveness.get("effectiveness_id")
        || promotion.get("merge_record_id") != merge.get("merge_id")
        || promotion.get("candidate_commit") != candidate.get("head_commit")
        || promotion.get("merged_commit") != merge.get("merge_commit")
    {
        return Err("PROMOTION_FIX_LIFECYCLE_REFERENCE_MISMATCH".to_owned());
    }
    output["promotion"] = promotion;
    output["state"] = Value::String("promoted".to_owned());
    Ok(())
}

fn require_str(record: &Value, path: &str, name: &str) -> Result<(), String> {
    required_str(record, path, name).map(|_| ())
}

fn required_str<'a>(record: &'a Value, path: &str, name: &str) -> Result<&'a str, String> {
    record
        .pointer(path)
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| format!("INVALID_RECORD:{name}:{path}"))
}

fn read_json_record(root: &Path, name: &str) -> Result<Value, String> {
    assert_no_symlink_components(
        root,
        &root.join(".appsdk").join("records"),
        "record_control",
    );
    let file = root.join(".appsdk").join("records").join(name);
    if fs::symlink_metadata(&file)
        .map(|metadata| metadata.file_type().is_symlink())
        .unwrap_or(false)
    {
        return Err(format!("GOVERNANCE_PATH_SYMLINK:record:{name}"));
    }
    let bytes = fs::read(&file).map_err(|_| format!("MISSING_RECORD:{name}"))?;
    serde_json::from_slice(&bytes).map_err(|_| format!("INVALID_RECORD:{name}"))
}

fn require_array(record: &Value, path: &str, name: &str) -> Result<(), String> {
    if record
        .pointer(path)
        .and_then(Value::as_array)
        .filter(|values| !values.is_empty())
        .is_none()
    {
        return Err(format!("INVALID_RECORD:{name}:{path}"));
    }
    Ok(())
}

fn require_array_or_empty(record: &Value, path: &str, name: &str) -> Result<(), String> {
    if record.pointer(path).and_then(Value::as_array).is_none() {
        return Err(format!("INVALID_RECORD:{name}:{path}"));
    }
    Ok(())
}

fn require_object(record: &Value, path: &str, name: &str) -> Result<(), String> {
    if record.pointer(path).and_then(Value::as_object).is_none() {
        return Err(format!("INVALID_RECORD:{name}:{path}"));
    }
    Ok(())
}

fn lifecycle_state_machine() -> Result<StateMachine, pipeline_runtime::CompileError> {
    StateMachine::new(
        [
            "open",
            "claimed",
            "candidate_verified",
            "architecture_reviewed",
            "effectiveness_verified",
            "remote_verified",
            "promoted",
        ]
        .into_iter()
        .map(str::to_owned),
        vec![
            transition("open", "claim", "claimed"),
            transition("claimed", "candidate", "candidate_verified"),
            transition("candidate_verified", "review_pass", "architecture_reviewed"),
            transition(
                "architecture_reviewed",
                "effectiveness_replay",
                "effectiveness_verified",
            ),
            transition(
                "effectiveness_verified",
                "remote_receipt",
                "remote_verified",
            ),
            transition("remote_verified", "promote", "promoted"),
        ],
    )
}

fn transition(from: &str, event: &str, to: &str) -> Transition {
    Transition {
        from: from.to_owned(),
        event: event.to_owned(),
        to: to.to_owned(),
    }
}

pub(super) struct UtcStamp;

impl UtcStamp {
    pub(super) fn now() -> u128 {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|duration| duration.as_nanos())
            .unwrap_or(0)
    }
}

#[cfg(test)]
#[path = "dagpipe_tests.rs"]
mod tests;
