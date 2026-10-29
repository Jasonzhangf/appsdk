#!/usr/bin/env bash
#
# Explicit repository-local activation entry for the AppSDK worktree-limit
# commit gate.
#
# Installs a repository-local pre-commit hook plus an executable sidecar into
# the target repository's common Git hooks directory. It never sets
# core.hooksPath, never writes global or template configuration, and never
# overwrites a hook, symlink, or sidecar it does not own.
#
# Usage: scripts/install-project-commit-gate.sh <target-repository>

set -euo pipefail

umask 077

script_path="${BASH_SOURCE[0]}"
script_dir="$(cd "$(dirname "$script_path")" && pwd -P)"
repo_root="$(cd "$script_dir/.." && pwd -P)"
gate_source="$repo_root/scripts/check-worktree-limit.sh"

hook_name='pre-commit'
sidecar_name='appsdk-worktree-limit-gate'

usage() {
  cat <<'USAGE'
Usage: scripts/install-project-commit-gate.sh <target-repository>

Installs the AppSDK worktree-limit commit gate into <target-repository> as a
repository-local pre-commit hook plus an executable sidecar. Refuses to run when
core.hooksPath is configured, when the hooks directory is a symlink, or when an
existing pre-commit hook or sidecar is not one this script already installed.
USAGE
}

die() {
  printf 'error: %s\n' "$*" >&2
  exit 1
}

# Emit the managed pre-commit wrapper. It resolves the sidecar from this hook's
# own directory so the installed hook stays valid after the candidate worktree
# is removed.
render_hook() {
  cat <<'HOOK'
#!/usr/bin/env bash
# Managed by AppSDK scripts/install-project-commit-gate.sh.
# Runs the worktree-limit gate sidecar from this hook's own directory.
set -euo pipefail
hook_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd -P)"
exec "$hook_dir/appsdk-worktree-limit-gate" "$@"
HOOK
}

