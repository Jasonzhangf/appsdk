use chrono::{DateTime, Utc};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::collections::BTreeSet;
use std::env;
use std::fs;
use std::fs::OpenOptions;
use std::io::{ErrorKind, Read, Write};
#[cfg(unix)]
use std::os::raw::c_int;
#[cfg(unix)]
use std::os::unix::io::AsRawFd;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

mod global_registry;
mod guidance;
mod long_horizon_policy;
mod long_horizon_role;
mod memory;

use long_horizon_policy::{generate_long_horizon_master_prompt, ExecutionRole, POLICY};
use long_horizon_role::execution_role;

mod communication;
mod dagpipe;

const SDK_BUNDLE_MANIFEST: &str = include_str!("../../contracts/sdk-bundle.manifest.json");
const SDK_VERSION: &str = env!("APPSDK_VERSION");
const PROJECT_AGENTS_TEMPLATE: &str = include_str!("../../templates/minimal/AGENTS.md");
const CANONICAL_ZONE_TRANSITION_CONTRACT: &str =
    include_str!("../../contracts/transitions/zone-transition.manifest.json");
const GOVERNANCE_MAP_NAMES: [&str; 4] = [
    "resource-map.json",
    "function-map.json",
    "mainline-call-map.json",
    "verification-map.json",
];
const CANONICAL_RECORD_CONTRACTS: [&str; 20] = [
    "contracts/records/worktree-record.schema.json",
    "contracts/records/reproduction-record.schema.json",
    "contracts/records/evidence-record.schema.json",
    "contracts/records/fix-candidate-record.schema.json",
    "contracts/records/goal-clarification-record.schema.json",
    "contracts/records/review-record.schema.json",
    "contracts/records/effectiveness-record.schema.json",
    "contracts/records/pre-review-validation-record.schema.json",
    "contracts/records/collaboration-record.schema.json",
    "contracts/records/collaboration-index.schema.json",
    "contracts/records/merge-queue-record.schema.json",
    "contracts/records/merge-queue-state.schema.json",
    "contracts/records/integration-record.schema.json",
    "contracts/records/mainline-receipt-record.schema.json",
    "contracts/records/collab-live-closure-record.schema.json",
    "contracts/records/merge-record.schema.json",
    "contracts/records/promotion-record.schema.json",
    "contracts/records/regression-report.schema.json",
    "contracts/records/freeze-record.schema.json",
    "contracts/records/record-graph.contract.json",
];
const SDK_BUNDLE_RESOURCES: &[(&str, &str, &str)] = &[
    (
        "contracts/sdk-bundle.manifest.json",
        "contracts",
        include_str!("../../contracts/sdk-bundle.manifest.json"),
    ),
    (
        "contracts/project.schema.json",
        "contracts",
        include_str!("../../contracts/project.schema.json"),
    ),
    (
        "contracts/test-governance.schema.json",
        "contracts",
        include_str!("../../contracts/test-governance.schema.json"),
    ),
    (
        "contracts/records/test-scenario-result-record.schema.json",
        "contracts",
        include_str!("../../contracts/records/test-scenario-result-record.schema.json"),
    ),
    (
        "docs/design/optional-test-governance.md",
        "docs",
        include_str!("../../docs/design/optional-test-governance.md"),
    ),
    (
        "contracts/communication/communication-request.schema.json",
        "contracts",
        include_str!("../../contracts/communication/communication-request.schema.json"),
    ),
    (
        "contracts/communication/communication-event.schema.json",
        "contracts",
        include_str!("../../contracts/communication/communication-event.schema.json"),
    ),
    (
        "contracts/communication/communication-capabilities.schema.json",
        "contracts",
        include_str!("../../contracts/communication/communication-capabilities.schema.json"),
    ),
    (
        "contracts/dagpipe/fix-lifecycle.graph.json",
        "contracts",
        include_str!("../../contracts/dagpipe/fix-lifecycle.graph.json"),
    ),
    (
        "contracts/dagpipe/notification.graph.json",
        "contracts",
        include_str!("../../contracts/dagpipe/notification.graph.json"),
    ),
    (
        "contracts/development-scenarios.manifest.json",
        "contracts",
        include_str!("../../contracts/development-scenarios.manifest.json"),
    ),
    (
        "contracts/guidance/guidance-manifest.schema.json",
        "contracts",
        include_str!("../../contracts/guidance/guidance-manifest.schema.json"),
    ),
    (
        "contracts/guidance/tour-review.schema.json",
        "contracts",
        include_str!("../../contracts/guidance/tour-review.schema.json"),
    ),
    (
        "contracts/memory/memory-entry.schema.json",
        "contracts",
        include_str!("../../contracts/memory/memory-entry.schema.json"),
    ),
    (
        "contracts/maps/resource-map.json",
        "contracts",
        include_str!("../../contracts/maps/resource-map.json"),
    ),
    (
        "contracts/maps/function-map.json",
        "contracts",
        include_str!("../../contracts/maps/function-map.json"),
    ),
    (
        "contracts/maps/mainline-call-map.json",
        "contracts",
        include_str!("../../contracts/maps/mainline-call-map.json"),
    ),
    (
        "contracts/maps/verification-map.json",
        "contracts",
        include_str!("../../contracts/maps/verification-map.json"),
    ),
    (
        "contracts/migrations/sdk-0.1.5-to-0.1.6.json",
        "contracts",
        include_str!("../../contracts/migrations/sdk-0.1.5-to-0.1.6.json"),
    ),
    (
        "contracts/migrations/sdk-0.1.6-to-0.1.0007.json",
        "contracts",
        include_str!("../../contracts/migrations/sdk-0.1.6-to-0.1.0007.json"),
    ),
    (
        "contracts/migrations/sdk-0.1.0007-to-0.1.0008.json",
        "contracts",
        include_str!("../../contracts/migrations/sdk-0.1.0007-to-0.1.0008.json"),
    ),
    (
        "contracts/migrations/sdk-0.1.0008-to-0.1.0009.json",
        "contracts",
        include_str!("../../contracts/migrations/sdk-0.1.0008-to-0.1.0009.json"),
    ),
    (
        "contracts/migrations/sdk-0.1.0009-to-0.1.0010.json",
        "contracts",
        include_str!("../../contracts/migrations/sdk-0.1.0009-to-0.1.0010.json"),
    ),
    (
        "contracts/migrations/sdk-0.1.0010-to-0.1.0011.json",
        "contracts",
        SDK_MAP_MIGRATION_0010_TO_0011,
    ),
    (
        "contracts/migrations/sdk-0.1.0011-to-0.1.0012.json",
        "contracts",
        SDK_MAP_MIGRATION_0011_TO_0012,
    ),
    (
        "contracts/migrations/sdk-0.1.0012-to-0.1.0013.json",
        "contracts",
        SDK_MAP_MIGRATION_0012_TO_0013,
    ),
    (
        "contracts/migrations/0.1.0012/governance-maps/resource-map.json",
        "contracts",
        include_str!("../../contracts/migrations/0.1.0012/governance-maps/resource-map.json"),
    ),
    (
        "contracts/migrations/0.1.0012/governance-maps/function-map.json",
        "contracts",
        include_str!("../../contracts/migrations/0.1.0012/governance-maps/function-map.json"),
    ),
    (
        "contracts/migrations/0.1.0012/governance-maps/mainline-call-map.json",
        "contracts",
        include_str!("../../contracts/migrations/0.1.0012/governance-maps/mainline-call-map.json"),
    ),
    (
        "contracts/migrations/0.1.0012/governance-maps/verification-map.json",
        "contracts",
        include_str!("../../contracts/migrations/0.1.0012/governance-maps/verification-map.json"),
    ),
    (
        "contracts/migrations/0.1.0011/governance-maps/resource-map.json",
        "contracts",
        include_str!("../../contracts/migrations/0.1.0011/governance-maps/resource-map.json"),
    ),
    (
        "contracts/migrations/0.1.0011/governance-maps/function-map.json",
        "contracts",
        include_str!("../../contracts/migrations/0.1.0011/governance-maps/function-map.json"),
    ),
    (
        "contracts/migrations/0.1.0011/governance-maps/mainline-call-map.json",
        "contracts",
        include_str!("../../contracts/migrations/0.1.0011/governance-maps/mainline-call-map.json"),
    ),
    (
        "contracts/migrations/0.1.0011/governance-maps/verification-map.json",
        "contracts",
        include_str!("../../contracts/migrations/0.1.0011/governance-maps/verification-map.json"),
    ),
    (
        "contracts/migrations/0.1.0010/governance-maps/resource-map.json",
        "contracts",
        include_str!("../../contracts/migrations/0.1.0010/governance-maps/resource-map.json"),
    ),
    (
        "contracts/migrations/0.1.0010/governance-maps/function-map.json",
        "contracts",
        include_str!("../../contracts/migrations/0.1.0010/governance-maps/function-map.json"),
    ),
    (
        "contracts/migrations/0.1.0010/governance-maps/mainline-call-map.json",
        "contracts",
        include_str!("../../contracts/migrations/0.1.0010/governance-maps/mainline-call-map.json"),
    ),
    (
        "contracts/migrations/0.1.0010/governance-maps/verification-map.json",
        "contracts",
        include_str!("../../contracts/migrations/0.1.0010/governance-maps/verification-map.json"),
    ),
    (
        "contracts/migrations/0.1.5/governance-maps/resource-map.json",
        "contracts",
        include_str!("../../contracts/migrations/0.1.5/governance-maps/resource-map.json"),
    ),
    (
        "contracts/migrations/0.1.5/governance-maps/function-map.json",
        "contracts",
        include_str!("../../contracts/migrations/0.1.5/governance-maps/function-map.json"),
    ),
    (
        "contracts/migrations/0.1.5/governance-maps/mainline-call-map.json",
        "contracts",
        include_str!("../../contracts/migrations/0.1.5/governance-maps/mainline-call-map.json"),
    ),
    (
        "contracts/migrations/0.1.5/governance-maps/verification-map.json",
        "contracts",
        include_str!("../../contracts/migrations/0.1.5/governance-maps/verification-map.json"),
    ),
    (
        "contracts/migrations/0.1.6/governance-maps/resource-map.json",
        "contracts",
        include_str!("../../contracts/migrations/0.1.6/governance-maps/resource-map.json"),
    ),
    (
        "contracts/migrations/0.1.6/governance-maps/function-map.json",
        "contracts",
        include_str!("../../contracts/migrations/0.1.6/governance-maps/function-map.json"),
    ),
    (
        "contracts/migrations/0.1.6/governance-maps/mainline-call-map.json",
        "contracts",
        include_str!("../../contracts/migrations/0.1.6/governance-maps/mainline-call-map.json"),
    ),
    (
        "contracts/migrations/0.1.6/governance-maps/verification-map.json",
        "contracts",
        include_str!("../../contracts/migrations/0.1.6/governance-maps/verification-map.json"),
    ),
    (
        "contracts/migrations/0.1.0007/governance-maps/resource-map.json",
        "contracts",
        include_str!("../../contracts/migrations/0.1.0007/governance-maps/resource-map.json"),
    ),
    (
        "contracts/migrations/0.1.0007/governance-maps/function-map.json",
        "contracts",
        include_str!("../../contracts/migrations/0.1.0007/governance-maps/function-map.json"),
    ),
    (
        "contracts/migrations/0.1.0007/governance-maps/mainline-call-map.json",
        "contracts",
        include_str!("../../contracts/migrations/0.1.0007/governance-maps/mainline-call-map.json"),
    ),
    (
        "contracts/migrations/0.1.0007/governance-maps/verification-map.json",
        "contracts",
        include_str!("../../contracts/migrations/0.1.0007/governance-maps/verification-map.json"),
    ),
    (
        "contracts/migrations/0.1.0008/governance-maps/resource-map.json",
        "contracts",
        include_str!("../../contracts/migrations/0.1.0008/governance-maps/resource-map.json"),
    ),
    (
        "contracts/migrations/0.1.0008/governance-maps/function-map.json",
        "contracts",
        include_str!("../../contracts/migrations/0.1.0008/governance-maps/function-map.json"),
    ),
    (
        "contracts/migrations/0.1.0008/governance-maps/mainline-call-map.json",
        "contracts",
        include_str!("../../contracts/migrations/0.1.0008/governance-maps/mainline-call-map.json"),
    ),
    (
        "contracts/migrations/0.1.0008/governance-maps/verification-map.json",
        "contracts",
        include_str!("../../contracts/migrations/0.1.0008/governance-maps/verification-map.json"),
    ),
    (
        "contracts/migrations/0.1.0009/governance-maps/resource-map.json",
        "contracts",
        include_str!("../../contracts/migrations/0.1.0009/governance-maps/resource-map.json"),
    ),
    (
        "contracts/migrations/0.1.0009/governance-maps/function-map.json",
        "contracts",
        include_str!("../../contracts/migrations/0.1.0009/governance-maps/function-map.json"),
    ),
    (
        "contracts/migrations/0.1.0009/governance-maps/mainline-call-map.json",
        "contracts",
        include_str!("../../contracts/migrations/0.1.0009/governance-maps/mainline-call-map.json"),
    ),
    (
        "contracts/migrations/0.1.0009/governance-maps/verification-map.json",
        "contracts",
        include_str!("../../contracts/migrations/0.1.0009/governance-maps/verification-map.json"),
    ),
    (
        "contracts/transitions/zone-transition.manifest.json",
        "contracts",
        CANONICAL_ZONE_TRANSITION_CONTRACT,
    ),
    (
        "contracts/lifecycle-state-machines.manifest.json",
        "contracts",
        include_str!("../../contracts/lifecycle-state-machines.manifest.json"),
    ),
    (
        "contracts/records/record-graph.contract.json",
        "contracts",
        include_str!("../../contracts/records/record-graph.contract.json"),
    ),
    (
        "contracts/records/worktree-record.schema.json",
        "contracts",
        include_str!("../../contracts/records/worktree-record.schema.json"),
    ),
    (
        "contracts/records/reproduction-record.schema.json",
        "contracts",
        include_str!("../../contracts/records/reproduction-record.schema.json"),
    ),
    (
        "contracts/records/evidence-record.schema.json",
        "contracts",
        include_str!("../../contracts/records/evidence-record.schema.json"),
    ),
    (
        "contracts/records/review-record.schema.json",
        "contracts",
        include_str!("../../contracts/records/review-record.schema.json"),
    ),
    (
        "contracts/records/fix-candidate-record.schema.json",
        "contracts",
        include_str!("../../contracts/records/fix-candidate-record.schema.json"),
    ),
    (
        "contracts/records/goal-clarification-record.schema.json",
        "contracts",
        include_str!("../../contracts/records/goal-clarification-record.schema.json"),
    ),
    (
        "contracts/records/user-requirement-request.schema.json",
        "contracts",
        include_str!("../../contracts/records/user-requirement-request.schema.json"),
    ),
    (
        "contracts/records/effectiveness-record.schema.json",
        "contracts",
        include_str!("../../contracts/records/effectiveness-record.schema.json"),
    ),
    (
        "contracts/records/pre-review-validation-record.schema.json",
        "contracts",
        include_str!("../../contracts/records/pre-review-validation-record.schema.json"),
    ),
    (
        "contracts/records/collaboration-record.schema.json",
        "contracts",
        include_str!("../../contracts/records/collaboration-record.schema.json"),
    ),
    (
        "contracts/records/collaboration-index.schema.json",
        "contracts",
        include_str!("../../contracts/records/collaboration-index.schema.json"),
    ),
    (
        "contracts/records/merge-queue-record.schema.json",
        "contracts",
        include_str!("../../contracts/records/merge-queue-record.schema.json"),
    ),
    (
        "contracts/records/merge-queue-state.schema.json",
        "contracts",
        include_str!("../../contracts/records/merge-queue-state.schema.json"),
    ),
    (
        "contracts/records/integration-record.schema.json",
        "contracts",
        include_str!("../../contracts/records/integration-record.schema.json"),
    ),
    (
        "contracts/records/mainline-receipt-record.schema.json",
        "contracts",
        include_str!("../../contracts/records/mainline-receipt-record.schema.json"),
    ),
    (
        "contracts/records/collab-live-closure-record.schema.json",
        "contracts",
        include_str!("../../contracts/records/collab-live-closure-record.schema.json"),
    ),
    (
        "contracts/records/merge-record.schema.json",
        "contracts",
        include_str!("../../contracts/records/merge-record.schema.json"),
    ),
    (
        "contracts/records/promotion-record.schema.json",
        "contracts",
        include_str!("../../contracts/records/promotion-record.schema.json"),
    ),
    (
        "contracts/records/freeze-record.schema.json",
        "contracts",
        include_str!("../../contracts/records/freeze-record.schema.json"),
    ),
    (
        "contracts/records/regression-report.schema.json",
        "contracts",
        include_str!("../../contracts/records/regression-report.schema.json"),
    ),
    (
        "contracts/records/plan-record.schema.json",
        "contracts",
        include_str!("../../contracts/records/plan-record.schema.json"),
    ),
    (
        "contracts/records/plan-revision-record.schema.json",
        "contracts",
        include_str!("../../contracts/records/plan-revision-record.schema.json"),
    ),
    (
        "contracts/records/step-execution-record.schema.json",
        "contracts",
        include_str!("../../contracts/records/step-execution-record.schema.json"),
    ),
    (
        "docs/design/appsdk-project-integration.md",
        "docs",
        include_str!("../../docs/design/appsdk-project-integration.md"),
    ),
    (
        "docs/design/apps-sdk-communication.md",
        "docs",
        include_str!("../../docs/design/apps-sdk-communication.md"),
    ),
    (
        "docs/design/fix-lifecycle-v2.md",
        "docs",
        include_str!("../../docs/design/fix-lifecycle-v2.md"),
    ),
    (
        "docs/design/development-scenarios.md",
        "docs",
        include_str!("../../docs/design/development-scenarios.md"),
    ),
    (
        "docs/design/appsdk-guidance-framework.md",
        "docs",
        include_str!("../../docs/design/appsdk-guidance-framework.md"),
    ),
    (
        "docs/test-design/appsdk-guidance-framework.md",
        "docs",
        include_str!("../../docs/test-design/appsdk-guidance-framework.md"),
    ),
    (
        "docs/architecture/development-process-control-harness.md",
        "docs",
        include_str!("../../docs/architecture/development-process-control-harness.md"),
    ),
    (
        "docs/architecture/appsdk-governance-architecture.md",
        "docs",
        include_str!("../../docs/architecture/appsdk-governance-architecture.md"),
    ),
    (
        "docs/design/project-memory.md",
        "docs",
        include_str!("../../docs/design/project-memory.md"),
    ),
    (
        "docs/design/appsdk-global-registry.md",
        "docs",
        include_str!("../../docs/design/appsdk-global-registry.md"),
    ),
    (
        "skills/appsdk-project-governance/SKILL.md",
        "rules",
        include_str!("../../sdk-skill-sources/appsdk-project-governance/SKILL.md"),
    ),
    (
        "skills/appsdk-project-governance/SKILL.md",
        "skills",
        include_str!("../../sdk-skill-sources/appsdk-project-governance/SKILL.md"),
    ),
    (
        "skills/appsdk-project-governance/appsdk-guidance.json",
        "skills",
        include_str!("../../sdk-skill-sources/appsdk-project-governance/appsdk-guidance.json"),
    ),
    (
        "skills/appsdk-project-governance/agents/openai.yaml",
        "skills",
        include_str!("../../sdk-skill-sources/appsdk-project-governance/agents/openai.yaml"),
    ),
    (
        "skills/appsdk-project-governance/references/bootstrap-migration.md",
        "skills",
        include_str!("../../sdk-skill-sources/appsdk-project-governance/references/bootstrap-migration.md"),
    ),
    (
        "skills/appsdk-project-governance/references/command-surface.md",
        "skills",
        include_str!("../../sdk-skill-sources/appsdk-project-governance/references/command-surface.md"),
    ),
    (
        "skills/appsdk-project-governance/references/contracts-and-failures.md",
        "skills",
        include_str!("../../sdk-skill-sources/appsdk-project-governance/references/contracts-and-failures.md"),
    ),
    (
        "skills/appsdk-project-governance/references/development-debug.md",
        "skills",
        include_str!("../../sdk-skill-sources/appsdk-project-governance/references/development-debug.md"),
    ),
    (
        "skills/appsdk-project-governance/references/goal-prompt.md",
        "skills",
        include_str!("../../sdk-skill-sources/appsdk-project-governance/references/goal-prompt.md"),
    ),
    (
        "skills/appsdk-project-governance/references/init-prompts.md",
        "skills",
        include_str!("../../sdk-skill-sources/appsdk-project-governance/references/init-prompts.md"),
    ),
    (
        "skills/appsdk-project-governance/references/process-control-harness.md",
        "skills",
        include_str!(
            "../../sdk-skill-sources/appsdk-project-governance/references/process-control-harness.md"
        ),
    ),
    (
        "skills/appsdk-project-governance/references/review-delivery.md",
        "skills",
        include_str!("../../sdk-skill-sources/appsdk-project-governance/references/review-delivery.md"),
    ),
    (
        REVIEW_TEMPLATE_SOURCE,
        "skills",
        REVIEW_TEMPLATE,
    ),
    (
        "skills/appsdk-project-governance/references/state-paths.md",
        "skills",
        include_str!("../../sdk-skill-sources/appsdk-project-governance/references/state-paths.md"),
    ),
    (
        "skills/appsdk-project-governance/references/subagents-config.md",
        "skills",
        include_str!("../../sdk-skill-sources/appsdk-project-governance/references/subagents-config.md"),
    ),
    (
        "skills/appsdk-migration/SKILL.md",
        "skills",
        include_str!("../../sdk-skill-sources/appsdk-migration/SKILL.md"),
    ),
    (
        "skills/project-memory/SKILL.md",
        "skills",
        include_str!("../../sdk-skill-sources/project-memory/SKILL.md"),
    ),
];

