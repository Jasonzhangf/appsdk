#!/usr/bin/env bash
# Black-box check of the Collab release tarball: the packaged install.sh must
# install binaries, skills and login autostart, and a launchd failure must
# fail the install instead of leaving binaries without autostart.

set -euo pipefail

script_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd -P)"
repo_root="$(cd "$script_dir/../../.." && pwd -P)"
packager="$repo_root/collab/scripts/package-collab-tarball.sh"

if [[ "$(uname -s)" != "Darwin" ]]; then
  echo "skip: tarball packaging is macOS-only (uname -s = $(uname -s))" >&2
  exit 0
fi

work="$(mktemp -d "${TMPDIR:-/tmp}/collab-tarball-test.XXXXXX")"
trap 'rm -rf "$work"' EXIT HUP INT TERM

fail() {
  echo "FAIL: $*" >&2
  exit 1
}

write_stub() {
  local dir="$1" autostart_exit="$2"
  mkdir -p "$dir"
  cat >"$dir/collab" <<STUB
#!/bin/sh
case "\$1" in
  --version) printf 'collab 0.2.9001\n' ;;
  install-skills)
    printf '%s\n' "\$*" >>"\${STUB_LOG:-/dev/null}"
    target=""
    previous=""
    for argument in "\$@"; do
      [ "\$previous" = "--target" ] && target="\$argument"
      previous="\$argument"
    done
    [ -n "\$target" ] || exit 2
    mkdir -p "\$target"
    printf '# stub skill\n' >"\$target/SKILL.md"
    ;;
  install-autostart)
    printf '%s\n' "\$*" >>"\${STUB_LOG:-/dev/null}"
    if [ $autostart_exit -ne 0 ]; then
      printf 'launchctl bootstrap failed\n' >&2
      exit $autostart_exit
    fi
    ;;
  *) printf '%s\n' "\$*" >>"\${STUB_LOG:-/dev/null}" ;;
esac
STUB
  cat >"$dir/collab-mcp" <<'STUB'
#!/bin/sh
exit 0
STUB
  chmod 755 "$dir/collab" "$dir/collab-mcp"
}

package_and_install() {
  local label="$1" autostart_exit="$2" log="$3"
  local stub="$work/stub-$label" out="$work/out-$label"

  write_stub "$stub" "$autostart_exit"
  sh "$packager" --skip-build --bin-dir "$stub" --out "$out" >"$work/package-$label.out" 2>&1 ||
    fail "packaging failed for $label: $(cat "$work/package-$label.out")"

  local tarball="$out/collab-0.2.9001-macos-$(uname -m).tar.gz"
  [[ -f "$tarball" ]] || fail "tarball missing for $label: $tarball"
  tar -tzf "$tarball" | grep -qx 'collab-0.2.9001-macos-.*/install.sh' ||
    fail "install.sh missing from tarball for $label"

  local extract="$work/extract-$label"
  mkdir -p "$extract"
  tar -xzf "$tarball" -C "$extract"

  local package
  package="$(find "$extract" -maxdepth 1 -mindepth 1 -type d | head -1)"
  [[ -n "$package" ]] || fail "no package directory for $label"
  local name
  name="$(basename "$package")"

  for entry in bin/collab bin/collab-mcp bin/collab-desktop-mcp \
    bin/collab-desktop-mcp.mjs skills/collab/SKILL.md install.sh README-INSTALL.md \
    INSTALL-ONBOARDING.md; do
    [[ -e "$package/$entry" ]] || fail "missing $entry in package $name"
  done
  grep -q 'YOLO is not required' "$package/INSTALL-ONBOARDING.md" ||
    fail "INSTALL-ONBOARDING.md lacks the permission profile guidance in $name"
  grep -q 'Git is not a Collab requirement' "$package/INSTALL-ONBOARDING.md" ||
    fail "INSTALL-ONBOARDING.md lacks the Git guidance in $name"
  grep -q 'tmux is placement, not identity' "$package/INSTALL-ONBOARDING.md" ||
    fail "INSTALL-ONBOARDING.md lacks the tmux guidance in $name"
  grep -q 'QQBot receiver and first bind' "$package/INSTALL-ONBOARDING.md" ||
    fail "INSTALL-ONBOARDING.md lacks the QQBot guidance in $name"
  grep -q '# stub skill' "$package/skills/collab/SKILL.md" ||
    fail "archived Skill bundle did not come from the packaged binary in $name"
  [[ -x "$package/install.sh" ]] || fail "install.sh is not executable in $name"
  grep -q 'install-autostart' "$package/install.sh" ||
    fail "install.sh does not run collab install-autostart in $name"
  grep -q "Collab 0.2.9001" "$package/README-INSTALL.md" ||
    fail "README-INSTALL.md lacks the packaged version in $name"

  local home="$work/home-$label"
  mkdir -p "$home"
  STUB_LOG="$log" HOME="$home" CARGO_HOME="$home/.cargo" \
    COLLAB_SKILLS_TARGET="$home/.agents/skills/collab" \
    sh "$package/install.sh" >"$work/install-$label.out" 2>&1 || {
    local status=$?
    cat "$work/install-$label.out" >&2
    return "$status"
  }

  [[ -x "$home/.cargo/bin/collab" ]] || fail "collab binary not installed for $label"
  [[ -x "$home/.cargo/bin/collab-mcp" ]] || fail "collab-mcp binary not installed for $label"
  [[ -x "$home/.cargo/bin/collab-desktop-mcp" ]] ||
    fail "collab-desktop-mcp not installed for $label"
  [[ -f "$home/.cargo/bin/collab-desktop-mcp.mjs" ]] ||
    fail "collab-desktop-mcp.mjs not installed for $label"
  grep -q '^install-skills --target ' "$log" || fail "install-skills was not invoked for $label"
  grep -q '^install-autostart' "$log" || fail "install-autostart was not invoked for $label"
}

package_and_install ok 0 "$work/invoke-ok.log"

if package_and_install broken 3 "$work/invoke-broken.log" 2>"$work/broken.err"; then
  fail "install.sh accepted a failing launchd autostart"
fi
grep -q 'launchctl bootstrap failed' "$work/broken.err" ||
  fail "install.sh hid the launchd failure instead of reporting it"

echo "package-collab-tarball: 2 checks passed"
