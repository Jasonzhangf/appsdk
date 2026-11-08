#!/bin/sh
# Publish one complete Collab release and compensate a handled failure.
#
# This file is the single publication/compensation owner for both the source
# installer and the installer embedded in the release tarball.

COLLAB_INSTALL_PREVIOUS=.collab-install-previous
COLLAB_INSTALL_LOCK=.collab-install.lock
COLLAB_INSTALL_RECOVERY=.collab-install-recovery
COLLAB_INSTALL_STAGE=.collab-install-stage

TX_BIN=
TX_SKILLS=
TX_HELPER_DIR=
TX_CANDIDATE=
TX_BUILD_SCRIPT=
TX_CWD=
TX_SCRATCH=
TX_LOCK=
TX_PREVIOUS=
TX_PUBLISHED=
TX_FINISHED=
TX_LOCK_OWNED=
TX_ROLLBACK=not_needed
TX_ROLLBACK_ERRORS=
TX_BASELINE=unverified
TX_BASELINE_REASON=not_classified
TX_FAILED_PHASE=none
TX_ORIGINAL_ERROR=none
TX_ORIGINAL_EXIT=0
TX_OUTCOME=
TX_EXIT=0
TX_REQUIRED=none
TX_VERSION=unknown
TX_PREVIOUS_RELEASE=none
TX_PREVIOUS_QUALITY=none
TX_LAST_ERROR=
TX_ROTATION_INCOMPLETE=
tx_capture_index=
tx_capture_source=
tx_capture_destination=
tx_capture_type=
tx_capture_mode=
tx_capture_target=
tx_restore_index=
tx_restore_destination=
tx_restore_source=
tx_restore_type=
tx_restore_mode=
tx_restore_target=
tx_restore_temp=
tx_rotation=
tx_old_previous=
tx_rotate_index=
tx_rotate_type=
tx_rotate_source=
tx_rotate_destination=
tx_descriptor_get_value=

tx_error() {
  TX_LAST_ERROR=$*
  printf 'collab-install: %s\n' "$*" >&2
}

tx_usage() {
  cat <<'USAGE'
Usage: install-global-collab.sh [options]
       install.sh [options]

  --bin-dir DIR       Install binaries into DIR
  --skills-dir DIR    Install the Collab Skill bundle into DIR
  --helper-dir DIR    Directory holding collab-desktop-mcp and its .mjs
  --candidate-dir DIR Directory holding the candidate collab and collab-mcp
  --build-script FILE Build the candidate before publication
USAGE
}

tx_parse() {
  while [ $# -gt 0 ]; do
    case $1 in
      --bin-dir)
        shift; [ $# -gt 0 ] || { tx_usage >&2; exit 64; }
        TX_BIN=$1
        ;;
      --skills-dir)
        shift; [ $# -gt 0 ] || { tx_usage >&2; exit 64; }
        TX_SKILLS=$1
        ;;
      --helper-dir)
        shift; [ $# -gt 0 ] || { tx_usage >&2; exit 64; }
        TX_HELPER_DIR=$1
        ;;
      --candidate-dir)
        shift; [ $# -gt 0 ] || { tx_usage >&2; exit 64; }
        TX_CANDIDATE=$1
        ;;
      --build-script)
        shift; [ $# -gt 0 ] || { tx_usage >&2; exit 64; }
        TX_BUILD_SCRIPT=$1
        ;;
      -h|--help)
        tx_usage
        exit 0
        ;;
      *)
        tx_error "unknown argument: $1"
        tx_usage >&2
        exit 64
        ;;
    esac
    shift
  done
  [ -n "$TX_BIN" ] && [ -n "$TX_SKILLS" ] && [ -n "$TX_HELPER_DIR" ] &&
    [ -n "$TX_CANDIDATE" ] || {
    tx_error "missing required installation path"
    tx_usage >&2
    exit 64
  }
}

tx_mode() {
  stat -f '%Lp' "$1" 2>/dev/null || stat -c '%a' "$1"
}

tx_first_line() {
  awk 'NF { print; exit }' "$1" 2>/dev/null
}

tx_post_check_required_action() {
  tx_post_check_required_action_value=$(
    sed -n 's/.*"required_action"[[:space:]]*:[[:space:]]*"\([^"]*\)".*/\1/p' \
      "$TX_SCRATCH/post-check.out" | head -1
  )
  case $tx_post_check_required_action_value in
    ''|*[!A-Za-z0-9_.-]*) return 1 ;;
  esac
  printf '%s\n' "$tx_post_check_required_action_value"
}

tx_emit_post_check_error() {
  [ -s "$TX_SCRATCH/post-check.err" ] || return 0
  sed 's/^/collab-install: /' "$TX_SCRATCH/post-check.err" >&2
}

tx_name() {
  case $1 in
    1) printf '%s\n' collab ;;
    2) printf '%s\n' collab-mcp ;;
    3) printf '%s\n' collab-desktop-mcp ;;
    4) printf '%s\n' collab-desktop-mcp.mjs ;;
    5) printf '%s\n' skills/collab ;;
  esac
}

