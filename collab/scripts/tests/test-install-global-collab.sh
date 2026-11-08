#!/usr/bin/env bash
# Black-box check of the Collab global installer transaction.
#
# The test drives the real installer script against a throwaway repo layout
# with stub candidate binaries, controlled failure injection, an isolated HOME
# and state root, and lifecycle spies that fail on any call. It never touches
# the real HOME, state root, launchd, daemon or tmux, and creates no socket.
#
# COLLAB_SUT_INSTALLER selects the installer file under test. The default is the
# working tree installer; export it to a copy of the original committed
# installer to demonstrate the RED failure before the transaction existed.

set -euo pipefail

script_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd -P)"
repo_root="$(cd "$script_dir/../../.." && pwd -P)"

SUT_INSTALLER="${COLLAB_SUT_INSTALLER:-"$repo_root/collab/scripts/install-global-collab.sh"}"
SUT_HELPER="${COLLAB_SUT_HELPER:-"$repo_root/collab/scripts/collab-install-transaction.sh"}"

base_tmp="${TMPDIR:-/tmp}"
work="$(mktemp -d "$base_tmp/collab-install-test.XXXXXX")"
trap 'rm -rf "$work"' EXIT HUP INT TERM

fail() {
  printf 'FAIL: %s\n' "$*" >&2
  exit 1
}

note() {
  printf 'ok: %s\n' "$*"
}

case_count=0

# Create an isolated fixture named $1 and point the F_* globals at it.
new_fixture() {
  local name="$1"
  F="$work/$name"
  F_REPO="$F/repo"
  F_HOME="$F/home"
  F_BIN="$F_HOME/.cargo/bin"
  F_SKILLS="$F_HOME/.agents/skills/collab"
  mkdir -p "$F_REPO/scripts" "$F_REPO/target/release" "$F_REPO/tools" \
    "$F_HOME" "$F/tmp" "$F/logs"
  cp "$SUT_INSTALLER" "$F_REPO/scripts/install-global-collab.sh"
  cp "$repo_root/collab/scripts/require-system-volume.sh" "$F_REPO/scripts/"
  if [ -f "$SUT_HELPER" ]; then
    cp "$SUT_HELPER" "$F_REPO/scripts/collab-install-transaction.sh"
  fi
  printf '#!/bin/sh\nexit 0\n' >"$F_REPO/scripts/build-collab.sh"
  chmod 755 "$F_REPO/scripts/build-collab.sh"
  printf '#!/bin/sh\nexit 0\n' >"$F_REPO/scripts/collab-desktop-mcp"
  printf '// stub desktop js\n' >"$F_REPO/scripts/collab-desktop-mcp.mjs"
  chmod 755 "$F_REPO/scripts/collab-desktop-mcp"
  chmod 644 "$F_REPO/scripts/collab-desktop-mcp.mjs"
  local spy
  for spy in launchctl tmux kickstart; do
    cat >"$F_REPO/tools/$spy" <<SPY
#!/bin/sh
printf '%s\n' "$spy \$*" >>"$F/logs/lifecycle.log"
exit 1
SPY
    chmod 755 "$F_REPO/tools/$spy"
  done
  : >"$F/logs/lifecycle.log"
}

# Write a collab/collab-mcp pair with a literal release version and marker.
write_pair() {
  local dir="$1" version="$2" marker="$3"
  mkdir -p "$dir"
  cat >"$dir/collab" <<PAIR
#!/bin/sh
case "\$1" in
  --version) printf 'collab $version\n' ;;
  install-skills)
    if [ "\${COLLAB_STUB_SKILLS_EXIT:-0}" -ne 0 ]; then
      printf '%s\n' "\${COLLAB_STUB_SKILLS_MSG:-skill export failed}" >&2
      exit "\${COLLAB_STUB_SKILLS_EXIT}"
    fi
    target=""; prev=""
    for a in "\$@"; do [ "\$prev" = "--target" ] && target="\$a"; prev="\$a"; done
    [ -n "\$target" ] || exit 2
    mkdir -p "\$target"
    printf '%s\n' "\${COLLAB_STUB_SKILL_MARKER:-$marker}" >"\$target/SKILL.md"
    ;;
  status)
    local_check=0
    for a in "\$@"; do [ "\$a" = "--local" ] && local_check=1; done
    if [ "\$local_check" -eq 1 ]; then
      [ "\${COLLAB_STUB_LOCAL_EXIT:-0}" -eq 0 ] ||
        { printf '%s\n' "\${COLLAB_STUB_LOCAL_MSG:-local check failed}" >&2; exit "\${COLLAB_STUB_LOCAL_EXIT}"; }
      exit 0
    fi
    if [ "\${COLLAB_STUB_DESTROY_CAPTURE:-0}" -eq 1 ]; then
      rm -rf "\$CARGO_HOME"/bin/.collab-install.*/capture/collab
    fi
    if [ "\${COLLAB_STUB_SLOW_FULL:-0}" -eq 1 ]; then
      : >"\${COLLAB_STUB_SLOW_MARKER:-/dev/null}"
      sleep 3
    fi
    if [ "\${COLLAB_STUB_FULL_EXIT:-0}" -eq 0 ]; then
      exit 0
    fi
    [ -z "\${COLLAB_STUB_FULL_JSON:-}" ] || printf '%s\n' "\$COLLAB_STUB_FULL_JSON"
    if [ -n "\${COLLAB_STUB_FULL_STDERR_FILE:-}" ] && [ -f "\$COLLAB_STUB_FULL_STDERR_FILE" ]; then
      cat "\$COLLAB_STUB_FULL_STDERR_FILE" >&2
    fi
    [ -z "\${COLLAB_STUB_FULL_MSG:-}" ] || printf '%s\n' "\$COLLAB_STUB_FULL_MSG" >&2
    exit "\${COLLAB_STUB_FULL_EXIT}"
    ;;
esac
exit 0
PAIR
  chmod 755 "$dir/collab"
  cat >"$dir/collab-mcp" <<PAIRMCP
#!/bin/sh
read -r _line
printf '{"jsonrpc":"2.0","id":1,"result":{"serverInfo":{"name":"collab-mcp","version":"%s"}}}\n' "$version"
PAIRMCP
  chmod 755 "$dir/collab-mcp"
}

write_candidate() {
  write_pair "$F_REPO/target/release" "$1" "${2:-candidate skill}"
}

