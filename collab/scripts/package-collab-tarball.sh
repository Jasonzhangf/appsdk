#!/bin/sh
# Build a Collab release tarball whose installer installs binaries and Skills.
#
# The tarball carries prebuilt binaries, the Collab Skill bundle and an
# install.sh with the same installation steps as install-global-collab.sh.
# Daemon activation belongs to `collab up`.
set -eu

script_dir=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
collab_root=$(CDPATH= cd -- "$script_dir/.." && pwd)
repo_root=$(CDPATH= cd -- "$collab_root/.." && pwd)

usage() {
  cat <<'USAGE'
Usage: package-collab-tarball.sh [--skip-build] [--out DIR] [--bin-dir DIR]

  --skip-build   Reuse already built binaries instead of running build-collab.sh
  --out DIR      Output directory (default: $COLLAB_RELEASE_DIR or <repo>/release-artifacts)
  --bin-dir DIR  Directory holding collab and collab-mcp
                 (default: $COLLAB_BIN_DIR or <collab>/target/release)
USAGE
}

fail() {
  printf 'package-collab-tarball: %s\n' "$*" >&2
  exit 1
}

build=1
out_dir=""
bin_dir=""
while [ $# -gt 0 ]; do
  case "$1" in
    --skip-build) build=0 ;;
    --out)
      shift
      [ $# -gt 0 ] || fail '--out requires a directory'
      out_dir=$1
      ;;
    --bin-dir)
      shift
      [ $# -gt 0 ] || fail '--bin-dir requires a directory'
      bin_dir=$1
      ;;
    -h|--help)
      usage
      exit 0
      ;;
    *) fail "unknown argument: $1" ;;
  esac
  shift
done

os=$(uname -s)
arch=$(uname -m)
case "$os-$arch" in
  Darwin-arm64) platform=macos-arm64 ;;
  Darwin-x86_64) platform=macos-x86_64 ;;
  *)
    fail "unsupported platform $os-$arch (supported: Darwin-arm64, Darwin-x86_64)"
    ;;
esac

[ -n "$bin_dir" ] || bin_dir=${COLLAB_BIN_DIR:-"$collab_root/target/release"}
[ -n "$out_dir" ] || out_dir=${COLLAB_RELEASE_DIR:-"$repo_root/release-artifacts"}

if [ "$build" -eq 1 ]; then
  "$collab_root/scripts/build-collab.sh"
fi

[ -x "$bin_dir/collab" ] || fail "missing executable $bin_dir/collab"
[ -x "$bin_dir/collab-mcp" ] || fail "missing executable $bin_dir/collab-mcp"

version=$("$bin_dir/collab" --version | awk '{print $2}')
case "$version" in
  0.2.[0-9]*) ;;
  *) fail "unexpected collab version: $version" ;;
esac

package_name="collab-$version-$platform"
mkdir -p "$out_dir"
out_dir=$(CDPATH= cd -- "$out_dir" && pwd)
stage="$out_dir/$package_name"
[ ! -e "$stage" ] || fail "$stage already exists; remove it before repackaging"
[ ! -e "$stage.tar.gz" ] || fail "$stage.tar.gz already exists; remove it before repackaging"

mkdir -p "$stage/bin" "$stage/skills"
cp "$bin_dir/collab" "$stage/bin/collab"
cp "$bin_dir/collab-mcp" "$stage/bin/collab-mcp"
for tool in collab-desktop-mcp collab-desktop-mcp.mjs; do
  source_file="$collab_root/scripts/$tool"
  [ -f "$source_file" ] || fail "missing $source_file"
  cp "$source_file" "$stage/bin/$tool"
done
chmod 755 "$stage/bin/collab" "$stage/bin/collab-mcp" "$stage/bin/collab-desktop-mcp"
chmod 644 "$stage/bin/collab-desktop-mcp.mjs"

# Export the Skill bundle from the packaged binary itself, so the archive and
# the installed Skills come from the same build even with --bin-dir.
"$stage/bin/collab" install-skills --target "$stage/skills/collab" --force >/dev/null
[ -s "$stage/skills/collab/SKILL.md" ] || fail 'Skill bundle is incomplete: SKILL.md missing'
[ -s "$collab_root/docs/install-onboarding.md" ] ||
  fail 'missing docs/install-onboarding.md'
cp "$collab_root/docs/install-onboarding.md" "$stage/INSTALL-ONBOARDING.md"
cp "$collab_root/scripts/require-system-volume.sh" "$stage/require-system-volume.sh"
cp "$collab_root/scripts/collab-install-transaction.sh" \
  "$stage/collab-install-transaction.sh"

cat >"$stage/install.sh" <<'INSTALL'
#!/bin/sh
# Install the packaged Collab binaries and Skill bundle.
set -eu

here=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
. "$here/require-system-volume.sh"
. "$here/collab-install-transaction.sh"
cargo_home=${CARGO_HOME:-"$HOME/.cargo"}
bin_dir="$cargo_home/bin"
skills_target=${COLLAB_SKILLS_TARGET:-"$HOME/.agents/skills/collab"}

require_system_volume "$bin_dir"
require_system_volume "$skills_target"

tx_main \
  --bin-dir "$bin_dir" \
  --skills-dir "$skills_target" \
  --helper-dir "$here/bin" \
  --candidate-dir "$here/bin"
INSTALL
chmod 755 "$stage/install.sh"

{
  printf '# Collab %s %s\n\n' "$version" "$platform"
  printf 'Artifact:\n'
  cat <<'README'
- bin/collab
- bin/collab-mcp
- bin/collab-desktop-mcp
- bin/collab-desktop-mcp.mjs
- skills/collab (Skill bundle exported with the binaries)
- INSTALL-ONBOARDING.md (first-install permissions, Git, tmux and QQBot notes)

README
  printf 'Install without compiling:\n\n'
  printf '```sh\ntar -xzf %s.tar.gz\ncd %s\n./install.sh\n```\n\n' \
    "$package_name" "$package_name"
  cat <<'README'
Install targets:
- `$CARGO_HOME/bin/collab` (default `$HOME/.cargo/bin`)
- `$CARGO_HOME/bin/collab-mcp`
- `$CARGO_HOME/bin/collab-desktop-mcp` and `collab-desktop-mcp.mjs`
- `$HOME/.agents/skills/collab`

On macOS the installer refuses an install target that resolves to an external
volume and stops before writing.

Install does not start or restart a daemon and does not touch tmux. After
install, run `collab up` from the project root. It writes the service record for
that project before loading the login LaunchAgent. A daemon started before the
install keeps the old binary until the next user-managed maintenance window.

First-install setup:
- do not run Codex with YOLO or danger-full-access;
- configure the named permission profile described in INSTALL-ONBOARDING.md;
- Collab does not require a Git repository;
- run `collab context` from the real Codex TUI for first registration;
- bind QQBot with one private message inside its ten-minute receiver window.
README
} >"$stage/README-INSTALL.md"

tar -C "$out_dir" -czf "$out_dir/$package_name.tar.gz" "$package_name"

printf 'tarball=%s\n' "$out_dir/$package_name.tar.gz"
printf 'package=%s\n' "$stage"
printf 'version=%s\n' "$version"
printf 'platform=%s\n' "$platform"
