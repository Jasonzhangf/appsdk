#!/usr/bin/env bash

set -euo pipefail

script_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd -P)"
repo_root="$(cd "$script_dir/../.." && pwd -P)"
installer="$repo_root/scripts/install-global-appsdk.sh"
fixture="$repo_root/rust/target/release/appsdk"

if [[ ! -x "$fixture" ]]; then
  echo "build the release binary before running installer tests" >&2
  exit 1
fi

assert_file_absent() {
  [[ ! -e "$1" && ! -L "$1" ]] || {
    echo "expected absent: $1" >&2
    exit 1
  }
}

run_install_test() {
  local test_root="$1"
  local fake_bin="$test_root/fake-cargo-bin"
  mkdir -p "$fake_bin" "$test_root/home/.local/bin" "$test_root/home/.local/lib/appsdk/0.1.6"
  cp "$fixture" "$test_root/fixture"
  printf '%s\n' 'keep this unrelated file' > "$test_root/home/.local/lib/appsdk/0.1.6/keep.txt"

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

  cp "$test_root/fixture" "$test_root/home/.local/bin/appsdk"
  cp "$test_root/fixture" "$test_root/home/.local/lib/appsdk/0.1.6/appsdk"
  PATH="$fake_bin:/usr/bin:/bin" HOME="$test_root/home" APPSDK_TEST_FIXTURE="$test_root/fixture" \
    bash "$installer" >/dev/null

  [[ -x "$fake_bin/appsdk" ]] || { echo 'canonical install missing' >&2; exit 1; }
  [[ "$($fake_bin/appsdk version)" == 'appsdk 0.1.6 (rust)' ]] || {
    echo 'canonical version mismatch' >&2
    exit 1
  }
  assert_file_absent "$test_root/home/.local/bin/appsdk"
  assert_file_absent "$test_root/home/.local/lib/appsdk/0.1.6/appsdk"
  [[ "$(<"$test_root/home/.local/lib/appsdk/0.1.6/keep.txt")" == 'keep this unrelated file' ]] || {
    echo 'unrelated file was changed' >&2
    exit 1
  }

  # A second run is a no-op replacement with the same result.
  PATH="$fake_bin:/usr/bin:/bin" HOME="$test_root/home" APPSDK_TEST_FIXTURE="$test_root/fixture" \
    bash "$installer" >/dev/null
  [[ "$(find "$fake_bin" -maxdepth 1 -name 'appsdk' -type f | wc -l | tr -d ' ')" == 1 ]] || {
    echo 'duplicate canonical entries found' >&2
    exit 1
  }
}

run_failed_build_test() {
  local test_root="$1"
  local fake_bin="$test_root/fake-cargo-bin"
  mkdir -p "$fake_bin" "$test_root/home"
  cp "$fixture" "$fake_bin/appsdk"
  local before
  before="$(shasum -a 256 "$fake_bin/appsdk")"

  cat > "$fake_bin/cargo" <<'FAKE_CARGO'
#!/usr/bin/env bash
exit 23
FAKE_CARGO
  chmod 0755 "$fake_bin/cargo"

  if PATH="$fake_bin:/usr/bin:/bin" HOME="$test_root/home" bash "$installer" >/dev/null 2>&1; then
    echo 'failed build unexpectedly installed' >&2
    exit 1
  fi
  [[ "$(shasum -a 256 "$fake_bin/appsdk")" == "$before" ]] || {
    echo 'failed build replaced the canonical binary' >&2
    exit 1
  }
}

test_root="$(mktemp -d "${TMPDIR:-/tmp}/appsdk-install-test.XXXXXX")"
failed_test_root=''
cleanup() {
  rm -rf -- "$test_root"
  [[ -z "$failed_test_root" ]] || rm -rf -- "$failed_test_root"
}
trap cleanup EXIT
run_install_test "$test_root"
failed_test_root="$(mktemp -d "${TMPDIR:-/tmp}/appsdk-install-failure.XXXXXX")"
run_failed_build_test "$failed_test_root"
echo 'installer tests: PASS'
