#!/usr/bin/env bash
set -euo pipefail

script_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd -P)"
repo_root="$(cd "$script_dir/.." && pwd -P)"
restart_daemon=false

case "${1:-}" in
  --help)
    cat <<'USAGE'
Usage: scripts/install-all.sh [--restart]

Builds and installs all three workspace products (dagpipe, appsdk + project-memory,
and collab + collab-mcp) along with their Skills, then verifies every binary and
Skill directory. Pass --restart to restart the supervised collab daemon if needed.
USAGE
    exit 0
    ;;
  --restart)
    restart_daemon=true
    ;;
  "")
    ;;
  *)
    echo "error: unknown option '$1' (expected --restart or --help)" >&2
    exit 2
    ;;
esac

echo "[1/4] Installing dagpipe..."
"$repo_root/scripts/install-global-dagpipe.sh"

echo "[2/4] Installing appsdk, project-memory, and SDK skills..."
"$repo_root/scripts/install-global-appsdk.sh"

echo "[3/4] Installing collab, collab-mcp, and collab skill..."
collab_rc=0
"$repo_root/scripts/install-global-collab.sh" || collab_rc=$?
if [[ "$collab_rc" -ne 0 && "$collab_rc" -ne 3 ]]; then
  echo "error: install-global-collab.sh failed with exit $collab_rc" >&2
  exit "$collab_rc"
fi

if [[ "$restart_daemon" == true ]]; then
  echo "Restarting supervised collab daemon..."
  collab up --restart
elif [[ "$collab_rc" -eq 3 ]]; then
  echo "note: collab binaries updated; run 'collab up --restart' to activate new daemon"
fi

echo "[4/4] Verifying installed binaries and skills..."
cargo_bin_dir="$(dirname "$(command -v cargo)")"
for bin in dagpipe appsdk project-memory collab collab-mcp; do
  if [[ ! -x "$cargo_bin_dir/$bin" ]]; then
    echo "error: missing installed binary $cargo_bin_dir/$bin" >&2
    exit 1
  fi
done

skills_root="${HOME:?HOME is required}/.agents/skills"
for skill in dagpipe-runtime appsdk-project-governance appsdk-migration project-memory collab; do
  if [[ ! -f "$skills_root/$skill/SKILL.md" ]]; then
    echo "error: missing installed skill $skills_root/$skill/SKILL.md" >&2
    exit 1
  fi
done

if command -v zterm >/dev/null 2>&1; then
  echo "external dependency: zterm found at $(command -v zterm)"
else
  echo "external dependency: zterm not found (optional cross-machine transport unavailable)"
fi

echo "OK: all binaries and skills installed and verified."
