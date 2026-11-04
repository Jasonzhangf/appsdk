#!/usr/bin/env bash
# Black-box check of the Collab release tarball: the packaged install.sh must
# install binaries and Skills only. It must not invoke `collab
# install-autostart`, `collab up`, or `launchctl`, and must not create a daemon
# socket. Activation belongs to `collab up`, which writes the service record for
# the current project before loading the login agent.

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
  local dir="$1"
  mkdir -p "$dir"
  cat >"$dir/launchctl" <<'LAUNCH'
#!/bin/sh
printf '%s\n' "$*" >>"$LAUNCHCTL_LOG"
exit 0
LAUNCH
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
    launchctl load -w "\$HOME/Library/LaunchAgents/com.appsdk.collab.plist"
    ;;
  *) printf '%s\n' "\$*" >>"\${STUB_LOG:-/dev/null}" ;;
esac
STUB
  cat >"$dir/collab-mcp" <<'STUB'
#!/bin/sh
exit 0
STUB
  chmod 755 "$dir/collab" "$dir/collab-mcp" "$dir/launchctl"
}

package_and_install() {
  local label="$1" log="$2"
  local stub="$work/stub-$label" out="$work/out-$label"

  write_stub "$stub"
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
  if grep -q 'install-autostart' "$package/install.sh"; then
    fail "install.sh activates the daemon in $name"
  fi
  grep -q 'collab up' "$package/README-INSTALL.md" ||
    fail "README-INSTALL.md lacks the collab up activation step in $name"
  grep -q 'collab up' "$package/INSTALL-ONBOARDING.md" ||
    fail "INSTALL-ONBOARDING.md lacks the collab up activation step in $name"
  if grep -q 'install-autostart' "$package/README-INSTALL.md" "$package/INSTALL-ONBOARDING.md"; then
    fail "packaged activation documentation still describes install-time autostart in $name"
  fi
  grep -q "Collab 0.2.9001" "$package/README-INSTALL.md" ||
    fail "README-INSTALL.md lacks the packaged version in $name"

  local home="$work/home-$label"
  mkdir -p "$home"
  local launchctl_log="$work/launchctl-$label.log"
  : >"$launchctl_log"
  PATH="$stub:$PATH" STUB_LOG="$log" LAUNCHCTL_LOG="$launchctl_log" \
    HOME="$home" CARGO_HOME="$home/.cargo" \
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
  if grep -Eq '^(install-autostart|up|serve|daemon|start)\b' "$log"; then
    fail "install.sh activated or started the daemon for $label: $(grep -E '^(install-autostart|up|serve|daemon|start)\b' "$log" | head -1)"
  fi
  if [[ -s "$launchctl_log" ]]; then
    fail "install.sh invoked launchctl for $label: $(cat "$launchctl_log")"
  fi
}

# Run the packaged install.sh against a target that must be refused: it must
# exit non-zero, report the external volume, and leave the target unwritten.
check_external_refused() {
  local label="$1" cargo_home="$2" log="$3" target_root="$4"
  local stub="$work/stub-$label" out="$work/out-$label"

  write_stub "$stub"
  sh "$packager" --skip-build --bin-dir "$stub" --out "$out" >"$work/package-$label.out" 2>&1 ||
    fail "packaging failed for $label: $(cat "$work/package-$label.out")"

  local tarball="$out/collab-0.2.9001-macos-$(uname -m).tar.gz"
  [[ -f "$tarball" ]] || fail "tarball missing for $label: $tarball"
  local extract="$work/extract-$label"
  mkdir -p "$extract"
  tar -xzf "$tarball" -C "$extract"
  local package
  package="$(find "$extract" -maxdepth 1 -mindepth 1 -type d | head -1)"
  [[ -n "$package" ]] || fail "no package directory for $label"

  local home="$work/home-$label"
  mkdir -p "$home"
  local status=0
  local launchctl_log="$work/launchctl-$label.log"
  : >"$launchctl_log"
  PATH="$stub:$PATH" STUB_LOG="$log" LAUNCHCTL_LOG="$launchctl_log" \
    HOME="$home" CARGO_HOME="$cargo_home" \
    COLLAB_SKILLS_TARGET="$home/.agents/skills/collab" \
    sh "$package/install.sh" >"$work/install-$label.out" 2>&1 || status=$?
  [[ "$status" -ne 0 ]] || fail "$label: install.sh accepted an external install target"
  grep -q 'external volume' "$work/install-$label.out" ||
    fail "$label: install.sh did not report the external volume refusal"
  [[ ! -e "$target_root" ]] || fail "$label: install.sh wrote $target_root before refusing"
  if grep -q '^install-skills ' "$log" 2>/dev/null; then
    fail "$label: install-skills ran for an external target"
  fi
  if grep -q '^install-autostart' "$log" 2>/dev/null; then
    fail "$label: install-autostart ran for an external target"
  fi
  if [[ -s "$launchctl_log" ]]; then
    fail "$label: install.sh invoked launchctl for an external target"
  fi
}

checks=0

package_and_install ok "$work/invoke-ok.log"
checks=$((checks + 1))

# macOS: a real external target and a system-volume symlink that points at an
# external volume must both be refused before any write. The positive install
# above already proves a system-volume target is accepted.
system_dev="$(stat -f '%d' /System/Volumes/Data 2>/dev/null || stat -f '%d' /)"
external_root=""
for candidate in /Volumes/*; do
  [[ -e "$candidate" ]] || continue
  resolved="$(cd -P "$candidate" 2>/dev/null && pwd -P)" || continue
  if [[ "$(stat -f '%d' "$resolved")" != "$system_dev" ]]; then
    external_root="$resolved"
    break
  fi
done

if [[ -n "$external_root" ]]; then
  real_target="$external_root/collab-ext-real-$$"
  check_external_refused external-real "$real_target" "$work/invoke-ext-real.log" "$real_target"
  checks=$((checks + 1))

  ln -s "$external_root" "$work/ext-link"
  link_target="$work/ext-link/collab-ext-link-$$"
  check_external_refused external-symlink "$link_target" "$work/invoke-ext-link.log" "$link_target"
  checks=$((checks + 1))
else
  echo "skip: no external volume under /Volumes to prove external refusal" >&2
fi

echo "package-collab-tarball: ${checks} checks passed"