# Install a mixed baseline whose canonical collab is a symlink to an existing
# directory. The referent directory must stay intact through rescue/rollback.
write_directory_symlink_baseline() {
  mkdir -p "$F/refdir" "$F_BIN" "$F_SKILLS"
  printf 'referent keep\n' >"$F/refdir/keep.txt"
  chmod 750 "$F/refdir"
  ln -s "$F/refdir" "$F_BIN/collab"
  write_pair "$F/swap" 0.2.8000 "mixed skill"
  cp "$F/swap/collab-mcp" "$F_BIN/collab-mcp"
  printf '# mixed skill\n' >"$F_SKILLS/SKILL.md"
}

assert_referent_intact() {
  [ -d "$F/refdir" ] || fail "referent directory missing"
  [ "$(cat "$F/refdir/keep.txt")" = "referent keep" ] || fail "referent contents changed"
  assert_mode "$F/refdir" 750
  [ "$(ls -1 "$F/refdir" | wc -l | tr -d ' ')" -eq 1 ] ||
    fail "referent directory gained entries: $(ls -1 "$F/refdir")"
}

# Install a baseline directly into the canonical bin/skills paths.
write_baseline() {
  local version="$1" launcher_mode="${2:-755}" js_mode="${3:-644}"
  mkdir -p "$F_BIN" "$F_SKILLS"
  write_pair "$F_BIN" "$version" "baseline skill"
  printf '#!/bin/sh\nexit 0\n' >"$F_BIN/collab-desktop-mcp"
  printf '// baseline desktop js\n' >"$F_BIN/collab-desktop-mcp.mjs"
  printf '%s\n' "baseline skill $version" >"$F_SKILLS/SKILL.md"
  chmod "$launcher_mode" "$F_BIN/collab-desktop-mcp"
  chmod "$js_mode" "$F_BIN/collab-desktop-mcp.mjs"
}

# Install a baseline whose canonical CLI never completes --version. The
# classifier must bound that probe and treat the baseline as unverified; the
# final version projection must not run it without the same bound.
write_hanging_version_baseline() {
  local version="$1"
  write_baseline "$version"
  cat >"$F_BIN/collab" <<'HANG'
#!/bin/sh
case "$1" in
  --version)
    printf '%s\n' "$$" >"${COLLAB_VERSION_HANG_PIDFILE:?}"
    exec sleep 30
    ;;
esac
exit 0
HANG
  chmod 755 "$F_BIN/collab"
}

# Install a PATH 'mv' shim that fails replacement of the canonical retained
# predecessor address. mode=once fails only the first such move, mode=always
# fails every such move; all other moves pass through to the real mv.
install_predecessor_mv_shim() {
  local d="$1" mode="$2"
  local real_mv state
  real_mv="$(command -v mv)"
  state="$d/logs/mv-rotation.state"
  rm -f "$state"
  cat >"$d/repo/tools/mv" <<SHIM
#!/bin/sh
last=
for arg in "\$@"; do last="\$arg"; done
case \$last in
  */.collab-install-previous)
    if [ "$mode" = always ]; then
      exit 1
    fi
    if [ -e "$state" ]; then
      exec "$real_mv" "\$@"
    fi
    : >"$state"
    exit 1
    ;;
esac
exec "$real_mv" "\$@"
SHIM
  chmod 755 "$d/repo/tools/mv"
}

# Install a GNU-shaped stat in the fixture PATH. GNU stat reports on the file
# operand for -f %Lp, then exits nonzero because %Lp is treated as another
# missing file. The -c %a path remains exact. System-volume device probes stay
# on the native stat so the fixture still exercises the macOS volume guard.
install_gnu_stat_shim() {
  local d="$1" gnu_stat
  gnu_stat="$(command -v gstat 2>/dev/null || true)"
  cat >"$d/repo/tools/stat" <<SHIM
#!/bin/sh
if [ "\$1" = "-f" ] && [ "\$2" = "%d" ]; then
  shift 2
  command -p stat -f '%d' "\$@"
  exit \$?
fi
if [ "\$1" = "-f" ] && [ "\$2" = "%Lp" ]; then
  shift 2
  if [ -n "$gnu_stat" ]; then
    "$gnu_stat" -f '%Lp' "\$@"
    exit \$?
  fi
  printf '  File: "%s"\n' "\$1"
  printf '%s\n' '    ID: shim Namelen: 255 Type: shim'
  printf '%s\n' 'Block size: 4096       Fundamental block size: 4096'
  printf '%s\n' 'Blocks: Total: 1 Free: 0 Available: 0'
  printf '%s\n' 'Inodes: Total: 1 Free: 0'
  exit 1
fi
if [ "\$1" = "-c" ] && [ "\$2" = "%a" ]; then
  shift 2
  if [ -n "$gnu_stat" ]; then
    "$gnu_stat" -c '%a' "\$@"
    exit \$?
  fi
  if command -p stat -f '%Lp' "\$@" >/dev/null 2>&1; then
    command -p stat -f '%Lp' "\$@"
    exit \$?
  fi
  command -p stat -c '%a' "\$@"
  exit \$?
fi
command -p stat "\$@"
SHIM
  chmod 755 "$d/repo/tools/stat"
}

run_installer() {
  local d="$1"; shift
  (
    set +e
    env -i \
      PATH="$d/repo/tools:/usr/bin:/bin:/usr/sbin:/sbin" \
      HOME="$d/home" \
      CARGO_HOME="$d/home/.cargo" \
      COLLAB_SKILLS_TARGET="$d/home/.agents/skills/collab" \
      COLLAB_STATE_DIR="$d/state" \
      COLLAB_BUILD_STATE_DIR="$d/build-state" \
      TMPDIR="$d/tmp" \
      COLLAB_STUB_LOG="$d/logs/stub.log" \
      "$@" \
      sh "$d/repo/scripts/install-global-collab.sh" >"$d/out" 2>"$d/err"
    printf '%s\n' "$?" >"$d/exit"
  )
}

run_installer_ok() {
  local d="$1"; shift
  run_installer "$d" "$@"
  local status
  status="$(cat "$d/exit")"
  [ "$status" -eq 0 ] || fail "$d: expected exit 0, got $status: $(cat "$d/err")"
}

