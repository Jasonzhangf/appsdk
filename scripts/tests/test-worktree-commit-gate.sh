#!/usr/bin/env bash
#
# Isolated real-Git black-box fixture for the AppSDK worktree-limit commit gate
# (scripts/check-worktree-limit.sh) and its project-local installer
# (scripts/install-project-commit-gate.sh).
#
# Every fixture repository is an independent, disposable `git init` repository
# under a test-owned temporary directory. They are not linked worktrees of
# AppSDK, so their intentional 11-worktree cases do not change AppSDK's own
# registered count. Only fixture-local hooks and fixture-local author identity
# are used. No hook-disabling flags, no core.hooksPath changes, no host clock
# changes.
#
# The fixed clock is injected only by a fixture-owned hook adapter that sources
# the candidate gate and replaces its private clock function. The production
# installer never emits that adapter.

set -euo pipefail

script_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd -P)"
repo_root="$(cd "$script_dir/../.." && pwd -P)"
gate="$repo_root/scripts/check-worktree-limit.sh"
installer="$repo_root/scripts/install-project-commit-gate.sh"

failures=0
scenario=''

fail() {
  printf 'FAIL [%s]: %s\n' "$scenario" "$*" >&2
  failures=$((failures + 1))
}

note() {
  printf '  %s\n' "$*"
}

work_root="$(mktemp -d "${TMPDIR:-/tmp}/appsdk-worktree-gate-test.XXXXXX")"
cleanup() {
  chmod -R u+w "$work_root" 2>/dev/null || true
  rm -rf "$work_root"
}
trap cleanup EXIT

note "fixture root: $work_root"

# --- fixture helpers -------------------------------------------------------

init_repo() {
  local dir="$1"
  mkdir -p "$dir"
  git -C "$dir" init -q
  git -C "$dir" config user.email fixture@appsdk.invalid
  git -C "$dir" config user.name 'AppSDK Fixture'
  git -C "$dir" config commit.gpgsign false
  printf 'seed\n' > "$dir/seed.txt"
  git -C "$dir" add seed.txt
  git -C "$dir" commit -q -m 'seed'
}

