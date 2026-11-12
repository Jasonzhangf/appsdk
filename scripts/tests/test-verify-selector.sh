#!/usr/bin/env bash
#
# Narrow contract test for scripts/ci/select-verify-scope.sh and its wiring in
# .github/workflows/verify.yml. It drives the real selector through fixture
# diffs using a stub git, so it stays a single source of truth for the mapping.

set -euo pipefail

script_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd -P)"
repo_root="$(cd "$script_dir/../.." && pwd -P)"
selector="$repo_root/scripts/ci/select-verify-scope.sh"
workflow="$repo_root/.github/workflows/verify.yml"

failures=0
SCENARIO=""

fail() {
  printf 'FAIL: %s\n' "$*" >&2
  failures=$((failures + 1))
}

# --- structural contract: the workflow must call the single-source selector ---
assert_workflow_contains() {
  if ! grep -Fq -- "$1" "$workflow"; then
    fail "workflow is missing: $1"
  fi
}

assert_workflow_contains 'bash scripts/ci/select-verify-scope.sh'
assert_workflow_contains 'bash scripts/tests/test-governance-reference-distribution.sh'
assert_workflow_contains 'workflow_contract: ${{ steps.scope.outputs.workflow_contract }}'
assert_workflow_contains 'npm: ${{ steps.scope.outputs.npm }}'
assert_workflow_contains 'rust_verify_foundation: ${{ steps.scope.outputs.rust_verify_foundation }}'
assert_workflow_contains 'cargo check --manifest-path rust/Cargo.toml --locked --all-targets'
assert_workflow_contains 'cargo test --manifest-path rust/Cargo.toml --locked --bin appsdk verify_foundation::tests'
assert_workflow_contains 'cargo test --manifest-path rust/Cargo.toml --locked --bin appsdk command_invocation::tests'
assert_workflow_contains 'cargo test --manifest-path rust/Cargo.toml --locked --test verify_foundation_cli'
assert_workflow_contains '  windows-verify-foundation:'
assert_workflow_contains 'dagpipe::tests::graph_manifest_covers_every_embedded_graph_and_validate_reports_contracted_designs'
assert_workflow_contains 'dagpipe_validate_notifications_records_invocation_outcomes'
assert_workflow_contains 'dagpipe_validate_notifications_isolates_invocation_log_failure'
assert_workflow_contains 'dagpipe_validate_notifications_rejects_invocation_log_link'
assert_workflow_contains 'dagpipe_validate_notifications_isolates_invocation_log_contention'
assert_workflow_contains 'dagpipe_validate_notifications_accepts_relative_project_and_graph_paths'
assert_workflow_contains 'git diff --check "$BASE_SHA...$HEAD_SHA"'
if ! awk '
  /^  windows-verify-foundation:/ { active = 1; next }
  active && /^  [A-Za-z0-9_-]+:/ { exit }
  active && /(git rev-parse|cargo build --release|cli_smoke|reset_platform_cli|test-install-global-appsdk|appsdk (new|init|compile|freeze|publish)|sha256|checksum|digest)/ { blocked = 1 }
  END { exit blocked }
' "$workflow"; then
  fail "windows-verify-foundation contains a prohibited smoke, package, broad test, digest, or identity probe"
fi
if ! awk '
  /^      - / { guidance = 0 }
  $0 == "        if: needs.changes.outputs.rust_guidance == '\''true'\''" { guidance = 1 }
  guidance && $0 == "        run: cargo test --manifest-path rust/Cargo.toml --locked --test guidance_ablation_cli" { found = 1 }
  END { exit !found }
' "$workflow"; then
  fail "workflow is missing guidance_ablation_cli under the Guidance condition"
fi
for job in workflow-contract npm macos-appsdk package-appsdk npm-consumers; do
  assert_workflow_contains "  $job:"
done
if grep -Fq -- '.github/workflows/verify.yml)' "$workflow"; then
  fail "workflow still classifies .github/workflows/verify.yml inline"
fi

# --- behavioral contract: drive the selector through fixture diffs ---
work_dir="$(mktemp -d "${TMPDIR:-/tmp}/verify-selector.XXXXXX")"
changed_file="$work_dir/changed.txt"
name_status_file="$work_dir/name-status.txt"
numstat_file="$work_dir/numstat.txt"
git_calls_file="$work_dir/git-calls.txt"
stub_dir="$work_dir/bin"
mkdir -p "$stub_dir"