#[path = "main/governance.rs"]
mod governance;
use governance::canonical_map::*;
use governance::*;
#[path = "main/registry.rs"]
mod registry;
use registry::*;
#[path = "main/compile.rs"]
mod compile;
use compile::*;
#[path = "main/project.rs"]
mod project;
use project::*;
#[path = "main/rehydrate.rs"]
mod rehydrate;
use rehydrate::*;
#[path = "main/producer.rs"]
mod producer;
use producer::*;
#[path = "main/producer_commit.rs"]
mod producer_commit;
use producer_commit::*;
#[path = "main/lifecycle_chain.rs"]
mod lifecycle_chain;
use lifecycle_chain::*;
#[path = "main/lifecycle_closure.rs"]
mod lifecycle_closure;
use lifecycle_closure::*;
#[path = "main/verification.rs"]
mod verification;
use verification::*;
#[path = "main/review_gates.rs"]
mod review_gates;
use review_gates::*;
#[path = "main/review_context.rs"]
mod review_context;
use review_context::*;
#[path = "main/merge_gates.rs"]
mod merge_gates;
#[path = "main/requirements.rs"]
mod requirements;
use merge_gates::*;
#[path = "main/promotion.rs"]
mod promotion;
use promotion::*;
#[path = "main/init.rs"]
mod init;
use init::*;
#[path = "main/migration.rs"]
mod migration;
use migration::*;
#[path = "main/reset_transaction.rs"]
mod reset_transaction;
use reset_transaction::*;
#[path = "main/reset_validate.rs"]
mod reset_validate;
use reset_validate::*;
#[path = "main/reset_run.rs"]
mod reset_run;
use reset_run::*;
#[path = "main/reset_governance.rs"]
mod reset_governance;
use reset_governance::*;
#[path = "main/bug_tracker.rs"]
mod bug_tracker;
use bug_tracker::*;
#[path = "main/bug_cli.rs"]
mod bug_cli;
use bug_cli::*;
#[path = "main/longhorizon.rs"]
mod longhorizon;
use longhorizon::*;
#[path = "main/goal.rs"]
mod goal;
use goal::*;
#[path = "main/cli.rs"]
mod cli;
use cli::*;
#[path = "main/test_governance.rs"]
mod test_governance;
use test_governance::*;
struct RetireRecordSnapshot {
    kind: &'static str,
    file_name: &'static str,
    source: PathBuf,
    bytes: Vec<u8>,
    value: Value,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum LifecycleProducer {
    Records,
    Chain,
}

enum RegistryBinding<'a> {
    Exact,
    Aggregate(&'a [Value]),
}

struct CompileControlSnapshot {
    files: Vec<(PathBuf, &'static str, String)>,
}

struct DevelopmentScenarios {
    multi_worker_collaboration: bool,
    multi_worktree_merge_queue: bool,
}

const COLLAB_LIVE_CLOSURE_PATHS: [&str; 7] = [
    "peer_to_peer",
    "peer_to_master",
    "master_to_peer",
    "master_to_master",
    "daemon_to_peer",
    "daemon_to_master",
    "restart_replay",
];

enum EvidenceValidationMode {
    Current(DateTime<Utc>),
    Historical,
    HistoricalAt(DateTime<Utc>),
}

const APPSDK_GITIGNORE_BEGIN: &str = "# BEGIN APPSDK MANAGED";
const APPSDK_GITIGNORE_END: &str = "# END APPSDK MANAGED";
const APPSDK_GITIGNORE_BLOCK: &str =
    "# BEGIN APPSDK MANAGED\n.appsdk-control/\n.appsdk/sdk.bin\n/active/lib/\n/generated/\n# END APPSDK MANAGED\n";

const COLLAB_INIT_TIMEOUT_MS: u64 = 120_000;
const COLLAB_INIT_TIMEOUT: Duration = Duration::from_millis(COLLAB_INIT_TIMEOUT_MS);

#[derive(Clone)]
struct ResetTransactionTarget {
    relative: String,
    original: PathBuf,
    backup: PathBuf,
    staged: Option<PathBuf>,
    kind: &'static str,
    original_exists: bool,
    quarantined: bool,
    published: bool,
}

// A governance reset and a fresh init are the same transactional operation.
// Both entries share the single staging/quarantine/publish/rollback owner
// below while retaining their existing output and cleanliness-scope behavior.
// Recording the mode keeps the write-ahead marker honest without claiming a
// separate engine. `fresh_init` records the same `reset_id`/`transaction_id`
// receipt shape as the historical fresh path so older recovery markers stay
// readable.
#[derive(Clone, Copy, PartialEq, Eq)]
enum ResetMode {
    FreshInit,
    DiscardLegacy,
}

impl ResetMode {
    fn record_mode(self) -> &'static str {
        match self {
            ResetMode::FreshInit => "fresh_init",
            ResetMode::DiscardLegacy => "discard_legacy_control_plane",
        }
    }

