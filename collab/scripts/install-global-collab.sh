#!/bin/sh
set -eu

repo_root=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
. "$repo_root/scripts/require-system-volume.sh"
. "$repo_root/scripts/collab-install-transaction.sh"
cargo_home=${CARGO_HOME:-"$HOME/.cargo"}
bin_dir="$cargo_home/bin"
skills_target=${COLLAB_SKILLS_TARGET:-"$HOME/.agents/skills/collab"}

require_system_volume "$bin_dir"
require_system_volume "$skills_target"

# Install installs binaries, MCP helper and Skills only. The daemon is not
# loaded or started here; activation belongs to `collab up`, which writes the
# service record for the current project before loading the login agent.

tx_main \
  --bin-dir "$bin_dir" \
  --skills-dir "$skills_target" \
  --helper-dir "$repo_root/scripts" \
  --candidate-dir "$repo_root/target/release" \
  --build-script "$repo_root/scripts/build-collab.sh"