cat > "$stub_dir/git" <<'STUB'
#!/usr/bin/env bash
set -euo pipefail
case "${1:-}" in
  cat-file)
    [[ "${SELECTOR_TEST_BASELINE:-0}" == "1" ]]
    ;;
  diff)
    printf '%s\n' "$*" >> "${SELECTOR_TEST_GIT_CALLS:?}"
    path_arg=""
    for arg in "$@"; do
      path_arg="$arg"
    done
    case " $* " in
      *" --name-status "*)
        if [[ -n "${SELECTOR_TEST_NAME_STATUS_FILE:-}" ]]; then
          awk -F '\t' -v path="$path_arg" '$2 == path || $3 == path { print }' \
            "$SELECTOR_TEST_NAME_STATUS_FILE"
        fi
        ;;
      *" --numstat "*)
        if [[ -n "${SELECTOR_TEST_NUMSTAT_FILE:-}" ]]; then
          awk -F '\t' -v path="$path_arg" '$3 == path { print }' \
            "$SELECTOR_TEST_NUMSTAT_FILE"
        fi
        ;;
      *)
        cat "${SELECTOR_TEST_CHANGED:?}"
        ;;
    esac
    ;;
  show-ref)
    [[ "${SELECTOR_TEST_DEFAULT_REF:-0}" == "1" ]]
    ;;
  *)
    echo "unexpected git invocation: $*" >&2
    exit 2
    ;;
esac
STUB
chmod 0755 "$stub_dir/git"

cleanup() {
  rm -rf -- "$work_dir"
}
trap cleanup EXIT

OUT=""
run_scenario() {
  local name="$1" event="$2" ref_type="$3" baseline="$4" changed="$5"
  local name_status="${6:-}" numstat="${7:-}" default_ref="${8:-0}"
  SCENARIO="$name"
  printf '%s' "$changed" > "$changed_file"
  printf '%s' "$name_status" > "$name_status_file"
  printf '%s' "$numstat" > "$numstat_file"
  : > "$git_calls_file"
  if ! OUT="$(GITHUB_OUTPUT="" SELECTOR_TEST_BASELINE="$baseline" SELECTOR_TEST_CHANGED="$changed_file" \
    SELECTOR_TEST_NAME_STATUS_FILE="$name_status_file" SELECTOR_TEST_NUMSTAT_FILE="$numstat_file" \
    SELECTOR_TEST_GIT_CALLS="$git_calls_file" SELECTOR_TEST_DEFAULT_REF="$default_ref" \
    PATH="$stub_dir:$PATH" \
    EVENT_NAME="$event" REF_TYPE="$ref_type" HEAD_SHA="deadbeef" \
    PUSH_BEFORE="cafebabe" PR_BASE="cafebabe" REF_NAME="feature" DEFAULT_BRANCH="main" \
    bash "$selector" 2>"$work_dir/err")"; then
    fail "$name: selector exited non-zero: $(cat "$work_dir/err")"
    OUT=""
  fi
}

val() {
  printf '%s\n' "$OUT" | sed -n "s/^$1=//p"
}

expect_true() {
  [[ "$(val "$1")" == "true" ]] || fail "$SCENARIO: expected $1=true, got '$(val "$1")'"
}

expect_false() {
  [[ "$(val "$1")" == "false" ]] || fail "$SCENARIO: expected $1=false, got '$(val "$1")'"
}

# Every flag the selector can emit, for the "nothing else selected" checks.
all_flags=(
  app_sdk_full rust_guidance rust_communication rust_memory rust_registry
  rust_producer rust_verify_foundation rust_cli_smoke rust_platform_lock_cli rust_registry_home_cli
  resources dagpipe collab installer docs windows_appsdk windows_dagpipe
  workflow_contract npm
)
expect_only() {
  local keep="$1" flag
  for flag in "${all_flags[@]}"; do
    if [[ " $keep " == *" $flag "* ]]; then
      expect_true "$flag"
    else
      expect_false "$flag"
    fi
  done
}