resolve_hooks_dir() {
  local repo="$1" common
  common="$(git -C "$repo" rev-parse --git-common-dir)"
  case "$common" in
    /*) ;;
    *) common="$repo/$common" ;;
  esac
  printf '%s/hooks\n' "$(cd "$common" && pwd -P)"
}

count_trees() {
  # Count NUL-delimited "worktree " record fields by reading each field with
  # `read -d ''`. This keeps any embedded newline (for example a path that
  # contains a newline followed by "worktree x") inside a single record, so it
  # cannot be counted as an extra entry. Only read a capture that succeeded.
  local repo="$1" capture record n
  capture="$(mktemp "$work_root/count.XXXXXX")"
  if ! git -C "$repo" worktree list --porcelain -z > "$capture"; then
    rm -f "$capture"
    return 1
  fi
  n=0
  while IFS= read -r -d '' record; do
    case "$record" in
      'worktree '*) n=$((n + 1)) ;;
    esac
  done < "$capture"
  rm -f "$capture"
  printf '%s\n' "$n"
}

add_worktrees() {
  local repo="$1" extra="$2" special="$3" i wt
  for ((i = 1; i <= extra; i++)); do
    wt="$repo-wt$i"
    if [[ "$special" == 'newline' && "$i" -eq "$extra" ]]; then
      wt="$repo"$'-wt with\nnewline'
    elif [[ "$special" == 'worktreeline' && "$i" -eq "$extra" ]]; then
      # A valid registered path whose embedded newline is followed by a
      # literal "worktree " token. A line-based count would treat the tail of
      # this path as an extra record and reject a legal ten-tree repository.
      wt="$repo"$'-wt special\nworktree x'
    fi
    git -C "$repo" worktree add -q -b "wt-$i" "$wt"
  done
}

install_adapter() {
  # Fixture-owned pre-commit that sources the candidate gate, replaces its
  # private clock, and calls the same main function production uses.
  local repo="$1" clock="$2"
  local hooks_dir hook clock_line
  hooks_dir="$(resolve_hooks_dir "$repo")"
  mkdir -p "$hooks_dir"
  case "$clock" in
    FAIL) clock_line='worktree_gate_now() { return 1; }' ;;
    *) clock_line="worktree_gate_now() { printf '%s\\n' '$clock'; }" ;;
  esac
  hook="$hooks_dir/pre-commit"
  {
    printf '#!/usr/bin/env bash\n'
    printf 'set -euo pipefail\n'
    printf 'source %s\n' "$(printf '%q' "$gate")"
    printf '%s\n' "$clock_line"
    printf 'worktree_gate_main "$@"\n'
  } > "$hook"
  chmod 0755 "$hook"
}

install_dispatcher() {
  # Fixture-owned dispatcher with a known insertion point: it runs the gate
  # between explicit before/after callbacks, so the supported composition
  # (callbacks preserved around the gate) is exercised.
  local repo="$1" clock="$2" after_action="$3" marker="$4"
  local hooks_dir hook clock_line
  hooks_dir="$(resolve_hooks_dir "$repo")"
  mkdir -p "$hooks_dir"
  case "$clock" in
    FAIL) clock_line='worktree_gate_now() { return 1; }' ;;
    *) clock_line="worktree_gate_now() { printf '%s\\n' '$clock'; }" ;;
  esac
  hook="$hooks_dir/pre-commit"
  {
    printf '#!/usr/bin/env bash\n'
    printf 'set -euo pipefail\n'
    printf 'source %s\n' "$(printf '%q' "$gate")"
    printf '%s\n' "$clock_line"
    printf 'printf "before\\n" >> %s\n' "$(printf '%q' "$marker")"
    printf 'worktree_gate_main "$@" || exit $?\n'
    printf 'printf "after\\n" >> %s\n' "$(printf '%q' "$marker")"
    if [[ -n "$after_action" ]]; then
      printf '%s\n' "$after_action"
    fi
    printf 'exit 0\n'
  } > "$hook"
  chmod 0755 "$hook"
}

attempt_commit() {
  # Returns 0 when git creates a commit, 1 otherwise. Captures combined output.
  local repo="$1" file="$2" text="$3" log="$4"
  printf '%s\n' "$text" > "$repo/$file"
  git -C "$repo" add -- "$file"
  if git -C "$repo" commit -q -m "$text" > "$log" 2>&1; then
    return 0
  fi
  return 1
}

# --- A. 11 registered worktrees (one path contains a space and a newline) ---

scenario='grace-11-allows'
repo_eleven="$work_root/eleven"
init_repo "$repo_eleven"
add_worktrees "$repo_eleven" 10 newline
special_path="$repo_eleven"$'-wt with\nnewline'
if [[ ! -d "$special_path" ]]; then
  fail 'worktree path with a space and a newline was not created'
fi
n="$(count_trees "$repo_eleven")"
if [[ "$n" -ne 11 ]]; then
  fail "eleven fixture registered $n worktrees, expected 11"
fi
install_adapter "$repo_eleven" 1793334959
if ! attempt_commit "$repo_eleven" grace.txt 'grace before deadline' "$work_root/grace.log"; then
  fail "grace-before-deadline commit was rejected: $(cat "$work_root/grace.log")"
else
  if ! git -C "$repo_eleven" log -1 --format=%s | grep -qx 'grace before deadline'; then
    fail 'grace commit did not create the expected commit'
  fi
fi

scenario='deadline-11-rejects'
install_adapter "$repo_eleven" 1793334960
git -C "$repo_eleven" reset -q
head_before="$(git -C "$repo_eleven" rev-parse HEAD)"
if attempt_commit "$repo_eleven" boundary.txt 'at deadline' "$work_root/deadline.log"; then
  fail 'commit at the exact deadline with 11 worktrees was allowed'
else
  if ! grep -q 'AppSDK commit blocked' "$work_root/deadline.log"; then
    fail 'deadline rejection is missing the blocked diagnostic'
  fi
  if ! grep -q '11 registered worktrees' "$work_root/deadline.log"; then
    fail 'deadline rejection is missing the count'
  fi
  if ! grep -q 'maximum of 10' "$work_root/deadline.log"; then
    fail 'deadline rejection is missing the maximum'
  fi
  if ! grep -q '2026-10-30 04:36 UTC' "$work_root/deadline.log"; then
    fail 'deadline rejection is missing the UTC deadline'
  fi
fi
head_after="$(git -C "$repo_eleven" rev-parse HEAD)"
if [[ "$head_before" != "$head_after" ]]; then
  fail 'a rejected commit created a commit'
fi
if ! git -C "$repo_eleven" diff --cached --name-only | grep -qx 'boundary.txt'; then
  fail 'a rejected commit did not keep the staged change staged'
fi

scenario='after-deadline-11-rejects'
install_adapter "$repo_eleven" 1793334961
git -C "$repo_eleven" reset -q
if attempt_commit "$repo_eleven" after.txt 'after deadline' "$work_root/after.log"; then
  fail 'commit after the deadline with 11 worktrees was allowed'
else
  if ! grep -q 'AppSDK commit blocked' "$work_root/after.log"; then
    fail 'post-deadline rejection is missing the blocked diagnostic'
  fi
fi

scenario='linked-worktree-11-rejects'
install_adapter "$repo_eleven" 1793334960
git -C "$repo_eleven" reset -q
linked="$repo_eleven-wt1"
if [[ ! -d "$linked" ]]; then
  fail 'linked worktree for the commit test is missing'
elif attempt_commit "$linked" linked.txt 'from linked worktree' "$work_root/linked.log"; then
  fail 'commit from a linked worktree with 11 total worktrees was allowed'
else
  if ! grep -q '11 registered worktrees' "$work_root/linked.log"; then
    fail 'linked-worktree rejection did not report the repository-wide count'
  fi
fi

# --- B. 10 registered worktrees -------------------------------------------

scenario='deadline-10-allows'
repo_ten="$work_root/ten"
init_repo "$repo_ten"
add_worktrees "$repo_ten" 9 plain
n="$(count_trees "$repo_ten")"
if [[ "$n" -ne 10 ]]; then
  fail "ten fixture registered $n worktrees, expected 10"
fi
install_adapter "$repo_ten" 1793334960
if ! attempt_commit "$repo_ten" ok.txt 'at deadline ten trees' "$work_root/ten.log"; then
  fail "commit at the deadline with 10 worktrees was rejected: $(cat "$work_root/ten.log")"
else
  if ! git -C "$repo_ten" log -1 --format=%s | grep -qx 'at deadline ten trees'; then
    fail 'ten-worktree allowed commit did not create the expected commit'
  fi
fi

# --- B2. 10 registered worktrees, one path contains a newline followed by a
#         literal "worktree " token (regression for NUL-field counting) ------

scenario='deadline-10-newline-worktree-allows'
repo_ten_nl="$work_root/ten-nl"
init_repo "$repo_ten_nl"
add_worktrees "$repo_ten_nl" 9 worktreeline
special_nl="$repo_ten_nl"$'-wt special\nworktree x'
if [[ ! -d "$special_nl" ]]; then
  fail 'worktree path with a newline and a "worktree " token was not created'
fi
n="$(count_trees "$repo_ten_nl")"
if [[ "$n" -ne 10 ]]; then
  fail "ten-newline fixture registered $n worktrees, expected 10"
fi
install_adapter "$repo_ten_nl" 1793334960
if ! attempt_commit "$repo_ten_nl" nlok.txt 'at deadline ten trees newline path' "$work_root/ten-nl.log"; then
  fail "commit at the deadline with 10 worktrees (newline+worktree path) was rejected: $(cat "$work_root/ten-nl.log")"
else
  if ! git -C "$repo_ten_nl" log -1 --format=%s | grep -qx 'at deadline ten trees newline path'; then
    fail 'ten-newline allowed commit did not create the expected commit'
  fi
fi

scenario='clock-failure-errors'
install_adapter "$repo_ten" FAIL
git -C "$repo_ten" reset -q
printf 'clock\n' > "$repo_ten/clock.txt"
git -C "$repo_ten" add clock.txt
if git -C "$repo_ten" commit -q -m 'clock failure' > "$work_root/clock.log" 2>&1; then
  fail 'commit succeeded despite a clock read failure'
else
  if ! grep -q 'failed to read the current UTC epoch' "$work_root/clock.log"; then
    fail 'clock failure did not name the failed operation'
  fi
  if grep -q 'AppSDK commit blocked' "$work_root/clock.log"; then
    fail 'clock failure was reported as a limit rejection'
  fi
fi

scenario='query-failure-errors'
install_adapter "$repo_ten" 1793334960
git -C "$repo_ten" reset -q
hooks_dir="$(resolve_hooks_dir "$repo_ten")"
cat > "$hooks_dir/pre-commit" <<QUERY_HOOK
#!/usr/bin/env bash
set -euo pipefail
source $(printf '%q' "$gate")
worktree_gate_now() { printf '%s\n' '1793334960'; }
# Fault injection at the query boundary: a Bash function overrides PATH lookup,
# because the invoking git prepends its own exec path to the hook's PATH.
git() {
  if [[ -n "\${1:-}" && "\$1" == 'worktree' && "\${2:-}" == 'list' ]]; then
    printf 'simulated worktree list failure\n' >&2
    return 128
  fi
  command git "\$@"
}
worktree_gate_main "\$@"
QUERY_HOOK
chmod 0755 "$hooks_dir/pre-commit"
printf 'query\n' > "$repo_ten/query.txt"
git -C "$repo_ten" add query.txt
if git -C "$repo_ten" commit -q -m 'query failure' > "$work_root/query.log" 2>&1; then
  fail 'commit succeeded despite a worktree list failure'
else
  if ! grep -q 'failed to list registered worktrees' "$work_root/query.log"; then
    fail 'worktree query failure did not name the failed operation'
  fi
  if ! grep -q 'simulated worktree list failure' "$work_root/query.log"; then
    fail 'worktree query failure did not retain the Git diagnostic'
  fi
  if grep -q 'AppSDK commit blocked' "$work_root/query.log"; then
    fail 'worktree query failure was reported as a limit rejection'
  fi
fi

scenario='owned-chain-allows'
chain_marker="$work_root/chain.marker"
: > "$chain_marker"
install_dispatcher "$repo_ten" 1793334960 '' "$chain_marker"
git -C "$repo_ten" reset -q
if ! attempt_commit "$repo_ten" chain.txt 'owned chain allow' "$work_root/chain.log"; then
  fail "owned hook chain rejected an allowed commit: $(cat "$work_root/chain.log")"
fi
if ! grep -qx 'before' "$chain_marker"; then
  fail 'the existing chain before-callback did not run'
fi
if ! grep -qx 'after' "$chain_marker"; then
  fail 'the existing chain after-callback did not run'
fi

scenario='owned-chain-failure-propagates'
chain_fail_marker="$work_root/chainfail.marker"
: > "$chain_fail_marker"
install_dispatcher "$repo_ten" 1793334960 \
  'printf "chain callback failed\n" >&2; exit 7' "$chain_fail_marker"
git -C "$repo_ten" reset -q
if attempt_commit "$repo_ten" chainfail.txt 'owned chain fail' "$work_root/chainfail.log"; then
  fail 'commit succeeded despite a failing existing hook callback'
else
  if ! grep -q 'chain callback failed' "$work_root/chainfail.log"; then
    fail 'the existing hook failure diagnostic was swallowed'
  fi
fi

# --- C. installer: refusals ------------------------------------------------

scenario='explicit-target-required'
if bash "$installer" > "$work_root/noarg.log" 2>&1; then
  fail 'installer accepted a call with no target argument'
fi

scenario='non-git-target-refused'
mkdir -p "$work_root/notgit"
# The fixture root is inside the AppSDK task worktree, so a bare directory would
# otherwise resolve to the enclosing repository. Stop upward discovery at the
# fixture root so this target is genuinely outside any Git repository.
if GIT_CEILING_DIRECTORIES="$work_root" bash "$installer" "$work_root/notgit" > "$work_root/notgit.log" 2>&1; then
  fail 'installer accepted a directory that is not a Git working tree'
else
  if ! grep -q 'not a Git working tree' "$work_root/notgit.log"; then
    fail 'non-Git target refusal was unclear'
  fi
fi

scenario='configured-hookspath-refused'
repo_cfg="$work_root/configured"
init_repo "$repo_cfg"
mkdir -p "$work_root/shared-hooks"
git -C "$repo_cfg" config core.hooksPath "$work_root/shared-hooks"
if bash "$installer" "$repo_cfg" > "$work_root/cfg.log" 2>&1; then
  fail 'installer installed despite a configured core.hooksPath'
else
  if ! grep -q 'core.hooksPath' "$work_root/cfg.log"; then
    fail 'configured-hooksPath refusal did not mention core.hooksPath'
  fi
fi
if [[ -e "$work_root/shared-hooks/pre-commit" || -e "$work_root/shared-hooks/appsdk-worktree-limit-gate" ]]; then
  fail 'installer wrote into the configured shared hooks path'
fi

scenario='empty-hookspath-refused'
repo_cfg_empty="$work_root/configured-empty"
init_repo "$repo_cfg_empty"
# Git treats an empty configured hooksPath as configured (it can suppress the
# repository-local hooks), so the installer must detect presence, not a
# non-empty value, and refuse without touching configuration or hook paths.
git -C "$repo_cfg_empty" config core.hooksPath ''
empty_hooks="$(resolve_hooks_dir "$repo_cfg_empty")"
if bash "$installer" "$repo_cfg_empty" > "$work_root/cfg-empty.log" 2>&1; then
  fail 'installer installed despite a configured empty core.hooksPath'
else
  if ! grep -q 'core.hooksPath' "$work_root/cfg-empty.log"; then
    fail 'empty-hooksPath refusal did not mention core.hooksPath'
  fi
  if grep -qi 'Installed the AppSDK worktree-limit gate' "$work_root/cfg-empty.log"; then
    fail 'empty-hooksPath refusal still claimed an active installation'
  fi
  if grep -qi 'already installed' "$work_root/cfg-empty.log"; then
    fail 'empty-hooksPath refusal reported an active installation'
  fi
fi
if ! git -C "$repo_cfg_empty" config --get core.hooksPath > /dev/null 2>&1; then
  fail 'empty-hooksPath fixture lost its configured core.hooksPath'
fi
if [[ -e "$empty_hooks/pre-commit" || -e "$empty_hooks/appsdk-worktree-limit-gate" ]]; then
  fail 'installer wrote hook files despite the empty core.hooksPath'
fi

scenario='partial-install-refused-preserves-files'
repo_partial="$work_root/partial"
init_repo "$repo_partial"
if ! bash "$installer" "$repo_partial" > "$work_root/partial-first.log" 2>&1; then
  fail "installer failed on the clean partial-install fixture: $(cat "$work_root/partial-first.log")"
fi
partial_hooks="$(resolve_hooks_dir "$repo_partial")"
# Remove only the sidecar to leave a managed hook with no sidecar: a partial
# install. The installer must report it and change nothing, without telling the
# user to delete the existing hook or sidecar.
rm -f "$partial_hooks/appsdk-worktree-limit-gate"
partial_hook_before="$(cat "$partial_hooks/pre-commit")"
if bash "$installer" "$repo_partial" > "$work_root/partial-second.log" 2>&1; then
  fail 'installer claimed success on a partial installation'
else
  if ! grep -q 'partially installed' "$work_root/partial-second.log"; then
    fail 'partial-install refusal did not report the partial state'
  fi
  if ! grep -qi 'coordinate with the hook owner' "$work_root/partial-second.log"; then
    fail 'partial-install refusal did not direct the user to coordinate with the hook owner'
  fi
  if grep -qi 'remove' "$work_root/partial-second.log"; then
    fail 'partial-install refusal still instructed the user to remove files'
  fi
  if grep -qi 'already installed' "$work_root/partial-second.log"; then
    fail 'partial-install was mistakenly reported as an active installation'
  fi
fi
if [[ "$(cat "$partial_hooks/pre-commit")" != "$partial_hook_before" ]]; then
  fail 'partial-install refusal modified the existing hook'
fi
if [[ -e "$partial_hooks/appsdk-worktree-limit-gate" ]]; then
  fail 'partial-install refusal recreated the sidecar'
fi

scenario='unmanaged-hook-refused'
repo_unmanaged="$work_root/unmanaged"
init_repo "$repo_unmanaged"
unmanaged_hooks="$(resolve_hooks_dir "$repo_unmanaged")"
unmanaged_marker="$work_root/unmanaged.marker"
: > "$unmanaged_marker"
cat > "$unmanaged_hooks/pre-commit" <<UNMANAGED
#!/usr/bin/env bash
printf 'original hook ran\n' >> $(printf '%q' "$unmanaged_marker")
exit 0
UNMANAGED
chmod 0755 "$unmanaged_hooks/pre-commit"
original_hook="$(cat "$unmanaged_hooks/pre-commit")"
if bash "$installer" "$repo_unmanaged" > "$work_root/unmanaged-install.log" 2>&1; then
  fail 'installer replaced an existing unmanaged pre-commit hook'
else
  if ! grep -qi 'refusing' "$work_root/unmanaged-install.log"; then
    fail 'unmanaged-hook refusal did not explain the rejection'
  fi
fi
if [[ "$(cat "$unmanaged_hooks/pre-commit")" != "$original_hook" ]]; then
  fail 'installer modified the existing unmanaged pre-commit hook'
fi
if [[ -e "$unmanaged_hooks/appsdk-worktree-limit-gate" ]]; then
  fail 'installer left a sidecar after refusing an unmanaged hook'
fi
if [[ -n "$(git -C "$repo_unmanaged" config --get core.hooksPath || true)" ]]; then
  fail 'installer set core.hooksPath after a refusal'
fi
if ! attempt_commit "$repo_unmanaged" u.txt 'unmanaged still runs' "$work_root/unmanaged.log"; then
  fail "a real commit through the original unmanaged hook failed: $(cat "$work_root/unmanaged.log")"
fi
if ! grep -q 'original hook ran' "$unmanaged_marker"; then
  fail 'the original unmanaged hook did not run on a real commit'
fi

# --- D. installer: production activation ----------------------------------

scenario='production-install-and-commit'
repo_prod="$work_root/production"
init_repo "$repo_prod"
if ! bash "$installer" "$repo_prod" > "$work_root/prod-install.log" 2>&1; then
  fail "installer failed on a clean repository: $(cat "$work_root/prod-install.log")"
fi
prod_hooks="$(resolve_hooks_dir "$repo_prod")"
if [[ ! -x "$prod_hooks/pre-commit" ]]; then
  fail 'installed pre-commit is missing or not executable'
fi
if [[ ! -x "$prod_hooks/appsdk-worktree-limit-gate" ]]; then
  fail 'installed gate sidecar is missing or not executable'
fi
if [[ -n "$(git -C "$repo_prod" config --get core.hooksPath || true)" ]]; then
  fail 'installer configured core.hooksPath'
fi
if ! bash "$installer" "$repo_prod" > "$work_root/prod-reinstall.log" 2>&1; then
  fail "idempotent reinstall failed: $(cat "$work_root/prod-reinstall.log")"
else
  if ! grep -qi 'already installed' "$work_root/prod-reinstall.log"; then
    fail 'reinstall did not report the installed state'
  fi
fi
if ! attempt_commit "$repo_prod" prod.txt 'production hook real commit' "$work_root/prod.log"; then
  fail "a real commit through the installed production hook failed: $(cat "$work_root/prod.log")"
fi
# Prove the hook is on the commit path: a missing sidecar must fail the commit.
if [[ -x "$prod_hooks/appsdk-worktree-limit-gate" ]]; then
  mv "$prod_hooks/appsdk-worktree-limit-gate" "$prod_hooks/appsdk-worktree-limit-gate.held"
  if attempt_commit "$repo_prod" prod2.txt 'should fail without sidecar' "$work_root/prod-missing.log"; then
    fail 'commit succeeded even though the installed gate sidecar was missing'
  fi
  mv "$prod_hooks/appsdk-worktree-limit-gate.held" "$prod_hooks/appsdk-worktree-limit-gate"
  if ! attempt_commit "$repo_prod" prod3.txt 'production hook after restore' "$work_root/prod-restore.log"; then
    fail "a real commit after restoring the sidecar failed: $(cat "$work_root/prod-restore.log")"
  fi
else
  fail 'installed gate sidecar was missing before the connectivity probe'
fi

# --- summary ---------------------------------------------------------------

if (( failures > 0 )); then
  printf 'worktree commit gate fixture: %d failure(s)\n' "$failures" >&2
  exit 1
fi
printf 'worktree commit gate fixture: PASS\n'
