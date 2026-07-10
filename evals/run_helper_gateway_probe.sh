#!/usr/bin/env bash
# shellcheck disable=SC1091
set -euo pipefail

script_dir="$(CDPATH='' cd -- "$(dirname -- "$0")" && pwd -P)"
. "$script_dir/host_failure_taxonomy.sh"
repo_root="$(CDPATH='' cd -- "$script_dir/.." && pwd -P)"

host=''
claude_bin='claude'
codex_bin='codex'
report_json_path=''
stage_name='helper_gateway_probe'
stage_status='failed'
failure_taxonomy='null'
host_json='null'
keep_fixtures='no'
tmp_root=''
log_file=''
response_file=''

usage() {
  cat <<'EOF'
Usage: run_helper_gateway_probe.sh --host <claude|codex> [options]

Run a real-host probe that verifies pre-commit-review attempts the bundled
collect_diff_context helper before direct Git source-selection commands.

Options:
  --host HOST           Required. One of: claude, codex
  --claude-bin PATH     Override the Claude CLI binary path
  --codex-bin PATH      Override the Codex CLI binary path
  --report-json PATH    Write a host-stage-report/v1 JSON report
  --keep-fixtures       Keep the generated probe workspace for debugging
  -h, --help            Show this help
EOF
}

build_stage_report_json() {
  jq -cn \
    --arg schema_version "host-stage-report/v1" \
    --arg stage "$stage_name" \
    --argjson host "$host_json" \
    --arg status "$stage_status" \
    --argjson failure_taxonomy "$failure_taxonomy" \
    --arg log_file "${log_file:-}" \
    --arg response_file "${response_file:-}" \
    '{
      schema_version: $schema_version,
      stage: $stage,
      host: $host,
      status: $status,
      failure_taxonomy: $failure_taxonomy,
      artifacts: {
        gateway_log: (if $log_file == "" then null else $log_file end),
        response_file: (if $response_file == "" then null else $response_file end)
      }
    }'
}

emit_stage_report() {
  [ -n "$report_json_path" ] || return 0
  build_stage_report_json >"$report_json_path"
}

fail() {
  emit_stage_report
  printf 'run helper gateway probe failed: %s\n' "$*" >&2
  exit 1
}

taxonomy_fail() {
  local failure_type="$1"
  shift
  failure_taxonomy="\"$failure_type\""
  fail "$*"
}

cleanup() {
  if [ "$keep_fixtures" != 'yes' ] && [ -n "$tmp_root" ]; then
    rm -rf "$tmp_root"
  elif [ "$keep_fixtures" = 'yes' ] && [ -n "$tmp_root" ]; then
    printf 'helper gateway probe fixtures kept at %s\n' "$tmp_root"
  fi
}
trap cleanup EXIT

require_command() {
  command -v "$1" >/dev/null 2>&1 \
    || taxonomy_fail 'missing-binary' "required command not found: $1"
}

copy_skill_package() {
  local target="$1"

  mkdir -p "$target"
  cp "$repo_root/SKILL.md" "$target/SKILL.md"
  cp -R "$repo_root/references" "$target/references"
  cp -R "$repo_root/scripts" "$target/scripts"
  cp -R "$repo_root/agents" "$target/agents"

  mv "$target/scripts/collect_diff_context.sh" "$target/scripts/collect_diff_context.real.sh"
  cat >"$target/scripts/collect_diff_context.sh" <<'EOF'
#!/usr/bin/env bash
set -euo pipefail

: "${PRE_COMMIT_REVIEW_GATEWAY_LOG:?}"
script_dir="$(CDPATH='' cd -- "$(dirname -- "$0")" && pwd -P)"
printf 'helper:start cwd=%s\n' "$PWD" >>"$PRE_COMMIT_REVIEW_GATEWAY_LOG"
export PRE_COMMIT_REVIEW_HELPER_ACTIVE=1
exec "$script_dir/collect_diff_context.real.sh" "$@"
EOF
  chmod +x "$target/scripts/collect_diff_context.sh" "$target/scripts/collect_diff_context.real.sh"
}

write_git_wrapper() {
  local wrapper_dir="$1"
  local real_git="$2"

  mkdir -p "$wrapper_dir"
  cat >"$wrapper_dir/git" <<EOF
#!/usr/bin/env bash
set -euo pipefail

real_git='$real_git'
log_file="\${PRE_COMMIT_REVIEW_GATEWAY_LOG:-}"
args=("\$@")
subcmd=''
i=0
while [ "\$i" -lt "\${#args[@]}" ]; do
  arg="\${args[\$i]}"
  case "\$arg" in
    -C|-c|--git-dir|--work-tree|--namespace)
      i=\$((i + 2))
      ;;
    --git-dir=*|--work-tree=*|--namespace=*|-c*)
      i=\$((i + 1))
      ;;
    --*)
      i=\$((i + 1))
      ;;
    -*)
      i=\$((i + 1))
      ;;
    *)
      subcmd="\$arg"
      break
      ;;
  esac
done

if [ -n "\$log_file" ]; then
  if [ "\${PRE_COMMIT_REVIEW_HELPER_ACTIVE:-0}" = "1" ]; then
    printf 'git:helper %s\n' "\${subcmd:-<none>}" >>"\$log_file"
  else
    case "\$subcmd" in
      status|diff|branch|merge-base|show-ref|ls-files)
        printf 'git:direct %s\n' "\$subcmd" >>"\$log_file"
        ;;
    esac
  fi
fi