# 1. Workflow-only change selects only the workflow contract gate.
run_scenario "workflow-only" "push" "branch" "1" ".github/workflows/verify.yml"
expect_only "workflow_contract"

# 2. Selector script change selects only the workflow contract gate.
run_scenario "selector-script" "push" "branch" "1" "scripts/ci/select-verify-scope.sh"
expect_only "workflow_contract"

# The worktree-limit commit gate, installer, and fixture select the same
# contract job and do not fan out the whole repository.
run_scenario "worktree-gate-check" "push" "branch" "1" "scripts/check-worktree-limit.sh"
expect_only "workflow_contract"

run_scenario "worktree-gate-installer" "push" "branch" "1" "scripts/install-project-commit-gate.sh"
expect_only "workflow_contract"

run_scenario "worktree-gate-fixture" "push" "branch" "1" "scripts/tests/test-worktree-commit-gate.sh"
expect_only "workflow_contract"

# 3. registry_home_cli target: two-test Ubuntu target plus native Windows AppSDK.
run_scenario "registry-home-cli" "push" "branch" "1" "rust/tests/registry_home_cli.rs"
expect_true "rust_registry_home_cli"
expect_true "windows_appsdk"
expect_false "app_sdk_full"
expect_false "windows_dagpipe"
expect_false "workflow_contract"

# The verify foundation owners are non-empty targeted selections. Shared owners
# and unknown paths must keep their existing fail-closed AppSDK selection.
for owner in \
  rust/src/main.rs \
  rust/src/main/project.rs \
  rust/src/main/governance.rs \
  rust/src/main/registry.rs \
  rust/src/main/compile.rs \
  rust/src/main/verification.rs \
  rust/src/main/promotion.rs \
  rust/src/main/command_invocation.rs \
  rust/src/dagpipe.rs \
  rust/src/dagpipe_notification.rs \
  rust/src/main/verify_foundation.rs \
  contracts/dagpipe/verify-foundation.graph.json \
  rust/tests/verify_foundation_cli.rs
do
  run_scenario "verify-foundation-owner:$owner" "push" "branch" "1" "$owner"
  expect_only "rust_verify_foundation"
done

run_scenario "verify-foundation-static-fixture" "push" "branch" "1" \
  "rust/tests/fixtures/verify_foundation/draft/.appsdk/project.json"
expect_only "rust_verify_foundation"

run_scenario "verify-foundation-manifest" "push" "branch" "1" \
  "docs/dagpipe/manifest.json"
expect_only "rust_verify_foundation docs"

run_scenario "verify-foundation-static-fixture-and-machine-contract" "push" "branch" "1" \
  $'rust/tests/fixtures/verify_foundation/selected-test-governance.json\ncontracts/records/example.schema.json'
expect_only "rust_verify_foundation app_sdk_full resources windows_appsdk"

run_scenario "verify-foundation-and-unrelated-source" "push" "branch" "1" \
  $'rust/src/main/verify_foundation.rs\nrust/src/memory.rs'
expect_only "rust_verify_foundation rust_memory windows_appsdk"

run_scenario "verify-foundation-and-producer" "push" "branch" "1" \
  $'rust/src/main.rs\nrust/src/main/producer.rs'
expect_only "rust_verify_foundation rust_producer rust_cli_smoke rust_platform_lock_cli windows_appsdk"

run_scenario "verify-foundation-and-dependency" "push" "branch" "1" \
  $'rust/src/main/verify_foundation.rs\nrust/Cargo.toml'
expect_only "rust_verify_foundation app_sdk_full resources windows_appsdk"

run_scenario "verify-foundation-and-machine-contract" "push" "branch" "1" \
  $'rust/tests/verify_foundation_cli.rs\ncontracts/records/example.schema.json'
expect_only "rust_verify_foundation app_sdk_full resources windows_appsdk"

run_scenario "verify-foundation-and-template" "push" "branch" "1" \
  $'rust/tests/verify_foundation_cli.rs\ntemplates/minimal/AGENTS.md'
expect_only "rust_verify_foundation rust_guidance resources windows_appsdk"

run_scenario "verify-foundation-and-unknown" "push" "branch" "1" \
  $'rust/tests/verify_foundation_cli.rs\nunknown/file.txt'
