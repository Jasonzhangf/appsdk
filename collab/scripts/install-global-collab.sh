#!/bin/sh
set -eu

repo_root=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
cargo_home=${CARGO_HOME:-"$HOME/.cargo"}
bin_dir="$cargo_home/bin"

cd "$repo_root"
"$repo_root/scripts/build-collab.sh"

candidate_collab="$repo_root/target/release/collab"
candidate_mcp="$repo_root/target/release/collab-mcp"
test -x "$candidate_collab"
test -x "$candidate_mcp"

mkdir -p "$bin_dir"
collab_tmp="$bin_dir/.collab.install.$$"
mcp_tmp="$bin_dir/.collab-mcp.install.$$"
desktop_tmp="$bin_dir/.collab-desktop-mcp.install.$$"
desktop_js_tmp="$bin_dir/.collab-desktop-mcp.mjs.install.$$"
trap 'rm -f "$collab_tmp" "$mcp_tmp" "$desktop_tmp" "$desktop_js_tmp"' EXIT HUP INT TERM

cp "$candidate_collab" "$collab_tmp"
cp "$candidate_mcp" "$mcp_tmp"
cp "$repo_root/scripts/collab-desktop-mcp" "$desktop_tmp"
cp "$repo_root/scripts/collab-desktop-mcp.mjs" "$desktop_js_tmp"
chmod 755 "$collab_tmp" "$mcp_tmp" "$desktop_tmp"
mv "$collab_tmp" "$bin_dir/collab"
mv "$mcp_tmp" "$bin_dir/collab-mcp"
mv "$desktop_js_tmp" "$bin_dir/collab-desktop-mcp.mjs"
mv "$desktop_tmp" "$bin_dir/collab-desktop-mcp"
trap - EXIT HUP INT TERM

"$bin_dir/collab" install-skills \
  --target "$HOME/.agents/skills/collab" \
  --force

# Install and load the per-user LaunchAgent so the daemon autostarts on login.
# On macOS this is a required part of the delivery, so a launchd failure must
# fail the install instead of leaving the binary installed without autostart.
case "$(uname -s)" in
  Darwin)
    "$bin_dir/collab" install-autostart
    ;;
  *)
    printf 'autostart=skipped: launchd is macOS-only (uname -s = %s)\n' "$(uname -s)" >&2
    ;;
esac

printf 'version=%s\n' "$("$bin_dir/collab" --version)"
