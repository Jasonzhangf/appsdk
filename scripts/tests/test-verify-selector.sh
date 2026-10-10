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
assert_workflow_contains 'workflow_contract: ${{ steps.scope.outputs.workflow_contract }}'
assert_workflow_contains 'npm: ${{ steps.scope.outputs.npm }}'
for job in workflow-contract npm macos-appsdk package-appsdk npm-consumers; do
  assert_workflow_contains "  $job:"
done
if grep -Fq -- '.github/workflows/verify.yml)' "$workflow"; then
  fail "workflow still classifies .github/workflows/verify.yml inline"
fi

# --- behavioral contract: drive the selector through fixture diffs ---
work_dir="$(mktemp -d "${TMPDIR:-/tmp}/verify-selector.XXXXXX")"
changed_file="$work_dir/changed.txt"
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
    cat "${SELECTOR_TEST_CHANGED:?}"
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
  SCENARIO="$name"
  printf '%s' "$changed" > "$changed_file"
  if ! OUT="$(SELECTOR_TEST_BASELINE="$baseline" SELECTOR_TEST_CHANGED="$changed_file" \
    PATH="$stub_dir:$PATH" \
    EVENT_NAME="$event" REF_TYPE="$ref_type" HEAD_SHA="deadbeef" \
    PUSH_BEFORE="cafebabe" PR_BASE="cafebabe" \
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
  rust_producer rust_cli_smoke rust_platform_lock_cli rust_registry_home_cli
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

# 3. registry_home_cli target: two-test Ubuntu target plus native Windows AppSDK.
run_scenario "registry-home-cli" "push" "branch" "1" "rust/tests/registry_home_cli.rs"
expect_true "rust_registry_home_cli"
expect_true "windows_appsdk"
expect_false "app_sdk_full"
expect_false "windows_dagpipe"
expect_false "workflow_contract"

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

# 11. Missing baseline (force push / first push) stays fail-closed.
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

# 14. Workflow-only change on a pull_request event also selects the contract gate.
run_scenario "pr-workflow" "pull_request" "branch" "1" ".github/workflows/verify.yml"
expect_only "workflow_contract"

if ((failures > 0)); then
  printf 'selector contract: %d failure(s)\n' "$failures" >&2
  exit 1
fi
echo 'selector contract: PASS'
