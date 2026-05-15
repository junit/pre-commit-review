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
install_scope='global'

usage() {
  cat <<'EOF'
Usage:
  ./install.sh <agent> [--copy|--link] [--project|--dir PATH] [--force] [--dry-run]
  ./install.sh --agent AGENT [--copy|--link] [--project|--dir PATH] [--force] [--dry-run]

Options:
  --agent NAME  Agent id to install for
  --copy       Copy this repository into the target skills directory (default)
  --link       Symlink this repository into the target skills directory
  --project    Install to the agent's project-local skills directory
  --dir PATH   Override the target skills directory
  --force      Replace an existing non-managed target directory
  --dry-run    Print planned actions without changing the filesystem
  --list-agents
               List supported agent ids and default paths
  --help       Show this help text

Environment overrides:
  CODEX_SKILLS_DIR
  CLAUDE_SKILLS_DIR
  GEMINI_SKILLS_DIR
  KIRO_SKILLS_DIR
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
    "~/"*) printf '%s/%s\n' "$HOME" "${1#\~/}" ;;
    *) printf '%s\n' "$1" ;;
  esac
}

canonical_agent() {
  case "$1" in
    claude) printf '%s\n' 'claude-code' ;;
    gemini) printf '%s\n' 'gemini-cli' ;;
    kiro) printf '%s\n' 'kiro-cli' ;;
    *) printf '%s\n' "$1" ;;
  esac
}

supported_agents() {
  cat <<'EOF'
aider-desk|.aider-desk/skills|~/.aider-desk/skills
amp|.agents/skills|~/.config/agents/skills
kimi-cli|.agents/skills|~/.config/agents/skills
replit|.agents/skills|~/.config/agents/skills
universal|.agents/skills|~/.config/agents/skills
antigravity|.agents/skills|~/.gemini/antigravity/skills
augment|.augment/skills|~/.augment/skills
bob|.bob/skills|~/.bob/skills
claude-code|.claude/skills|~/.claude/skills
openclaw|skills|~/.openclaw/skills
cline|.agents/skills|~/.agents/skills
dexto|.agents/skills|~/.agents/skills
warp|.agents/skills|~/.agents/skills
codearts-agent|.codeartsdoer/skills|~/.codeartsdoer/skills
codebuddy|.codebuddy/skills|~/.codebuddy/skills
codemaker|.codemaker/skills|~/.codemaker/skills
codestudio|.codestudio/skills|~/.codestudio/skills
codex|.agents/skills|~/.codex/skills
command-code|.commandcode/skills|~/.commandcode/skills
continue|.continue/skills|~/.continue/skills
cortex|.cortex/skills|~/.snowflake/cortex/skills
crush|.crush/skills|~/.config/crush/skills
cursor|.agents/skills|~/.cursor/skills
deepagents|.agents/skills|~/.deepagents/agent/skills
devin|.devin/skills|~/.config/devin/skills
droid|.factory/skills|~/.factory/skills
firebender|.agents/skills|~/.firebender/skills
forgecode|.forge/skills|~/.forge/skills
gemini-cli|.agents/skills|~/.gemini/skills
github-copilot|.agents/skills|~/.copilot/skills
goose|.goose/skills|~/.config/goose/skills
hermes-agent|.hermes/skills|~/.hermes/skills
junie|.junie/skills|~/.junie/skills
iflow-cli|.iflow/skills|~/.iflow/skills
kilo|.kilocode/skills|~/.kilocode/skills
kiro-cli|.kiro/skills|~/.kiro/skills
kode|.kode/skills|~/.kode/skills
mcpjam|.mcpjam/skills|~/.mcpjam/skills
mistral-vibe|.vibe/skills|~/.vibe/skills
mux|.mux/skills|~/.mux/skills
opencode|.agents/skills|~/.config/opencode/skills
openhands|.openhands/skills|~/.openhands/skills
pi|.pi/skills|~/.pi/agent/skills
qoder|.qoder/skills|~/.qoder/skills
qwen-code|.qwen/skills|~/.qwen/skills
rovodev|.rovodev/skills|~/.rovodev/skills
roo|.roo/skills|~/.roo/skills
tabnine-cli|.tabnine/agent/skills|~/.tabnine/agent/skills
trae|.trae/skills|~/.trae/skills
trae-cn|.trae/skills|~/.trae-cn/skills
windsurf|.windsurf/skills|~/.codeium/windsurf/skills
zencoder|.zencoder/skills|~/.zencoder/skills
neovate|.neovate/skills|~/.neovate/skills
pochi|.pochi/skills|~/.pochi/skills
adal|.adal/skills|~/.adal/skills
EOF
}