tx_entry() {
  case $1 in
    1) printf '%s\n' "$TX_BIN/collab" ;;
    2) printf '%s\n' "$TX_BIN/collab-mcp" ;;
    3) printf '%s\n' "$TX_BIN/collab-desktop-mcp" ;;
    4) printf '%s\n' "$TX_BIN/collab-desktop-mcp.mjs" ;;
    5) printf '%s\n' "$TX_SKILLS" ;;
  esac
}

tx_staged() {
  case $1 in
    1) printf '%s\n' "$TX_SCRATCH/stage/collab" ;;
    2) printf '%s\n' "$TX_SCRATCH/stage/collab-mcp" ;;
    3) printf '%s\n' "$TX_SCRATCH/stage/collab-desktop-mcp" ;;
    4) printf '%s\n' "$TX_SCRATCH/stage/collab-desktop-mcp.mjs" ;;
    5) printf '%s\n' "$TX_SCRATCH/stage/skills/collab" ;;
  esac
}

tx_captured() {
  case $1 in
    1) printf '%s\n' "$TX_SCRATCH/capture/collab" ;;
    2) printf '%s\n' "$TX_SCRATCH/capture/collab-mcp" ;;
    3) printf '%s\n' "$TX_SCRATCH/capture/collab-desktop-mcp" ;;
    4) printf '%s\n' "$TX_SCRATCH/capture/collab-desktop-mcp.mjs" ;;
    5) printf '%s\n' "$TX_SCRATCH/capture/skills/collab" ;;
  esac
}

tx_previous_entry() {
  case $1 in
    1) printf '%s\n' "$TX_PREVIOUS/bin/collab" ;;
    2) printf '%s\n' "$TX_PREVIOUS/bin/collab-mcp" ;;
    3) printf '%s\n' "$TX_PREVIOUS/bin/collab-desktop-mcp" ;;
    4) printf '%s\n' "$TX_PREVIOUS/bin/collab-desktop-mcp.mjs" ;;
    5) printf '%s\n' "$TX_PREVIOUS/bin/skills/collab" ;;
  esac
}

tx_rotation_entry() {
  case $1 in
    1) printf '%s\n' "$tx_rotation/bin/collab" ;;
    2) printf '%s\n' "$tx_rotation/bin/collab-mcp" ;;
    3) printf '%s\n' "$tx_rotation/bin/collab-desktop-mcp" ;;
    4) printf '%s\n' "$tx_rotation/bin/collab-desktop-mcp.mjs" ;;
    5) printf '%s\n' "$tx_rotation/bin/skills/collab" ;;
  esac
}

tx_descriptor_path() {
  printf '%s\n' "$1/descriptor"
}

tx_descriptor_get() {
  tx_descriptor_get_root=$1
  tx_descriptor_get_index=$2
  tx_descriptor_get_field=$3
  case $tx_descriptor_get_field in
    type) tx_descriptor_get_line=$((tx_descriptor_get_index * 4 - 2)) ;;
    mode) tx_descriptor_get_line=$((tx_descriptor_get_index * 4 - 1)) ;;
    target) tx_descriptor_get_line=$((tx_descriptor_get_index * 4)) ;;
    *) return 1 ;;
  esac
  tx_descriptor_get_value=$(sed -n "${tx_descriptor_get_line}p" \
    "$(tx_descriptor_path "$tx_descriptor_get_root")")
  printf '%s\n' "${tx_descriptor_get_value#*=}"
}

tx_is_regular() {
  [ -f "$1" ] && [ ! -L "$1" ]
}

tx_require_regular() {
  tx_require_path=$1
  tx_require_label=$2
  if [ -L "$tx_require_path" ] || [ ! -f "$tx_require_path" ]; then
    tx_error "$tx_require_label is not a regular file: $tx_require_path"
    return 1
  fi
}

tx_copy_regular() {
  tx_copy_source=$1
  tx_copy_destination=$2
  tx_copy_label=$3
  tx_require_regular "$tx_copy_source" "$tx_copy_label" || return
  cp -p "$tx_copy_source" "$tx_copy_destination" || {
    tx_error "cannot copy $tx_copy_label from $tx_copy_source"
    return 1
  }
}

tx_replace() {
  tx_replace_temp=$1
  tx_replace_destination=$2
  tx_replace_backup="$(dirname "$tx_replace_destination")/.collab-replace-old.$$"
  rm -rf "$tx_replace_backup"
  # A physical directory or a symlink that resolves to one must be swapped out
  # as a path object. A plain mv would follow a directory symlink and move the
  # staged object into its referent instead of replacing the link.
  if [ -d "$tx_replace_destination" ]; then
    mv "$tx_replace_destination" "$tx_replace_backup" || return 1
    if ! mv "$tx_replace_temp" "$tx_replace_destination"; then
      mv "$tx_replace_backup" "$tx_replace_destination"
      return 1
    fi
    rm -rf "$tx_replace_backup"
  else
    mv "$tx_replace_temp" "$tx_replace_destination"
  fi
}

