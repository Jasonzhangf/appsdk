#!/usr/bin/env bash
# Focused regression for the shared installer transaction helper's bounded
# child execution. It proves that a child that finishes normally returns its
# own status instead of being misclassified as a timeout, and that a real
# timeout returns 124 without leaving the child or timeout marker behind.

set -euo pipefail

script_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd -P)"
repo_root="$(cd "$script_dir/../../.." && pwd -P)"
helper="${COLLAB_SUT_HELPER:-"$repo_root/collab/scripts/collab-install-transaction.sh"}"

work="$(mktemp -d "${TMPDIR:-/tmp}/collab-bounded-test.XXXXXX")"
trap 'rm -rf "$work"' EXIT HUP INT TERM

mkdir -p "$work/home" "$work/cargo" "$work/tmp" "$work/state" "$work/build-state"
HOME="$work/home"
CARGO_HOME="$work/cargo"
COLLAB_STATE_DIR="$work/state"
COLLAB_BUILD_STATE_DIR="$work/build-state"
TMPDIR="$work/tmp"
export HOME CARGO_HOME COLLAB_STATE_DIR COLLAB_BUILD_STATE_DIR TMPDIR

fail() {
  printf 'FAIL: %s\n' "$*" >&2
  exit 1
}

expect_bounded() {
  local want="$1" seconds="$2" label="$3"
  shift 3
  local out="$work/$label.out" err="$work/$label.err" rc=0
  set +e
  sh -c '
    helper=$1
    seconds=$2
    out=$3
    err=$4
    shift 4
    . "$helper"
    tx_bounded "$seconds" "$out" "$err" "$@"
  ' sh "$helper" "$seconds" "$out" "$err" "$@"
  rc=$?
  set -e
  [ "$rc" -eq "$want" ] || fail "$label: expected exit $want got $rc"
}

expect_bounded 0 2 fast-success sh -c 'exit 0'
expect_bounded 7 2 nonzero sh -c 'exit 7'
expect_bounded 0 2 output sh -c 'printf "bounded-ok\n"'
[ "$(cat "$work/output.out")" = "bounded-ok" ] ||
  fail "output: expected bounded-ok got [$(cat "$work/output.out")]"
expect_bounded 3 2 stderr sh -c 'printf "bounded-err\n" >&2; exit 3'
[ "$(cat "$work/stderr.err")" = "bounded-err" ] ||
  fail "stderr: expected bounded-err got [$(cat "$work/stderr.err")]"
expect_bounded 124 1 real-timeout sh -c 'sleep 3'

for marker in "$work"/tmp/.collab-bounded.*; do
  [ -e "$marker" ] || continue
  fail "timeout marker left behind: $marker"
done

printf 'install-transaction-bounded: 5 checks passed\n'