# Run the public installer under a fixture-owned wall-clock bound. On timeout,
# stop only the installer and the hanging version fixture recorded by the case,
# and record the outer timeout instead of the installer result.
run_installer_bounded() {
  local d="$1" limit="$2"; shift 2
  rm -f "$d/exit" "$d/timeout" "$d/installer.pid" "$d/version-hang.pid"
  (
    set +e
    env -i \
      PATH="$d/repo/tools:/usr/bin:/bin:/usr/sbin:/sbin" \
      HOME="$d/home" \
      CARGO_HOME="$d/home/.cargo" \
      COLLAB_SKILLS_TARGET="$d/home/.agents/skills/collab" \
      COLLAB_STATE_DIR="$d/state" \
      COLLAB_BUILD_STATE_DIR="$d/build-state" \
      TMPDIR="$d/tmp" \
      COLLAB_STUB_LOG="$d/logs/stub.log" \
      "$@" \
      sh "$d/repo/scripts/install-global-collab.sh" >"$d/out" 2>"$d/err" &
    installer_pid=$!
    printf '%s\n' "$installer_pid" >"$d/installer.pid"
    wait "$installer_pid"
    printf '%s\n' "$?" >"$d/exit"
  ) &
  local runner_pid=$!
  local ticks=$((limit * 10))
  local tick=0
  while [ "$tick" -lt "$ticks" ]; do
    if [ -e "$d/exit" ]; then
      break
    fi
    sleep 0.1
    tick=$((tick + 1))
  done
  if [ -e "$d/exit" ]; then
    wait "$runner_pid" 2>/dev/null || true
    return 0
  fi
  local installer_pid hang_pid
  installer_pid="$(cat "$d/installer.pid" 2>/dev/null || true)"
  hang_pid="$(cat "$d/version-hang.pid" 2>/dev/null || true)"
  [ -z "$installer_pid" ] || kill -TERM "$installer_pid" 2>/dev/null || true
  [ -z "$hang_pid" ] || kill -TERM "$hang_pid" 2>/dev/null || true
  sleep 1
  installer_pid="$(cat "$d/installer.pid" 2>/dev/null || true)"
  hang_pid="$(cat "$d/version-hang.pid" 2>/dev/null || true)"
  [ -z "$installer_pid" ] || kill -KILL "$installer_pid" 2>/dev/null || true
  [ -z "$hang_pid" ] || kill -KILL "$hang_pid" 2>/dev/null || true
  kill -KILL "$runner_pid" 2>/dev/null || true
  wait "$runner_pid" 2>/dev/null || true
  printf '124\n' >"$d/exit"
  : >"$d/timeout"
  return 0
}

field() {
  local file="$1" key="$2"
  awk -F= -v k="$key" '$1 == k { sub(/^[^=]*=/, ""); print; exit }' "$file"
}

assert_field() {
  local file="$1" key="$2" want="$3" got
  got="$(field "$file" "$key")"
  [ "$got" = "$want" ] || fail "$key: expected [$want] got [$got]"
}

assert_contains() {
  local text="$1" needle="$2" label="$3"
  case $text in
    *"$needle"*) ;;
    *) fail "$label: [$needle] not found in [$text]" ;;
  esac
}

assert_present() {
  [ -e "$1" ] || fail "expected present: $1"
}

assert_absent() {
  [ ! -e "$1" ] && [ ! -L "$1" ] || fail "expected absent: $1"
}

assert_symlink_to() {
  [ -L "$1" ] || fail "expected symlink: $1"
  [ "$(readlink "$1")" = "$2" ] ||
    fail "symlink $1: expected -> $2 got -> $(readlink "$1")"
}

assert_mode() {
  local got
  got="$(stat -f '%Lp' "$1" 2>/dev/null || stat -c '%a' "$1")"
  [ "$got" = "$2" ] || fail "mode $1: expected $2 got $got"
}

assert_same_file() {
  cmp -s "$1" "$2" || fail "content differs: $1 vs $2"
}

assert_no_lifecycle() {
  [ ! -s "$1/logs/lifecycle.log" ] ||
    fail "$1: lifecycle spy was invoked: $(cat "$1/logs/lifecycle.log")"
}

assert_no_leftovers() {
  local leftover
  for leftover in "$F_BIN"/.collab-install.* ; do
    [ -e "$leftover" ] || continue
    fail "$F: unexpected leftover $leftover"
  done
}

# --- cases ----------------------------------------------------------------

case_success_first_install() {
  new_fixture success_first
  write_candidate 0.2.9001
  run_installer_ok "$F"
  assert_field "$F/out" outcome success
  assert_field "$F/out" baseline absent
  assert_field "$F/out" rollback not_needed
  assert_field "$F/out" previous_release none
  assert_field "$F/out" previous_release_quality none
  assert_field "$F/out" daemon_changed false
  assert_field "$F/out" required_action none
  assert_present "$F_BIN/collab"
  assert_present "$F_BIN/collab-mcp"
  assert_present "$F_BIN/collab-desktop-mcp"
  assert_present "$F_BIN/collab-desktop-mcp.mjs"
  assert_present "$F_SKILLS/SKILL.md"
  assert_mode "$F_BIN/collab" 755
  assert_mode "$F_BIN/collab-mcp" 755
  assert_mode "$F_BIN/collab-desktop-mcp" 755
  assert_mode "$F_BIN/collab-desktop-mcp.mjs" 644
  grep -q 'candidate skill' "$F_SKILLS/SKILL.md" || fail "candidate skill not installed"
  assert_absent "$F_BIN/.collab-install-previous"
  assert_no_leftovers
  assert_no_lifecycle "$F"
}

case_pending_activation() {
  new_fixture pending
  write_candidate 0.2.9001
  run_installer "$F" \
    COLLAB_STUB_FULL_EXIT=3 \
    COLLAB_STUB_FULL_JSON='{"outcome":"pending_user_activation","required_action":"caller_context_required"}'
  [ "$(cat "$F/exit")" -eq 3 ] || fail "pending: expected exit 3 got $(cat "$F/exit")"
  assert_field "$F/out" outcome pending_user_activation
  assert_field "$F/out" required_action caller_context_required
  assert_field "$F/out" rollback not_needed
  assert_present "$F_BIN/collab"
  assert_present "$F_SKILLS/SKILL.md"
  assert_field "$F/out" previous_release none
  assert_no_leftovers
  assert_no_lifecycle "$F"
}

case_pending_without_typed_reason_fails() {
  new_fixture pending_without_reason
  write_candidate 0.2.9001
  run_installer "$F" COLLAB_STUB_FULL_EXIT=3 COLLAB_STUB_FULL_MSG="pending reason missing"
  [ "$(cat "$F/exit")" -eq 3 ] || fail "pending-without-reason: expected exit 3 got $(cat "$F/exit")"
  assert_field "$F/out" outcome failure_with_rollback
  assert_field "$F/out" failed_phase check
  assert_field "$F/out" required_action none
  assert_contains "$(field "$F/out" original_error)" "pending reason missing" "pending error"
  assert_field "$F/out" rollback restored
  assert_absent "$F_BIN/collab"
  assert_absent "$F_SKILLS"
  assert_no_leftovers
  assert_no_lifecycle "$F"
}

