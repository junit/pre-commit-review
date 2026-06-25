#!/usr/bin/env bash
set -euo pipefail

script_dir="$(CDPATH='' cd -- "$(dirname -- "$0")" && pwd -P)"
repo_root="$(CDPATH='' cd -- "$script_dir/.." && pwd -P)"
tmp_dir="$(mktemp -d)"
trap 'rm -rf "$tmp_dir"' EXIT

fail() {
  printf 'install agent matrix test failed: %s\n' "$*" >&2
  exit 1
}

assert_target() {
  local output_file="$1"
  local expected="$2"

  grep -Fq "Target: $expected" "$output_file" || {
    printf '%s\n' '--- output ---' >&2
    cat "$output_file" >&2
    printf '%s\n' '--------------' >&2
    fail "expected target: $expected"
  }
}

home_dir="$tmp_dir/home"
project_dir="$tmp_dir/project"
mkdir -p "$home_dir" "$project_dir"

while IFS='|' read -r agent project_path global_path; do
  [ -n "$agent" ] || continue

  global_output="$tmp_dir/${agent}-global.out"
  HOME="$home_dir" "$repo_root/install.sh" --agent "$agent" --dry-run >"$global_output"
  expected_global="${global_path/#\~/$home_dir}"
  assert_target "$global_output" "${expected_global%/}/pre-commit-review"

  project_output="$tmp_dir/${agent}-project.out"
  (
    cd "$project_dir"
    "$repo_root/install.sh" --agent "$agent" --project --dry-run
  ) >"$project_output"
  assert_target "$project_output" "${project_path%/}/pre-commit-review"
done <<'EOF'
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

for alias in claude gemini kiro; do
  "$repo_root/install.sh" "$alias" --dry-run >/dev/null
done

printf 'install agent matrix tests passed\n'
