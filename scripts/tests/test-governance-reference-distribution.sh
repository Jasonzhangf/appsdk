#!/usr/bin/env bash
# Exercise the real installer's document-copy path without building or running
# a product binary. Fixture executables satisfy its version/help interface.
set -euo pipefail

script_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd -P)"
repo_root="$(cd "$script_dir/../.." && pwd -P)"
work_dir="$(mktemp -d "${TMPDIR:-/tmp}/governance-reference-distribution.XXXXXX")"
cleanup() {
  rm -r -- "$work_dir"
}
trap cleanup EXIT

fixture_repo="$work_dir/repo"
fixture_bin="$work_dir/bin"
fixture_home="$work_dir/home"
mkdir -p "$fixture_repo/scripts" "$fixture_repo/rust/target/release" \
  "$fixture_bin" "$fixture_home/.agents/skills/appsdk-project-governance/references" \
  "$work_dir/tmp"
cp "$repo_root/scripts/install-global-appsdk.sh" "$fixture_repo/scripts/"
cp -R "$repo_root/sdk-skill-sources" "$fixture_repo/"
cp "$repo_root/rust/release-version" "$fixture_repo/rust/release-version"

cat > "$fixture_bin/cargo" <<'FIXTURE'
#!/usr/bin/env bash
exit 0
FIXTURE
cat > "$fixture_repo/rust/target/release/appsdk" <<'FIXTURE'
#!/usr/bin/env bash
set -euo pipefail
[[ "${1:-}" == version ]] || exit 2
printf 'appsdk %s (rust)\n' "${APPSDK_REFERENCE_TEST_VERSION:?}"
FIXTURE
cat > "$fixture_repo/rust/target/release/project-memory" <<'FIXTURE'
#!/usr/bin/env bash
[[ "${1:-}" == help ]] || exit 2
printf '{"commands":["help"]}\n'
FIXTURE
chmod 0755 "$fixture_bin/cargo" "$fixture_repo/rust/target/release/appsdk" \
  "$fixture_repo/rust/target/release/project-memory"

installed_skill="$fixture_home/.agents/skills/appsdk-project-governance"
printf 'previous reference\n' > "$installed_skill/references/bootstrap-migration.md"

install_references() {
  PATH="$fixture_bin:/usr/bin:/bin" HOME="$fixture_home" TMPDIR="$work_dir/tmp" \
    APPSDK_REFERENCE_TEST_VERSION="$(tr -d '\r\n' < "$fixture_repo/rust/release-version")" \
    bash "$fixture_repo/scripts/install-global-appsdk.sh"
}

check_navigation() {
  local reference="$installed_skill/references/bootstrap-migration.md"
  grep -Fq 'references/bootstrap-migration.md#skeleton-operations-and-operators' \
    "$installed_skill/SKILL.md" || return 1
  grep -Fq 'bootstrap-migration.md#skeleton-operations-and-operators' \
    "$installed_skill/references/development-debug.md" || return 1
  [[ -s "$reference" ]] || return 1
  grep -Fxq '## Skeleton, Operations and operators' "$reference"
}

install_references > "$work_dir/install.log" 2>&1 || {
  cat "$work_dir/install.log" >&2
  exit 1
}
check_navigation || { echo 'installed governance navigation failed' >&2; exit 1; }
echo 'PASS installed reference replaces the previous reference and is reachable'

unlink "$installed_skill/references/bootstrap-migration.md"
if check_navigation; then
  echo 'missing reference was accepted' >&2
  exit 1
fi
echo 'PASS missing installed target is detected'

install_references > "$work_dir/reinstall.log" 2>&1 || {
  cat "$work_dir/reinstall.log" >&2
  exit 1
}
check_navigation || { echo 'reinstalled governance navigation failed' >&2; exit 1; }
printf '[missing](references/missing.md)\n' > "$installed_skill/SKILL.md"
if check_navigation; then
  echo 'broken installed route was accepted' >&2
  exit 1
fi
echo 'PASS broken installed route is detected'
echo 'governance reference distribution: PASS'