exec "\$real_git" "\${args[@]}"
EOF
  chmod +x "$wrapper_dir/git"
}

init_probe_repo() {
  local workdir="$1"
  local real_git="$2"

  mkdir -p "$workdir"
  "$real_git" -C "$workdir" init -q
  "$real_git" -C "$workdir" config user.email a@example.com
  "$real_git" -C "$workdir" config user.name A
  printf '# Probe\n\nBefore.\n' >"$workdir/README.md"
  "$real_git" -C "$workdir" add README.md
  "$real_git" -C "$workdir" commit -q -m init
  printf '# Probe\n\nAfter.\n' >"$workdir/README.md"
  "$real_git" -C "$workdir" add README.md
}

run_host_probe() {
  local workdir="$1"
  local skill_dir="$2"
  local wrapper_bin_dir="$3"
  local prompt_text

  prompt_text='Review staged changes before commit using the pre-commit-review skill. Follow the skill instructions exactly. There is no user-provided diff in this prompt, so use local repository access if available.'

  case "$host" in
    claude)
      if [ ! -x "$claude_bin" ] && ! command -v "$claude_bin" >/dev/null 2>&1; then
        taxonomy_fail 'missing-binary' "claude binary not found: $claude_bin"
      fi
      mkdir -p "$workdir/.claude/skills"
      ln -sfn "$skill_dir" "$workdir/.claude/skills/pre-commit-review"
      (
        cd "$workdir"
        env PATH="$wrapper_bin_dir:$PATH" \
          PRE_COMMIT_REVIEW_GATEWAY_LOG="$log_file" \
          "$claude_bin" \
            -p \
            --output-format text \
            --permission-mode dontAsk \
            --no-session-persistence \
            "/pre-commit-review

$prompt_text" >"$response_file"
      ) || taxonomy_fail 'runner-exit-nonzero' 'host command exited non-zero during helper gateway probe'
      ;;
    codex)
      if [ ! -x "$codex_bin" ] && ! command -v "$codex_bin" >/dev/null 2>&1; then
        taxonomy_fail 'missing-binary' "codex binary not found: $codex_bin"
      fi
      mkdir -p "$workdir/.agents/skills"
      ln -sfn "$skill_dir" "$workdir/.agents/skills/pre-commit-review"
      (
        cd "$workdir"
        printf '%s\n' "$prompt_text" | env PATH="$wrapper_bin_dir:$PATH" \
          PRE_COMMIT_REVIEW_GATEWAY_LOG="$log_file" \
          "$codex_bin" \
            exec \
            --skip-git-repo-check \
            --ephemeral \
            --sandbox workspace-write \
            --color never \
            -o "$response_file" \
            -
      ) || taxonomy_fail 'runner-exit-nonzero' 'host command exited non-zero during helper gateway probe'
      ;;
    *)
      taxonomy_fail 'unsupported-host' "unsupported host: $host"
      ;;
  esac
}

assert_helper_gateway_order() {
  local first_repo_event

  [ -s "$response_file" ] \
    || taxonomy_fail 'protocol-mismatch' 'host did not write a response during helper gateway probe'
  if ! grep -Fq 'helper:start' "$log_file"; then
    taxonomy_fail 'helper-gateway-violation' 'collect_diff_context helper was not called'
  fi

  first_repo_event="$(grep -E '^(helper:start|git:direct )' "$log_file" | head -n 1 || true)"
  case "$first_repo_event" in
    helper:start*)
      return 0
      ;;
    git:direct*)
      taxonomy_fail 'helper-gateway-violation' "direct Git source-selection command ran before helper: $first_repo_event"
      ;;
    *)
      taxonomy_fail 'helper-gateway-violation' 'could not determine helper/direct Git ordering from gateway log'
      ;;
  esac
}

while [ "$#" -gt 0 ]; do
  case "$1" in
    --host)
      shift
      [ "$#" -gt 0 ] || fail '--host requires a value'
      host="$1"
      ;;
    --claude-bin)
      shift
      [ "$#" -gt 0 ] || fail '--claude-bin requires a value'
      claude_bin="$1"
      ;;
    --codex-bin)
      shift
      [ "$#" -gt 0 ] || fail '--codex-bin requires a value'
      codex_bin="$1"
      ;;
    --report-json)
      shift
      [ "$#" -gt 0 ] || fail '--report-json requires a value'
      report_json_path="$1"
      ;;
    --keep-fixtures)
      keep_fixtures='yes'
      ;;
    -h|--help)
      usage
      exit 0
      ;;
    *)
      fail "unknown argument: $1"
      ;;
  esac
  shift
done

[ -n "$host" ] || fail '--host is required'
case "$host" in
  claude|codex) host_json="\"$host\"" ;;
  *) taxonomy_fail 'unsupported-host' "unsupported host: $host" ;;
esac

require_command git
require_command jq

real_git="$(command -v git)"
tmp_root="$(mktemp -d)"
skill_dir="$tmp_root/skill/pre-commit-review"
workdir="$tmp_root/workdir"
wrapper_bin_dir="$tmp_root/bin"
log_file="$tmp_root/gateway.log"
response_file="$tmp_root/response.md"

copy_skill_package "$skill_dir"
write_git_wrapper "$wrapper_bin_dir" "$real_git"
init_probe_repo "$workdir" "$real_git"
run_host_probe "$workdir" "$skill_dir" "$wrapper_bin_dir"
assert_helper_gateway_order

stage_status='passed'
failure_taxonomy='null'
emit_stage_report
printf 'helper gateway probe passed [%s]\n' "$host"