    fn applied_message(self) -> &'static str {
        match self {
            ResetMode::FreshInit => "governance fresh init applied",
            ResetMode::DiscardLegacy => "governance reset applied",
        }
    }
}

#[cfg(unix)]
const RESET_TRANSACTION_LOCK_EX: c_int = 2;
#[cfg(unix)]
const RESET_TRANSACTION_LOCK_NB: c_int = 4;

#[cfg(unix)]
unsafe extern "C" {
    fn flock(fd: i32, operation: i32) -> i32;
}

#[cfg(test)]
mod bug_triage_tests {
    use super::*;

    #[test]
    fn canonical_intake_triage_modes_validate_with_bound_dedup_query() {
        let issue_id = "issue-1";
        let query = "git-bug bug \"same title\" -f json";
        for (mode, reopened_from, matched_issue) in [
            ("new_confirmed", None, None),
            ("reused", None, Some(issue_id)),
            ("reopened_same_record", Some(issue_id), Some(issue_id)),
        ] {
            let records = serde_json::json!([{
                "human_id": issue_id,
                "title": "same title",
                "labels": ["classification:bug"]
            }]);
            let empty_records = serde_json::json!([]);
            let (triage, binding) = bug_intake_triage(
                issue_id,
                query,
                if matched_issue.is_some() {
                    &records
                } else {
                    &empty_records
                },
                mode,
                reopened_from,
                matched_issue,
                matched_issue.map(|_| "same title"),
                matched_issue.map(|_| "bug"),
            );
            assert_eq!(binding, bug_triage_binding(issue_id, &triage));
            assert_eq!(
                assert_bug_triage_query_result(
                    &triage,
                    issue_id,
                    query,
                    matched_issue,
                    matched_issue.map(|_| "same title"),
                    matched_issue.map(|_| "bug"),
                    None,
                ),
                if matched_issue.is_some() {
                    Err("BUG_TRIAGE_QUERY_RESULT_UNVERIFIED".into())
                } else {
                    Ok(())
                }
            );
        }
    }