agent_record() {
  local requested="$1"
  local agent
  local canonical
  local project_path
  local global_path

  canonical="$(canonical_agent "$requested")"
  while IFS='|' read -r agent project_path global_path; do
    if [ "$agent" = "$canonical" ]; then
      printf '%s|%s|%s\n' "$agent" "$project_path" "$global_path"
      return 0
    fi
  done <<EOF
$(supported_agents)
EOF

  return 1
}

list_agents() {
  printf '%-20s %-28s %s\n' 'Agent' 'Project Path' 'Global Path'
  printf '%-20s %-28s %s\n' '-----' '------------' '-----------'
  supported_agents | while IFS='|' read -r agent project_path global_path; do
    printf '%-20s %-28s %s\n' "$agent" "$project_path" "$global_path"
  done
}

agent_env_override() {
  case "$1" in
    codex)
      if [ -n "${CODEX_SKILLS_DIR:-}" ]; then
        printf '%s\n' "$CODEX_SKILLS_DIR"
      fi
      ;;
    claude-code)
      if [ -n "${CLAUDE_SKILLS_DIR:-}" ]; then
        printf '%s\n' "$CLAUDE_SKILLS_DIR"
      fi
      ;;
    gemini-cli)
      if [ -n "${GEMINI_SKILLS_DIR:-}" ]; then
        printf '%s\n' "$GEMINI_SKILLS_DIR"
      fi
      ;;
    kiro-cli)
      if [ -n "${KIRO_SKILLS_DIR:-}" ]; then
        printf '%s\n' "$KIRO_SKILLS_DIR"
      fi
      ;;
  esac
}

resolve_default_skills_dir() {
  local record="$1"
  local canonical
  local project_path
  local global_path
  local env_override

  IFS='|' read -r canonical project_path global_path <<EOF
$record
EOF

  if [ "$install_scope" = 'project' ]; then
    printf '%s\n' "$project_path"
    return 0
  fi

  env_override="$(agent_env_override "$canonical")"
  if [ -n "$env_override" ]; then
    printf '%s\n' "$env_override"
  elif [ -n "${AGENT_SKILLS_DIR:-}" ]; then
    printf '%s\n' "$AGENT_SKILLS_DIR"
  elif [ "$canonical" = 'codex' ] && [ -n "${CODEX_HOME:-}" ]; then
    printf '%s/skills\n' "${CODEX_HOME%/}"
  else
    printf '%s\n' "$global_path"
  fi
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
    --agent)
      shift
      [ "$#" -gt 0 ] || die "--agent requires a value"
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
    --project)
      install_scope='project'
      ;;
    --global)
      install_scope='global'
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
    --list-agents)
      list_agents
      exit 0
      ;;
    --help|-h)
      usage
      exit 0
      ;;
    *)
      if [ -z "$host" ]; then
        host="$1"
      else
        die "unknown argument: $1"
      fi
      ;;
  esac
  shift
done

[ -n "$host" ] || {
  usage
  exit 64
}

agent_info="$(agent_record "$host" || true)"
if [ -z "$agent_info" ]; then
  die "unsupported agent: $host (run ./install.sh --list-agents)"
fi

if [ -z "$skills_dir" ]; then
  skills_dir="$(resolve_default_skills_dir "$agent_info")"
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
log "Scope: $install_scope"
log "Target: $target_dir"
log "Restart $host or start a new session so the skill can be discovered."