case_build_failure_preserves_baseline() {
  new_fixture build_fail
  write_baseline 0.2.8000
  write_candidate 0.2.9001
  cat >"$F_REPO/scripts/build-collab.sh" <<'BUILD'
#!/bin/sh
printf 'compiler exploded\n' >&2
exit 23
BUILD
  chmod 755 "$F_REPO/scripts/build-collab.sh"
  run_installer "$F"
  [ "$(cat "$F/exit")" -eq 23 ] || fail "build: expected exit 23 got $(cat "$F/exit")"
  assert_field "$F/out" failed_phase build
  assert_field "$F/out" original_exit 23
  assert_contains "$(field "$F/out" original_error)" "compiler exploded" "build error"
  grep -q '0.2.8000' "$F_BIN/collab" || fail "baseline collab changed on build failure"
  assert_absent "$F_BIN/.collab-install-previous"
  assert_no_lifecycle "$F"
}

case_stage_skill_export_failure_no_mutation() {
  new_fixture stage_skill_fail
  write_candidate 0.2.9001
  run_installer "$F" COLLAB_STUB_SKILLS_EXIT=7 COLLAB_STUB_SKILLS_MSG="skill export denied"
  [ "$(cat "$F/exit")" -eq 7 ] || fail "stage-skill: expected exit 7 got $(cat "$F/exit")"
  assert_field "$F/out" failed_phase stage
  assert_field "$F/out" original_exit 7
  assert_contains "$(field "$F/out" original_error)" "skill export denied" "stage error"
  assert_absent "$F_BIN/collab"
  assert_absent "$F_SKILLS/SKILL.md"
  assert_field "$F/out" rollback not_needed
  assert_no_lifecycle "$F"
}

case_local_check_failure_no_mutation() {
  new_fixture local_check_fail
  write_candidate 0.2.9001
  run_installer "$F" COLLAB_STUB_LOCAL_EXIT=5 COLLAB_STUB_LOCAL_MSG="local pair denied"
  [ "$(cat "$F/exit")" -eq 5 ] || fail "local-check: expected exit 5 got $(cat "$F/exit")"
  assert_field "$F/out" failed_phase stage
  assert_contains "$(field "$F/out" original_error)" "local pair denied" "local error"
  assert_absent "$F_BIN/collab"
  assert_field "$F/out" rollback not_needed
  assert_no_lifecycle "$F"
}

case_missing_artifact_no_mutation() {
  new_fixture missing_artifact
  write_candidate 0.2.9001
  rm -f "$F_REPO/target/release/collab-mcp"
  run_installer "$F"
  [ "$(cat "$F/exit")" -ne 0 ] || fail "missing artifact: expected failure"
  assert_contains "$(field "$F/out" original_error)" "candidate collab-mcp" "missing artifact error"
  assert_absent "$F_BIN/collab"
  assert_no_lifecycle "$F"
}

case_postcheck_failure_restores() {
  new_fixture postcheck_fail
  write_baseline 0.2.8000
  write_candidate 0.2.9001
  run_installer "$F" COLLAB_STUB_FULL_EXIT=9 COLLAB_STUB_FULL_MSG="tcc denied"
  [ "$(cat "$F/exit")" -eq 9 ] || fail "post-check: expected exit 9 got $(cat "$F/exit")"
  assert_field "$F/out" outcome failure_with_rollback
  assert_field "$F/out" failed_phase check
  assert_field "$F/out" original_exit 9
  assert_contains "$(field "$F/out" original_error)" "tcc denied" "check error"
  assert_field "$F/out" rollback restored
  assert_field "$F/out" restored_baseline_quality recoverable_release
  assert_field "$F/out" version 0.2.8000
  grep -q '0.2.8000' "$F_BIN/collab" || fail "baseline collab was not restored"
  grep -q 'baseline skill' "$F_SKILLS/SKILL.md" || fail "baseline skill was not restored"
  assert_no_leftovers
  assert_no_lifecycle "$F"
}

case_postcheck_multiline_error_preserves_chain() {
  new_fixture postcheck_multiline
  write_baseline 0.2.8000
  write_candidate 0.2.9001
  printf 'health check failed\ncause: local MCP frame invalid\n' >"$F/logs/post-check.stderr"
  run_installer "$F" \
    COLLAB_STUB_FULL_EXIT=9 \
    COLLAB_STUB_FULL_STDERR_FILE="$F/logs/post-check.stderr"
  [ "$(cat "$F/exit")" -eq 9 ] || fail "postcheck-multiline: expected exit 9 got $(cat "$F/exit")"
  assert_field "$F/out" failed_phase check
  assert_field "$F/out" original_exit 9
  assert_contains "$(field "$F/out" original_error)" "health check failed" "first line"
  assert_contains "$(cat "$F/err")" "cause: local MCP frame invalid" "full stderr chain"
  assert_field "$F/out" rollback restored
  grep -q '0.2.8000' "$F_BIN/collab" || fail "postcheck-multiline: baseline collab not restored"
  assert_no_lifecycle "$F"
}

