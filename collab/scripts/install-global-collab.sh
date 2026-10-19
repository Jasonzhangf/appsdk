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
trap 'rm -f "$collab_tmp" "$mcp_tmp"' EXIT HUP INT TERM

cp "$candidate_collab" "$collab_tmp"
cp "$candidate_mcp" "$mcp_tmp"
chmod 755 "$collab_tmp" "$mcp_tmp"
mv "$collab_tmp" "$bin_dir/collab"
mv "$mcp_tmp" "$bin_dir/collab-mcp"
trap - EXIT HUP INT TERM

"$bin_dir/collab" install-skills \
  --target "$HOME/.agents/skills/collab" \
  --force

printf 'version=%s\n' "$("$bin_dir/collab" --version)"
