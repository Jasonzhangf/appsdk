#!/bin/sh
set -eu

repo_dir=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
user_home=${HOME:-${USERPROFILE:-}}
if [ -z "$user_home" ]; then
    echo "cannot determine the user home directory" >&2
    exit 1
fi
skill_source="$repo_dir/.agents/skills/dagpipe-runtime/SKILL.md"
skill_target="$user_home/.agents/skills/dagpipe-runtime/SKILL.md"
sdk_target="$user_home/.local/share/dagpipe/sdk"
if [ -L "$sdk_target" ]; then
    echo "SDK path is a symlink at $sdk_target; refusing to follow it" >&2
    exit 1
fi
if [ -e "$sdk_target" ]; then
    if [ ! -d "$sdk_target" ] || [ ! -f "$sdk_target/Cargo.toml" ] || [ ! -d "$sdk_target/src" ]; then
        echo "existing SDK path is not a DAGpipe SDK directory at $sdk_target; refusing to overwrite it" >&2
        exit 1
    fi
    existing_metadata=$(cargo metadata --no-deps --format-version 1 --manifest-path "$sdk_target/Cargo.toml")
    case "$existing_metadata" in
        *'"name":"pipeline_runtime"'*'"repository":"https://github.com/Jasonzhangf/DAGpipe"'*) ;;
        *'"name":"pipeline_runtime"'*'"repository":"https://github.com/Jasonzhangf/appsdk"'*) ;;
        *)
            echo "existing SDK manifest is not pipeline_runtime at $sdk_target; refusing to overwrite it" >&2
            exit 1
            ;;
    esac
    if [ -L "$sdk_target/Cargo.toml" ] || [ -L "$sdk_target/src" ] || [ -L "$sdk_target/.agents" ]; then
        echo "existing SDK contains a symlinked copy target at $sdk_target; refusing to follow it" >&2
        exit 1
    fi
    if { [ -d "$sdk_target/src" ] && find "$sdk_target/src" -type l -print -quit | grep -q .; } || \
        { [ -d "$sdk_target/.agents" ] && find "$sdk_target/.agents" -type l -print -quit | grep -q .; }; then
        echo "existing SDK contains a symlink in a copy tree at $sdk_target; refusing to follow it" >&2
        exit 1
    fi
fi
if [ -L "$skill_target" ]; then
    echo "skill path is a symlink at $skill_target; refusing to follow it" >&2
    exit 1
fi
if [ -e "$skill_target" ]; then
    if ! cmp -s "$skill_source" "$skill_target"; then
        installed_name=$(head -n 2 "$skill_target" | tail -n 1 || true)
        if [ "$installed_name" != "name: dagpipe-runtime" ]; then
            echo "existing Skill at $skill_target is not DAGpipe Runtime; refusing overwrite" >&2
            exit 1
        fi
    fi