# The optional final version projection must not bypass the installer's
# bounded command helper after compensation. A hanging restored CLI would
# otherwise block the original failure result and leave handled state behind.
case_final_version_probe_is_bounded_after_restore() {
  new_fixture final_version_bound
  write_hanging_version_baseline 0.2.8000
  mkdir -p "$F_BIN/.collab-install-previous/bin/skills"
  printf 'retained previous\n' >"$F_BIN/.collab-install-previous/bin/collab"
  cp -p "$F_BIN/collab" "$F/expected-collab"
  write_candidate 0.2.9001
  run_installer_bounded "$F" 15 \
    COLLAB_VERSION_HANG_PIDFILE="$F/version-hang.pid" \
    COLLAB_STUB_FULL_EXIT=9 \
    COLLAB_STUB_FULL_MSG="forced post-check failure"
  if [ -f "$F/version-hang.pid" ]; then
    local hang_pid
    hang_pid="$(cat "$F/version-hang.pid")"
    if kill -0 "$hang_pid" 2>/dev/null; then
      fail "final-version: owned hanging fixture $hang_pid is still running"
    fi
  fi
  if [ -e "$F/timeout" ]; then
    fail "final-version: outer fixture timeout fired before the original result (exit $(cat "$F/exit"), out [$(cat "$F/out")])"
  fi
  [ "$(cat "$F/exit")" -eq 9 ] || fail "final-version: expected exit 9 got $(cat "$F/exit")"
  assert_field "$F/out" outcome failure_with_rollback
  assert_field "$F/out" failed_phase check
  assert_field "$F/out" original_exit 9
  assert_contains "$(field "$F/out" original_error)" "forced post-check failure" "original health error"
  assert_field "$F/out" rollback restored
  assert_field "$F/out" restored_baseline_quality unverified
  assert_field "$F/out" version unknown
  assert_same_file "$F/expected-collab" "$F_BIN/collab"
  assert_mode "$F_BIN/collab" 755
  grep -q 'retained previous' "$F_BIN/.collab-install-previous/bin/collab" ||
    fail "final-version: pre-existing previous release was not preserved"
  assert_absent "$F_BIN/.collab-install.lock"
  assert_absent "$F_BIN/.collab-install-recovery"
  local leftover
  for leftover in "$F_BIN"/.collab-install.* "$F_BIN"/.collab-final-version.*; do
    [ -e "$leftover" ] || continue
    [ "$leftover" = "$F_BIN/.collab-install-previous" ] && continue
    fail "final-version: unexpected handled leftover $leftover"
  done
  for leftover in "$F/tmp"/.collab-bounded.*; do
    [ -e "$leftover" ] || continue
    fail "final-version: bounded timeout marker left behind: $leftover"
  done
  assert_no_lifecycle "$F"
}

case_first_install_failure_restores_absence() {
  new_fixture first_fail
  write_candidate 0.2.9001
  run_installer "$F" COLLAB_STUB_FULL_EXIT=9 COLLAB_STUB_FULL_MSG="boom"
  [ "$(cat "$F/exit")" -eq 9 ] || fail "first-fail: expected exit 9 got $(cat "$F/exit")"
  assert_field "$F/out" baseline absent
  assert_field "$F/out" rollback restored
  assert_field "$F/out" restored_baseline_quality absent
  assert_absent "$F_BIN/collab"
  assert_absent "$F_BIN/collab-mcp"
  assert_absent "$F_BIN/collab-desktop-mcp"
  assert_absent "$F_BIN/collab-desktop-mcp.mjs"
  assert_absent "$F_SKILLS"
  assert_field "$F/out" previous_release none
  assert_no_leftovers
  assert_no_lifecycle "$F"
}

case_exact_restore_modes_and_absence() {
  new_fixture exact_restore
  mkdir -p "$F_BIN" "$F_SKILLS" "$F/expected"
  write_pair "$F_BIN" 0.2.8000 "baseline skill"
  chmod 700 "$F_BIN/collab"
  chmod 750 "$F_BIN/collab-mcp"
  printf '#!/bin/sh\nexit 0\n' >"$F_BIN/collab-desktop-mcp"
  chmod 700 "$F_BIN/collab-desktop-mcp"
  # collab-desktop-mcp.mjs is intentionally absent.
  mkdir -p "$F_SKILLS"
  printf 'baseline skill body\n' >"$F_SKILLS/SKILL.md"
  chmod 750 "$F_SKILLS"
  cp -p "$F_BIN/collab" "$F/expected/collab"
  cp -p "$F_BIN/collab-mcp" "$F/expected/collab-mcp"
  cp -p "$F_BIN/collab-desktop-mcp" "$F/expected/collab-desktop-mcp"
  write_candidate 0.2.9001
  run_installer "$F" COLLAB_STUB_FULL_EXIT=9 COLLAB_STUB_FULL_MSG="post fail"
  [ "$(cat "$F/exit")" -eq 9 ] || fail "exact: expected exit 9 got $(cat "$F/exit")"
  assert_field "$F/out" rollback restored
  assert_field "$F/out" baseline unverified
  assert_same_file "$F/expected/collab" "$F_BIN/collab"
  assert_same_file "$F/expected/collab-mcp" "$F_BIN/collab-mcp"
  assert_same_file "$F/expected/collab-desktop-mcp" "$F_BIN/collab-desktop-mcp"
  assert_mode "$F_BIN/collab" 700
  assert_mode "$F_BIN/collab-mcp" 750
  assert_mode "$F_BIN/collab-desktop-mcp" 700
  assert_mode "$F_SKILLS" 750
  assert_absent "$F_BIN/collab-desktop-mcp.mjs"
  grep -q 'baseline skill body' "$F_SKILLS/SKILL.md" || fail "skills body not restored"
  assert_no_leftovers
  assert_no_lifecycle "$F"
}

case_rescue_symlink_and_restore_link() {
  new_fixture rescue_symlink
  mkdir -p "$F_BIN" "$F_SKILLS"
  ln -s "$F/nowhere/debug/collab" "$F_BIN/collab"
  write_pair "$F/swap" 0.2.8000 "mixed skill"
  cp "$F/swap/collab-mcp" "$F_BIN/collab-mcp"
  printf '# mixed skill\n' >"$F_SKILLS/SKILL.md"
  write_candidate 0.2.9001
  run_installer "$F" COLLAB_STUB_FULL_EXIT=9 COLLAB_STUB_FULL_MSG="post fail"
  [ "$(cat "$F/exit")" -eq 9 ] || fail "rescue: expected exit 9 got $(cat "$F/exit")"
  assert_field "$F/out" baseline unverified
  assert_field "$F/out" rollback restored
  assert_symlink_to "$F_BIN/collab" "$F/nowhere/debug/collab"
  assert_no_lifecycle "$F"
}

case_rescue_unverified_success() {
  new_fixture rescue_success
  mkdir -p "$F_BIN" "$F_SKILLS"
  ln -s "$F/nowhere/debug/collab" "$F_BIN/collab"
  write_pair "$F/swap" 0.2.8000 "mixed skill"
  cp "$F/swap/collab-mcp" "$F_BIN/collab-mcp"
  printf '# mixed skill\n' >"$F_SKILLS/SKILL.md"
  { printf 'r1\n'; } >"$F/logs/x"
  write_candidate 0.2.9001
  run_installer_ok "$F"
  assert_field "$F/out" baseline unverified
  assert_field "$F/out" outcome success
  [ ! -L "$F_BIN/collab" ] || fail "symlink baseline was not replaced by the release candidate"
  grep -q 'candidate skill' "$F_SKILLS/SKILL.md" || fail "candidate skill not installed"
  assert_field "$F/out" previous_release none
  assert_absent "$F_BIN/.collab-install-previous"
  assert_no_lifecycle "$F"
}