tx_bounded() {
  tx_bounded_seconds=$1
  tx_bounded_out=$2
  tx_bounded_err=$3
  shift 3
  tx_bounded_marker="${TMPDIR:-/tmp}/.collab-bounded.$$"
  rm -f "$tx_bounded_marker"
  "$@" >"$tx_bounded_out" 2>"$tx_bounded_err" &
  tx_bounded_pid=$!
  (
    tx_bounded_ticks=$((tx_bounded_seconds * 20))
    tx_bounded_i=0
    while [ "$tx_bounded_i" -lt "$tx_bounded_ticks" ]; do
      kill -0 "$tx_bounded_pid" 2>/dev/null || exit 0
      sleep 0.05 2>/dev/null || sleep 1
      tx_bounded_i=$((tx_bounded_i + 1))
    done
    if kill -0 "$tx_bounded_pid" 2>/dev/null; then
      if kill -TERM "$tx_bounded_pid" 2>/dev/null; then
        : >"$tx_bounded_marker"
        tx_bounded_i=0
        while [ "$tx_bounded_i" -lt 40 ] && kill -0 "$tx_bounded_pid" 2>/dev/null; do
          sleep 0.05 2>/dev/null || sleep 1
          tx_bounded_i=$((tx_bounded_i + 1))
        done
        kill -KILL "$tx_bounded_pid" 2>/dev/null
      fi
    fi
  ) &
  tx_bounded_watchdog=$!
  wait "$tx_bounded_pid" 2>/dev/null
  tx_bounded_status=$?
  wait "$tx_bounded_watchdog" 2>/dev/null
  if [ -e "$tx_bounded_marker" ]; then
    rm -f "$tx_bounded_marker"
    return 124
  fi
  rm -f "$tx_bounded_marker"
  return "$tx_bounded_status"
}

tx_acquire_lock() {
  if [ -e "$TX_BIN/$COLLAB_INSTALL_RECOVERY" ]; then
    tx_error "unfinished installation recovery is pending at $TX_BIN/$COLLAB_INSTALL_RECOVERY"
    return 1
  fi
  mkdir -p "$TX_BIN" || return 1
  TX_LOCK=$TX_BIN/$COLLAB_INSTALL_LOCK
  if ! mkdir "$TX_LOCK" 2>/dev/null; then
    tx_error "unfinished or active installation holds $TX_LOCK"
    for tx_leftover in "$TX_BIN"/.collab-install.*; do
      [ -e "$tx_leftover" ] || continue
      tx_error "recovery material: $tx_leftover"
    done
    for tx_leftover in "$TX_BIN/$COLLAB_INSTALL_RECOVERY" \
      "$TX_BIN/$COLLAB_INSTALL_PREVIOUS"; do
      [ -e "$tx_leftover" ] || continue
      tx_error "recovery material: $tx_leftover"
    done
    return 1
  fi
  TX_LOCK_OWNED=1
  if [ -e "$TX_BIN/$COLLAB_INSTALL_RECOVERY" ]; then
    tx_error "unfinished installation recovery is pending at $TX_BIN/$COLLAB_INSTALL_RECOVERY"
    return 1
  fi
}

tx_release_lock() {
  [ -z "$TX_LOCK_OWNED" ] || rm -rf "$TX_LOCK"
  TX_LOCK_OWNED=
}

tx_capture_entry() {
  tx_capture_index=$1
  tx_capture_source=$(tx_entry "$tx_capture_index")
  tx_capture_destination=$(tx_captured "$tx_capture_index")
  tx_capture_type=absent
  tx_capture_mode=
  tx_capture_target=
  if [ -L "$tx_capture_source" ]; then
    tx_capture_type=symlink
    tx_capture_target=$(readlink "$tx_capture_source") || {
      tx_error "cannot read symlink $tx_capture_source"
      return 1
    }
    case $tx_capture_target in
      *"
"*)
        tx_error "symlink target contains newline: $tx_capture_source"
        return 1
        ;;
    esac
  elif [ -f "$tx_capture_source" ]; then
    tx_capture_type=file
    tx_capture_mode=$(tx_mode "$tx_capture_source") || return 1
    mkdir -p "$(dirname "$tx_capture_destination")" &&
      cp -p "$tx_capture_source" "$tx_capture_destination" || {
      tx_error "cannot capture $tx_capture_source"
      return 1
    }
  elif [ -d "$tx_capture_source" ]; then
    tx_capture_type=directory
    tx_capture_mode=$(tx_mode "$tx_capture_source") || return 1
    mkdir -p "$(dirname "$tx_capture_destination")" &&
      cp -Rp "$tx_capture_source" "$tx_capture_destination" || {
      tx_error "cannot capture $tx_capture_source"
      return 1
    }
  elif [ -e "$tx_capture_source" ]; then
    tx_error "unsupported entry type: $tx_capture_source"
    return 1
  fi
  {
    printf 'entry=%s\n' "$(tx_name "$tx_capture_index")"
    printf 'type=%s\n' "$tx_capture_type"
    printf 'mode=%s\n' "$tx_capture_mode"
    printf 'target=%s\n' "$tx_capture_target"
  } >>"$(tx_descriptor_path "$TX_SCRATCH")" || {
    tx_error "cannot write recovery descriptor"
    return 1
  }
}

