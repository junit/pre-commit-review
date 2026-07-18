#!/usr/bin/env bash
set -euo pipefail

skill_name='pre-commit-review'
script_dir="$(CDPATH='' cd -- "$(dirname -- "$0")" && pwd -P)"
source_dir="$script_dir"

mode='copy'
force='no'
dry_run='no'
download_gitleaks='yes'
doctor='no'
host=''
skills_dir=''
install_scope='global'
active_staging_dir=''

# shellcheck disable=SC2329 # Invoked indirectly by the EXIT trap.
cleanup_install_staging() {
  if [ -n "$active_staging_dir" ] && [ -e "$active_staging_dir" ]; then
    rm -rf "$active_staging_dir"
  fi
}
trap cleanup_install_staging EXIT

usage() {
  cat <<'EOF'
Usage:
  ./install.sh <agent> [--copy|--link] [--project|--dir PATH] [--force] [--dry-run] [--no-download]
  ./install.sh --agent AGENT [--copy|--link] [--project|--dir PATH] [--force] [--dry-run] [--no-download]
  ./install.sh --doctor

Options:
  --agent NAME  Agent id to install for
  --copy       Copy the minimal runtime skill payload into the target skills directory (default)
  --link       Symlink this repository into the target skills directory
  --project    Install to the agent's project-local skills directory
  --dir PATH   Override the target skills directory
  --force      Replace an existing non-managed target directory
  --dry-run    Print planned actions without changing the filesystem
  --no-download
               Skip optional Gitleaks download; review remains available without secret redaction
  --doctor     Verify Gitleaks source, version, integrity, configuration, and stdin/JSON capability
  --list-agents
               List supported agent ids and default paths
  --help       Show this help text

Environment overrides:
  PRE_COMMIT_REVIEW_GITLEAKS_BIN
  PRE_COMMIT_REVIEW_GITLEAKS_CONFIG
  PRE_COMMIT_REVIEW_FETCH_PROGRESS
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
  local input="$1"
  local stripped
  # Expand a leading literal "~" or "~/" to $HOME. The pattern uses a literal
  # tilde character, not tilde expansion, so it matches input paths like "~/".
  case "$input" in
    '~')
      printf '%s\n' "$HOME"
      ;;
    '~'/*)
      stripped="${input#'~/'}"
      printf '%s/%s\n' "$HOME" "$stripped"
      ;;
    *)
      printf '%s\n' "$input"
      ;;
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

  case "$path" in
    /|'') die "refusing to remove root or empty path: '$path'" ;;
  esac

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

validate_target() {
  local target="$1"

  if [ ! -e "$target" ] && [ ! -L "$target" ]; then
    return 0
  fi
  if is_managed_skill_dir "$target" || [ "$force" = 'yes' ]; then
    return 0
  fi
  die "refusing to replace existing path that is not managed by this installer: $target"
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

resolve_gitleaks_platform() {
  local os_name
  local arch_name

  case "$(uname -s | tr '[:upper:]' '[:lower:]')" in
    darwin) os_name='darwin' ;;
    linux) os_name='linux' ;;
    msys*|mingw*|cygwin*) os_name='windows' ;;
    *) die 'unsupported operating system for bundled Gitleaks' ;;
  esac

  case "$(uname -m)" in
    arm64|aarch64) arch_name='arm64' ;;
    x86_64|amd64) arch_name='amd64' ;;
    *) die 'unsupported architecture for bundled Gitleaks' ;;
  esac

  printf '%s-%s\n' "$os_name" "$arch_name"
}

gitleaks_binary_name() {
  local platform="$1"
  local suffix=''
  case "$platform" in
    windows-*) suffix='.exe' ;;
  esac
  printf 'gitleaks-%s%s\n' "$platform" "$suffix"
}

gitleaks_is_compatible() {
  local executable="$1"
  gitleaks_version_matches "$executable" "$source_dir/scripts/gitleaks.version" \
    && gitleaks_smoke_scan "$executable" "$source_dir/references/security/gitleaks.toml"
}

gitleaks_is_trusted_bundled() {
  local executable="$1"
  local binary_name="$2"
  gitleaks_hash_matches \
    "$executable" \
    "$source_dir/scripts/gitleaks-binaries.sha256" \
    "$binary_name" \
    && gitleaks_is_compatible "$executable"
}

explicit_gitleaks() {
  local executable="${PRE_COMMIT_REVIEW_GITLEAKS_BIN:-}"
  [ -n "$executable" ] \
    && gitleaks_path_is_absolute "$executable" \
    && [ -x "$executable" ] \
    || return 1
  printf '%s\n' "$executable"
}

provision_gitleaks() {
  local runtime_root="$1"
  local platform="$2"
  local binary_name="$3"
  local bundled_path="$runtime_root/scripts/bin/$binary_name"

  if [ "$dry_run" = 'yes' ] && [ "$download_gitleaks" = 'yes' ]; then
    if [ -x "$bundled_path" ]; then
      log "DRY RUN validate bundled $binary_name and replace it if version, integrity, or capability checks fail"
    else
      log "DRY RUN fetch pinned Gitleaks for $platform into $runtime_root/scripts/bin"
    fi
    return 0
  fi
  if [ "$dry_run" = 'yes' ]; then
    log "DRY RUN skip optional Gitleaks download (--no-download)"
    return 0
  fi
  if gitleaks_is_trusted_bundled "$bundled_path" "$binary_name"; then
    log "Gitleaks: using bundled $binary_name"
    return 0
  fi

  if [ "$download_gitleaks" = 'no' ]; then
    local configured_gitleaks
    configured_gitleaks="$(explicit_gitleaks || true)"
    if gitleaks_is_compatible "$configured_gitleaks"; then
      log "Gitleaks: using explicitly trusted $configured_gitleaks (--no-download)"
      return 0
    fi
    log "Gitleaks: optional scanner unavailable (--no-download); review will continue without secret redaction"
    return 0
  fi

  local fetch_script="$source_dir/scripts/fetch_gitleaks.sh"
  if [ ! -x "$fetch_script" ]; then
    log "Warning: optional Gitleaks fetch script is unavailable; review will continue without secret redaction"
    return 0
  fi
  if ! "$fetch_script" --platform "$platform" --dest "$runtime_root/scripts/bin"; then
    log "Warning: optional Gitleaks download failed; review will continue without secret redaction"
    return 0
  fi
  if gitleaks_is_trusted_bundled "$bundled_path" "$binary_name"; then
    log "Gitleaks: installed pinned $binary_name"
  else
    log "Warning: downloaded Gitleaks did not pass validation; review will continue without secret redaction"
  fi
}

copy_payload() {
  local target="$1"
  local platform="$2"
  local binary_name="$3"
  local staging_dir="${target}.tmp.$$"

  if [ "$dry_run" = 'yes' ]; then
    local plan_root="$target"
    if [ -x "$source_dir/scripts/bin/$binary_name" ]; then
      plan_root="$source_dir"
    fi
    prepare_target "$target"
    log "DRY RUN copy runtime payload $source_dir -> $target"
    provision_gitleaks "$plan_root" "$platform" "$binary_name"
    return 0
  fi

  remove_target "$staging_dir"
  active_staging_dir="$staging_dir"
  mkdir -p "$staging_dir"

  cp "$source_dir/SKILL.md" "$staging_dir/"
  cp "$source_dir/LICENSE" "$staging_dir/"
  cp -R "$source_dir/agents" "$staging_dir/"
  cp -R "$source_dir/references" "$staging_dir/"
  cp -R "$source_dir/scripts" "$staging_dir/"
  if [ -d "$source_dir/THIRD_PARTY_LICENSES" ]; then
    cp -R "$source_dir/THIRD_PARTY_LICENSES" "$staging_dir/"
  fi

  provision_gitleaks "$staging_dir" "$platform" "$binary_name"

  prepare_target "$target"
  mv "$staging_dir" "$target"
  active_staging_dir=''
}

link_payload() {
  local target="$1"
  local platform="$2"
  local binary_name="$3"

  provision_gitleaks "$source_dir" "$platform" "$binary_name"
  prepare_target "$target"

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
    --no-download)
      download_gitleaks='no'
      ;;
    --doctor)
      doctor='yes'
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

# shellcheck source=scripts/lib/gitleaks_integrity.sh
source "$source_dir/scripts/lib/gitleaks_integrity.sh"

if [ "$doctor" = 'yes' ]; then
  [ -z "$host" ] || die '--doctor does not accept an agent argument'
  exec "$source_dir/scripts/check_gitleaks.sh"
fi

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
gitleaks_platform="$(resolve_gitleaks_platform)"
gitleaks_binary="$(gitleaks_binary_name "$gitleaks_platform")"

validate_target "$target_dir"
ensure_parent_dir "$skills_dir"

case "$mode" in
  copy) copy_payload "$target_dir" "$gitleaks_platform" "$gitleaks_binary" ;;
  link) link_payload "$target_dir" "$gitleaks_platform" "$gitleaks_binary" ;;
  *) die "unsupported mode: $mode" ;;
esac

log "Installed $skill_name for $host"
log "Mode: $mode"
log "Scope: $install_scope"
log "Target: $target_dir"
log "Restart $host or start a new session so the skill can be discovered."