    #[test]
    fn query_result_hash_rejects_drift() {
        let (mut triage, _binding) = bug_intake_triage(
            "issue-1",
            "git-bug bug \"same title\" -f json",
            &serde_json::json!([{
                "human_id":"issue-1",
                "title":"same title",
                "labels":["classification:bug"]
            }]),
            "reused",
            None,
            Some("issue-1"),
            Some("same title"),
            Some("bug"),
        );
        triage["query_result"]["records"][0]["title"] = Value::String("drifted title".into());
        assert_eq!(
            assert_bug_triage_query_result(
                &triage,
                "issue-1",
                triage["query"].as_str().unwrap(),
                Some("issue-1"),
                Some("same title"),
                Some("bug"),
                None,
            ),
            Err("BUG_TRIAGE_QUERY_RESULT_HASH_MISMATCH".into())
        );
    }

    #[test]
    fn matched_issue_must_be_present_in_query_result() {
        let (triage, _binding) = bug_intake_triage(
            "issue-1",
            "git-bug bug \"same title\" -f json",
            &serde_json::json!([{
                "human_id":"different-issue",
                "title":"same title",
                "labels":["classification:bug"]
            }]),
            "reused",
            None,
            Some("issue-1"),
            Some("same title"),
            Some("bug"),
        );
        assert_eq!(
            assert_bug_triage_query_result(
                &triage,
                "issue-1",
                triage["query"].as_str().unwrap(),
                Some("issue-1"),
                Some("same title"),
                Some("bug"),
                None,
            ),
            Err("BUG_TRIAGE_MATCHED_ISSUE_NOT_IN_RESULT".into())
        );
    }

