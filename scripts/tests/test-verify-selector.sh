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
  if ! OUT="$(GITHUB_OUTPUT="" SELECTOR_TEST_BASELINE="$baseline" SELECTOR_TEST_CHANGED="$changed_file" \
    PATH="$stub_dir:$PATH" \
    EVENT_NAME="$event" REF_TYPE="$ref_type" HEAD_SHA="deadbeef" \
    PUSH_BEFORE="cafebabe" PR_BASE="cafebabe" REF_NAME="" DEFAULT_BRANCH="" \
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
expect_only "docs workflow_contract app_sdk_full windows_appsdk"

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
expect_only "app_sdk_full windows_appsdk"

run_history_scenario "feature-push-default-ref-missing" consumer-docs missing feature-before HEAD
expect_only "app_sdk_full windows_appsdk"

run_history_scenario "feature-push-name-missing" "" main feature-before HEAD
expect_only "app_sdk_full windows_appsdk"
run_history_scenario "feature-push-default-name-missing" consumer-docs "" feature-before HEAD
expect_only "app_sdk_full windows_appsdk"
run_history_scenario "feature-push-ref-type-missing" consumer-docs main feature-before HEAD push unknown
expect_only "app_sdk_full windows_appsdk"
run_history_scenario "pr-with-branch-metadata" consumer-docs main feature-before HEAD pull_request
expect_only "app_sdk_full windows_appsdk"

run_history_scenario "tag-with-branch-metadata" consumer-docs main feature-before HEAD push tag
expect_true "release"
expect_true "resources"
expect_true "windows_appsdk"
run_history_scenario "dispatch-with-branch-metadata" consumer-docs main feature-before HEAD workflow_dispatch
expect_true "release"
expect_true "resources"
expect_true "windows_appsdk"

run_history_scenario "feature-push-before-missing" consumer-docs main missing-before HEAD
expect_true "app_sdk_full"
expect_true "resources"
expect_true "windows_appsdk"
expect_true "windows_dagpipe"
run_history_scenario "feature-push-before-zero" consumer-docs main 00000000 HEAD
expect_true "app_sdk_full"
expect_true "collab"

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

if ((failures > 0)); then
  printf 'selector contract: %d failure(s)\n' "$failures" >&2
  exit 1
fi
echo 'selector contract: PASS'