# A canonical entry that is a symlink to an existing directory must be replaced
# as a path object. The staged file must become the canonical binary instead of
# being moved into the symlink target, and the referent must stay intact.
case_rescue_symlink_to_directory_replaces_link() {
  new_fixture rescue_dirlink
  write_directory_symlink_baseline
  write_candidate 0.2.9001
  run_installer_ok "$F"
  assert_field "$F/out" outcome success
  assert_field "$F/out" baseline unverified
  assert_field "$F/out" previous_release none
  [ -f "$F_BIN/collab" ] || fail "canonical collab is not a regular file"
  [ ! -L "$F_BIN/collab" ] || fail "canonical collab is still a symlink"
  [ -f "$F_BIN/collab-mcp" ] && [ ! -L "$F_BIN/collab-mcp" ] ||
    fail "canonical collab-mcp is not a regular file"
  grep -q '0.2.9001' "$F_BIN/collab" || fail "candidate binary was not published"
  grep -q 'candidate skill' "$F_SKILLS/SKILL.md" || fail "candidate skill not installed"
  assert_referent_intact
  assert_no_leftovers
  assert_no_lifecycle "$F"
}

# If a later failure triggers compensation, the exact directory link must come
# back while its referent keeps contents and modes.
case_rescue_symlink_to_directory_restores_link() {
  new_fixture rescue_dirlink_fail
  write_directory_symlink_baseline
  write_candidate 0.2.9001
  run_installer "$F" COLLAB_STUB_FULL_EXIT=9 COLLAB_STUB_FULL_MSG="post fail"
  [ "$(cat "$F/exit")" -eq 9 ] || fail "dirlink-fail: expected exit 9 got $(cat "$F/exit")"
  assert_field "$F/out" baseline unverified
  assert_field "$F/out" rollback restored
  assert_field "$F/out" restored_baseline_quality unverified
  assert_symlink_to "$F_BIN/collab" "$F/refdir"
  assert_referent_intact
  grep -q '0.2.8000' "$F_BIN/collab-mcp" || fail "baseline collab-mcp was not restored"
  assert_no_leftovers
  assert_no_lifecycle "$F"
}

case_predecessor_rotation() {
  new_fixture rotation
  write_baseline 0.2.8000
  write_candidate 0.2.9001
  run_installer_ok "$F"
  assert_field "$F/out" previous_release "$F_BIN/.collab-install-previous"
  assert_field "$F/out" previous_release_quality locally_checked
  grep -q '0.2.8000' "$F_BIN/.collab-install-previous/bin/collab" ||
    fail "previous release is not the immediate predecessor"
  write_candidate 0.2.9002
  run_installer_ok "$F"
  grep -q '0.2.9001' "$F_BIN/.collab-install-previous/bin/collab" ||
    fail "second upgrade did not keep the immediate predecessor"
  if grep -q '0.2.8000' "$F_BIN/.collab-install-previous/bin/collab"; then
    fail "second upgrade kept more than one predecessor"
  fi
  ls -d "$F_BIN"/.collab-install-stage.* >/dev/null 2>&1 &&
    fail "rotation left a staging directory" || true
  assert_no_lifecycle "$F"
}

# A retained predecessor must return to the canonical address when only the
# second rotation move fails. The transaction still reports the rotation error
# and compensates the five active entries.
case_rotation_second_move_failure_restores_predecessor() {
  new_fixture rotation_second_move_fail
  write_baseline 0.2.8000
  mkdir -p "$F_BIN/.collab-install-previous/bin/skills"
  printf 'retained predecessor\n' >"$F_BIN/.collab-install-previous/bin/collab"
  write_candidate 0.2.9001
  install_predecessor_mv_shim "$F" once
  run_installer "$F"
  [ "$(cat "$F/exit")" -eq 1 ] || fail "rotation-second-move: expected exit 1 got $(cat "$F/exit")"
  assert_field "$F/out" outcome failure_with_rollback
  assert_field "$F/out" failed_phase previous_rotation
  assert_field "$F/out" original_exit 1
  assert_contains "$(field "$F/out" original_error)" "previous release rotation failed" "rotation error"
  assert_field "$F/out" rollback restored
  assert_field "$F/out" restored_baseline_quality recoverable_release
  grep -q 'retained predecessor' "$F_BIN/.collab-install-previous/bin/collab" ||
    fail "displaced predecessor did not return to the canonical retained path"
  grep -q '0.2.8000' "$F_BIN/collab" || fail "baseline collab was not compensated"
  ls -d "$F_BIN"/.collab-install-stage.* >/dev/null 2>&1 &&
    fail "recoverable rotation failure left a staging directory" || true
  assert_no_lifecycle "$F"
}

# When the retained predecessor cannot be put back, the transaction must keep
# both predecessors and the recovery marker, report an explicit location and
# return rollback=incomplete instead of deleting the material.
case_rotation_predecessor_restore_failure_reports_recovery() {
  new_fixture rotation_restore_fail
  write_baseline 0.2.8000
  mkdir -p "$F_BIN/.collab-install-previous/bin/skills"
  printf 'retained predecessor\n' >"$F_BIN/.collab-install-previous/bin/collab"
  write_candidate 0.2.9001
  install_predecessor_mv_shim "$F" always
  run_installer "$F"
  [ "$(cat "$F/exit")" -ne 0 ] || fail "rotation-restore-fail: expected failure"
  assert_field "$F/out" outcome failure_with_rollback
  assert_field "$F/out" failed_phase previous_rotation
  assert_field "$F/out" original_exit 1
  assert_contains "$(field "$F/out" original_error)" "previous release rotation failed" "rotation error"
  assert_contains "$(cat "$F/err")" "cannot restore the retained previous release" \
    "distinct predecessor rollback error"
  assert_field "$F/out" rollback incomplete
  assert_contains "$(field "$F/out" rollback_errors)" "restore:previous" "predecessor rollback error"
  # Five active entries are still compensated even though the predecessor
  # recovery is incomplete.
  grep -q '0.2.8000' "$F_BIN/collab" || fail "active collab was not compensated"
  grep -q '0.2.8000' "$F_BIN/collab-mcp" || fail "active collab-mcp was not compensated"
  assert_present "$F_BIN/collab-desktop-mcp"
  assert_present "$F_BIN/collab-desktop-mcp.mjs"
  grep -q 'baseline skill' "$F_SKILLS/SKILL.md" || fail "active Skills were not compensated"
  assert_absent "$F_BIN/.collab-install-previous"
  assert_present "$F_BIN/.collab-install-recovery"
  assert_present "$F_BIN/.collab-install.lock"
  # Old and new predecessor material both remain for recovery.
  local preserved preserved_new
  preserved="$(ls -d "$F_BIN"/.collab-install-stage.old.*/bin/collab 2>/dev/null | head -1)"
  [ -n "$preserved" ] || fail "displaced predecessor material was not preserved"
  grep -q 'retained predecessor' "$preserved" || fail "preserved predecessor content is wrong"
  preserved_new="$(ls -d "$F_BIN"/.collab-install-stage.*/bin/collab 2>/dev/null | grep -v '\.old\.' | head -1)"
  [ -n "$preserved_new" ] || fail "new predecessor material was not preserved"
  grep -q '0.2.8000' "$preserved_new" || fail "new predecessor material content is wrong"
  assert_contains "$(cat "$F/err")" ".collab-install-stage.old" "explicit recovery location"
  local old_dir
  old_dir="$(dirname "$(dirname "$preserved")")"
  assert_field "$F/out" previous_release "$old_dir"
  assert_field "$F/out" previous_release_quality unverified
  assert_no_lifecycle "$F"
}