expect_only "rust_verify_foundation app_sdk_full windows_appsdk windows_dagpipe"

legacy_numstat=$'0\t49\trust/tests/cli_smoke/part_07.rs\n0\t19\trust/tests/cli_smoke/part_08.rs\n0\t27\trust/tests/sdk_resource_refresh_cli.rs'
candidate_legacy_status=$'M\trust/tests/cli_smoke/part_07.rs\nM\trust/tests/cli_smoke/part_08.rs\nM\trust/tests/sdk_resource_refresh_cli.rs'

run_scenario "legacy-part-07-deletion" "push" "branch" "1" \
  "rust/tests/cli_smoke/part_07.rs" "$candidate_legacy_status" "$legacy_numstat"
expect_only "rust_verify_foundation"

run_scenario "legacy-part-08-deletion" "push" "branch" "1" \
  "rust/tests/cli_smoke/part_08.rs" "$candidate_legacy_status" "$legacy_numstat"
expect_only "rust_verify_foundation"

run_scenario "legacy-sdk-resource-refresh-deletion" "push" "branch" "1" \
  "rust/tests/sdk_resource_refresh_cli.rs" "$candidate_legacy_status" "$legacy_numstat"
expect_only "rust_verify_foundation"

run_scenario "legacy-deletion-metadata-default-ref" "push" "branch" "0" \
  "rust/tests/cli_smoke/part_07.rs" "$candidate_legacy_status" "$legacy_numstat" "1"
expect_only "rust_verify_foundation"
for expected_call in \
  'diff --name-only refs/remotes/origin/main...deadbeef' \
  'diff --name-status --find-renames refs/remotes/origin/main...deadbeef -- rust/tests/cli_smoke/part_07.rs' \
  'diff --numstat --find-renames refs/remotes/origin/main...deadbeef -- rust/tests/cli_smoke/part_07.rs'
do
  if ! grep -Fxq -- "$expected_call" "$git_calls_file"; then
    fail "legacy-deletion-metadata-default-ref: missing Git call '$expected_call'"
  fi
done
[[ "$(val base_sha)" == "refs/remotes/origin/main" ]] ||
  fail "legacy-deletion-metadata-default-ref: expected the default ref as base_sha"

run_scenario "legacy-part-07-addition" "push" "branch" "1" \
  "rust/tests/cli_smoke/part_07.rs" \
  $'A\trust/tests/cli_smoke/part_07.rs' \
  $'49\t0\trust/tests/cli_smoke/part_07.rs'
expect_only "rust_cli_smoke windows_appsdk"

run_scenario "legacy-part-08-rename" "push" "branch" "1" \
  "rust/tests/cli_smoke/part_08.rs" \
  $'R100\trust/tests/cli_smoke/part_08.rs\trust/tests/cli_smoke/part_08_moved.rs' \
  $'0\t0\trust/tests/cli_smoke/part_08.rs => rust/tests/cli_smoke/part_08_moved.rs'
expect_only "rust_cli_smoke windows_appsdk"

run_scenario "legacy-sdk-resource-refresh-binary" "push" "branch" "1" \
  "rust/tests/sdk_resource_refresh_cli.rs" \
  $'D\trust/tests/sdk_resource_refresh_cli.rs' \
  $'-\t-\trust/tests/sdk_resource_refresh_cli.rs'
expect_only "app_sdk_full windows_appsdk"

run_scenario "legacy-part-07-unknown-modification" "push" "branch" "1" \
  "rust/tests/cli_smoke/part_07.rs" \
  $'M\trust/tests/cli_smoke/part_07.rs' \
  $'1\t1\trust/tests/cli_smoke/part_07.rs'
expect_only "rust_cli_smoke windows_appsdk"

run_scenario "legacy-test-support-change" "push" "branch" "1" \
  "rust/tests/cli_smoke/main.rs"
expect_only "rust_cli_smoke windows_appsdk"

