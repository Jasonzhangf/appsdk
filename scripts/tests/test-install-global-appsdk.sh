#!/usr/bin/env bash

set -euo pipefail

script_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd -P)"
repo_root="$(cd "$script_dir/../.." && pwd -P)"
installer="$repo_root/scripts/install-global-appsdk.sh"
fixture="$repo_root/rust/target/release/appsdk"
memory_fixture="$repo_root/rust/target/release/project-memory"

for skill in appsdk-migration appsdk-project-governance project-memory; do
  [[ ! -e "$repo_root/skills/$skill" ]] || {
    echo "project-local duplicate Skill remains discoverable: $skill" >&2
    exit 1
  }
  [[ -s "$repo_root/sdk-skill-sources/$skill/SKILL.md" ]] || {
    echo "AppSDK Skill source is missing: $skill" >&2
    exit 1
  }
done

for required in "$fixture" "$memory_fixture"; do
  [[ -x "$required" ]] || {
    echo "build the release binaries before running installer tests: $required" >&2
    exit 1
  }
done
source_version="$(tr -d '\r\n' < "$repo_root/rust/release-version")"
expected_version="appsdk $source_version (rust)"
health_token='old-program-available'

assert_file_absent() {
  [[ ! -e "$1" && ! -L "$1" ]] || {
    echo "expected absent: $1" >&2
    exit 1
  }
}

assert_file_present() {
  [[ -e "$1" || -L "$1" ]] || {
    echo "expected present: $1" >&2
    exit 1
  }
}

# No stage or previous directory may survive an install attempt.
assert_no_staging_leak() {
  local skills_root="$1"
  [[ -z "$(find "$skills_root" -maxdepth 1 \
    \( -name '.appsdk-skills.stage.*' -o -name '.appsdk-skills.previous.*' \) \
    -print -quit)" ]] || {
    echo "staging state leaked under: $skills_root" >&2
    exit 1
  }
}

# Stand-in for the previously installed appsdk program. Its health command is
# distinct from any real candidate so a same-version wrong replacement fails
# the rollback assertion.
write_old_program() {
  local bin="$1"
  cat > "$bin" <<EOF
#!/usr/bin/env bash
set -euo pipefail
if [[ "\${1:-}" == "version" ]]; then
  printf 'appsdk $source_version (rust)\n'
  exit 0
fi
if [[ "\${1:-}" == "installer-test-health" ]]; then
  printf 'old-program-available\n'
  exit 0
fi
printf 'old-program: unexpected command: %s\n' "\$*" >&2
exit 2
EOF
  chmod 0755 "$bin"
}

# Call the old program's public health entry by absolute path. The old program
# must still run its declared behavior after a failed install.
assert_old_program_available() {
  local bin="$1"
  local output
  output="$("$bin" installer-test-health)" || {
    echo "old program is not runnable: $bin" >&2
    exit 1
  }
  [[ "$output" == "$health_token" ]] || {
    echo "old program health mismatch: $output" >&2
    exit 1
  }
}

# Every scenario runs against a self-contained repo copy so the installer and
# its fake cargo only touch the scenario's own temp tree.
setup_isolated_repo() {
  local test_root="$1"
  local repo="$test_root/repo"
  mkdir -p "$repo/scripts" "$repo/sdk-skill-sources" \
    "$repo/rust/target/release" "$test_root/tmp"
  cp "$installer" "$repo/scripts/install-global-appsdk.sh"
  cp -R "$repo_root/sdk-skill-sources/." "$repo/sdk-skill-sources/"
  printf '%s\n' "$source_version" > "$repo/rust/release-version"
  cp "$fixture" "$repo/rust/target/release/appsdk"
  cp "$memory_fixture" "$repo/rust/target/release/project-memory"
}

