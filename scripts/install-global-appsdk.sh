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

Builds the Rust release, atomically installs appsdk and project-memory beside
the active cargo executable, removes exact legacy user-local AppSDK copies,
and links the legacy Memory entry to its canonical executable. Installs the
AppSDK project-governance, migration, and project-memory Skills from the same
release source.
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
memory_release="$repo_root/rust/target/release/project-memory"
memory_bin="$cargo_bin_dir/project-memory"

# The Skills are part of the same release surface. Validate and stage them
# before mutating any installed binary so a missing or invalid Skill cannot
# leave a partially upgraded environment.
skills=(appsdk-project-governance appsdk-migration project-memory)
skills_root="$user_home/.agents/skills"
skills_stage=''
skills_previous=''
skill_install_committed=false
installed_skills=()
stage_file=''

cleanup_skill_install() {
  if [[ -n "$skills_stage" && ( -e "$skills_stage" || -L "$skills_stage" ) ]]; then
    rm -rf -- "$skills_stage"
  fi
  if [[ "$skill_install_committed" != true && -n "$skills_previous" ]]; then
    for skill in "${skills[@]}"; do
      local previous="$skills_previous/$skill"
      local target="$skills_root/$skill"
      if [[ -e "$previous" || -L "$previous" ]]; then
        if [[ -e "$target" || -L "$target" ]]; then
          rm -rf -- "$target"
        fi
        mv -- "$previous" "$target"
      elif ((${#installed_skills[@]} > 0)); then
        for installed in "${installed_skills[@]}"; do
          if [[ "$installed" == "$skill" ]]; then
            if [[ -e "$target" || -L "$target" ]]; then
              rm -rf -- "$target"
            fi
            break
          fi
        done
      fi
    done
  fi
  if [[ -n "$skills_previous" && ( -e "$skills_previous" || -L "$skills_previous" ) ]]; then
    rm -rf -- "$skills_previous"
  fi
}

cleanup_install() {
  if [[ -n "$stage_file" && ( -e "$stage_file" || -L "$stage_file" ) ]]; then
    rm -f -- "$stage_file"
  fi
  cleanup_skill_install
}
trap cleanup_install EXIT

for skill in "${skills[@]}"; do
  source="$repo_root/skills/$skill"
  if [[ ! -s "$source/SKILL.md" ]]; then
    echo "error: AppSDK Skill source is missing: $source/SKILL.md" >&2
    exit 1
  fi
done

mkdir -p "$skills_root"
skills_stage="$(mktemp -d "$skills_root/.appsdk-skills.stage.XXXXXX")"
for skill in "${skills[@]}"; do
  cp -R "$repo_root/skills/$skill" "$skills_stage/$skill"
  if [[ ! -s "$skills_stage/$skill/SKILL.md" ]]; then
    echo "error: staged AppSDK Skill is invalid: $skills_stage/$skill" >&2
    exit 1
  fi
done

skills_previous="$(mktemp -d "$skills_root/.appsdk-skills.previous.XXXXXX")"
for skill in "${skills[@]}"; do
  target="$skills_root/$skill"
  if [[ -e "$target" || -L "$target" ]]; then
    mv -- "$target" "$skills_previous/$skill"
  fi
done

echo "Building AppSDK release from $repo_root"
cargo build --release --manifest-path "$repo_root/rust/Cargo.toml"

if [[ ! -x "$memory_release" ]]; then
  echo 'error: project-memory release binary was not produced' >&2
  exit 1
fi
"$memory_release" help >/dev/null

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
stage_file="$(mktemp "$cargo_bin_dir/.project-memory-install.XXXXXX")"
cp "$memory_release" "$stage_file"
chmod 0755 "$stage_file"
"$stage_file" help >/dev/null
mv -f -- "$stage_file" "$memory_bin"
mkdir -p "$user_home/.local/bin"
if [[ "$user_home/.local/bin/project-memory" != "$memory_bin" ]]; then
  stage_file="$(mktemp "$user_home/.local/bin/.project-memory-link.XXXXXX")"
  rm -f -- "$stage_file"
  ln -s "$memory_bin" "$stage_file"
  mv -f -- "$stage_file" "$user_home/.local/bin/project-memory"
fi

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

for skill in "${skills[@]}"; do
  mv -- "$skills_stage/$skill" "$skills_root/$skill"
  installed_skills+=("$skill")
done
rmdir -- "$skills_stage"
skills_stage=''
skill_install_committed=true
installed_skills=()
rm -rf -- "$skills_previous"
skills_previous=''
for skill in "${skills[@]}"; do
  printf 'Skill installed: %s\n' "$skills_root/$skill"
done
printf 'Memory installed: %s (legacy PATH entry links here)\n' "$memory_bin"
