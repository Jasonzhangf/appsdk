require_system_volume() {
  target=$1
  if [ "$(uname -s)" != Darwin ]; then
    printf 'system volume location is not verified on this platform\n' >&2
    return 0
  fi

  system_dev=$(stat -f '%d' /System/Volumes/Data) || {
    printf 'error: cannot read system data volume\n' >&2
    return 1
  }
  existing=$target
  while [ ! -e "$existing" ] && [ "$existing" != / ]; do
    existing=$(dirname "$existing")
  done
  physical=$(CDPATH= cd -P "$existing" 2>/dev/null && pwd -P) || {
    printf 'error: cannot resolve install target %s\n' "$target" >&2
    return 1
  }
  target_dev=$(stat -f '%d' "$physical") || return 1
  if [ "$target_dev" != "$system_dev" ]; then
    printf 'error: %s resolves to an external volume; refusing to install Collab there\n' "$target" >&2
    return 1
  fi
}
