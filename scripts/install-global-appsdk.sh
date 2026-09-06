#!/usr/bin/env bash

set -euo pipefail

umask 077

script_path="${BASH_SOURCE[0]}"
script_dir="$(cd "$(dirname "$script_path")" && pwd -P)"
repo_root="$(cd "$script_dir/.." && pwd -P)"
user_home="${HOME:?HOME is required to resolve the user-local install roots}"

if [[ $# -eq 1 && "${1:-}" == "--help" ]]; then
  cat <<'USAGE'
Usage: scripts/install-global-appsdk.sh

Builds the Rust release, atomically installs one appsdk beside the active
cargo executable, removes exact legacy user-local AppSDK copies, and checks
that only the canonical managed entry remains.
USAGE
  exit 0
fi

if [[ $# -ne 0 ]]; then
  echo "error: no arguments are supported; run this script from any directory" >&2
  exit 2
fi

cargo_path="$(command -v cargo || true)"
if [[ -z "$cargo_path" || ! -x "$cargo_path" ]]; then
  echo "error: cargo is not available on PATH" >&2
  exit 1
fi

cargo_bin_dir="$(dirname "$cargo_path")"
canonical_bin="$cargo_bin_dir/appsdk"
release_bin="$repo_root/rust/target/release/appsdk"

echo "Building AppSDK release from $repo_root"
cargo build --release --manifest-path "$repo_root/rust/Cargo.toml"

if [[ ! -x "$release_bin" ]]; then
  echo "error: release binary was not produced: $release_bin" >&2
  exit 1
fi

release_version="$($release_bin version)"
if [[ ! "$release_version" =~ ^appsdk[[:space:]][0-9]+\.[0-9]+\.[0-9]+[[:space:]]\(rust\)$ ]]; then
  echo "error: release binary returned an invalid version: $release_version" >&2
  exit 1
fi

mkdir -p "$cargo_bin_dir"
stage_file="$(mktemp "$cargo_bin_dir/.appsdk-install.XXXXXX")"
cleanup_stage() {
  if [[ -e "$stage_file" || -L "$stage_file" ]]; then
    rm -f -- "$stage_file"
  fi
}
trap cleanup_stage EXIT

cp "$release_bin" "$stage_file"
chmod 0755 "$stage_file"
staged_version="$($stage_file version)"
if [[ "$staged_version" != "$release_version" ]]; then
  echo "error: staged binary changed during install" >&2
  exit 1
fi

# mv within the cargo bin directory is the only replacement point. A failed
# build, copy, chmod, or version check leaves the previous canonical binary.
mv -f -- "$stage_file" "$canonical_bin"
trap - EXIT

remove_exact_copy() {
  local candidate="$1"
  if [[ "$candidate" == "$canonical_bin" ]]; then
    return 0
  fi
  if [[ -e "$candidate" || -L "$candidate" ]]; then
    echo "Removing legacy AppSDK copy: $candidate"
    rm -f -- "$candidate"
  fi
}

# These are AppSDK-managed user-local locations only. Do not scan or delete
# arbitrary files under the home directory.
remove_exact_copy "$user_home/.local/bin/appsdk"
remove_exact_copy "$user_home/.cargo/bin/appsdk"
for legacy_copy in "$user_home"/.local/lib/appsdk/*/appsdk; do
  [[ -e "$legacy_copy" || -L "$legacy_copy" ]] || continue
  remove_exact_copy "$legacy_copy"
done

if [[ ! -x "$canonical_bin" || "$($canonical_bin version)" != "$release_version" ]]; then
  echo "error: canonical install verification failed: $canonical_bin" >&2
  exit 1
fi

remaining=()
append_unique_remaining() {
  local candidate="$1"
  local existing
  if ((${#remaining[@]} > 0)); then
    for existing in "${remaining[@]}"; do
      [[ "$existing" == "$candidate" ]] && return 0
    done
  fi
  remaining+=("$candidate")
}

for managed_copy in "$canonical_bin" "$user_home/.local/bin/appsdk" "$user_home/.cargo/bin/appsdk"; do
  [[ "$managed_copy" == "$canonical_bin" ]] && {
    append_unique_remaining "$managed_copy"
    continue
  }
  [[ -e "$managed_copy" || -L "$managed_copy" ]] && append_unique_remaining "$managed_copy"
done
for legacy_copy in "$user_home"/.local/lib/appsdk/*/appsdk; do
  [[ -e "$legacy_copy" || -L "$legacy_copy" ]] || continue
  [[ "$legacy_copy" == "$canonical_bin" ]] || append_unique_remaining "$legacy_copy"
done

if [[ "${#remaining[@]}" -ne 1 || "${remaining[0]}" != "$canonical_bin" ]]; then
  printf '%s\n' "error: more than one managed AppSDK binary remains:" >&2
  printf '  %s\n' "${remaining[@]}" >&2
  exit 1
fi

digest_line="$(shasum -a 256 "$canonical_bin")"
digest="${digest_line%% *}"
printf 'Installed: %s\nVersion: %s\nSHA-256 (diagnostic): %s\n' \
  "$canonical_bin" "$release_version" "$digest"
printf '%s\n' 'Refresh the current shell command cache with: rehash (zsh) or hash -r (bash)'
