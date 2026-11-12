#!/usr/bin/env bash
#
# Single-source CI scope selector for .github/workflows/verify.yml.
#
# Reads the triggering event from the environment, derives the changed paths
# from git, and emits "<flag>=<true|false>" lines to $GITHUB_OUTPUT (or stdout
# when run outside Actions, e.g. from scripts/tests/test-verify-selector.sh).
#
# Inputs (all optional; absent values are treated as empty):
#   EVENT_NAME  github.event_name
#   REF_TYPE    github.ref_type
#   HEAD_SHA    github.sha
#   PUSH_BEFORE github.event.before
#   PR_BASE     github.event.pull_request.base.sha
#   REF_NAME    github.ref_name
#   DEFAULT_BRANCH github.event.repository.default_branch
#
# Behavior is fail-closed: an unknown, missing, or empty diff selects the full
# repository, and a workflow/selector-only diff selects only the workflow
# contract gate.

set -euo pipefail

# In Actions $GITHUB_OUTPUT is always set; outside Actions the flags go to
# stdout so scripts/tests/test-verify-selector.sh can capture them.
emit() {
  if [[ -n "${GITHUB_OUTPUT:-}" ]]; then
    printf '%s=%s\n' "$1" "$2" >> "$GITHUB_OUTPUT"
  else
    printf '%s=%s\n' "$1" "$2"
  fi
}

event_name="${EVENT_NAME:-}"
ref_type="${REF_TYPE:-}"
head_sha="${HEAD_SHA:-}"
push_before="${PUSH_BEFORE:-}"
pr_base="${PR_BASE:-}"
ref_name="${REF_NAME:-}"
default_branch="${DEFAULT_BRANCH:-}"

release=false
if [[ "$event_name" == "workflow_dispatch" || "$ref_type" == "tag" ]]; then
  release=true
fi

base=""
if [[ "$event_name" == "pull_request" ]]; then
  base="$pr_base"
elif [[ "$event_name" == "push" ]]; then
  base="$push_before"
fi

app_sdk_full=false
source_registry=false
rust_guidance=false
rust_communication=false
rust_memory=false
rust_registry=false
rust_producer=false
rust_verify_foundation=false
rust_ontology_governance=false
rust_cli_smoke=false
rust_platform_lock_cli=false
rust_registry_home_cli=false
resources=false
dagpipe=false
dagpipe_installer=false
collab=false
installer=false
docs=false
windows_appsdk=false
windows_dagpipe=false
workflow_contract=false
npm=false

if [[ "$release" == true ]]; then
  app_sdk_full=true
  resources=true
  installer=true
  dagpipe=true
  dagpipe_installer=true
  rust_ontology_governance=true
  windows_appsdk=true
  windows_dagpipe=true
  workflow_contract=true
  npm=true
fi

baseline_ok=false
if [[ -n "$base" && ! "$base" =~ ^0+$ ]] && git cat-file -e "$base^{commit}" 2>/dev/null; then
  baseline_ok=true
fi

changed=""
diff_args=()
default_ref="refs/remotes/origin/${default_branch}"
default_ref_available=false
if [[ "$release" != true && "$event_name" == "push" && "$ref_type" == "branch" &&
      -n "$ref_name" && -n "$default_branch" && "$ref_name" != "$default_branch" ]] &&
   git show-ref --verify --quiet "$default_ref"; then
  default_ref_available=true
fi

if [[ "$default_ref_available" == true ]]; then
  # Select the complete feature contribution from the fetched default ref,
  # including on first push. A diff error remains an error.
  base="$default_ref"
  diff_args=("$default_ref...$head_sha")
  changed="$(git diff --name-only "${diff_args[@]}")"
elif [[ "$baseline_ok" == true ]]; then
  diff_args=("$base" "$head_sha")
  changed="$(git diff --name-only "${diff_args[@]}")"
elif [[ "$release" == true ]]; then
  base=""
else
  app_sdk_full=true
  resources=true
  dagpipe=true
  dagpipe_installer=true
  collab=true
  installer=true
  docs=true
  rust_ontology_governance=true
  windows_appsdk=true
  windows_dagpipe=true
  workflow_contract=true
  npm=true
  base=""
fi

if [[ -z "$changed" && "$release" != true ]]; then
  app_sdk_full=true
  resources=true
  dagpipe=true
  dagpipe_installer=true
  collab=true
  installer=true
  docs=true
  rust_ontology_governance=true
  windows_appsdk=true
  windows_dagpipe=true
  workflow_contract=true
  npm=true
fi