case_unverified_preserves_previous() {
  new_fixture unverified_keep
  write_baseline 0.2.8000
  write_candidate 0.2.9001
  run_installer_ok "$F"
  # Replace the canonical CLI with a debug-version file: unverified baseline.
  write_pair "$F_BIN" 0.2.0000 "debug"
  write_candidate 0.2.9002
  run_installer_ok "$F"
  assert_field "$F/out" baseline unverified
  grep -q '0.2.8000' "$F_BIN/.collab-install-previous/bin/collab" ||
    fail "unverified baseline displaced the retained previous release"
  assert_field "$F/out" previous_release_quality unverified
  assert_no_lifecycle "$F"
}

case_baseline_mcp_silent_failure_reports_exit() {
  new_fixture mcp_silent_failure
  write_baseline 0.2.8000
  cat >"$F_BIN/collab-mcp" <<'MCP'
#!/bin/sh
read -r _line
exit 23
MCP
  chmod 755 "$F_BIN/collab-mcp"
  mkdir -p "$F_BIN/.collab-install-previous/bin/skills"
  printf 'retained previous\n' >"$F_BIN/.collab-install-previous/bin/collab"
  write_candidate 0.2.9001
  run_installer_ok "$F"
  assert_field "$F/out" outcome success
  assert_field "$F/out" baseline unverified
  assert_contains "$(field "$F/out" baseline_reason)" \
    "baseline collab-mcp initialize failed with exit 23" "silent MCP exit"
  assert_field "$F/out" original_exit 0
  assert_field "$F/out" previous_release "$F_BIN/.collab-install-previous"
  grep -q 'retained previous' "$F_BIN/.collab-install-previous/bin/collab" ||
    fail "silent MCP failure displaced the retained previous release"
  assert_no_lifecycle "$F"
}

case_baseline_mcp_explicit_failure_reports_exit() {
  new_fixture mcp_explicit_failure
  write_baseline 0.2.8000
  cat >"$F_BIN/collab-mcp" <<'MCP'
#!/bin/sh
read -r _line
printf 'unique baseline MCP failure\n' >&2
exit 37
MCP
  chmod 755 "$F_BIN/collab-mcp"
  mkdir -p "$F_BIN/.collab-install-previous/bin/skills"
  printf 'retained previous\n' >"$F_BIN/.collab-install-previous/bin/collab"
  write_candidate 0.2.9001
  run_installer_ok "$F"
  assert_field "$F/out" outcome success
  assert_field "$F/out" baseline unverified
  assert_contains "$(field "$F/out" baseline_reason)" \
    "baseline collab-mcp initialize failed with exit 37: unique baseline MCP failure" \
    "explicit MCP exit and diagnostic"
  assert_field "$F/out" original_exit 0
  assert_field "$F/out" previous_release "$F_BIN/.collab-install-previous"
  grep -q 'retained previous' "$F_BIN/.collab-install-previous/bin/collab" ||
    fail "explicit MCP failure displaced the retained previous release"
  assert_no_lifecycle "$F"
}

case_debug_entries_untouched() {
  new_fixture debug_isolation
  mkdir -p "$F_BIN"
  printf 'debug collab\n' >"$F_BIN/collab-debug"
  printf 'debug mcp\n' >"$F_BIN/collab-mcp-debug"
  chmod 755 "$F_BIN/collab-debug" "$F_BIN/collab-mcp-debug"
  write_candidate 0.2.9001
  run_installer_ok "$F"
  grep -q 'debug collab' "$F_BIN/collab-debug" || fail "collab-debug was overwritten on success"
  grep -q 'debug mcp' "$F_BIN/collab-mcp-debug" || fail "collab-mcp-debug was overwritten on success"
  write_candidate 0.2.9002
  run_installer "$F" COLLAB_STUB_FULL_EXIT=9 COLLAB_STUB_FULL_MSG="post fail"
  grep -q 'debug collab' "$F_BIN/collab-debug" || fail "collab-debug was overwritten on failure"
  grep -q 'debug mcp' "$F_BIN/collab-mcp-debug" || fail "collab-mcp-debug was overwritten on failure"
  assert_no_lifecycle "$F"
}

case_gnu_stat_partial_stdout_mode() {
  new_fixture gnu_stat_mode
  write_baseline 0.2.8000
  install_gnu_stat_shim "$F"
  write_candidate 0.2.9001
  run_installer_ok "$F"
  assert_field "$F/out" outcome success
  assert_field "$F/out" baseline recoverable_release
  assert_mode "$F_BIN/collab" 755
  assert_mode "$F_BIN/collab-mcp" 755
  assert_mode "$F_BIN/collab-desktop-mcp" 755
  assert_mode "$F_BIN/collab-desktop-mcp.mjs" 644
  assert_no_leftovers "$F"
  assert_no_lifecycle "$F"
}

case_stale_lock_refusal() {
  new_fixture stale_lock
  mkdir -p "$F_BIN/.collab-install.lock"
  write_candidate 0.2.9001
  run_installer "$F"
  [ "$(cat "$F/exit")" -ne 0 ] || fail "stale lock: expected refusal"
  assert_contains "$(cat "$F/err")" "unfinished or active installation" "lock refusal"
  assert_contains "$(cat "$F/err")" "recovery material" "lock recovery pointer"
  assert_absent "$F_BIN/collab"
  assert_present "$F_BIN/.collab-install.lock"
  assert_no_lifecycle "$F"
}