tx_capture() {
  mkdir -p "$TX_SCRATCH/capture/skills" || return 1
  : >"$(tx_descriptor_path "$TX_SCRATCH")" || return 1
  tx_capture_index=1
  while [ "$tx_capture_index" -le 5 ]; do
    tx_capture_entry "$tx_capture_index" || return 1
    tx_capture_index=$((tx_capture_index + 1))
  done
  [ "$(wc -l <"$(tx_descriptor_path "$TX_SCRATCH")" | tr -d ' ')" -eq 20 ] || {
    tx_error "recovery descriptor is incomplete"
    return 1
  }
}

tx_version_from() {
  tx_version_binary=$1
  tx_version_out=$TX_SCRATCH/version.out
  tx_version_err=$TX_SCRATCH/version.err
  tx_version_command_status=0
  tx_bounded 5 "$tx_version_out" "$tx_version_err" \
    "$tx_version_binary" --version
  tx_version_command_status=$?
  [ "$tx_version_command_status" -eq 0 ] || return "$tx_version_command_status"
  tx_version_value=$(awk 'NR == 1 && $1 == "collab" { print $2 }' "$tx_version_out")
  tx_version_suffix=${tx_version_value#0.2.}
  if [ "$tx_version_value" = "${tx_version_value#0.2.}" ]; then
    return 1
  fi
  case $tx_version_suffix in
    ''|*[!0-9]*) return 1 ;;
  esac
  printf '%s\n' "$tx_version_value"
}

tx_mcp_version() {
  tx_mcp_binary=$1
  tx_mcp_request=$TX_SCRATCH/mcp-init.req
  tx_mcp_out=$TX_SCRATCH/mcp-init.out
  tx_mcp_err=$TX_SCRATCH/mcp-init.err
  tx_mcp_command_status=0
  printf '%s\n' '{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2024-11-05","capabilities":{},"clientInfo":{"name":"collab-installer","version":"1"}}}' \
    >"$tx_mcp_request"
  tx_mcp_command_status=$?
  [ "$tx_mcp_command_status" -eq 0 ] || return "$tx_mcp_command_status"
  tx_bounded 5 "$tx_mcp_out" "$tx_mcp_err" sh -c \
    'exec "$0" <"$1"' "$tx_mcp_binary" "$tx_mcp_request"
  tx_mcp_command_status=$?
  [ "$tx_mcp_command_status" -eq 0 ] || return "$tx_mcp_command_status"
  sed -n 's/.*"serverInfo".*"version":"\([^"]*\)".*/\1/p' "$tx_mcp_out" | head -1
}