    #[test]
    fn historical_legacy_triage_binds_to_legacy_issue_without_store_query() {
        let worktree = serde_json::json!({
            "bug_triage": {
                "query_executed": true,
                "query": "appsdk bug list -q legacy-issue-1",
                "mode": "historical_legacy",
                "reopened_from_issue_id": null
            }
        });

        assert_bug_tracker_triage_evidence(&worktree, "legacy-issue-1", None, false);
    }
}

struct GoalLock {
    _file: fs::File,
    path: PathBuf,
}

impl GoalLock {
    fn acquire(root: &Path, owner: &str) -> Result<Self, String> {
        let control_dir = root.join(".appsdk-control");
        fs::create_dir_all(&control_dir)
            .map_err(|error| format!("GOAL_CONTROL_DIR_CREATE_FAILED:{}", error))?;
        let path = control_dir.join("long-task-goal.lock");
        for _ in 0..3 {
            let mut lock = match OpenOptions::new()
                .read(true)
                .write(true)
                .create_new(true)
                .open(&path)
            {
                Ok(lock) => {
                    if let Err(error) = goal_try_advisory_lock(&lock) {
                        return Err(error);
                    }
                    lock
                }
                Err(error) if error.kind() == ErrorKind::AlreadyExists => {
                    let existing = OpenOptions::new()
                        .read(true)
                        .write(true)
                        .open(&path)
                        .map_err(|open_error| format!("GOAL_LOCK_ACQUIRE_FAILED:{}", open_error))?;
                    if let Err(error) = goal_try_advisory_lock(&existing) {
                        if error == "GOAL_LOCK_BUSY" {
                            let metadata = fs::read_to_string(&path).unwrap_or_default();
                            return Err(if metadata.trim().is_empty() {
                                "GOAL_LOCK_BUSY: an active goal lifecycle operation holds the advisory lock; metadata is empty".into()
                            } else {
                                format!(
                                    "GOAL_LOCK_BUSY: an active goal lifecycle operation holds the advisory lock ({})",
                                    metadata.trim()
                                )
                            });
                        }
                        return Err(error);
                    }
                    let original = fs::read_to_string(&path).unwrap_or_default();
                    let quarantined = control_dir.join(format!(
                        "long-task-goal.lock.recovered.{}.{}",
                        std::process::id(),
                        goal_now_ms()
                    ));
                    match fs::rename(&path, &quarantined) {
                        Ok(()) => {
                            if let Err(receipt_error) = goal_lock_recovery_receipt(
                                &control_dir,
                                &quarantined,
                                &original,
                                if original.trim().is_empty() {
                                    "empty metadata"
                                } else if goal_lock_metadata(&quarantined).is_err() {
                                    "invalid or truncated metadata"
                                } else {
                                    "advisory lock released with stale metadata"
                                },
                            ) {
                                return Err(receipt_error);
                            }
                            continue;
                        }
                        Err(rename_error) if rename_error.kind() == ErrorKind::NotFound => {
                            continue;
                        }
                        Err(rename_error) => {
                            return Err(format!(
                                "GOAL_LOCK_STALE_RECOVERY_FAILED:{}",
                                rename_error
                            ));
                        }
                    }
                }
                Err(error) => return Err(format!("GOAL_LOCK_ACQUIRE_FAILED:{}", error)),
            };
            if let Err(error) = lock.set_len(0).and_then(|_| {
                writeln!(lock, "pid={} owner={}", std::process::id(), owner)?;
                lock.sync_all()
            }) {
                let _ = fs::remove_file(&path);
                return Err(format!("GOAL_LOCK_WRITE_FAILED:{}", error));
            }
            return Ok(Self { _file: lock, path });
        }
        Err("GOAL_LOCK_BUSY: stale lock changed during recovery".into())
    }
}