fi
cargo_install_root=${CARGO_INSTALL_ROOT:-${CARGO_HOME:-"$user_home/.cargo"}}
sdk_parent=$(dirname -- "$sdk_target")
mkdir -p "$sdk_parent"
sdk_stage=$(mktemp -d "$sdk_parent/.sdk-stage.XXXXXX")
sdk_inventory=$(mktemp "$sdk_parent/.sdk-inventory.XXXXXX")
sdk_expected=$(mktemp "$sdk_parent/.sdk-expected.XXXXXX")
sdk_tree=$(mktemp "$sdk_parent/.sdk-tree.XXXXXX")
cleanup_stage() {
    if [ -n "$sdk_stage" ] && [ -d "$sdk_stage" ]; then
        rm -rf -- "$sdk_stage"
    fi
    rm -f -- "$sdk_inventory" "$sdk_expected" "$sdk_tree"
}
trap cleanup_stage EXIT HUP INT TERM
mkdir -p "$sdk_stage/.agents/skills/dagpipe-runtime"
cp "$repo_dir/Cargo.toml" "$sdk_stage/Cargo.toml"
cp -R "$repo_dir/src" "$sdk_stage/src"
cp "$skill_source" "$sdk_stage/.agents/skills/dagpipe-runtime/SKILL.md"
cmp -s "$repo_dir/Cargo.toml" "$sdk_stage/Cargo.toml"
diff -qr "$repo_dir/src" "$sdk_stage/src"
cmp -s "$skill_source" "$sdk_stage/.agents/skills/dagpipe-runtime/SKILL.md"
(cd "$sdk_stage" && find . -type f ! -path './.installed-files' ! -path './.installed-tree' -exec shasum -a 256 {} + | LC_ALL=C sort) > "$sdk_stage/.installed-files"
(cd "$sdk_stage" && find . ! -path './.installed-files' ! -path './.installed-tree' -print | LC_ALL=C sort) > "$sdk_stage/.installed-tree"
if [ -e "$sdk_target" ]; then
    if find "$sdk_target" ! -type f ! -type d -print -quit | grep -q .; then
        echo "existing SDK contains a symlink or special entry; refusing to replace it" >&2
        exit 1
    fi
    if { [ -e "$sdk_target/.installed-files" ] && [ ! -f "$sdk_target/.installed-files" ]; } ||
       { [ -e "$sdk_target/.installed-tree" ] && [ ! -f "$sdk_target/.installed-tree" ]; }; then
        echo "existing SDK install manifest has the wrong type; refusing to replace it" >&2
        exit 1
    fi
    (cd "$sdk_target" && find . ! -path './.installed-files' ! -path './.installed-tree' -print | LC_ALL=C sort) > "$sdk_tree"
    if [ -f "$sdk_target/.installed-files" ]; then
        (cd "$sdk_target" && find . -type f ! -path './.installed-files' ! -path './.installed-tree' -exec shasum -a 256 {} + | LC_ALL=C sort) > "$sdk_inventory"
        if ! cmp -s "$sdk_target/.installed-files" "$sdk_inventory"; then
            echo "existing SDK differs from the last installed file manifest; refusing to replace it" >&2
            exit 1
        fi
        if [ -f "$sdk_target/.installed-tree" ]; then
            tree_reference="$sdk_target/.installed-tree"
        else
            tree_reference="$sdk_stage/.installed-tree"
        fi
        if ! cmp -s "$tree_reference" "$sdk_tree"; then
            echo "existing SDK tree differs from the last installed tree; refusing to replace it" >&2
            exit 1
        fi
    else
        (cd "$sdk_target" && find . -type f | LC_ALL=C sort) > "$sdk_inventory"
        (cd "$sdk_stage" && find . -type f ! -path './.installed-files' ! -path './.installed-tree' | LC_ALL=C sort) > "$sdk_expected"
        if ! cmp -s "$sdk_expected" "$sdk_inventory" || ! cmp -s "$sdk_stage/.installed-tree" "$sdk_tree" || ! diff -qr "$repo_dir/src" "$sdk_target/src"; then
            echo "legacy SDK contains extra or modified source; refusing to replace it" >&2
            exit 1
        fi
    fi
fi
cargo install --path "$repo_dir" --locked --force --root "$cargo_install_root"
"$cargo_install_root/bin/dagpipe" skill install
sdk_previous=''
if [ -e "$sdk_target" ]; then
    sdk_previous=$(mktemp -d "$sdk_parent/.sdk-previous.XXXXXX")
    rmdir -- "$sdk_previous"
    mv -- "$sdk_target" "$sdk_previous"
fi
if ! mv -- "$sdk_stage" "$sdk_target"; then
    if [ -n "$sdk_previous" ]; then
        mv -- "$sdk_previous" "$sdk_target"
    fi
    exit 1
fi
sdk_stage=''
if [ -n "$sdk_previous" ]; then
    rm -rf -- "$sdk_previous"
fi