tx_classify() {
  TX_BASELINE=unverified
  TX_BASELINE_REASON=
  tx_absent=0
  tx_classify_index=1
  while [ "$tx_classify_index" -le 5 ]; do
    tx_classify_type=$(tx_descriptor_get "$TX_SCRATCH" "$tx_classify_index" type) || return 1
    case $tx_classify_type in
      absent) tx_absent=$((tx_absent + 1)) ;;
      symlink)
        TX_BASELINE_REASON="symlink in publication set: $(tx_name "$tx_classify_index")"
        return 0
        ;;
    esac
    tx_classify_index=$((tx_classify_index + 1))
  done
  if [ "$tx_absent" -eq 5 ]; then
    TX_BASELINE=absent
    TX_BASELINE_REASON="all five publication entries absent"
    return 0
  fi
  tx_classify_cli=$(tx_captured 1)
  tx_classify_mcp=$(tx_captured 2)
  tx_classify_launcher=$(tx_captured 3)
  tx_classify_js=$(tx_captured 4)
  tx_classify_skills=$(tx_captured 5)
  if ! tx_is_regular "$tx_classify_cli"; then
    TX_BASELINE_REASON="baseline collab is not a regular file"
  elif ! tx_is_regular "$tx_classify_mcp"; then
    TX_BASELINE_REASON="baseline collab-mcp is not a regular file"
  elif [ ! -x "$tx_classify_cli" ] || [ ! -x "$tx_classify_mcp" ]; then
    TX_BASELINE_REASON="baseline binaries are not executable"
  fi
  if [ -z "$TX_BASELINE_REASON" ]; then
    tx_version_from "$tx_classify_cli" >"$TX_SCRATCH/version.value"
    tx_version_status=$?
    tx_old_version=$(cat "$TX_SCRATCH/version.value")
    if [ "$tx_version_status" -ne 0 ]; then
      if [ "$tx_version_command_status" -ne 0 ]; then
        TX_BASELINE_REASON="baseline collab version inspection failed with exit $tx_version_command_status: $(tx_first_line "$TX_SCRATCH/version.err")"
      else
        TX_BASELINE_REASON="baseline collab version is not a release: $(tx_first_line "$TX_SCRATCH/version.err")"
      fi
    elif [ "$tx_old_version" = 0.2.0000 ]; then
      TX_BASELINE_REASON="baseline collab version is the debug build 0.2.0000"
    fi
  fi
  if [ -z "$TX_BASELINE_REASON" ]; then
    tx_mcp_version "$tx_classify_mcp" >"$TX_SCRATCH/mcp-init.version"
    tx_mcp_status=$?
    tx_old_mcp_version=$(cat "$TX_SCRATCH/mcp-init.version")
    if [ "$tx_mcp_status" -ne 0 ]; then
      if [ "$tx_mcp_command_status" -ne 0 ]; then
        TX_BASELINE_REASON="baseline collab-mcp initialize failed with exit $tx_mcp_command_status: $(tx_first_line "$TX_SCRATCH/mcp-init.err")"
      else
        TX_BASELINE_REASON="baseline collab-mcp initialize did not report a version: $(tx_first_line "$TX_SCRATCH/mcp-init.err")"
      fi
    elif [ "$tx_old_mcp_version" != "$tx_old_version" ]; then
      TX_BASELINE_REASON="baseline CLI and MCP versions differ"
    fi
  fi
  if [ -z "$TX_BASELINE_REASON" ]; then
    [ -d "$tx_classify_skills" ] && [ ! -L "$tx_classify_skills" ] &&
      [ -s "$tx_classify_skills/SKILL.md" ] ||
      TX_BASELINE_REASON="baseline Skills are missing or incomplete"
  fi
  if [ -z "$TX_BASELINE_REASON" ]; then
    tx_launcher_type=$(tx_descriptor_get "$TX_SCRATCH" 3 type)
    tx_launcher_mode=$(tx_descriptor_get "$TX_SCRATCH" 3 mode)
    tx_js_type=$(tx_descriptor_get "$TX_SCRATCH" 4 type)
    tx_js_mode=$(tx_descriptor_get "$TX_SCRATCH" 4 mode)
    if [ "$tx_launcher_type" = file ] && [ "$tx_launcher_mode" != 755 ]; then
      TX_BASELINE_REASON="baseline collab-desktop-mcp mode is $tx_launcher_mode, expected 755"
    elif [ "$tx_js_type" = file ] && [ "$tx_js_mode" != 644 ]; then
      TX_BASELINE_REASON="baseline collab-desktop-mcp.mjs mode is $tx_js_mode, expected 644"
    fi
  fi
  if [ -z "$TX_BASELINE_REASON" ]; then
    TX_BASELINE=recoverable_release
    TX_BASELINE_REASON="local release pair, Skills and helpers are recoverable"
  fi
  return 0
}

tx_stage() {
  mkdir -p "$TX_SCRATCH/stage/skills" || return 1
  tx_copy_regular "$TX_CANDIDATE/collab" "$TX_SCRATCH/stage/collab" "candidate collab" &&
    tx_copy_regular "$TX_CANDIDATE/collab-mcp" "$TX_SCRATCH/stage/collab-mcp" "candidate collab-mcp" &&
    tx_copy_regular "$TX_HELPER_DIR/collab-desktop-mcp" \
      "$TX_SCRATCH/stage/collab-desktop-mcp" "Desktop launcher" &&
    tx_copy_regular "$TX_HELPER_DIR/collab-desktop-mcp.mjs" \
      "$TX_SCRATCH/stage/collab-desktop-mcp.mjs" "Desktop JS" || return 1
  chmod 755 "$TX_SCRATCH/stage/collab" "$TX_SCRATCH/stage/collab-mcp" \
    "$TX_SCRATCH/stage/collab-desktop-mcp" || return 1
  chmod 644 "$TX_SCRATCH/stage/collab-desktop-mcp.mjs" || return 1
  "$TX_SCRATCH/stage/collab" install-skills \
    --target "$TX_SCRATCH/stage/skills/collab" --force \
    >"$TX_SCRATCH/skills-export.out" 2>"$TX_SCRATCH/skills-export.err"
  tx_skills_export_status=$?
  [ "$tx_skills_export_status" -eq 0 ] || {
    tx_error "candidate Skill export failed: $(tx_first_line "$TX_SCRATCH/skills-export.err")"
    return "$tx_skills_export_status"
  }
  [ -d "$TX_SCRATCH/stage/skills/collab" ] &&
    [ ! -L "$TX_SCRATCH/stage/skills/collab" ] &&
    [ -s "$TX_SCRATCH/stage/skills/collab/SKILL.md" ] || {
    tx_error "candidate Skill export is incomplete"
    return 1
  }
  (cd "$TX_CWD" && "$TX_SCRATCH/stage/collab" status --check --local) \
    >"$TX_SCRATCH/local-check.out" 2>"$TX_SCRATCH/local-check.err"
  tx_stage_check_status=$?
  [ "$tx_stage_check_status" -eq 0 ] || {
    tx_error "candidate local check failed with exit $tx_stage_check_status: $(tx_first_line "$TX_SCRATCH/local-check.err")"
    return "$tx_stage_check_status"
  }
}

