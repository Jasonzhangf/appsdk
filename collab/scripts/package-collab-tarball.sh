#!/bin/sh
# Build a Collab release tarball whose installer enables daemon autostart.
#
# The tarball carries prebuilt binaries, the Collab Skill bundle and an
# install.sh with the same delivery steps as install-global-collab.sh, so a
# machine without a checkout gets login autostart instead of binaries only.
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

cp -R "$collab_root/skills/collab" "$stage/skills/collab"
[ -s "$stage/skills/collab/SKILL.md" ] || fail 'Skill bundle is incomplete: SKILL.md missing'

cat >"$stage/install.sh" <<'INSTALL'
#!/bin/sh
# Install the packaged Collab binaries, Skill bundle and login autostart.
set -eu

here=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
cargo_home=${CARGO_HOME:-"$HOME/.cargo"}
bin_dir="$cargo_home/bin"
skills_target=${COLLAB_SKILLS_TARGET:-"$HOME/.agents/skills/collab"}

test -x "$here/bin/collab"
test -x "$here/bin/collab-mcp"
test -x "$here/bin/collab-desktop-mcp"

mkdir -p "$bin_dir"
collab_tmp="$bin_dir/.collab.install.$$"
mcp_tmp="$bin_dir/.collab-mcp.install.$$"
desktop_tmp="$bin_dir/.collab-desktop-mcp.install.$$"
desktop_js_tmp="$bin_dir/.collab-desktop-mcp.mjs.install.$$"
trap 'rm -f "$collab_tmp" "$mcp_tmp" "$desktop_tmp" "$desktop_js_tmp"' EXIT HUP INT TERM

cp "$here/bin/collab" "$collab_tmp"
cp "$here/bin/collab-mcp" "$mcp_tmp"
cp "$here/bin/collab-desktop-mcp" "$desktop_tmp"
cp "$here/bin/collab-desktop-mcp.mjs" "$desktop_js_tmp"
chmod 755 "$collab_tmp" "$mcp_tmp" "$desktop_tmp"
mv "$collab_tmp" "$bin_dir/collab"
mv "$mcp_tmp" "$bin_dir/collab-mcp"
mv "$desktop_js_tmp" "$bin_dir/collab-desktop-mcp.mjs"
mv "$desktop_tmp" "$bin_dir/collab-desktop-mcp"
trap - EXIT HUP INT TERM

"$bin_dir/collab" install-skills --target "$skills_target" --force

# Login autostart is a required part of the delivery on macOS, so a launchd
# failure must fail the install instead of leaving binaries without autostart.
case "$(uname -s)" in
  Darwin)
    "$bin_dir/collab" install-autostart
    ;;
  *)
    printf 'autostart=skipped: launchd is macOS-only (uname -s = %s)\n' "$(uname -s)" >&2
    ;;
esac

printf 'version=%s\n' "$("$bin_dir/collab" --version)"
printf 'collab=%s\n' "$bin_dir/collab"
printf 'collab_mcp=%s\n' "$bin_dir/collab-mcp"
printf 'skills=%s\n' "$skills_target"
printf 'daemon_note=install does not restart a running daemon; switch binaries in a user-managed maintenance window\n'
INSTALL
chmod 755 "$stage/install.sh"

source_revision=$(git -C "$repo_root" rev-parse --short HEAD)
{
  printf '# Collab %s %s\n\n' "$version" "$platform"
  printf 'Source revision: %s\n\n' "$source_revision"
  printf 'Artifact:\n'
  cat <<'README'
- bin/collab
- bin/collab-mcp
- bin/collab-desktop-mcp
- bin/collab-desktop-mcp.mjs
- skills/collab (Skill bundle exported with the binaries)

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
- launchd LaunchAgent `com.appsdk.collab` via `collab install-autostart`

Install does not restart a running daemon and does not touch tmux. A daemon
started before the install keeps the old binary until the user-managed
maintenance window loads the LaunchAgent.
README
} >"$stage/README-INSTALL.md"

tar -C "$out_dir" -czf "$out_dir/$package_name.tar.gz" "$package_name"

printf 'tarball=%s\n' "$out_dir/$package_name.tar.gz"
printf 'package=%s\n' "$stage"
printf 'version=%s\n' "$version"
printf 'platform=%s\n' "$platform"
printf 'source_revision=%s\n' "$source_revision"
