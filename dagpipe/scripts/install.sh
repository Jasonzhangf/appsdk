#!/bin/sh
set -eu

repo_dir=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
candidate_root=$(mktemp -d "${TMPDIR:-/tmp}/dagpipe-install.XXXXXX")
cleanup_candidate() {
    rm -rf -- "$candidate_root"
}
trap cleanup_candidate EXIT HUP INT TERM

cargo install --path "$repo_dir" --locked --force --root "$candidate_root"
"$candidate_root/bin/dagpipe" install --source "$repo_dir"