tx_publish_entry() {
  tx_publish_index=$1
  tx_publish_source=$(tx_staged "$tx_publish_index")
  tx_publish_destination=$(tx_entry "$tx_publish_index")
  tx_publish_temp="$(dirname "$tx_publish_destination")/.collab-publish-$tx_publish_index-$$"
  rm -rf "$tx_publish_temp"
  if [ "$tx_publish_index" -eq 5 ]; then
    cp -Rp "$tx_publish_source" "$tx_publish_temp" || {
      tx_error "cannot publish $(tx_name "$tx_publish_index")"
      return 1
    }
  else
    cp -p "$tx_publish_source" "$tx_publish_temp" || {
      tx_error "cannot publish $(tx_name "$tx_publish_index")"
      return 1
    }
  fi
  tx_replace "$tx_publish_temp" "$tx_publish_destination" || {
    tx_error "cannot publish $(tx_name "$tx_publish_index")"
    return 1
  }
}

tx_publish() {
  TX_PUBLISHED=1
  mkdir -p "$TX_BIN" "$(dirname "$TX_SKILLS")" || return 1
  tx_publish_index=1
  while [ "$tx_publish_index" -le 5 ]; do
    tx_publish_entry "$tx_publish_index" || return 1
    tx_publish_index=$((tx_publish_index + 1))
  done
}

tx_restore_entry() {
  tx_restore_index=$1
  tx_restore_destination=$(tx_entry "$tx_restore_index")
  tx_restore_source=$(tx_captured "$tx_restore_index")
  tx_restore_type=$(tx_descriptor_get "$TX_SCRATCH" "$tx_restore_index" type) || return 1
  tx_restore_mode=$(tx_descriptor_get "$TX_SCRATCH" "$tx_restore_index" mode)
  tx_restore_target=$(tx_descriptor_get "$TX_SCRATCH" "$tx_restore_index" target)
  tx_restore_temp="$(dirname "$tx_restore_destination")/.collab-restore-$tx_restore_index-$$"
  rm -rf "$tx_restore_temp"
  case $tx_restore_type in
    absent)
      rm -rf "$tx_restore_destination"
      ;;
    symlink)
      ln -s "$tx_restore_target" "$tx_restore_temp" || return 1
      tx_replace "$tx_restore_temp" "$tx_restore_destination"
      ;;
    file)
      cp -p "$tx_restore_source" "$tx_restore_temp" || return 1
      chmod "$tx_restore_mode" "$tx_restore_temp" || return 1
      tx_replace "$tx_restore_temp" "$tx_restore_destination"
      ;;
    directory)
      cp -Rp "$tx_restore_source" "$tx_restore_temp" || return 1
      chmod "$tx_restore_mode" "$tx_restore_temp" || return 1
      tx_replace "$tx_restore_temp" "$tx_restore_destination"
      ;;
    *)
      return 1
      ;;
  esac
}

tx_restore() {
  TX_ROLLBACK_ERRORS=
  tx_restore_index=1
  while [ "$tx_restore_index" -le 5 ]; do
    tx_restore_entry "$tx_restore_index" || {
      TX_ROLLBACK_ERRORS="$TX_ROLLBACK_ERRORS restore:$(tx_name "$tx_restore_index")"
    }
    tx_restore_index=$((tx_restore_index + 1))
  done
  if [ -n "$TX_ROTATION_INCOMPLETE" ]; then
    TX_ROLLBACK_ERRORS="$TX_ROLLBACK_ERRORS restore:previous"
  fi
  if [ -z "$TX_ROLLBACK_ERRORS" ]; then
    TX_ROLLBACK=restored
    rm -rf "$TX_SCRATCH"
  else
    TX_ROLLBACK=incomplete
    if ! mv "$TX_SCRATCH" "$TX_BIN/$COLLAB_INSTALL_RECOVERY" 2>/dev/null; then
      tx_error "cannot preserve recovery material at $TX_BIN/$COLLAB_INSTALL_RECOVERY"
    fi
  fi
}