current_candidate_paths=$(printf '%s\n' \
  '.agents/skills/appsdk-dev/SKILL.md' \
  '.github/workflows/verify.yml' \
  'AGENTS.md' \
  'contracts/dagpipe/verify-foundation.graph.json' \
  'docs/dagpipe/manifest.json' \
  'docs/design/appsdk-local-operator-adoption.md' \
  'rust/src/dagpipe.rs' \
  'rust/src/dagpipe_notification.rs' \
  'rust/src/main.rs' \
  'rust/src/main/command_invocation.rs' \
  'rust/src/main/compile.rs' \
  'rust/src/main/governance.rs' \
  'rust/src/main/project.rs' \
  'rust/src/main/promotion.rs' \
  'rust/src/main/registry.rs' \
  'rust/src/main/verification.rs' \
  'rust/src/main/verify_foundation.rs' \
  'rust/tests/cli_smoke/part_07.rs' \
  'rust/tests/cli_smoke/part_08.rs' \
  'rust/tests/fixtures/verify_foundation/compiled/.appsdk/project.json' \
  'rust/tests/fixtures/verify_foundation/compiled/.appsdk/sdk.lock' \
  'rust/tests/fixtures/verify_foundation/draft/.appsdk/maps/function-map.json' \
  'rust/tests/fixtures/verify_foundation/draft/.appsdk/maps/mainline-call-map.json' \
  'rust/tests/fixtures/verify_foundation/draft/.appsdk/maps/module-registry.json' \
  'rust/tests/fixtures/verify_foundation/draft/.appsdk/maps/resource-map.json' \
  'rust/tests/fixtures/verify_foundation/draft/.appsdk/maps/verification-map.json' \
  'rust/tests/fixtures/verify_foundation/draft/.appsdk/project.json' \
  'rust/tests/fixtures/verify_foundation/draft/.appsdk/sdk.lock' \
  'rust/tests/fixtures/verify_foundation/selected-test-governance.json' \
  'rust/tests/sdk_resource_refresh_cli.rs' \
  'rust/tests/verify_foundation_cli.rs' \
  'scripts/ci/select-verify-scope.sh' \
  'scripts/tests/test-verify-selector.sh')
run_scenario "current-candidate-diff" "push" "branch" "1" \
  "$current_candidate_paths" "$candidate_legacy_status" "$legacy_numstat"
expect_only "rust_verify_foundation docs workflow_contract"

# 4. npm change selects only the npm Node checks on a normal push.
run_scenario "npm-only" "push" "branch" "1" "npm/appsdk/lib/launcher.js"
expect_only "npm"

# 5. Mixed workflow + npm change keeps both risk-based targets.
run_scenario "workflow-and-npm" "push" "branch" "1" $'.github/workflows/verify.yml\nnpm/scripts/release-artifacts.js'
expect_only "workflow_contract npm"

# 6. Tag release keeps the declared full release outputs.
run_scenario "tag-release" "push" "tag" "0" ""
expect_true "release"
expect_true "app_sdk_full"
expect_true "resources"
expect_true "installer"
expect_true "dagpipe"
expect_true "windows_appsdk"
expect_true "windows_dagpipe"
expect_true "workflow_contract"
expect_true "npm"

# 7. Manual dispatch matches the tag release outputs.
run_scenario "dispatch-release" "workflow_dispatch" "branch" "0" ""
expect_true "release"
expect_true "app_sdk_full"
expect_true "windows_appsdk"
expect_true "windows_dagpipe"
expect_true "workflow_contract"
expect_true "npm"

# 8. Producer change keeps its existing targeted selection.
run_scenario "producer" "push" "branch" "1" "rust/src/main/producer.rs"
expect_true "rust_producer"
expect_true "rust_cli_smoke"
expect_true "rust_platform_lock_cli"
expect_true "windows_appsdk"
expect_false "app_sdk_full"
expect_false "workflow_contract"

# 9. DAGPipe library change keeps DAGPipe plus its Windows consumers.
run_scenario "dagpipe-lib" "push" "branch" "1" "dagpipe/src/lib.rs"
expect_true "dagpipe"
expect_true "windows_dagpipe"
expect_true "windows_appsdk"
expect_false "app_sdk_full"

# 10. Unknown path stays fail-closed to the full repository.
run_scenario "unknown-path" "push" "branch" "1" "some/new/file.txt"
expect_true "app_sdk_full"
expect_true "windows_appsdk"
expect_true "windows_dagpipe"
expect_false "workflow_contract"
expect_false "npm"

