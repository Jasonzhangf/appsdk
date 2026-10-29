#!/usr/bin/env bash
#
# AppSDK project-local worktree-limit commit gate.
#
# Counts all Git-registered worktrees (including the main worktree) and rejects
# a commit when the repository exceeds the maximum and the UTC grace deadline
# has passed. Before the deadline, excess worktrees do not block commits.
#
# This file is the single runtime owner of the policy. It is invoked by a
# repository-local pre-commit hook, which is installed by
# scripts/install-project-commit-gate.sh. The hook resolves the gate from its
# own directory so the gate keeps working after the candidate worktree is gone.
#
# There is no public time, deadline, count, or skip override. The production
# clock always reads the real UTC epoch.

set -euo pipefail

# --- policy constants ------------------------------------------------------

# Maximum number of registered Git worktrees, including the main worktree.
APPSDK_WORKTREE_MAX=10

# UTC deadline after which the limit is enforced (2026-10-30 04:36:00 UTC).
APPSDK_WORKTREE_DEADLINE_EPOCH=1793334960

# --- private clock (replaced by the test adapter only) ---------------------

worktree_gate_now() {
  date -u +%s
}

# --- policy implementation -------------------------------------------------

# Print the count of registered worktrees. Returns 0 on success. On a Git
# failure it prints the failure and diagnostic to stderr and returns non-zero,
# so a query failure is never mistaken for an empty list.
worktree_gate_count() {
  local tmp out err status count record
  tmp="$(mktemp -d "${TMPDIR:-/tmp}/appsdk-worktree-gate.XXXXXX")" || {
    printf 'AppSDK commit gate error: failed to create a temporary capture directory.\n' >&2
    return 1
  }
  out="$tmp/list"
  err="$tmp/err"
  if git worktree list --porcelain -z >"$out" 2>"$err"; then
    status=0
  else
    status=$?
  fi
  if [[ "$status" -ne 0 ]]; then
    printf 'AppSDK commit gate error: failed to list registered worktrees.\n' >&2
    if [[ -s "$err" ]]; then
      cat "$err" >&2
    fi
    rm -rf "$tmp"
    return 1
  fi
  # Count the actual NUL-delimited "worktree " record fields by reading each
  # field with `read -d ''`. A path may contain a newline followed by a
  # literal "worktree " token; reading whole fields keeps that inside one
  # record, where a line-based count would invent an extra entry.
  count=0
  while IFS= read -r -d '' record; do
    case "$record" in
      'worktree '*) count=$((count + 1)) ;;
    esac
  done < "$out"
  rm -rf "$tmp"
  printf '%s\n' "$count"
  return 0
}

# Main decision path. Returns 0 to allow the commit, non-zero to block it.
worktree_gate_main() {
  local now
  if ! now="$(worktree_gate_now)"; then
    printf 'AppSDK commit gate error: failed to read the current UTC epoch.\n' >&2
    return 1
  fi
  if [[ ! "$now" =~ ^[0-9]+$ ]]; then
    printf 'AppSDK commit gate error: invalid current UTC epoch.\n' >&2
    return 1
  fi

  if [[ "$now" -lt "$APPSDK_WORKTREE_DEADLINE_EPOCH" ]]; then
    return 0
  fi

  local count
  if ! count="$(worktree_gate_count)"; then
    return 1
  fi

  if [[ "$count" -gt "$APPSDK_WORKTREE_MAX" ]]; then
    printf 'AppSDK commit blocked: %d registered worktrees exceed the maximum of %d, including main.\n' \
      "$count" "$APPSDK_WORKTREE_MAX" >&2
    printf 'The limit applies from 2026-10-30 04:36 UTC.\n' >&2
    printf 'Coordinate worktree cleanup with the project owner, then retry the commit.\n' >&2
    return 1
  fi

  return 0
}

# --- production entry ------------------------------------------------------
#
# When executed directly the gate runs the decision path and exits with its
# status. When sourced (fixture harness) only the functions are defined.

if [[ "${BASH_SOURCE[0]}" == "${0}" ]]; then
  worktree_gate_main "$@"
  exit $?
fi