case_concurrent_second_refused_then_retry() {
  new_fixture concurrency
  write_candidate 0.2.9001
  mkdir -p "$F_BIN/.collab-install.lock"
  run_installer "$F"
  [ "$(cat "$F/exit")" -ne 0 ] || fail "concurrent: expected refusal while lock held"
  assert_absent "$F_BIN/collab"
  rm -rf "$F_BIN/.collab-install.lock"
  run_installer_ok "$F"
  assert_present "$F_BIN/collab"
  assert_no_lifecycle "$F"
}

case_recovery_marker_refusal() {
  new_fixture recovery_marker
  mkdir -p "$F_BIN/.collab-install-recovery"
  printf 'saved\n' >"$F_BIN/.collab-install-recovery/marker"
  write_candidate 0.2.9001
  run_installer "$F"
  [ "$(cat "$F/exit")" -ne 0 ] || fail "recovery marker: expected refusal"
  assert_contains "$(cat "$F/err")" "recovery is pending" "recovery refusal"
  assert_absent "$F_BIN/collab"
  assert_present "$F_BIN/.collab-install-recovery"
  assert_absent "$F_BIN/.collab-install.lock"
  assert_no_lifecycle "$F"
}

case_permission_denied_candidate_no_mutation() {
  new_fixture perm_candidate
  write_candidate 0.2.9001
  chmod 000 "$F_REPO/target/release/collab"
  run_installer "$F"
  [ "$(cat "$F/exit")" -ne 0 ] || fail "perm candidate: expected failure"
  assert_contains "$(field "$F/out" original_error)" "candidate collab" "candidate copy error"
  assert_absent "$F_BIN/collab"
  assert_no_lifecycle "$F"
}

case_incomplete_restoration() {
  new_fixture incomplete
  write_baseline 0.2.8000
  write_candidate 0.2.9001
  run_installer "$F" COLLAB_STUB_FULL_EXIT=9 COLLAB_STUB_FULL_MSG="post fail" \
    COLLAB_STUB_DESTROY_CAPTURE=1
  [ "$(cat "$F/exit")" -eq 9 ] || fail "incomplete: expected exit 9 got $(cat "$F/exit")"
  assert_field "$F/out" outcome failure_with_rollback
  assert_field "$F/out" rollback incomplete
  assert_contains "$(field "$F/out" rollback_errors)" "restore:collab" "rollback errors"
  local found=0
  local leftover
  for leftover in "$F_BIN"/.collab-install.* ; do
    [ -e "$leftover" ] || continue
    found=1
  done
  [ "$found" -eq 1 ] || fail "incomplete restoration did not preserve recovery material"
  assert_present "$F_BIN/.collab-install.lock"
  assert_present "$F_BIN/.collab-install-recovery"
  run_installer "$F"
  [ "$(cat "$F/exit")" -ne 0 ] || fail "incomplete: next invocation accepted unfinished recovery"
  assert_contains "$(cat "$F/err")" "unfinished installation recovery is pending" \
    "unfinished recovery refusal"
  assert_present "$F_BIN/.collab-install-recovery"
  assert_no_lifecycle "$F"
}

case_catchable_signal_restores() {
  new_fixture signal
  write_baseline 0.2.8000
  write_candidate 0.2.9001
  env -i \
    PATH="$F_REPO/tools:/usr/bin:/bin:/usr/sbin:/sbin" \
    HOME="$F_HOME" CARGO_HOME="$F_HOME/.cargo" \
    COLLAB_SKILLS_TARGET="$F_SKILLS" \
    COLLAB_STATE_DIR="$F/state" COLLAB_BUILD_STATE_DIR="$F/build-state" \
    TMPDIR="$F/tmp" \
    COLLAB_STUB_SLOW_FULL=1 COLLAB_STUB_SLOW_MARKER="$F/logs/slow" \
    COLLAB_STUB_LOG="$F/logs/stub.log" \
    sh "$F_REPO/scripts/install-global-collab.sh" >"$F/out" 2>"$F/err" &
  local pid=$!
  local i=0
  while [ "$i" -lt 100 ] && [ ! -e "$F/logs/slow" ]; do
    sleep 0.1
    i=$((i + 1))
  done
  [ -e "$F/logs/slow" ] || fail "signal: slow post-check never started"
  kill -TERM "$pid" 2>/dev/null
  local status=0
  wait "$pid" || status=$?
  [ "$status" -eq 143 ] || fail "signal: expected exit 143, got $status"
  printf '%s\n' "$status" >"$F/exit"
  assert_field "$F/out" outcome failure_with_rollback
  assert_field "$F/out" original_exit 143
  assert_contains "$(field "$F/out" original_error)" "signal 143" "signal error"
  assert_field "$F/out" rollback restored
  grep -q '0.2.8000' "$F_BIN/collab" || fail "signal did not restore the baseline"
  assert_no_lifecycle "$F"
}

# --- driver ---------------------------------------------------------------

run_case() {
  local name="$1"
  case_count=$((case_count + 1))
  local before_secs after_secs
  "$name"
  note "$name"
}

run_case case_success_first_install
run_case case_pending_activation
run_case case_pending_without_typed_reason_fails
run_case case_build_failure_preserves_baseline
run_case case_stage_skill_export_failure_no_mutation
run_case case_local_check_failure_no_mutation
run_case case_missing_artifact_no_mutation
run_case case_postcheck_failure_restores
run_case case_postcheck_multiline_error_preserves_chain
run_case case_final_version_probe_is_bounded_after_restore
run_case case_first_install_failure_restores_absence
run_case case_exact_restore_modes_and_absence
run_case case_rescue_symlink_and_restore_link
run_case case_rescue_unverified_success
run_case case_rescue_symlink_to_directory_replaces_link
run_case case_rescue_symlink_to_directory_restores_link
run_case case_predecessor_rotation
run_case case_rotation_second_move_failure_restores_predecessor
run_case case_rotation_predecessor_restore_failure_reports_recovery
run_case case_unverified_preserves_previous
run_case case_baseline_mcp_silent_failure_reports_exit
run_case case_baseline_mcp_explicit_failure_reports_exit
run_case case_debug_entries_untouched
run_case case_gnu_stat_partial_stdout_mode
run_case case_stale_lock_refusal
run_case case_concurrent_second_refused_then_retry
run_case case_recovery_marker_refusal
run_case case_permission_denied_candidate_no_mutation
run_case case_incomplete_restoration
run_case case_catchable_signal_restores

printf 'install-global-collab: %d cases passed\n' "$case_count"