if [[ $# -eq 1 && "${1:-}" == "--help" ]]; then
  usage
  exit 0
fi
if [[ $# -ne 1 ]]; then
  usage >&2
  exit 2
fi

target_arg="$1"
[[ -n "$target_arg" ]] || die 'the target repository path must not be empty.'
[[ -d "$target_arg" ]] || die "target is not a directory: $target_arg"
[[ -f "$gate_source" ]] || die "gate source is missing: $gate_source"

target="$(cd "$target_arg" && pwd -P)"

# Resolve the repository without changing it. A target must be a real working
# tree; a bare or non-Git directory is rejected.
is_work_tree="$(git -C "$target" rev-parse --is-inside-work-tree 2>/dev/null || true)"
[[ "$is_work_tree" == "true" ]] || die "target is not a Git working tree: $target"

# Detect a configured core.hooksPath by command exit status, not by value. Git
# treats an empty string as configured (it can suppress the repository-local
# hooks), so the installer must refuse whenever the key is set, including
# when it is empty. Propagate any unexpected query failure instead of treating
# it as "not configured".
if configured_hooks="$(git -C "$target" config --get core.hooksPath)"; then
  status=0
else
  status=$?
fi
if [[ "$status" -ne 0 && "$status" -ne 1 ]]; then
  die "failed to query core.hooksPath in $target; refusing to install."
fi
[[ "$status" -eq 1 ]] || die "core.hooksPath is configured ('$configured_hooks'); refusing to install because the effective hooks directory is not the repository-local default."

common_dir="$(git -C "$target" rev-parse --git-common-dir 2>/dev/null)" || die 'failed to resolve the Git common directory.'
case "$common_dir" in
  /*) ;;
  *) common_dir="$target/$common_dir" ;;
esac
common_dir="$(cd "$common_dir" && pwd -P)" || die "failed to resolve the Git common directory: $common_dir"

hooks_dir="$common_dir/hooks"
if [[ -L "$hooks_dir" ]]; then
  die "the hooks directory is a symlink ($hooks_dir); refusing to install into a shared or unexpected location."
fi
if [[ -e "$hooks_dir" ]]; then
  [[ -d "$hooks_dir" ]] || die "the hooks path exists but is not a directory: $hooks_dir"
else
  mkdir -p "$hooks_dir" || die "failed to create the hooks directory: $hooks_dir"
fi

hook_target="$hooks_dir/$hook_name"
sidecar_target="$hooks_dir/$sidecar_name"

sidecar_exists=false
if [[ -e "$sidecar_target" || -L "$sidecar_target" ]]; then
  sidecar_exists=true
fi
hook_exists=false
if [[ -e "$hook_target" || -L "$hook_target" ]]; then
  hook_exists=true
fi

sidecar_managed=false
if [[ "$sidecar_exists" == true ]]; then
  [[ -f "$sidecar_target" && ! -L "$sidecar_target" ]] || die "an existing sidecar at $sidecar_target is not a regular file; refusing to overwrite it."
  [[ -x "$sidecar_target" ]] || die "an existing sidecar at $sidecar_target is not executable; refusing to overwrite it."
  cmp -s "$gate_source" "$sidecar_target" || die "an existing sidecar at $sidecar_target does not match the AppSDK gate; refusing to overwrite it."
  sidecar_managed=true
fi

hook_managed=false
if [[ "$hook_exists" == true ]]; then
  [[ -f "$hook_target" && ! -L "$hook_target" ]] || die "an existing pre-commit hook at $hook_target is not a regular file or is a symlink; refusing to replace it."
  [[ -x "$hook_target" ]] || die "an existing pre-commit hook at $hook_target is not executable; refusing to replace it."
  expected_hook="$(render_hook)"
  actual_hook="$(cat "$hook_target")"
  [[ "$actual_hook" == "$expected_hook" ]] || die "an existing pre-commit hook at $hook_target is not managed by AppSDK; refusing to replace it."
  hook_managed=true
fi

if [[ "$sidecar_managed" == true && "$hook_managed" == true ]]; then
  printf 'AppSDK worktree-limit gate is already installed in %s\n' "$target"
  printf '  hook:    %s\n' "$hook_target"
  printf '  sidecar: %s\n' "$sidecar_target"
  exit 0
fi
if [[ "$sidecar_exists" == true || "$hook_exists" == true ]]; then
  die "the AppSDK worktree-limit gate is only partially installed in $target (hook present: $hook_exists, sidecar present: $sidecar_exists); refusing to change any files. Inspect the existing hook and sidecar and report this partial state to the hook owner; coordinate with the hook owner before retrying."
fi

# Stage both files in the hooks directory, then publish them atomically with
# non-clobbering hard links. If another writer appears between the checks and
# publication, the link fails and this run removes only what it created.
stage_dir="$(mktemp -d "$hooks_dir/.appsdk-worktree-gate.XXXXXX")" || die "failed to create a staging directory under $hooks_dir"

created_sidecar=false
created_hook=false
cleanup() {
  if [[ -n "${stage_dir:-}" && -d "$stage_dir" ]]; then
    rm -rf "$stage_dir"
  fi
  if [[ "$created_sidecar" == true && "$created_hook" != true ]]; then
    rm -f "$sidecar_target"
  fi
}
trap cleanup EXIT

cp "$gate_source" "$stage_dir/$sidecar_name" || die 'failed to stage the gate sidecar.'
chmod 0755 "$stage_dir/$sidecar_name" || die 'failed to make the staged sidecar executable.'
render_hook > "$stage_dir/$hook_name" || die 'failed to stage the pre-commit hook.'
chmod 0755 "$stage_dir/$hook_name" || die 'failed to make the staged hook executable.'

ln "$stage_dir/$sidecar_name" "$sidecar_target" || die "failed to publish the gate sidecar; an entry already exists at $sidecar_target."
created_sidecar=true
if ! ln "$stage_dir/$hook_name" "$hook_target"; then
  die "failed to publish the pre-commit hook; an entry already exists at $hook_target. Refusing to replace it."
fi
created_hook=true

trap - EXIT
rm -rf "$stage_dir"
printf 'Installed the AppSDK worktree-limit gate in %s\n' "$target"
printf '  hook:    %s\n' "$hook_target"
printf '  sidecar: %s\n' "$sidecar_target"