# The Windows AppSDK reset harness selects its native owner only.
run_scenario "windows-appsdk-reset" "push" "branch" "1" "scripts/tests/test-windows-appsdk-reset.ps1"
expect_only "windows_appsdk"

# The Windows-capable reset CLI integration target selects the same native owner.
run_scenario "reset-platform-cli" "push" "branch" "1" "rust/tests/reset_platform_cli.rs"
expect_only "windows_appsdk"

# 11. No usable comparison baseline stays fail-closed.
run_scenario "no-baseline" "push" "branch" "0" ""
expect_true "app_sdk_full"
expect_true "collab"
expect_true "installer"
expect_true "docs"
expect_true "windows_appsdk"
expect_true "windows_dagpipe"
expect_true "workflow_contract"
expect_true "npm"

# 12. Empty diff on a normal push stays fail-closed.
run_scenario "empty-diff" "push" "branch" "1" ""
expect_true "app_sdk_full"
expect_true "windows_dagpipe"
expect_true "workflow_contract"
expect_true "npm"

# 13. Docs-only change selects only docs.
run_scenario "docs-only" "push" "branch" "1" "README.md"
expect_only "docs"

# Project development Skill edits are documentation, not unknown product paths.
run_scenario "appsdk-dev-skill-only" "push" "branch" "1" ".agents/skills/appsdk-dev/SKILL.md"
expect_only "docs"

# 14. Workflow-only change on a pull_request event also selects the contract gate.
# Governance reference prose needs text and distribution checks. Machine
# resources and mixed product changes retain their existing consumers.
run_scenario "governance-reference" "pull_request" "branch" "1" \
  "sdk-skill-sources/appsdk-project-governance/references/bootstrap-migration.md"
expect_only "docs workflow_contract"

run_scenario "reference-distribution-test" "push" "branch" "1" \
  "scripts/tests/test-governance-reference-distribution.sh"
expect_only "workflow_contract"

run_scenario "reference-and-guidance" "pull_request" "branch" "1" \
  $'sdk-skill-sources/appsdk-project-governance/references/bootstrap-migration.md\nsdk-skill-sources/appsdk-project-governance/appsdk-guidance.json'
expect_only "docs workflow_contract rust_guidance resources installer windows_appsdk"

run_scenario "reference-non-markdown" "push" "branch" "1" \
  "sdk-skill-sources/appsdk-project-governance/references/runtime.json"
expect_only "rust_guidance resources installer windows_appsdk"

run_scenario "reference-and-schema" "pull_request" "branch" "1" \
  $'sdk-skill-sources/appsdk-project-governance/references/bootstrap-migration.md\ncontracts/records/example.schema.json'
expect_only "docs workflow_contract app_sdk_full resources windows_appsdk"

run_scenario "reference-and-template" "pull_request" "branch" "1" \
  $'sdk-skill-sources/appsdk-project-governance/references/bootstrap-migration.md\ntemplates/minimal/AGENTS.md'
expect_only "docs workflow_contract rust_guidance resources windows_appsdk"

run_scenario "reference-and-rust" "pull_request" "branch" "1" \
  $'sdk-skill-sources/appsdk-project-governance/references/bootstrap-migration.md\nrust/src/main.rs'
expect_only "docs workflow_contract rust_verify_foundation"

run_scenario "reference-and-unknown" "pull_request" "branch" "1" \
  $'sdk-skill-sources/appsdk-project-governance/references/bootstrap-migration.md\nunknown/file.txt'
expect_only "docs workflow_contract app_sdk_full windows_appsdk windows_dagpipe"

run_scenario "pr-workflow" "pull_request" "branch" "1" ".github/workflows/verify.yml"
expect_only "workflow_contract"

# 15. The public Guidance ablation CLI target selects its exact product owners.
run_scenario "guidance-ablation-cli" "pull_request" "branch" "1" "rust/tests/guidance_ablation_cli.rs"
expect_only "rust_guidance windows_appsdk"

