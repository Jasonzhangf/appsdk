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
rust_guidance=false
rust_communication=false
rust_memory=false
rust_registry=false
rust_producer=false
rust_cli_smoke=false
rust_platform_lock_cli=false
rust_registry_home_cli=false
resources=false
dagpipe=false
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
if [[ "$baseline_ok" == true ]]; then
  changed="$(git diff --name-only "$base" "$head_sha")"
elif [[ "$release" == true ]]; then
  base=""
else
  app_sdk_full=true
  resources=true
  dagpipe=true
  collab=true
  installer=true
  docs=true
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
  collab=true
  installer=true
  docs=true
  windows_appsdk=true
  windows_dagpipe=true
  workflow_contract=true
  npm=true
fi

while IFS= read -r path; do
  [[ -n "$path" ]] || continue
  case "$path" in
    .github/workflows/verify.yml|scripts/ci/*|scripts/tests/test-verify-selector.sh)
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
      windows_dagpipe=true
      ;;
    scripts/tests/test-windows-dagpipe.ps1)
      windows_dagpipe=true
      ;;
    scripts/tests/test-windows-appsdk-reset.ps1)
      windows_appsdk=true
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
    rust/src/guidance.rs|rust/src/guidance/*)
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
    dagpipe/Cargo.toml|dagpipe/Cargo.lock|dagpipe/src/lib.rs)
      dagpipe=true
      windows_dagpipe=true
      windows_appsdk=true
      ;;
    dagpipe/src/bin/*)
      dagpipe=true
      windows_dagpipe=true
      ;;
    dagpipe/src/*)
      dagpipe=true
      windows_dagpipe=true
      windows_appsdk=true
      ;;
    dagpipe/*)
      dagpipe=true
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
      windows_appsdk=true
      windows_dagpipe=true
      ;;
  esac
done <<< "$changed"

emit app_sdk_full "$app_sdk_full"
emit rust_guidance "$rust_guidance"
emit rust_communication "$rust_communication"
emit rust_memory "$rust_memory"
emit rust_registry "$rust_registry"
emit rust_producer "$rust_producer"
emit rust_cli_smoke "$rust_cli_smoke"
emit rust_platform_lock_cli "$rust_platform_lock_cli"
emit rust_registry_home_cli "$rust_registry_home_cli"
emit resources "$resources"
emit dagpipe "$dagpipe"
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