run_install_test() {
  local test_root="$1"
  local fake_bin="$test_root/fake-cargo-bin"
  local repo="$test_root/repo"
  setup_isolated_repo "$test_root"
  mkdir -p \
    "$fake_bin" \
    "$test_root/home/.local/bin" \
    "$test_root/home/.local/lib/appsdk/0.1.0007" \
    "$test_root/home/.local/share/appsdk/0.1.0007" \
    "$test_root/home/.local/share/appsdk/not-appsdk"
  printf '%s\n' 'keep this unrelated file' \
    > "$test_root/home/.local/lib/appsdk/0.1.0007/keep.txt"
  cp "$repo_root/contracts/sdk-bundle.manifest.json" \
    "$test_root/home/.local/share/appsdk/0.1.0007/sdk-bundle.manifest.json"
  printf '%s\n' 'keep this unrelated bundle file' \
    > "$test_root/home/.local/share/appsdk/not-appsdk/keep.txt"

  cat > "$fake_bin/cargo" <<'FAKE_CARGO'
#!/usr/bin/env bash
set -euo pipefail
manifest=''
while [[ $# -gt 0 ]]; do
  if [[ "$1" == "--manifest-path" ]]; then
    manifest="$2"
    shift 2
  else
    shift
  fi
done
root="$(cd "$(dirname "$manifest")" && pwd -P)"
mkdir -p "$root/target/release"
cp "${APPSDK_TEST_FIXTURE:?}" "$root/target/release/appsdk"
FAKE_CARGO
  chmod 0755 "$fake_bin/cargo"

  cp "$fixture" "$test_root/home/.local/bin/appsdk"
  cp "$memory_fixture" "$test_root/home/.local/bin/project-memory"
  cp "$fixture" "$test_root/home/.local/lib/appsdk/0.1.0007/appsdk"
  PATH="$fake_bin:/usr/bin:/bin" HOME="$test_root/home" \
    TMPDIR="$test_root/tmp" APPSDK_TEST_FIXTURE="$fixture" \
    bash "$repo/scripts/install-global-appsdk.sh" >/dev/null

  [[ -x "$fake_bin/appsdk" ]] || { echo 'canonical install missing' >&2; exit 1; }
  [[ -x "$fake_bin/project-memory" ]] || { echo 'memory install missing' >&2; exit 1; }
  [[ "$(readlink "$test_root/home/.local/bin/project-memory")" == "$fake_bin/project-memory" ]] || {
    echo 'memory legacy entry does not link to the canonical binary' >&2
    exit 1
  }
  [[ "$($fake_bin/appsdk version)" == "$expected_version" ]] || {
    echo 'canonical version mismatch' >&2
    exit 1
  }
  local memory_help
  memory_help="$("$fake_bin/project-memory" help)" || {
    echo 'project-memory help failed' >&2
    exit 1
  }
  [[ "$memory_help" == *'"commands"'* ]] || {
    echo 'project-memory help did not expose its command list' >&2
    exit 1
  }
  for skill in appsdk-project-governance appsdk-migration project-memory; do
    [[ -s "$test_root/home/.agents/skills/$skill/SKILL.md" ]] || {
      echo "installed Skill missing: $skill" >&2
      exit 1
    }
    grep -q "^name: $skill$" "$test_root/home/.agents/skills/$skill/SKILL.md" || {
      echo "installed Skill entry mismatch: $skill" >&2
      exit 1
    }
  done
  [[ -s "$test_root/home/.agents/skills/appsdk-project-governance/references/bootstrap-migration.md" ]] || {
    echo 'governance reference missing after install' >&2
    exit 1
  }
  grep -q 'bootstrap-migration.md' \
    "$test_root/home/.agents/skills/appsdk-project-governance/SKILL.md" || {
    echo 'governance reference not reachable from SKILL.md' >&2
    exit 1
  }
  assert_file_absent "$test_root/home/.local/bin/appsdk"
  assert_file_absent "$test_root/home/.local/lib/appsdk/0.1.0007/appsdk"
  assert_file_present "$test_root/home/.local/share/appsdk/0.1.0007"
  [[ "$(<"$test_root/home/.local/lib/appsdk/0.1.0007/keep.txt")" == 'keep this unrelated file' ]] || {
    echo 'unrelated file was changed' >&2
    exit 1
  }
  [[ "$(<"$test_root/home/.local/share/appsdk/not-appsdk/keep.txt")" == \
    'keep this unrelated bundle file' ]] || {
    echo 'unrelated bundle directory was changed' >&2
    exit 1
  }
  assert_no_staging_leak "$test_root/home/.agents/skills"

  # A second run is a no-op replacement with the same public result.
  PATH="$fake_bin:/usr/bin:/bin" HOME="$test_root/home" \
    TMPDIR="$test_root/tmp" APPSDK_TEST_FIXTURE="$fixture" \
    bash "$repo/scripts/install-global-appsdk.sh" >/dev/null
  [[ "$(find "$fake_bin" -maxdepth 1 -name 'appsdk' -type f | wc -l | tr -d ' ')" == 1 ]] || {
    echo 'duplicate canonical entries found' >&2
    exit 1
  }
  [[ "$($fake_bin/appsdk version)" == "$expected_version" ]] || {
    echo 'canonical version mismatch after reinstall' >&2
    exit 1
  }
  [[ "$(readlink "$test_root/home/.local/bin/project-memory")" == "$fake_bin/project-memory" ]] || {
    echo 'memory legacy entry changed after reinstall' >&2
    exit 1
  }
  assert_no_staging_leak "$test_root/home/.agents/skills"
}

run_failed_build_test() {
  local test_root="$1"
  local fake_bin="$test_root/fake-cargo-bin"
  local repo="$test_root/repo"
  setup_isolated_repo "$test_root"
  mkdir -p "$fake_bin" "$test_root/home"
  mkdir -p "$test_root/home/.agents/skills/appsdk-project-governance"
  printf '%s\n' 'preserve existing governance skill' \
    > "$test_root/home/.agents/skills/appsdk-project-governance/SKILL.md"

  write_old_program "$fake_bin/appsdk"
  assert_old_program_available "$fake_bin/appsdk"

  cat > "$fake_bin/cargo" <<'FAKE_CARGO'
#!/usr/bin/env bash
exit 23
FAKE_CARGO
  chmod 0755 "$fake_bin/cargo"

  local status=0
  PATH="$fake_bin:/usr/bin:/bin" HOME="$test_root/home" TMPDIR="$test_root/tmp" \
    bash "$repo/scripts/install-global-appsdk.sh" >/dev/null 2>&1 || status=$?
  [[ "$status" -eq 23 ]] || {
    echo "failed build returned unexpected status: $status" >&2
    exit 1
  }
  assert_old_program_available "$fake_bin/appsdk"
  [[ "$(<"$test_root/home/.agents/skills/appsdk-project-governance/SKILL.md")" == \
    'preserve existing governance skill' ]] || {
    echo 'failed build left the existing Skill absent' >&2
    exit 1
  }
  assert_no_staging_leak "$test_root/home/.agents/skills"
}

run_missing_skill_source_test() {
  local test_root="$1"
  local fake_bin="$test_root/fake-cargo-bin"
  local repo="$test_root/repo"
  setup_isolated_repo "$test_root"
  rm -f "$repo/sdk-skill-sources/appsdk-migration/SKILL.md"
  mkdir -p "$fake_bin" "$test_root/home"

  # cargo must be discoverable so the run reaches the Skill preflight; it must
  # never be executed for a missing Skill source.
  cat > "$fake_bin/cargo" <<'FAKE_CARGO'
#!/usr/bin/env bash
: > "${APPSDK_TEST_CARGO_MARKER:?}"
exit 99
FAKE_CARGO
  chmod 0755 "$fake_bin/cargo"

  write_old_program "$fake_bin/appsdk"
  assert_old_program_available "$fake_bin/appsdk"

  mkdir -p "$test_root/home/.agents/skills/appsdk-project-governance"
  printf '%s\n' 'preserve existing governance skill' \
    > "$test_root/home/.agents/skills/appsdk-project-governance/SKILL.md"

  local output
  if output="$(PATH="$fake_bin:/usr/bin:/bin" HOME="$test_root/home" TMPDIR="$test_root/tmp" \
    APPSDK_TEST_CARGO_MARKER="$test_root/cargo-was-called" \
    bash "$repo/scripts/install-global-appsdk.sh" 2>&1)"; then
    echo 'missing skill source unexpectedly installed' >&2
    exit 1
  fi
  [[ "$output" == *'Skill source is missing'* ]] || {
    echo 'missing skill source failed for an unexpected reason' >&2
    echo "$output" >&2
    exit 1
  }
  [[ ! -e "$test_root/cargo-was-called" ]] || {
    echo 'cargo was invoked before the Skill preflight' >&2
    exit 1
  }
  assert_old_program_available "$fake_bin/appsdk"
  [[ "$(<"$test_root/home/.agents/skills/appsdk-project-governance/SKILL.md")" == \
    'preserve existing governance skill' ]] || {
    echo 'missing skill source replaced an existing Skill' >&2
    exit 1
  }
  assert_no_staging_leak "$test_root/home/.agents/skills"
}

test_root="$(mktemp -d "${TMPDIR:-/tmp}/appsdk-install-test.XXXXXX")"
failed_test_root=''
missing_skill_test_root=''
cleanup() {
  rm -rf -- "$test_root"
  [[ -z "$failed_test_root" ]] || rm -rf -- "$failed_test_root"
  [[ -z "$missing_skill_test_root" ]] || rm -rf -- "$missing_skill_test_root"
}
trap cleanup EXIT
run_install_test "$test_root"
failed_test_root="$(mktemp -d "${TMPDIR:-/tmp}/appsdk-install-failure.XXXXXX")"
run_failed_build_test "$failed_test_root"
missing_skill_test_root="$(mktemp -d "${TMPDIR:-/tmp}/appsdk-install-missing-skill.XXXXXX")"
run_missing_skill_source_test "$missing_skill_test_root"
echo 'installer tests: PASS'