# 16. The Guidance milestone keeps its resource, installer, and workflow owners.
run_scenario "guidance-selector-milestone" "pull_request" "branch" "1" \
  $'rust/tests/guidance_ablation_cli.rs\nrust/src/guidance/compiler.rs\nsdk-skill-sources/appsdk-project-governance/appsdk-guidance.json\nscripts/ci/select-verify-scope.sh\nscripts/tests/test-verify-selector.sh\n.github/workflows/verify.yml'
expect_only "rust_guidance windows_appsdk resources installer workflow_contract"

# Real Git history proves that a feature push excludes changes inherited from
# main while preserving its own changes. Branch names provide the fixture refs.
history_repo="$work_dir/history"
git init -q "$history_repo"
git -C "$history_repo" config user.name 'Selector Fixture'
git -C "$history_repo" config user.email 'selector@appsdk.invalid'
git -C "$history_repo" config commit.gpgsign false
git -C "$history_repo" checkout -q -b main
printf 'seed\n' > "$history_repo/seed.txt"
git -C "$history_repo" add seed.txt
git -C "$history_repo" commit -q -m seed
git -C "$history_repo" branch main-before
git -C "$history_repo" checkout -q -b consumer-docs
mkdir -p "$history_repo/sdk-skill-sources/appsdk-project-governance/references"
printf 'guide\n' > "$history_repo/sdk-skill-sources/appsdk-project-governance/references/bootstrap-migration.md"
git -C "$history_repo" add sdk-skill-sources
git -C "$history_repo" commit -q -m 'consumer guide'
git -C "$history_repo" branch feature-before
git -C "$history_repo" checkout -q main
mkdir -p "$history_repo/rust/src"
printf 'upstream\n' > "$history_repo/rust/src/main.rs"
git -C "$history_repo" add rust/src/main.rs
git -C "$history_repo" commit -q -m 'upstream source'
git -C "$history_repo" update-ref refs/remotes/origin/main main
git -C "$history_repo" checkout -q consumer-docs
git -C "$history_repo" merge -q --no-edit main

run_history_scenario() {
  SCENARIO="$1"
  OUT="$(cd "$history_repo" && GITHUB_OUTPUT="" \
    REF_NAME="$2" DEFAULT_BRANCH="$3" PUSH_BEFORE="$4" HEAD_SHA="$5" \
    PR_BASE="$4" EVENT_NAME="${6:-push}" REF_TYPE="${7:-branch}" \
    bash "$selector")"
}

git -C "$history_repo" diff --name-only feature-before HEAD | grep -Fxq rust/src/main.rs
run_history_scenario "feature-push-after-main-merge" consumer-docs main feature-before HEAD
expect_only "docs workflow_contract"

run_history_scenario "default-branch-push" main main main-before main
expect_only "rust_verify_foundation"

run_history_scenario "feature-push-default-ref-missing" consumer-docs missing feature-before HEAD
expect_only "rust_verify_foundation"

run_history_scenario "feature-push-name-missing" "" main feature-before HEAD
expect_only "rust_verify_foundation"
run_history_scenario "feature-push-default-name-missing" consumer-docs "" feature-before HEAD
expect_only "rust_verify_foundation"
run_history_scenario "feature-push-ref-type-missing" consumer-docs main feature-before HEAD push unknown
expect_only "rust_verify_foundation"
run_history_scenario "pr-with-branch-metadata" consumer-docs main feature-before HEAD pull_request
expect_only "rust_verify_foundation"

run_history_scenario "tag-with-branch-metadata" consumer-docs main feature-before HEAD push tag
expect_true "release"
expect_true "resources"
expect_true "windows_appsdk"
run_history_scenario "dispatch-with-branch-metadata" consumer-docs main feature-before HEAD workflow_dispatch
expect_true "release"
expect_true "resources"
expect_true "windows_appsdk"

run_history_scenario "feature-push-before-missing" consumer-docs main missing-before HEAD
expect_only "docs workflow_contract"
run_history_scenario "feature-push-before-zero" consumer-docs main 00000000 HEAD
expect_only "docs workflow_contract"
run_history_scenario "feature-push-before-empty" consumer-docs main "" HEAD
expect_only "docs workflow_contract"
[[ "$(val base_sha)" == "refs/remotes/origin/main" ]] ||
  fail "feature-push-before-empty: expected default ref as the Docs comparison base"