impl Drop for GoalLock {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.path);
    }
}

const GOAL_COLLAB_READ_TIMEOUT: Duration = Duration::from_secs(120);
const GOAL_COLLAB_WRITE_TIMEOUT: Duration = Duration::from_secs(120);

// A goal deadline is armed with an absolute at-ms that Collab validates when it
// handles the request. That call can queue behind an active daemon batch for up
// to the write budget, so an interval shorter than the budget can already be in
// the past by the time the subscription is created. Rejecting it before any
// local record mutation keeps `subscription_id` from being stranded.
const GOAL_MIN_INTERVAL_MS: u64 = GOAL_COLLAB_WRITE_TIMEOUT.as_secs() * 1000;

#[cfg(test)]
#[path = "main/goal_collab_command_tests.rs"]
mod goal_collab_command_tests;

const CLI_USAGE: &str = "Usage: appsdk <command> [project] [options]\n\nProject-scoped commands default to the current working directory. An explicit project path remains optional.\nManaged child compatibility entry: appsdk subworker <start|list|status|snapshot|send|close> ...";

fn main() {
    let argv = env::args().skip(1).collect::<Vec<_>>();
    // Collab owns configuration interpretation, coordination, and subworker runtime truth.
    // Forward argv/environment unchanged; do not create an AppSDK registry.
    if argv.first().is_some_and(|arg| {
        arg == "subagent" || arg == "subworker" || arg == "config" || arg == "collab"
    }) {
        let collab_args = if argv[0] == "collab" {
            argv[1..].to_vec()
        } else if argv[0] == "subworker" {
            let mut args = vec!["subagent".to_owned()];
            args.extend(argv[1..].iter().cloned());
            args
        } else {
            argv.clone()
        };
        let status = Command::new("collab")
            .args(&collab_args)
            .status()
            .unwrap_or_else(|error| fail(format!("COLLAB_UNAVAILABLE:{error}")));
        std::process::exit(status.code().unwrap_or(1));
    }
    if argv.is_empty() || is_help(&argv[0]) {
        print_cli_help(None);
        return;
    }
    if argv.len() == 2 && is_help(&argv[1]) && argv[0] != "guide" {
        print_cli_help(Some(&argv[0]));
        return;
    }
    let mut args = argv.into_iter().peekable();
    match args.next().as_deref() {
        Some("version") => println!("appsdk {SDK_VERSION} (rust)"),
        Some("verify-sdk-source-registry") => {
            assert_sdk_source_registry(Path::new(&args.next().unwrap_or_else(|| ".".into())))
        }
        Some("verify") => verify_cli(&mut args),
        Some("requirements") => requirements::requirements_cli(&mut args),
        Some("review-context") => {
            let root = project_root_or_cwd(&mut args);
            if args.next().as_deref() != Some("--module") {
                fail("USAGE: appsdk review-context [project] --module <id>");
            }
            let module = args
                .next()
                .unwrap_or_else(|| fail("USAGE: appsdk review-context [project] --module <id>"));
            if args.next().is_some() {
                fail("USAGE: appsdk review-context [project] --module <id>");
            }
            review_context(&root, &module);
        }
        Some("guide") => guidance::run(&mut args),
        Some("memory") | Some("project-memory") => memory::run(&mut args),
        Some("bug") => bug_cli(&mut args),
        Some("setup-deps") => {
            let check_only = args.peek().is_some_and(|a| a == "--check");
            setup_deps(check_only);
        }
        Some("goal") => {
            let mut root = PathBuf::from(".");
            if let Some(first) = args.peek() {
                if !matches!(
                    first.as_str(),
                    "subscribe"
                        | "register"
                        | "status"
                        | "cancel"
                        | "prompt"
                        | "help"
                        | "--help"
                        | "-h"
                ) && !first.starts_with('-')
                {
                    root = PathBuf::from(args.next().unwrap());
                }
            }
            handle_goal_command(&root, args);
        }
        Some("longhorizon") | Some("long-horizon") => {
            let mut root = PathBuf::from(".");
            if let Some(first) = args.peek() {
                // Only an existing directory is a project path; anything else is
                // a subcommand and must reach the handler so typos surface.
                if !first.starts_with('-') && Path::new(first).is_dir() {
                    root = PathBuf::from(args.next().unwrap());
                }
            }
            handle_longhorizon_command(&root, args);
        }
        Some("task") => {
            let mut root = PathBuf::from(".");
            if let Some(first) = args.peek() {
                if !matches!(
                    first.as_str(),
                    "block"
                        | "register"
                        | "relocate"
                        | "update"
                        | "wait"
                        | "deliver"
                        | "close"
                        | "status"
                        | "help"
                        | "--help"
                        | "-h"
                ) && !first.starts_with('-')
                {
                    root = PathBuf::from(args.next().unwrap());
                }
            }
            handle_task_command(&root, args);
        }
        Some("pin-lock") => {
            let root = project_root_or_cwd(&mut args);
            if args.next().as_deref() != Some("--binary") {
                fail("USAGE: appsdk pin-lock [project] --binary <path>");
            }
            let binary = args
                .next()
                .unwrap_or_else(|| fail("USAGE: appsdk pin-lock [project] --binary <path>"));
            if args.next().is_some() {
                fail("USAGE: appsdk pin-lock [project] --binary <path>");
            }
            pin_lock(&root, Path::new(&binary));
        }
        Some("sdk-witness") => {
            let root = project_root_or_cwd(&mut args);
            let usage = "USAGE: appsdk sdk-witness [project] [--binary <path>]";
            let mut binary = env::current_exe().unwrap_or_else(|_| fail("SDK_BINARY_MISSING"));
            match args.next().as_deref() {
                None => {}
                Some("--binary") => {
                    binary = PathBuf::from(args.next().unwrap_or_else(|| fail(usage)));
                }
                Some(_) => fail(usage),
            }
            if args.next().is_some() {
                fail(usage);
            }
            resolve_sdk_witness(&root, &binary);
        }
        Some("reset-governance") => {
            let root = project_root_or_cwd(&mut args);
            if args.next().as_deref() != Some("--discard-legacy") || args.next().is_some() {
                fail("USAGE: appsdk reset-governance [project] --discard-legacy");
            }
            reset_governance(&root, true);
        }
        Some("compile") => {
            let root = project_root_or_cwd(&mut args);
            if args.peek().is_some_and(|value| value == "--module") {
                args.next();
                let module = args
                    .next()
                    .unwrap_or_else(|| fail("USAGE: appsdk compile [project] [--module <id>]"));
                if args.next().is_some() {
                    fail("USAGE: appsdk compile [project] [--module <id>]");
                }
                compile_module(&root, &module);
            } else {
                if args.next().is_some() {
                    fail("USAGE: appsdk compile [project] [--module <id>]");
                }
                compile(&root);
            }
        }
        Some("compile-module") => {
            let root = project_root_or_cwd(&mut args);
            if args.next().as_deref() != Some("--module") {
                fail("USAGE: appsdk compile-module [project] --module <id>");
            }
            let module = args
                .next()
                .unwrap_or_else(|| fail("USAGE: appsdk compile-module [project] --module <id>"));
            if args.next().is_some() {
                fail("USAGE: appsdk compile-module [project] --module <id>");
            }
            compile_module(&root, &module);
        }
        Some("begin-version") => {
            let root = project_root_or_cwd(&mut args);
            if args.next().as_deref() != Some("--module") {
                fail("USAGE: appsdk begin-version [project] --module <id> --from <version> --to <version>");
            }
            let module_id = args.next().unwrap_or_else(|| fail("USAGE: appsdk begin-version [project] --module <id> --from <version> --to <version>"));
            if args.next().as_deref() != Some("--from") {
                fail("USAGE: appsdk begin-version [project] --module <id> --from <version> --to <version>");
            }
            let from = args.next().unwrap_or_else(|| fail("USAGE: appsdk begin-version [project] --module <id> --from <version> --to <version>"));
            if args.next().as_deref() != Some("--to") {
                fail("USAGE: appsdk begin-version [project] --module <id> --from <version> --to <version>");
            }
            let to = args.next().unwrap_or_else(|| fail("USAGE: appsdk begin-version [project] --module <id> --from <version> --to <version>"));
            if args.next().is_some() {
                fail("USAGE: appsdk begin-version [project] --module <id> --from <version> --to <version>");
            }
            begin_version(&root, &module_id, &from, &to);
        }
        Some("rehydrate-frozen") => {
            let root = project_root_or_cwd(&mut args);
            if args.next().as_deref() != Some("--module") {
                fail("USAGE: appsdk rehydrate-frozen [project] --module <id>");
            }
            let module_id = args
                .next()
                .unwrap_or_else(|| fail("USAGE: appsdk rehydrate-frozen [project] --module <id>"));
            if args.next().is_some() {
                fail("USAGE: appsdk rehydrate-frozen [project] --module <id>");
            }
            rehydrate_frozen(&root, &module_id);
        }
        Some("promote") => {
            let root = project_root_or_cwd(&mut args);
            if args.next().as_deref() != Some("--to") {
                fail("USAGE: appsdk promote [project] --to <stage>");
            }
            let stage = args
                .next()
                .unwrap_or_else(|| fail("USAGE: appsdk promote [project] --to <stage>"));
            if args.next().is_some() {
                fail("USAGE: appsdk promote [project] --to <stage>");
            }
            promote(&root, &stage);
        }
        Some("promote-module") => {
            let root = project_root_or_cwd(&mut args);
            if args.next().as_deref() != Some("--module") {
                fail("USAGE: appsdk promote-module [project] --module <id> --to <stage>");
            }
            let module_id = args.next().unwrap_or_else(|| {
                fail("USAGE: appsdk promote-module [project] --module <id> --to <stage>")
            });
            if args.next().as_deref() != Some("--to") {
                fail("USAGE: appsdk promote-module [project] --module <id> --to <stage>");
            }
            let target = args.next().unwrap_or_else(|| {
                fail("USAGE: appsdk promote-module [project] --module <id> --to <stage>")
            });
            if args.next().is_some() {
                fail("USAGE: appsdk promote-module [project] --module <id> --to <stage>");
            }
            promote_module(&root, &module_id, &target);
        }
        Some("produce-lifecycle-records") => {
            let root = project_root_or_cwd(&mut args);
            if args.next().as_deref() != Some("--module") {
                fail("USAGE: appsdk produce-lifecycle-records [project] --module <id> --input <json>");
            }
            let module_id = args.next().unwrap_or_else(|| {
                fail("USAGE: appsdk produce-lifecycle-records [project] --module <id> --input <json>")
            });
            if args.next().as_deref() != Some("--input") {
                fail("USAGE: appsdk produce-lifecycle-records [project] --module <id> --input <json>");
            }
            let input = args.next().unwrap_or_else(|| {
                fail("USAGE: appsdk produce-lifecycle-records [project] --module <id> --input <json>")
            });
            if args.next().is_some() {
                fail("USAGE: appsdk produce-lifecycle-records [project] --module <id> --input <json>");
            }
            produce_lifecycle_records(&root, &module_id, &input);
        }
        Some("retire-lifecycle-records") => {
            let root = project_root_or_cwd(&mut args);
            if args.next().as_deref() != Some("--module") {
                fail("USAGE: appsdk retire-lifecycle-records [project] --module <id> --issue <id>");
            }
            let module_id = args.next().unwrap_or_else(|| {
                fail("USAGE: appsdk retire-lifecycle-records [project] --module <id> --issue <id>")
            });
            if args.next().as_deref() != Some("--issue") {
                fail("USAGE: appsdk retire-lifecycle-records [project] --module <id> --issue <id>");
            }
            let issue_id = args.next().unwrap_or_else(|| {
                fail("USAGE: appsdk retire-lifecycle-records [project] --module <id> --issue <id>")
            });
            if args.next().is_some() {
                fail("USAGE: appsdk retire-lifecycle-records [project] --module <id> --issue <id>");
            }
            retire_lifecycle_records(&root, &module_id, &issue_id);
        }
        Some("produce-lifecycle-chain") => {
            let root = project_root_or_cwd(&mut args);
            if args.next().as_deref() != Some("--module") {
                fail("USAGE: appsdk produce-lifecycle-chain [project] --module <id> --phase <architecture|effectiveness|merge|promotion> --input <json>");
            }
            let module_id = args.next().unwrap_or_else(|| {
                fail("USAGE: appsdk produce-lifecycle-chain [project] --module <id> --phase <architecture|effectiveness|merge|promotion> --input <json>")
            });
            if args.next().as_deref() != Some("--phase") {
                fail("USAGE: appsdk produce-lifecycle-chain [project] --module <id> --phase <architecture|effectiveness|merge|promotion> --input <json>");
            }
            let phase = args.next().unwrap_or_else(|| {
                fail("USAGE: appsdk produce-lifecycle-chain [project] --module <id> --phase <architecture|effectiveness|merge|promotion> --input <json>")
            });
            if args.next().as_deref() != Some("--input") {
                fail("USAGE: appsdk produce-lifecycle-chain [project] --module <id> --phase <architecture|effectiveness|merge|promotion> --input <json>");
            }
            let input = args.next().unwrap_or_else(|| {
                fail("USAGE: appsdk produce-lifecycle-chain [project] --module <id> --phase <architecture|effectiveness|merge|promotion> --input <json>")
            });
            if args.next().is_some() {
                fail("USAGE: appsdk produce-lifecycle-chain [project] --module <id> --phase <architecture|effectiveness|merge|promotion> --input <json>");
            }
            produce_lifecycle_chain(&root, &module_id, &phase, &input);
        }
        Some("freeze") => {
            let root = project_root_or_cwd(&mut args);
            if args.next().as_deref() != Some("--module") {
                fail("USAGE: appsdk freeze [project] --module <id>");
            }
            let module = args
                .next()
                .unwrap_or_else(|| fail("USAGE: appsdk freeze [project] --module <id>"));
            if args.next().is_some() {
                fail("USAGE: appsdk freeze [project] --module <id>");
            }
            freeze_module(&root, &module);
        }
        Some("publish-active") => {
            let root = project_root_or_cwd(&mut args);
            if args.next().as_deref() != Some("--module") {
                fail("USAGE: appsdk publish-active [project] --module <id> --version <version>");
            }
            let module_id = args.next().unwrap_or_else(|| {
                fail("USAGE: appsdk publish-active [project] --module <id> --version <version>")
            });
            if args.next().as_deref() != Some("--version") {
                fail("USAGE: appsdk publish-active [project] --module <id> --version <version>");
            }
            let version = args.next().unwrap_or_else(|| {
                fail("USAGE: appsdk publish-active [project] --module <id> --version <version>")
            });
            if args.next().is_some() {
                fail("USAGE: appsdk publish-active [project] --module <id> --version <version>");
            }
            publish_active(&root, &module_id, &version);
        }
        Some("reset-staging-scaffold") => {
            let root = PathBuf::from(args.next().unwrap_or_else(|| {
                fail("USAGE: appsdk reset-staging-scaffold <project> <transaction-dir> <transaction-id>")
            }));
            let transaction_dir = PathBuf::from(args.next().unwrap_or_else(|| {
                fail("USAGE: appsdk reset-staging-scaffold <project> <transaction-dir> <transaction-id>")
            }));
            let transaction_id = args.next().unwrap_or_else(|| {
                fail("USAGE: appsdk reset-staging-scaffold <project> <transaction-dir> <transaction-id>")
            });
            if args.next().is_some() {
                fail("USAGE: appsdk reset-staging-scaffold <project> <transaction-dir> <transaction-id>");
            }
            reset_staging_scaffold(&root, &transaction_dir, &transaction_id);
        }
        Some("new") => {
            let root = project_root_or_cwd(&mut args);
            if args.next().is_some() {
                fail("USAGE: appsdk new [project]");
            }
            new_project(&root, true);
        }
        Some("init") => {
            let workspace = project_root_or_cwd(&mut args);
            let usage =
                "USAGE: appsdk init [workspace] [--project-root <relative-path>] [--fresh --discard-legacy]";
            let mut project_root = None;
            let mut fresh = false;
            let mut discard_legacy = false;
            while let Some(option) = args.next() {
                match option.as_str() {
                    "--project-root" => {
                        if project_root.is_some() {
                            fail(usage);
                        }
                        let value = args.next().unwrap_or_else(|| fail(usage));
                        if value.starts_with('-') {
                            fail(usage);
                        }
                        project_root = Some(value);
                    }
                    "--fresh" => {
                        if fresh {
                            fail(usage);
                        }
                        fresh = true;
                    }
                    "--discard-legacy" => {
                        if discard_legacy {
                            fail(usage);
                        }
                        discard_legacy = true;
                    }
                    _ => fail(usage),
                }
            }
            if fresh && !discard_legacy {
                fail("INIT_FRESH_REQUIRES_DISCARD_LEGACY_CONFIRMATION");
            }
            if discard_legacy && !fresh {
                fail("INIT_DISCARD_LEGACY_REQUIRES_FRESH");
            }
            let workspace_path = workspace.as_path();
            if fresh {
                let root = canonical_init_target(workspace_path, project_root.as_deref());
                init_project(&root, true, discard_legacy);
            } else if let Some(root) = existing_init_target(workspace_path, project_root.as_deref())
            {
                init_project(&root, false, false);
            } else if let Some(root) =
                existing_collab_control_target(workspace_path, project_root.as_deref())
            {
                init_collab_control_project(&root);
            } else {
                let (preparation, preparation_workspace) = read_init_preparation(workspace_path);
                let prepared_root = preparation
                    .get("project_root")
                    .and_then(Value::as_str)
                    .unwrap_or_else(|| fail("PREPARATION_PROJECT_ROOT_MISSING"));
                if let Some(explicit_root) = project_root.as_deref() {
                    let _ = resolve_init_target(&preparation_workspace, Some(explicit_root));
                }
                if project_root
                    .as_deref()
                    .is_some_and(|root| root != prepared_root)
                {
                    fail("PREPARATION_PROJECT_ROOT_MISMATCH");
                }
                let root = resolve_init_target(&preparation_workspace, Some(prepared_root));
                init_project(&root, false, false);
            }
        }
        Some("prepare") => {
            let workspace = project_root_or_cwd(&mut args);
            if args.next().is_some() {
                fail("USAGE: appsdk prepare [workspace]");
            }
            prepare_project(&workspace);
        }
        Some("communication") | Some("comm") => {
            if let Err(error) = communication::run_cli(args.collect()) {
                fail(error.to_string());
            }
        }
        Some("dagpipe") => {
            dagpipe::run_cli(&mut args);
        }
        _ => fail(CLI_USAGE),
    }
}