tx_rotate_previous() {
  tx_rotation="$TX_BIN/$COLLAB_INSTALL_STAGE.$$"
  tx_old_previous="$TX_BIN/$COLLAB_INSTALL_STAGE.old.$$"
  rm -rf "$tx_rotation" "$tx_old_previous"
  mkdir -p "$tx_rotation/bin/skills" || return 1
  tx_rotate_index=1
  while [ "$tx_rotate_index" -le 5 ]; do
    tx_rotate_type=$(tx_descriptor_get "$TX_SCRATCH" "$tx_rotate_index" type) || return 1
    tx_rotate_source=$(tx_captured "$tx_rotate_index")
    tx_rotate_destination=$(tx_rotation_entry "$tx_rotate_index")
    case $tx_rotate_type in
      file)
        cp -p "$tx_rotate_source" "$tx_rotate_destination" || return 1
        ;;
      directory)
        cp -Rp "$tx_rotate_source" "$tx_rotate_destination" || return 1
        ;;
    esac
    tx_rotate_index=$((tx_rotate_index + 1))
  done
  cp -p "$(tx_descriptor_path "$TX_SCRATCH")" "$tx_rotation/descriptor" || return 1
  if [ -d "$TX_PREVIOUS" ]; then
    mv "$TX_PREVIOUS" "$tx_old_previous" || return 1
  fi
  if ! mv "$tx_rotation" "$TX_PREVIOUS"; then
    if [ ! -d "$tx_old_previous" ]; then
      rm -rf "$tx_rotation"
      return 1
    fi
    if mv "$tx_old_previous" "$TX_PREVIOUS"; then
      rm -rf "$tx_rotation"
      return 1
    fi
    # The retained release cannot be put back at the canonical address. Keep
    # both predecessors where the recovery marker can point at them, and let
    # compensation finish independently instead of claiming restoration.
    TX_ROTATION_INCOMPLETE=1
    TX_PREVIOUS_RELEASE=$tx_old_previous
    TX_PREVIOUS_QUALITY=unverified
    tx_error "cannot restore the retained previous release to $TX_PREVIOUS"
    tx_error "previous release recovery material preserved at $tx_old_previous and $tx_rotation"
    return 1
  fi
  rm -rf "$tx_old_previous"
}

tx_resolve_version() {
  [ -x "$TX_BIN/collab" ] || return 0
  tx_resolve_dir=$TX_BIN/.collab-final-version.$$
  rm -rf "$tx_resolve_dir"
  mkdir -p "$tx_resolve_dir" || return 1
  tx_resolve_status=0
  tx_bounded 5 "$tx_resolve_dir/out" "$tx_resolve_dir/err" \
    "$TX_BIN/collab" --version
  tx_resolve_status=$?
  if [ "$tx_resolve_status" -eq 0 ]; then
    awk 'NR == 1 { print $2 }' "$tx_resolve_dir/out"
  fi
  rm -rf "$tx_resolve_dir"
  return "$tx_resolve_status"
}

tx_emit() {
  printf 'version=%s\n' "$TX_VERSION"
  printf 'collab=%s\n' "$TX_BIN/collab"
  printf 'collab_mcp=%s\n' "$TX_BIN/collab-mcp"
  printf 'skills=%s\n' "$TX_SKILLS"
  printf 'outcome=%s\n' "$TX_OUTCOME"
  printf 'exit=%s\n' "$TX_EXIT"
  printf 'required_action=%s\n' "$TX_REQUIRED"
  printf 'failed_phase=%s\n' "$TX_FAILED_PHASE"
  printf 'original_exit=%s\n' "$TX_ORIGINAL_EXIT"
  printf 'original_error=%s\n' "$TX_ORIGINAL_ERROR"
  printf 'baseline=%s\n' "$TX_BASELINE"
  printf 'baseline_reason=%s\n' "$TX_BASELINE_REASON"
  printf 'rollback=%s\n' "$TX_ROLLBACK"
  [ "$TX_ROLLBACK" != restored ] ||
    printf 'restored_baseline_quality=%s\n' "$TX_BASELINE"
  [ -z "$TX_ROLLBACK_ERRORS" ] ||
    printf 'rollback_errors=%s\n' "$TX_ROLLBACK_ERRORS"
  printf 'previous_release=%s\n' "$TX_PREVIOUS_RELEASE"
  printf 'previous_release_quality=%s\n' "$TX_PREVIOUS_QUALITY"
  printf 'daemon_changed=false\n'
}

tx_finish() {
  tx_finish_outcome=$1
  tx_finish_exit=$2
  tx_finish_required=${3:-none}
  if [ -n "$TX_FINISHED" ]; then
    exit "$tx_finish_exit"
  fi
  TX_FINISHED=1
  trap - EXIT HUP INT TERM
  TX_OUTCOME=$tx_finish_outcome
  TX_EXIT=$tx_finish_exit
  TX_REQUIRED=$tx_finish_required
  if [ -d "$TX_PREVIOUS" ] && [ ! -L "$TX_PREVIOUS" ]; then
    TX_PREVIOUS_RELEASE=$TX_PREVIOUS
    TX_PREVIOUS_QUALITY=unverified
  fi
  if [ "$tx_finish_outcome" = success ] || [ "$tx_finish_outcome" = pending_user_activation ]; then
    if [ "$TX_BASELINE" = recoverable_release ]; then
      if tx_rotate_previous; then
        TX_PREVIOUS_RELEASE=$TX_PREVIOUS
        TX_PREVIOUS_QUALITY=locally_checked
      else
        TX_FAILED_PHASE=previous_rotation
        TX_ORIGINAL_ERROR="previous release rotation failed"
        TX_ORIGINAL_EXIT=1
        TX_OUTCOME=failure_with_rollback
        TX_EXIT=1
        TX_REQUIRED=none
        tx_restore
      fi
    fi
  elif [ "$tx_finish_outcome" = failure_with_rollback ] && [ -n "$TX_PUBLISHED" ]; then
    tx_restore
  fi
  TX_VERSION=$(tx_resolve_version) || TX_VERSION=unknown
  [ -n "$TX_VERSION" ] || TX_VERSION=unknown
  if [ "$TX_ROLLBACK" != incomplete ]; then
    rm -rf "$TX_SCRATCH"
    tx_release_lock
  fi
  tx_emit
  exit "$TX_EXIT"
}

