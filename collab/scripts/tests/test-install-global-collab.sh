#!/usr/bin/env bash
# Black-box check of the global installer: a down-state install must install
# the binaries, MCP helper and Skills only. It must not invoke
# `collab install-autostart`, must not call launchctl, and must not create a
# daemon socket. Daemon activation belongs to `collab up`, which writes the
# service record for the current project before loading the agent.
#
# The test runs a copy of the real installer against a throwaway repo layout
# with a fake collab, a fake launchctl and a temporary HOME. It never touches
# the real HOME, state root, launchd, daemon or tmux, and creates no socket.

set -euo pipefail

script_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd -P)"
repo_root="$(cd "$script_dir/../../.." && pwd -P)"

work="$(mktemp -d "${TMPDIR:-/tmp}/collab-install-test.XXXXXX")"
trap 'rm -rf "$work"' EXIT HUP INT TERM

fail() {
  echo "FAIL: $*" >&2
  exit 1
}

# Repo layout the installer expects; copy the real installer source so this
# test fails when the installer regresses back to activating the daemon.
layout="$work/repo"
mkdir -p "$layout/scripts" "$layout/target/release" "$layout/tools"
cp "$repo_root/collab/scripts/install-global-collab.sh" \
  "$layout/scripts/install-global-collab.sh"
cp "$repo_root/collab/scripts/require-system-volume.sh" \
  "$layout/scripts/require-system-volume.sh"

# Fake build: the installer only requires the candidate binaries to exist.
cat >"$layout/scripts/build-collab.sh" <<'BUILD'
#!/bin/sh
exit 0
BUILD

# Fake launchctl: any invocation means the installer tried to activate or start
# the daemon.
cat >"$layout/tools/launchctl" <<'LAUNCH'
#!/bin/sh
printf '%s\n' "$*" >>"$LAUNCHCTL_LOG"
exit 0
LAUNCH

# Fake collab: records subcommands and installs Skills. If the installer ever
# asks for autostart again, model the real path so the launchctl guard fires.
cat >"$layout/target/release/collab" <<'COLLAB'
#!/bin/sh
printf '%s\n' "$*" >>"$COLLAB_LOG"
case "$1" in
  --version)
    printf 'collab fake 0.0.0\n'
    ;;
  install-skills)
    target=""
    previous=""
    for argument in "$@"; do
      [ "$previous" = "--target" ] && target="$argument"
      previous="$argument"
    done
    [ -n "$target" ] || exit 2
    mkdir -p "$target"
    printf '# stub skill\n' >"$target/SKILL.md"
    ;;
  install-autostart)
    launchctl load -w "$HOME/Library/LaunchAgents/com.appsdk.collab.plist"
    ;;
esac
COLLAB
printf '#!/bin/sh\nexit 0\n' >"$layout/target/release/collab-mcp"
printf '#!/bin/sh\nexit 0\n' >"$layout/scripts/collab-desktop-mcp"
printf '// stub\n' >"$layout/scripts/collab-desktop-mcp.mjs"
chmod 755 "$layout/target/release/collab" \
  "$layout/target/release/collab-mcp" \
  "$layout/scripts/build-collab.sh" \
  "$layout/tools/launchctl" \
  "$layout/scripts/collab-desktop-mcp"

home="$work/home"
mkdir -p "$home"
: >"$work/collab.log"
: >"$work/launchctl.log"

# Put the fake launchctl first so a real load can never touch launchd.
PATH="$layout/tools:$PATH" \
  HOME="$home" \
  CARGO_HOME="$home/.cargo" \
  COLLAB_LOG="$work/collab.log" \
  LAUNCHCTL_LOG="$work/launchctl.log" \
  sh "$layout/scripts/install-global-collab.sh" >"$work/install.out" 2>&1 ||
  fail "installer exited non-zero: $(cat "$work/install.out")"

grep -q '^install-skills ' "$work/collab.log" ||
  fail "installer did not install Skills"
[ -f "$home/.agents/skills/collab/SKILL.md" ] ||
  fail "installer did not write the Skill bundle"
[ -x "$home/.cargo/bin/collab" ] || fail "collab binary not installed"
[ -x "$home/.cargo/bin/collab-mcp" ] || fail "collab-mcp binary not installed"
[ -x "$home/.cargo/bin/collab-desktop-mcp" ] ||
  fail "collab-desktop-mcp not installed"

if grep -q '^install-autostart' "$work/collab.log"; then
  fail "down-state install ran collab install-autostart"
fi
if [ -s "$work/launchctl.log" ]; then
  fail "down-state install invoked launchctl: $(cat "$work/launchctl.log")"
fi
[ ! -S "$home/.collab/server.sock" ] ||
  fail "installer created a daemon socket"

echo "install-global-collab: down-state install performs no daemon activation"