is_deletion_only() {
  local path="$1"
  local name_status numstat status metadata_path extra
  local additions deletions numeric_path

  name_status="$(git diff --name-status --find-renames "${diff_args[@]}" -- "$path")"
  [[ "$(printf '%s\n' "$name_status" | awk 'NF { count++ } END { print count + 0 }')" == 1 ]] ||
    return 1
  IFS=$'\t' read -r status metadata_path extra <<< "$name_status"
  [[ ( "$status" == "M" || "$status" == "D" ) &&
     "$metadata_path" == "$path" && -z "$extra" ]] || return 1

  numstat="$(git diff --numstat --find-renames "${diff_args[@]}" -- "$path")"
  [[ "$(printf '%s\n' "$numstat" | awk 'NF { count++ } END { print count + 0 }')" == 1 ]] ||
    return 1
  IFS=$'\t' read -r additions deletions numeric_path <<< "$numstat"
  [[ "$additions" == "0" && "$deletions" =~ ^[1-9][0-9]*$ && "$numeric_path" == "$path" ]]
}

while IFS= read -r path; do
  [[ -n "$path" ]] || continue
  case "$path" in
    .github/workflows/verify.yml|scripts/ci/*|scripts/tests/test-verify-selector.sh|scripts/tests/test-governance-reference-distribution.sh)
      # The workflow file and its selector are validated by the dedicated
      # workflow-contract gate instead of fanning out the whole repository.
      workflow_contract=true
      ;;
    npm/*)
      npm=true
      ;;
    scripts/install-global-appsdk.sh)
      resources=true
      installer=true
      ;;
    scripts/tests/test-install-global-appsdk.sh)
      installer=true
      ;;
    scripts/*collab*|scripts/build-collab.sh|scripts/install-global-collab.sh)
      collab=true
      ;;
    scripts/install-global-dagpipe.sh)
      dagpipe=true
      dagpipe_installer=true
      windows_dagpipe=true
      ;;
    scripts/tests/test-windows-dagpipe.ps1)
      dagpipe_installer=true
      windows_dagpipe=true
      ;;
    scripts/tests/test-windows-appsdk-reset.ps1)
      windows_appsdk=true
      ;;
    scripts/check-worktree-limit.sh|scripts/install-project-commit-gate.sh|scripts/tests/test-worktree-commit-gate.sh)
      # The worktree-limit commit gate, its installer, and the isolated
      # real-Git fixture are validated by the dedicated workflow-contract job
      # instead of fanning out the whole repository.
      workflow_contract=true
      ;;
    rust/release-version|rust/Cargo.toml|rust/Cargo.lock|rust/build.rs|contracts/sdk-bundle.manifest.json|contracts/migrations/*|templates/minimal/.appsdk/project.json|templates/minimal/.appsdk/sdk.lock)
      app_sdk_full=true
      resources=true
      windows_appsdk=true
      ;;
    rust/src/main/producer.rs)
      rust_producer=true
      rust_cli_smoke=true
      rust_platform_lock_cli=true
      windows_appsdk=true
      ;;
    rust/tests/cli_smoke/part_07.rs|rust/tests/cli_smoke/part_08.rs|rust/tests/sdk_resource_refresh_cli.rs)
      # These legacy targets are retained, so deletion-only changes select the
      # replacement foundation contract plus all-target compilation. Any
      # addition, rename, binary/unknown metadata, or mixed support change
      # keeps the normal broader route.
      if is_deletion_only "$path"; then
        rust_verify_foundation=true
      elif [[ "$path" == "rust/tests/sdk_resource_refresh_cli.rs" ]]; then
        app_sdk_full=true
        windows_appsdk=true
      else
        rust_cli_smoke=true
        windows_appsdk=true
      fi
      ;;
    rust/src/guidance/compiler.rs|rust/tests/ontology_governance_cli.rs|rust/tests/fixtures/ontology-governance/*)
      rust_ontology_governance=true
      ;;
    rust/src/main.rs|rust/src/main/project.rs|rust/src/main/governance.rs|rust/src/main/registry.rs|rust/src/main/compile.rs|rust/src/main/verification.rs|rust/src/main/promotion.rs|rust/src/main/command_invocation.rs|rust/src/dagpipe.rs|rust/src/dagpipe_notification.rs|rust/src/main/verify_foundation.rs|contracts/dagpipe/verify-foundation.graph.json|rust/tests/verify_foundation_cli.rs|rust/tests/fixtures/verify_foundation/*)
      rust_verify_foundation=true
      ;;
    docs/dagpipe/manifest.json)
      rust_verify_foundation=true
      docs=true
      ;;
    rust/src/guidance.rs|rust/src/guidance/*|rust/tests/guidance_ablation_cli.rs)
      rust_guidance=true
      windows_appsdk=true
      ;;
    rust/src/communication.rs|rust/src/communication/*)
      rust_communication=true
      rust_platform_lock_cli=true
      windows_appsdk=true
      ;;
    rust/src/memory.rs|rust/src/memory_cli.rs|rust/src/bin/project-memory.rs)
      rust_memory=true
      windows_appsdk=true
      ;;
    rust/src/global_registry.rs|rust/src/global_registry_communication.rs|rust/src/global_registry_tests.rs)
      rust_registry=true
      rust_communication=true
      windows_appsdk=true
      ;;
    rust/tests/cli_smoke/main.rs|rust/tests/cli_smoke/goal_subscribe_rearm.rs|rust/tests/cli_smoke/part_0[1-9].rs|rust/tests/cli_smoke/part_1[0-9].rs|rust/tests/cli_smoke/part_2[0-5].rs)
      rust_cli_smoke=true
      windows_appsdk=true
      ;;
    rust/tests/communication_cli/main.rs|rust/tests/communication_cli/master_wake.rs|rust/tests/communication_cli/idle_wakeup.rs|rust/tests/communication_cli/routing_discovery.rs|rust/tests/communication_cli/rebind_loops.rs|rust/tests/communication_cli/runtime_identity.rs|rust/tests/communication_cli/delivery_retry.rs|rust/tests/communication_event_schema.rs|rust/tests/communication_request_schema.rs)
      rust_communication=true
      windows_appsdk=true
      ;;
    rust/tests/platform_lock_cli.rs)
      rust_platform_lock_cli=true
      windows_appsdk=true
      ;;
    rust/tests/registry_home_cli.rs)
      rust_registry_home_cli=true
      windows_appsdk=true
      ;;
    contracts/guidance/guidance-manifest.schema.json)
      rust_ontology_governance=true
      ;;
    contracts/maps/module-registry.json|contracts/maps/function-map.json)
      source_registry=true
      ;;
    rust/tests/reset_platform_cli.rs)
      windows_appsdk=true
      ;;
    rust/src/*|rust/tests/*)
      app_sdk_full=true
      windows_appsdk=true
      ;;
    dagpipe/docs/*|dagpipe/*.md)
      docs=true
      ;;
    dagpipe/src/lib.rs)
      dagpipe=true
      windows_dagpipe=true
      ;;
    dagpipe/src/governance.rs|dagpipe/tests/ontology_governance.rs|dagpipe/tests/support/ontology_governance.rs|dagpipe/examples/governed_catalog.rs|dagpipe/tests/governed_catalog_cli.rs|dagpipe/tests/fixtures/ontology-governance/*)
      dagpipe=true
      windows_dagpipe=true
      ;;
    dagpipe/Cargo.toml|dagpipe/Cargo.lock)
      dagpipe=true
      dagpipe_installer=true
      windows_dagpipe=true
      windows_appsdk=true
      ;;
    dagpipe/src/bin/*)
      dagpipe=true
      dagpipe_installer=true
      windows_dagpipe=true
      ;;
    dagpipe/src/*)
      dagpipe=true
      windows_dagpipe=true
      windows_appsdk=true
      ;;
    dagpipe/*)
      dagpipe=true
      dagpipe_installer=true
      windows_dagpipe=true
      ;;
    collab/docs/*|collab/*.md)
      docs=true
      ;;
    collab/*)
      collab=true
      ;;
    .agents/skills/appsdk-dev/*)
      docs=true
      ;;
    sdk-skill-sources/appsdk-project-governance/references/*.md)
      # Prose references need installed navigation and text checks. Machine
      # guidance, other assets, and mixed product paths keep their own jobs.
      docs=true
      workflow_contract=true
      ;;
    sdk-skill-sources/project-memory/*)
      resources=true
      rust_memory=true
      installer=true
      windows_appsdk=true
      ;;
    sdk-skill-sources/*)
      resources=true
      rust_guidance=true
      installer=true
      windows_appsdk=true
      ;;
    templates/*)
      resources=true
      rust_guidance=true
      windows_appsdk=true
      ;;
    contracts/*)
      app_sdk_full=true
      resources=true
      windows_appsdk=true
      ;;
    docs/*|README.md|MEMORY.md|*.md)
      docs=true
      ;;
    *)
      app_sdk_full=true
      dagpipe_installer=true
      windows_appsdk=true
      windows_dagpipe=true
      ;;
  esac
done <<< "$changed"

emit app_sdk_full "$app_sdk_full"
emit source_registry "$source_registry"
emit rust_guidance "$rust_guidance"
emit rust_communication "$rust_communication"
emit rust_memory "$rust_memory"
emit rust_registry "$rust_registry"
emit rust_producer "$rust_producer"
emit rust_verify_foundation "$rust_verify_foundation"
emit rust_ontology_governance "$rust_ontology_governance"
emit rust_cli_smoke "$rust_cli_smoke"
emit rust_platform_lock_cli "$rust_platform_lock_cli"
emit rust_registry_home_cli "$rust_registry_home_cli"
emit resources "$resources"
emit dagpipe "$dagpipe"
emit dagpipe_installer "$dagpipe_installer"
emit collab "$collab"
emit installer "$installer"
emit docs "$docs"
emit windows_appsdk "$windows_appsdk"
emit windows_dagpipe "$windows_dagpipe"
emit workflow_contract "$workflow_contract"
emit npm "$npm"
emit release "$release"
emit base_sha "$base"
emit head_sha "$head_sha"