tx_fail() {
  tx_fail_phase=$1
  tx_fail_message=$2
  tx_fail_exit=$3
  TX_FAILED_PHASE=$tx_fail_phase
  TX_ORIGINAL_ERROR=$tx_fail_message
  TX_ORIGINAL_EXIT=$tx_fail_exit
  tx_finish failure_with_rollback "$tx_fail_exit" none
}

tx_signal() {
  tx_signal_number=$1
  if [ -n "$TX_FINISHED" ]; then
    exit "$tx_signal_number"
  fi
  TX_FAILED_PHASE=signal
  TX_ORIGINAL_ERROR="installation interrupted by signal $tx_signal_number"
  TX_ORIGINAL_EXIT=$tx_signal_number
  tx_finish failure_with_rollback "$tx_signal_number" none
}

tx_exit_trap() {
  tx_exit_status=$?
  if [ -n "$TX_FINISHED" ]; then
    exit "$tx_exit_status"
  fi
  tx_fail "${TX_FAILED_PHASE:-prepare}" "installation exited $tx_exit_status" "$tx_exit_status"
}

tx_main() {
  # Scope the transaction's error handling here so sourcing this file does not
  # change the caller's shell options before its own preflight checks.
  set -u
  set +e
  tx_parse "$@"
  TX_CWD=$(pwd -P) || {
    tx_error "cannot preserve invocation cwd"
    exit 1
  }
  TX_SCRATCH=$TX_BIN/.collab-install.$$
  TX_PREVIOUS=$TX_BIN/$COLLAB_INSTALL_PREVIOUS
  trap tx_exit_trap EXIT
  trap 'tx_signal 129' HUP
  trap 'tx_signal 130' INT
  trap 'tx_signal 143' TERM
  tx_acquire_lock || tx_fail prepare "${TX_LAST_ERROR:-cannot acquire installation lock}" 1
  mkdir "$TX_SCRATCH" || tx_fail prepare "cannot create installation scratch" 1
  tx_capture || tx_fail capture "${TX_LAST_ERROR:-baseline capture failed}" 1
  tx_classify || tx_fail classify "${TX_LAST_ERROR:-baseline classification failed}" 1
  if [ -n "$TX_BUILD_SCRIPT" ]; then
    "$TX_BUILD_SCRIPT" >"$TX_SCRATCH/build.out" 2>"$TX_SCRATCH/build.err"
    tx_build_status=$?
    if [ "$tx_build_status" -ne 0 ]; then
      tx_build_error=$(tx_first_line "$TX_SCRATCH/build.err")
      [ -n "$tx_build_error" ] || tx_build_error="source build failed with exit $tx_build_status"
      tx_fail build "$tx_build_error" "$tx_build_status"
    fi
  fi
  tx_stage
  tx_stage_status=$?
  [ "$tx_stage_status" -eq 0 ] ||
    tx_fail stage "${TX_LAST_ERROR:-$(tx_first_line "$TX_SCRATCH/local-check.err")}" "$tx_stage_status"
  tx_publish || tx_fail publish "${TX_LAST_ERROR:-publication failed}" 1
  (cd "$TX_CWD" && "$TX_BIN/collab" status --check) \
    >"$TX_SCRATCH/post-check.out" 2>"$TX_SCRATCH/post-check.err"
  tx_check_status=$?
  if [ "$tx_check_status" -eq 0 ]; then
    tx_finish success 0 none
  elif [ "$tx_check_status" -eq 3 ]; then
    tx_pending_action=$(tx_post_check_required_action) || {
      tx_check_error=$(tx_first_line "$TX_SCRATCH/post-check.err")
      [ -n "$tx_check_error" ] || tx_check_error="candidate check was pending without a usable required action"
      tx_emit_post_check_error
      tx_fail check "$tx_check_error" "$tx_check_status"
    }
    tx_finish pending_user_activation 3 "$tx_pending_action"
  else
    tx_check_error=$(tx_first_line "$TX_SCRATCH/post-check.err")
    [ -n "$tx_check_error" ] || tx_check_error="candidate check failed with exit $tx_check_status"
    tx_emit_post_check_error
    tx_fail check "$tx_check_error" "$tx_check_status"
  fi
}