run_history_scenario "feature-push-default-ref-missing-before-empty" consumer-docs missing "" HEAD
expect_true "app_sdk_full"
expect_true "collab"
run_history_scenario "feature-push-default-ref-missing-before-zero" consumer-docs missing 00000000 HEAD
expect_true "app_sdk_full"
expect_true "installer"
run_history_scenario "feature-push-default-ref-missing-before-unparseable" consumer-docs missing not-a-commit HEAD
expect_true "app_sdk_full"
expect_true "windows_dagpipe"

# A newer default branch must not appear as deletions in an older feature.
git -C "$history_repo" checkout -q main
mkdir -p "$history_repo/contracts"
printf '{}\n' > "$history_repo/contracts/new.schema.json"
git -C "$history_repo" add contracts
git -C "$history_repo" commit -q -m 'later upstream schema'
git -C "$history_repo" update-ref refs/remotes/origin/main main
git -C "$history_repo" checkout -q consumer-docs
run_history_scenario "feature-push-behind-default" consumer-docs main feature-before HEAD
expect_only "docs workflow_contract"

printf '{}\n' > "$history_repo/sdk-skill-sources/appsdk-project-governance/appsdk-guidance.json"
git -C "$history_repo" add sdk-skill-sources
git -C "$history_repo" commit -q -m 'feature guidance'
run_history_scenario "feature-owned-machine-change" consumer-docs main feature-before HEAD
expect_only "docs workflow_contract rust_guidance resources installer windows_appsdk"

git -C "$history_repo" branch no-contribution main
run_history_scenario "feature-empty-contribution" no-contribution main feature-before no-contribution
expect_true "app_sdk_full"
expect_true "collab"
run_history_scenario "feature-empty-contribution-zero-before" no-contribution main 00000000 no-contribution
expect_true "app_sdk_full"
expect_true "windows_dagpipe"

git -C "$history_repo" checkout -q --orphan unrelated
git -C "$history_repo" commit -q -m 'unrelated history'
git -C "$history_repo" update-ref refs/remotes/origin/unrelated unrelated
git -C "$history_repo" checkout -q consumer-docs
SCENARIO="feature-diff-no-common-ancestor"
if (cd "$history_repo" && EVENT_NAME=push REF_TYPE=branch REF_NAME=consumer-docs \
  DEFAULT_BRANCH=unrelated PUSH_BEFORE=feature-before HEAD_SHA=HEAD GITHUB_OUTPUT="" \
  bash "$selector" > "$work_dir/unrelated.out" 2> "$work_dir/unrelated.err"); then
  fail 'uncomputable feature diff was accepted'
fi

SCENARIO="feature-diff-no-common-ancestor-zero-before"
if (cd "$history_repo" && EVENT_NAME=push REF_TYPE=branch REF_NAME=consumer-docs \
  DEFAULT_BRANCH=unrelated PUSH_BEFORE=00000000 HEAD_SHA=HEAD GITHUB_OUTPUT="" \
  bash "$selector" > "$work_dir/unrelated-zero.out" 2> "$work_dir/unrelated-zero.err"); then
  fail 'uncomputable feature diff with zero before was accepted'
fi

git -C "$history_repo" checkout -q main
mkdir -p "$history_repo/rust/tests/cli_smoke"
printf 'retained fixture\n' > "$history_repo/rust/tests/cli_smoke/part_07.rs"
git -C "$history_repo" add rust/tests/cli_smoke/part_07.rs
git -C "$history_repo" commit -q -m 'retained legacy target'
git -C "$history_repo" update-ref refs/remotes/origin/main main
git -C "$history_repo" checkout -q -b legacy-deletion
git -C "$history_repo" rm -q rust/tests/cli_smoke/part_07.rs
git -C "$history_repo" commit -q -m 'retire legacy assertions'
run_history_scenario "feature-push-deletion-only-zero-before" legacy-deletion main 00000000 HEAD
expect_only "rust_verify_foundation"
run_history_scenario "feature-push-deletion-only-empty-before" legacy-deletion main "" HEAD
expect_only "rust_verify_foundation"

if ((failures > 0)); then
  printf 'selector contract: %d failure(s)\n' "$failures" >&2
  exit 1
fi
echo 'selector contract: PASS'
