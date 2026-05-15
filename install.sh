#!/usr/bin/env bash
set -euo pipefail

skill_name='pre-commit-review'
script_dir="$(CDPATH='' cd -- "$(dirname -- "$0")" && pwd -P)"
source_dir="$script_dir"

mode='copy'
force='no'
dry_run='no'
host=''
skills_dir=''

usage() {
  cat <<'EOF'
Usage:
  ./install.sh <codex|claude|gemini> [--copy|--link] [--dir PATH] [--force] [--dry-run]

Options:
  --copy       Copy this repository into the target skills directory (default)
  --link       Symlink this repository into the target skills directory
  --dir PATH   Override the target skills directory
  --force      Replace an existing non-managed target directory
  --dry-run    Print planned actions without changing the filesystem
  --help       Show this help text

Environment overrides:
  CODEX_SKILLS_DIR
  CLAUDE_SKILLS_DIR
  GEMINI_SKILLS_DIR
  AGENT_SKILLS_DIR
  CODEX_HOME
EOF
}

log() {
  printf '%s\n' "$*"
}

die() {
  printf 'error: %s\n' "$*" >&2
  exit 1
}

expand_home() {
  case "$1" in
    "~") printf '%s\n' "$HOME" ;;
    "~/"*) printf '%s/%s\n' "$HOME" "${1#~/}" ;;
    *) printf '%s\n' "$1" ;;
  esac
}

resolve_default_skills_dir() {
  case "$host" in
    codex)
      if [ -n "${CODEX_SKILLS_DIR:-}" ]; then
        printf '%s\n' "$CODEX_SKILLS_DIR"
      elif [ -n "${AGENT_SKILLS_DIR:-}" ]; then
        printf '%s\n' "$AGENT_SKILLS_DIR"
      elif [ -n "${CODEX_HOME:-}" ]; then
        printf '%s/skills\n' "${CODEX_HOME%/}"
      else
        printf '%s/.codex/skills\n' "$HOME"
      fi
      ;;
    claude)
      if [ -n "${CLAUDE_SKILLS_DIR:-}" ]; then
        printf '%s\n' "$CLAUDE_SKILLS_DIR"
      else
        printf '%s/.claude/skills\n' "$HOME"
      fi
      ;;
    gemini)
      if [ -n "${GEMINI_SKILLS_DIR:-}" ]; then
        printf '%s\n' "$GEMINI_SKILLS_DIR"
      elif [ -n "${AGENT_SKILLS_DIR:-}" ]; then
        printf '%s\n' "$AGENT_SKILLS_DIR"
      else
        printf '%s/.agents/skills\n' "$HOME"
      fi
      ;;
  esac
}

is_managed_skill_dir() {
  local path="$1"

  if [ ! -f "$path/SKILL.md" ]; then
    return 1
  fi

  grep -Eq '^name: "?pre-commit-review"?$' "$path/SKILL.md"
}

remove_target() {
  local path="$1"

  if [ "$dry_run" = 'yes' ]; then
    log "DRY RUN remove $path"
    return 0
  fi

  rm -rf "$path"
}

ensure_parent_dir() {
  local path="$1"

  if [ "$dry_run" = 'yes' ]; then
    log "DRY RUN mkdir -p $path"
    return 0
  fi

  mkdir -p "$path"
}

prepare_target() {
  local target="$1"

  if [ ! -e "$target" ] && [ ! -L "$target" ]; then
    return 0
  fi

  if is_managed_skill_dir "$target"; then
    remove_target "$target"
    return 0
  fi

  if [ "$force" = 'yes' ]; then
    remove_target "$target"
    return 0
  fi

  die "refusing to replace existing path that is not managed by this installer: $target"
}

copy_payload() {
  local target="$1"
  local staging_dir="${target}.tmp.$$"

  if [ "$dry_run" = 'yes' ]; then
    log "DRY RUN copy $source_dir -> $target"
    return 0
  fi

  rm -rf "$staging_dir"
  mkdir -p "$staging_dir"

  cp "$source_dir/SKILL.md" "$staging_dir/"
  cp "$source_dir/README.md" "$staging_dir/"
  cp "$source_dir/README.zh-CN.md" "$staging_dir/"
  cp "$source_dir/LICENSE" "$staging_dir/"
  cp "$source_dir/install.sh" "$staging_dir/"
  cp -R "$source_dir/agents" "$staging_dir/"
  cp -R "$source_dir/references" "$staging_dir/"
  cp -R "$source_dir/scripts" "$staging_dir/"

  mv "$staging_dir" "$target"
}

link_payload() {
  local target="$1"

  if [ "$dry_run" = 'yes' ]; then
    log "DRY RUN ln -s $source_dir $target"
    return 0
  fi

  ln -s "$source_dir" "$target"
}

while [ "$#" -gt 0 ]; do
  case "$1" in
    codex|claude|gemini)
      if [ -n "$host" ]; then
        die "host specified more than once"
      fi
      host="$1"
      ;;
    --copy)
      mode='copy'
      ;;
    --link)
      mode='link'
      ;;
    --dir)
      shift
      [ "$#" -gt 0 ] || die "--dir requires a value"
      skills_dir="$1"
      ;;
    --force)
      force='yes'
      ;;
    --dry-run)
      dry_run='yes'
      ;;
    --help|-h)
      usage
      exit 0
      ;;
    *)
      die "unknown argument: $1"
      ;;
  esac
  shift
done

[ -n "$host" ] || {
  usage
  exit 64
}

if [ -z "$skills_dir" ]; then
  skills_dir="$(resolve_default_skills_dir)"
fi

skills_dir="$(expand_home "$skills_dir")"
target_dir="${skills_dir%/}/$skill_name"

ensure_parent_dir "$skills_dir"
prepare_target "$target_dir"

case "$mode" in
  copy) copy_payload "$target_dir" ;;
  link) link_payload "$target_dir" ;;
  *) die "unsupported mode: $mode" ;;
esac

log "Installed $skill_name for $host"
log "Mode: $mode"
log "Target: $target_dir"
log "Restart $host or start a new session so the skill can be discovered."
