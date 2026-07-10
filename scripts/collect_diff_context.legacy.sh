#!/usr/bin/env bash
set -euo pipefail

# Collect Git diff context for the pre-commit-review skill.
# This script is read-only. It does not fetch, stage, reset, install, or mutate files.
# Optional environment variable:
#   PRE_COMMIT_REVIEW_MAX_DIFF_BYTES  default 200000; set 0 for no truncation
#   PRE_COMMIT_REVIEW_INLINE_DIFF_BYTES  default 60000; default output omits global diff above this size
#   PRE_COMMIT_REVIEW_CONTEXT_QUERY_LIMIT  default 20; max matches per context query
#   PRE_COMMIT_REVIEW_GROUP_TARGET_BYTES  default 120000; soft group size target
#   PRE_COMMIT_REVIEW_GROUP_HARD_BYTES   default 160000; hard group size limit
#
# Section index (single-file by design for portability; no `source` deps):
#   1. Env/config              - MAX_DIFF_BYTES, GROUP_*_BYTES, REQUEST_* parsing
#   2. Helpers                 - print_kv, sanitize_tsv, shell_quote, usage, fail_no_repo
#   3. Diff detection          - has_staged/unstaged_changes, has_diff_for_ref, detect_base_branch
#   4. Stat/numstat            - summarize_numstat, join_lines_csv, file_*_for_path
#   5. Risk classifiers (awk)  - high_risk_paths_from_name_status, content_risk_paths_from_diff,
#                                generated_like_*, lock_paths_*, configured_*_risk, top_churn_*
#   6. Path predicates         - path_has_high_risk_signal, path_is_generated_like, path_is_lockfile
#   7. Grouping                - safe_group_component, group_component_for_path
#   8. Commands & splitting    - review/context_command_for_path|group, emit_hunk_split_*
#   9. Manifest & JSON emit    - build_review_manifest_tmp, emit_review_plan_json,
#                                emit_reducer_state_snapshot_template, emit_review_manifest_and_groups,
#                                emit_requested_group_context, emit_dependency_summary,
#                                emit_semantic_context_queries, emit_diff_limited
#  10. Orchestration (main)    - run_diff, select_branch_ref, source selection, final output

MAX_DIFF_BYTES="${PRE_COMMIT_REVIEW_MAX_DIFF_BYTES:-200000}"
case "$MAX_DIFF_BYTES" in
  ''|*[!0-9]*) MAX_DIFF_BYTES=200000 ;;
esac
INLINE_DIFF_BYTES="${PRE_COMMIT_REVIEW_INLINE_DIFF_BYTES:-60000}"
case "$INLINE_DIFF_BYTES" in
  ''|*[!0-9]*) INLINE_DIFF_BYTES=60000 ;;
esac
CONTEXT_QUERY_LIMIT="${PRE_COMMIT_REVIEW_CONTEXT_QUERY_LIMIT:-20}"
case "$CONTEXT_QUERY_LIMIT" in
  ''|*[!0-9]*) CONTEXT_QUERY_LIMIT=20 ;;
esac
GROUP_TARGET_BYTES="${PRE_COMMIT_REVIEW_GROUP_TARGET_BYTES:-120000}"
case "$GROUP_TARGET_BYTES" in
  ''|*[!0-9]*) GROUP_TARGET_BYTES=120000 ;;
esac
GROUP_HARD_BYTES="${PRE_COMMIT_REVIEW_GROUP_HARD_BYTES:-160000}"
case "$GROUP_HARD_BYTES" in
  ''|*[!0-9]*) GROUP_HARD_BYTES=160000 ;;
esac
if [ "$GROUP_TARGET_BYTES" -gt "$GROUP_HARD_BYTES" ]; then
  GROUP_TARGET_BYTES="$GROUP_HARD_BYTES"
fi

TEMP_FILES=''
register_temp_file() {
  [ -n "${1:-}" ] || return 0
  TEMP_FILES="${TEMP_FILES}${TEMP_FILES:+
}$1"
}

cleanup_temp_files() {
  [ -n "${TEMP_FILES:-}" ] || return 0
  local temp_file
  while IFS= read -r temp_file; do
    [ -n "$temp_file" ] && rm -f "$temp_file"
  done <<EOF_CLEANUP
$TEMP_FILES
EOF_CLEANUP
}
trap cleanup_temp_files EXIT

TAB="$(printf '\t')"

print_kv() {
  printf '%s: %s\n' "$1" "$2"
}

sanitize_tsv() {
  printf '%s' "$1" | tr '\t' ' '
}

shell_quote() {
  printf '%q' "$1"
}

SCRIPT_PATH="${PRE_COMMIT_REVIEW_HELPER_PATH:-$(CDPATH='' cd -- "$(dirname -- "$0")" && pwd -P)/$(basename -- "$0")}"
REQUEST_PATH=''
REQUEST_SOURCE=''
REQUEST_GROUP=''
INCLUDE_DIFF_MODE="${PRE_COMMIT_REVIEW_INCLUDE_DIFF:-auto}"

usage() {
  cat <<'USAGE'
Usage: collect_diff_context.sh [--source staged|unstaged|branch] [--path PATH | --group GROUP_ID] [--plan-only | --include-diff auto|never|always]

Collect read-only Git diff context for pre-commit review.

Options:
  --source SOURCE  Read from one diff source: staged, unstaged, or branch.
  --path PATH      Emit file-specific context for one changed path only.
  --group GROUP_ID Emit group-specific context for one review group only.
  --plan-only      Emit only planning metadata for the selected diff source; omit the global raw diff.
  --include-diff MODE
                   Control global diff inclusion for default output: auto, never, or always.
  -h, --help    Show this help.
USAGE
}

while [ "$#" -gt 0 ]; do
  case "$1" in
    --path)
      shift
      if [ "$#" -eq 0 ]; then
        echo 'collect_diff_context.sh: --path requires a value' >&2
        exit 2
      fi
      REQUEST_PATH="$1"
      ;;
    --group)
      shift
      if [ "$#" -eq 0 ]; then
        echo 'collect_diff_context.sh: --group requires a value' >&2
        exit 2
      fi
      REQUEST_GROUP="$1"
      ;;
    --source)
      shift
      if [ "$#" -eq 0 ]; then
        echo 'collect_diff_context.sh: --source requires a value' >&2
        exit 2
      fi
      case "$1" in
        staged|unstaged|branch) REQUEST_SOURCE="$1" ;;
        *)
          printf 'collect_diff_context.sh: invalid --source value: %s\n' "$1" >&2
          usage >&2
          exit 2
          ;;
      esac
      ;;
    --plan-only)
      INCLUDE_DIFF_MODE='never'
      ;;
    --include-diff)
      shift
      if [ "$#" -eq 0 ]; then
        echo 'collect_diff_context.sh: --include-diff requires a value' >&2
        exit 2
      fi
      case "$1" in
        auto|never|always) INCLUDE_DIFF_MODE="$1" ;;
        *)
          printf 'collect_diff_context.sh: invalid --include-diff value: %s\n' "$1" >&2
          usage >&2
          exit 2
          ;;
      esac
      ;;
    -h|--help)
      usage
      exit 0
      ;;
    *)
      printf 'collect_diff_context.sh: unknown argument: %s\n' "$1" >&2
      usage >&2
      exit 2
      ;;
  esac
  shift
done

if [ -n "$REQUEST_PATH" ] && [ -n "$REQUEST_GROUP" ]; then
  echo 'collect_diff_context.sh: --path and --group are mutually exclusive' >&2
  usage >&2
  exit 2
fi

fail_no_repo() {
  echo '# Pre-Commit Review Diff Context'
  echo
  print_kv 'repository' 'not a git repository'
  print_kv 'diff_source' 'unavailable'
  print_kv 'review_limits' 'no local repository access'
  echo
  echo 'No diff available. Stage your changes or provide a diff to review.'
  exit 0
}

if ! repo_root="$(git rev-parse --show-toplevel 2>/dev/null)"; then
  fail_no_repo
fi

cd "$repo_root"

has_staged_changes() {
  set +e
  git diff --cached --quiet --exit-code -- . >/dev/null 2>&1
  local rc=$?
  set -e
  [ "$rc" -eq 1 ]
}

has_unstaged_changes() {
  set +e
  git diff --quiet --exit-code -- . >/dev/null 2>&1
  local rc=$?
  set -e
  [ "$rc" -eq 1 ]
}

has_diff_for_ref() {
  local ref="$1"
  set +e
  git diff --quiet --exit-code "$ref...HEAD" -- . >/dev/null 2>&1
  local rc=$?
  set -e
  [ "$rc" -eq 1 ]
}

detect_base_branch() {
  local base=''
  base="$(git symbolic-ref --quiet --short refs/remotes/origin/HEAD 2>/dev/null | sed 's|^origin/||' || true)"

  if [ -z "$base" ]; then
    if git rev-parse --verify --quiet origin/main >/dev/null; then
      base='main'
    elif git rev-parse --verify --quiet origin/master >/dev/null; then
      base='master'
    elif git rev-parse --verify --quiet main >/dev/null; then
      base='main'
    elif git rev-parse --verify --quiet master >/dev/null; then
      base='master'
    else
      base='main'
    fi
  fi

  printf '%s\n' "$base"
}

summarize_numstat() {
  awk '
    BEGIN { files=0; add=0; del=0 }
    NF >= 3 {
      files += 1
      if ($1 ~ /^[0-9]+$/) add += $1
      if ($2 ~ /^[0-9]+$/) del += $2
    }
    END { printf "%d files, %d insertions(+), %d deletions(-)", files, add, del }
  '
}

join_lines_csv() {
  awk '
    NF {
      if (!first_seen) {
        first_seen=1
      } else {
        printf ", "
      }
      printf "%s", $0
    }
    END {
      if (!first_seen) {
        printf "none"
      }
      printf "\n"
    }
  '
}

high_risk_paths_from_name_status() {
  awk -F '\t' '
    NF >= 2 {
      path=$2
      if ($1 ~ /^[RC]/ && NF >= 3) path=$3
      lower=tolower(path)
      risk=0
      if (lower ~ /(^|\/|[_-])(auth|authentication|permission|permissions|security|oauth|session|sessions|jwt|token|tokens|acl|rbac)(\/|[_\.-]|$)/) risk=1
      if (lower ~ /(^|\/)(db|database|sql)\/.*(migration|migrations|schema)/) risk=1
      if (lower ~ /(^|\/)(migration|migrations)(\/|$)/) risk=1
      if (lower ~ /(^|\/)(payment|payments|billing|invoice|invoices|checkout)(\/|[_\.-]|$)/) risk=1
      if (lower ~ /(^|\/)(config|configs|deploy|deployment|infra|infrastructure|terraform|k8s|kubernetes|docker|\.github\/workflows)(\/|$)/) risk=1
      if (lower ~ /(^|\/|[_-])(concurrency|async|retry|queue|worker|scheduler|delete|deletion|destroy|destructive)(\/|[_\.-]|$)/) risk=1
      if (lower ~ /(^|\/|[_-])(crypto|cryptographic|encrypt|decrypt|hash|hashing|sha|sha256|md5|rsa|aes|tls|ssl|cert|certificate|bcrypt|argon2)(\/|[_\.-]|$)/) risk=1
      if (lower ~ /(^|\/|[_-])(secret|secrets|credential|credentials|api[_-]?key|apikey|vault|keychain)(\/|[_\.-]|$)/) risk=1
      if (lower ~ /(^|\/|[_-])(cors|csrf|xss|sanitize|sanitizer|escape)(\/|[_\.-]|$)/) risk=1
      if (lower ~ /(^|\/|[_-])(role|roles|admin|superuser|root|sudo|policy|policies)(\/|[_\.-]|$)/) risk=1
      if (lower ~ /(^|\/|[_-])(exec|eval|spawn|subprocess|shell|command|cmd)(\/|[_\.-]|$)/) risk=1
      if (lower ~ /(^|\/|[_-])(upload|download|attachment|attachments|file|files)(\/|[_\.-]|$)/) risk=1
      if (lower ~ /(^|\/|[_-])(env|environment|settings|configure)(\/|[_\.-]|$)/) risk=1
      if (risk) {
        print path
      }
    }
  ' | sort -u
}

content_risk_paths_from_diff() {
  awk '
    /^\+\+\+ b\// {
      current=$0
      sub(/^\+\+\+ b\//, "", current)
      next
    }
    /^\+\+\+ / {
      current=""
      next
    }
    /^[+-][^+-]/ {
      if (current == "") next
      lower=tolower($0)
      risk=0
      if (lower ~ /(authorization|authenticate|authentication|permission|permissions|is_admin|oauth|jwt|session|token|secret|password|credential)/) risk=1
      if (lower ~ /(alter[[:space:]]+table|drop[[:space:]]+table|delete[[:space:]]+from|truncate[[:space:]]+table|grant[[:space:]]+|revoke[[:space:]]+)/) risk=1
      if (lower ~ /(payment|billing|invoice|checkout|refund)/) risk=1
      if (lower ~ /(retry|timeout|queue|worker|scheduler|transaction)/) risk=1
      if (lower ~ /(crypto\.|createcipher|hashlib\.|sha256|sha512|md5|bcrypt\.compare|argon2|aes|rsa|x509|tls|ssl)/) risk=1
      if (lower ~ /(process\.env\.[a-z0-9_]*(secret|token|key|password)|os\.environ.*(secret|token|key|password)|api[_-]?key|secret[_-]?key|private[_-]?key)/) risk=1
      if (lower ~ /(eval[[:space:]]*\(|exec[[:space:]]*\(|subprocess\.|child_process|spawn[[:space:]]*\(|system[[:space:]]*\()/) risk=1
      if (lower ~ /(cors|csrf|xss|sanitize|sanitizer|escapehtml|escape_html)/) risk=1
      if (lower ~ /(fs\.unlink|os\.remove|drop[[:space:]]+database|grant[[:space:]]+all|chmod[[:space:]]+777|sudo[[:space:]])/) risk=1
      if (risk) {
        print current
      }
    }
  ' "$1" | sort -u
}

generated_like_paths_from_name_status() {
  awk -F '\t' '
    NF >= 2 {
      path=$2
      if ($1 ~ /^[RC]/ && NF >= 3) path=$3
      lower=tolower(path)
      generated=0
      if (lower ~ /(^|\/)(__snapshots__|snapshots|generated|vendor|vendors|dist|build|coverage)(\/|$)/) generated=1
      if (lower ~ /(\.snap|\.snapshot|\.generated\.|_generated\.|\.min\.(js|css))$/) generated=1
      if (generated) {
        print path
      }
    }
  ' | sort -u
}

lock_paths_from_name_status() {
  awk -F '\t' '
    NF >= 2 {
      path=$2
      if ($1 ~ /^[RC]/ && NF >= 3) path=$3
      lower=tolower(path)
      if (lower ~ /(^|\/)(package-lock\.json|npm-shrinkwrap\.json|yarn\.lock|pnpm-lock\.yaml|poetry\.lock|pipfile\.lock|cargo\.lock|gemfile\.lock|composer\.lock|go\.sum)$/) {
        print path
      }
    }
  ' | sort -u
}

configured_risk_paths_from_name_status() {
  local patterns_file="$repo_root/.pre-commit-review/risk-paths"

  if [ ! -f "$patterns_file" ]; then
    cat >/dev/null
    return 0
  fi
  awk -F '\t' '
    NR == FNR {
      line=$0
      sub(/\r$/, "", line)
      if (line ~ /^[[:space:]]*($|#)/) next
      sub(/^[[:space:]]+/, "", line)
      sub(/[[:space:]]+$/, "", line)
      patterns[++pattern_count]=line
      next
    }
    pattern_count && NF >= 2 {
      path=$2
      if ($1 ~ /^[RC]/ && NF >= 3) path=$3
      for (i=1; i<=pattern_count; i++) {
        if (path ~ patterns[i]) {
          print path
          break
        }
      }
    }
  ' "$patterns_file" - | sort -u
}

configured_content_risk_paths_from_diff() {
  local patterns_file="$repo_root/.pre-commit-review/risk-content"
  local diff_file="$1"

  [ -f "$patterns_file" ] || return 0
  awk '
    NR == FNR {
      line=$0
      sub(/\r$/, "", line)
      if (line ~ /^[[:space:]]*($|#)/) next
      sub(/^[[:space:]]+/, "", line)
      sub(/[[:space:]]+$/, "", line)
      patterns[++pattern_count]=line
      next
    }
    /^\+\+\+ b\// {
      current=$0
      sub(/^\+\+\+ b\//, "", current)
      next
    }
    /^\+\+\+ / {
      current=""
      next
    }
    /^[+-][^+-]/ {
      if (current == "" || !pattern_count) next
      line=substr($0, 2)
      for (i=1; i<=pattern_count; i++) {
        if (line ~ patterns[i]) {
          print current
          break
        }
      }
    }
  ' "$patterns_file" "$diff_file" | sort -u
}

top_churn_paths_from_numstat() {
  awk -F '\t' '
    NF >= 3 {
      add=($1 ~ /^[0-9]+$/) ? $1 : 0
      del=($2 ~ /^[0-9]+$/) ? $2 : 0
      total=add + del
      printf "%010d\t%s (+%d/-%d)\n", total, $3, add, del
    }
  ' | sort -rn | head -n 5 | cut -f2-
}

lines_contains_path() {
  local lines="$1"
  local path="$2"

  [ -n "$lines" ] || return 1
  grep -Fxq "$path" <<EOF_LINES
$lines
EOF_LINES
}

path_has_high_risk_signal() {
  local path="$1"

  awk -v path="$path" '
    BEGIN {
      lower=tolower(path)
      risk=0
      if (lower ~ /(^|\/|[_-])(auth|authentication|permission|permissions|security|oauth|session|sessions|jwt|token|tokens|acl|rbac)(\/|[_\.-]|$)/) risk=1
      if (lower ~ /(^|\/)(db|database|sql)\/.*(migration|migrations|schema)/) risk=1
      if (lower ~ /(^|\/)(migration|migrations)(\/|$)/) risk=1
      if (lower ~ /(^|\/)(payment|payments|billing|invoice|invoices|checkout)(\/|[_\.-]|$)/) risk=1
      if (lower ~ /(^|\/)(config|configs|deploy|deployment|infra|infrastructure|terraform|k8s|kubernetes|docker|\.github\/workflows)(\/|$)/) risk=1
      if (lower ~ /(^|\/|[_-])(concurrency|async|retry|queue|worker|scheduler|delete|deletion|destroy|destructive)(\/|[_\.-]|$)/) risk=1
      if (lower ~ /(^|\/|[_-])(crypto|cryptographic|encrypt|decrypt|hash|hashing|sha|sha256|md5|rsa|aes|tls|ssl|cert|certificate|bcrypt|argon2)(\/|[_\.-]|$)/) risk=1
      if (lower ~ /(^|\/|[_-])(secret|secrets|credential|credentials|api[_-]?key|apikey|vault|keychain)(\/|[_\.-]|$)/) risk=1
      if (lower ~ /(^|\/|[_-])(cors|csrf|xss|sanitize|sanitizer|escape)(\/|[_\.-]|$)/) risk=1
      if (lower ~ /(^|\/|[_-])(role|roles|admin|superuser|root|sudo|policy|policies)(\/|[_\.-]|$)/) risk=1
      if (lower ~ /(^|\/|[_-])(exec|eval|spawn|subprocess|shell|command|cmd)(\/|[_\.-]|$)/) risk=1
      if (lower ~ /(^|\/|[_-])(upload|download|attachment|attachments|file|files)(\/|[_\.-]|$)/) risk=1
      if (lower ~ /(^|\/|[_-])(env|environment|settings|configure)(\/|[_\.-]|$)/) risk=1
      exit risk ? 0 : 1
    }
  '
}

path_is_generated_like() {
  local path="$1"

  awk -v path="$path" '
    BEGIN {
      lower=tolower(path)
      generated=0
      if (lower ~ /(^|\/)(__snapshots__|snapshots|generated|vendor|vendors|dist|build|coverage)(\/|$)/) generated=1
      if (lower ~ /(\.snap|\.snapshot|\.generated\.|_generated\.|\.min\.(js|css))$/) generated=1
      exit generated ? 0 : 1
    }
  '
}

path_is_lockfile() {
  local path="$1"

  awk -v path="$path" '
    BEGIN {
      lower=tolower(path)
      exit (lower ~ /(^|\/)(package-lock\.json|npm-shrinkwrap\.json|yarn\.lock|pnpm-lock\.yaml|poetry\.lock|pipfile\.lock|cargo\.lock|gemfile\.lock|composer\.lock|go\.sum)$/) ? 0 : 1
    }
  '
}

safe_group_component() {
  printf '%s' "$1" | sed 's/[^A-Za-z0-9._-]/_/g'
}

group_component_for_path() {
  local path="$1"
  local first
  local rest
  local second

  first="${path%%/*}"
  rest="${path#*/}"
  if [ "$rest" != "$path" ]; then
    second="${rest%%/*}"
    case "$second" in
      migration|migrations|schema|schemas)
        printf '%s-%s\n' "$first" "$second"
        return
        ;;
    esac
  fi
  printf '%s\n' "$first"
}

file_diff_for_path() {
  local path="$1"

  case "$mode" in
    staged) git -c color.ui=false diff --no-ext-diff --find-renames --cached -- "$path" ;;
    unstaged) git -c color.ui=false diff --no-ext-diff --find-renames -- "$path" ;;
    branch) git -c color.ui=false diff --no-ext-diff --find-renames "$selected_ref...HEAD" -- "$path" ;;
    *) return 1 ;;
  esac
}

file_content_for_path() {
  local path="$1"

  case "$mode" in
    staged)
      git show ":$path" 2>/dev/null || { [ -f "$path" ] && cat -- "$path"; }
      ;;
    unstaged)
      [ -f "$path" ] && cat -- "$path"
      ;;
    branch)
      git show "HEAD:$path" 2>/dev/null || { [ -f "$path" ] && cat -- "$path"; }
      ;;
    *)
      return 1
      ;;
  esac
}

file_name_status_for_path() {
  local path="$1"

  case "$mode" in
    staged) git -c color.ui=false diff --no-ext-diff --find-renames --cached --name-status -- "$path" ;;
    unstaged) git -c color.ui=false diff --no-ext-diff --find-renames --name-status -- "$path" ;;
    branch) git -c color.ui=false diff --no-ext-diff --find-renames --name-status "$selected_ref...HEAD" -- "$path" ;;
    *) return 1 ;;
  esac
}

file_numstat_for_path() {
  local path="$1"

  case "$mode" in
    staged) git -c color.ui=false diff --no-ext-diff --find-renames --cached --numstat -- "$path" ;;
    unstaged) git -c color.ui=false diff --no-ext-diff --find-renames --numstat -- "$path" ;;
    branch) git -c color.ui=false diff --no-ext-diff --find-renames --numstat "$selected_ref...HEAD" -- "$path" ;;
    *) return 1 ;;
  esac
}

file_stat_for_path() {
  local path="$1"

  case "$mode" in
    staged) git -c color.ui=false diff --no-ext-diff --find-renames --cached --stat -- "$path" ;;
    unstaged) git -c color.ui=false diff --no-ext-diff --find-renames --stat -- "$path" ;;
    branch) git -c color.ui=false diff --no-ext-diff --find-renames --stat "$selected_ref...HEAD" -- "$path" ;;
    *) return 1 ;;
  esac
}

file_numstat_counts_for_path() {
  local path="$1"

  file_numstat_for_path "$path" | awk -F '\t' '
    NR == 1 {
      add=($1 ~ /^[0-9]+$/) ? $1 : 0
      del=($2 ~ /^[0-9]+$/) ? $2 : 0
      printf "%s\t%s\n", add, del
      found=1
      exit
    }
    END {
      if (!found) {
        print "0\t0"
      }
    }
  '
}

file_numstat_counts_for_rename() {
  local old_path="$1"
  local new_path="$2"

  run_diff --numstat | awk -F '\t' -v old_path="$old_path" -v new_path="$new_path" '
    NF >= 3 && index($3, "=>") && index($3, old_path) && index($3, new_path) {
      add=($1 ~ /^[0-9]+$/) ? $1 : 0
      del=($2 ~ /^[0-9]+$/) ? $2 : 0
      printf "%s\t%s\n", add, del
      found=1
      exit
    }
    END {
      if (!found) {
        print "0\t0"
      }
    }
  '
}

review_command_for_path() {
  local path="$1"
  local quoted_path
  local quoted_ref_expr

  quoted_path="$(shell_quote "$path")"

  case "$mode" in
    staged) printf 'git diff --cached -- %s\n' "$quoted_path" ;;
    unstaged) printf 'git diff -- %s\n' "$quoted_path" ;;
    branch)
      quoted_ref_expr="$(shell_quote "${selected_ref}...HEAD")"
      printf 'git diff %s -- %s\n' "$quoted_ref_expr" "$quoted_path"
      ;;
    *) printf 'unavailable\n' ;;
  esac
}

context_command_for_path() {
  local path="$1"
  local quoted_script
  local quoted_path
  local quoted_source

  quoted_script="$(shell_quote "$SCRIPT_PATH")"
  quoted_path="$(shell_quote "$path")"
  quoted_source="$(shell_quote "$mode")"
  printf '%s --source %s --path %s\n' "$quoted_script" "$quoted_source" "$quoted_path"
}

context_command_for_group() {
  local group_id="$1"
  local quoted_script
  local quoted_group
  local quoted_source

  quoted_script="$(shell_quote "$SCRIPT_PATH")"
  quoted_group="$(shell_quote "$group_id")"
  quoted_source="$(shell_quote "$mode")"
  printf '%s --source %s --group %s\n' "$quoted_script" "$quoted_source" "$quoted_group"
}

emit_hunk_split_suggestions_for_path() {
  local parent_group="$1"
  local path="$2"
  local review_command="$3"

  file_diff_for_path "$path" | LC_ALL=C awk -v parent_group="$parent_group" -v path="$path" -v review_command="$review_command" '
    function emit_hunk() {
      if (hunk_index > 0) {
        clean_header=hunk_header
        gsub(/\t/, " ", clean_header)
        printf "%s\thunk:%s:%d\t%s\thunk\t%d\t%s\t%s\n", parent_group, path, hunk_index, path, hunk_bytes, clean_header, review_command
      }
    }
    /^@@ / {
      emit_hunk()
      hunk_index += 1
      hunk_header=$0
      hunk_bytes=length($0) + 1
      next
    }
    hunk_index > 0 {
      hunk_bytes += length($0) + 1
    }
    END {
      emit_hunk()
      if (hunk_index == 0) {
        printf "%s\tfile:%s\t%s\tfile\t0\tnone\t%s\n", parent_group, path, path, review_command
      }
    }
  '
}

emit_hunk_split_previews_for_path() {
  local parent_group="$1"
  local path="$2"

  file_diff_for_path "$path" | awk -v parent_group="$parent_group" -v path="$path" '
    function emit_hunk() {
      if (hunk_index > 0) {
        printf "unit_id: hunk:%s:%d\n", path, hunk_index
        printf "parent_group_id: %s\n", parent_group
        print "```diff"
        printf "%s", hunk_text
        print "```"
      }
    }
    /^@@ / {
      emit_hunk()
      hunk_index += 1
      hunk_text=$0 "\n"
      next
    }
    hunk_index > 0 {
      hunk_text=hunk_text $0 "\n"
    }
    END {
      emit_hunk()
    }
  '
}

build_review_manifest_tmp() {
  local manifest_tmp="$1"
  local add
  local del
  local path
  local status
  local diff_bytes
  local risk_tags
  local group_id
  local top_component
  local safe_component
  local review_command
  local context_command
  local old_path
  local new_path

  printf 'unit_id\tpath\tstatus\tadditions\tdeletions\tdiff_bytes\trisk_tags\tgroup_id\treview_command\tcontext_command\n' > "$manifest_tmp"

  run_diff --name-status | while IFS="$(printf '\t')" read -r status path new_path; do
    [ -n "$path" ] || continue
    old_path=''
    case "$status" in
      R*|C*)
        old_path="$path"
        path="$new_path"
        ;;
    esac
    [ -n "$path" ] || continue
    if [ -n "$old_path" ]; then
      read -r add del < <(file_numstat_counts_for_rename "$old_path" "$path")
    else
      read -r add del < <(file_numstat_counts_for_path "$path")
    fi
    diff_bytes="$(file_diff_for_path "$path" | wc -c | tr -d ' ')"
    top_component="$(group_component_for_path "$path")"
    safe_component="$(safe_group_component "$top_component")"

    if lines_contains_path "$high_risk_candidate_lines" "$path" || path_has_high_risk_signal "$path"; then
      risk_tags='high-risk'
      group_id="high-risk-${safe_component}"
    elif lines_contains_path "$generated_like_file_lines" "$path" || path_is_generated_like "$path"; then
      risk_tags='generated-like'
      group_id="consistency-${safe_component}"
    elif lines_contains_path "$lock_file_lines" "$path" || path_is_lockfile "$path"; then
      risk_tags='lockfile'
      group_id='consistency-lockfiles'
    else
      risk_tags='medium'
      group_id="module-${safe_component}"
    fi

    review_command="$(review_command_for_path "$path")"
    context_command="$(context_command_for_path "$path")"
    printf 'file:%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\n' "$path" "$path" "${status:-unknown}" "$add" "$del" "$diff_bytes" "$risk_tags" "$group_id" "$review_command" "$context_command" >> "$manifest_tmp"
  done
}

emit_review_plan_json() {
  local manifest_tmp="$1"
  local quoted_script
  local quoted_source

  quoted_script="$(shell_quote "$SCRIPT_PATH")"
  quoted_source="$(shell_quote "$mode")"

  echo '## Review Plan JSON'
  awk -F "$TAB" -v source="$mode" -v target="$GROUP_TARGET_BYTES" -v hard="$GROUP_HARD_BYTES" -v quoted_script="$quoted_script" -v quoted_source="$quoted_source" '
    function json_escape(value) {
      gsub(/\\/, "\\\\", value)
      gsub(/"/, "\\\"", value)
      gsub(/\r/, "\\r", value)
      gsub(/\t/, "\\t", value)
      return value
    }
    function json_string(value) {
      return "\"" json_escape(value) "\""
    }
    function append_json(array_value, value) {
      if (array_value == "") return json_string(value)
      return array_value "," json_string(value)
    }
    NR == 1 { next }
    {
      manifest_units += 1
      group=$8
      if (!(group in seen_groups)) {
        seen_groups[group]=1
        group_order[++group_count]=group
      }
      bytes[group] += $6
      units[group]=append_json(units[group], $1)
      files[group]=append_json(files[group], $2)
      commands[group]=append_json(commands[group], $9)
      if ($7 == "high-risk") {
        risk[group]="high"
        reason[group]="path-or-content-risk"
        high_risk_units += 1
      } else if ($7 == "generated-like") {
        if (risk[group] != "high") {
          risk[group]="consistency"
          reason[group]="generated-like"
        }
      } else if ($7 == "lockfile") {
        if (risk[group] != "high") {
          risk[group]="consistency"
          reason[group]="lockfile"
        }
      } else if (risk[group] == "") {
        risk[group]="medium"
        reason[group]="module"
      }
    }
    END {
      for (i=1; i<=group_count; i++) {
        group=group_order[i]
        budget_status[group]="ok"
        action[group]="review"
        split_source[group]="none"
        priority[group]=4
        notes[group]="review-complete-group-before-coverage-validation"
        if (bytes[group] > hard) {
          budget_status[group]="split-required"
          action[group]="split"
          split_source[group]="Split Suggestions and Split Unit Diff Preview"
          notes[group]="replace-with-split-suggestions-before-review"
          priority[group]=1
          split_required_groups += 1
        } else if (bytes[group] > target) {
          budget_status[group]="over-target"
          if (risk[group] == "high") priority[group]=2
          else if (risk[group] == "consistency") priority[group]=3
        } else if (risk[group] == "high") {
          priority[group]=2
        } else if (risk[group] == "consistency") {
          priority[group]=3
        }
      }
      for (i=1; i<=group_count; i++) {
        for (j=i+1; j<=group_count; j++) {
          left=group_order[i]
          right=group_order[j]
          if (priority[right] < priority[left] || (priority[right] == priority[left] && right < left)) {
            group_order[i]=right
            group_order[j]=left
          }
        }
      }

      printf "{\n"
      printf "  \"schema_version\":1,\n"
      printf "  \"source\":%s,\n", json_string(source)
      printf "  \"group_target_bytes\":%d,\n", target
      printf "  \"group_hard_bytes\":%d,\n", hard
      printf "  \"manifest_units\":%d,\n", manifest_units
      printf "  \"review_groups\":%d,\n", group_count
      printf "  \"split_required_groups\":%d,\n", split_required_groups
      printf "  \"high_risk_units\":%d,\n", high_risk_units
      printf "  \"context_mode\":\"group\",\n"
      printf "  \"state_snapshot_section\":\"Reducer State Snapshot Template\",\n"
      printf "  \"semantic_context_section\":\"Semantic Context Queries\",\n"
      printf "  \"groups\":["
      for (i=1; i<=group_count; i++) {
        group=group_order[i]
        if (i > 1) printf ","
        context_command=quoted_script " --source " quoted_source " --group " group
        printf "\n    {"
        printf "\"group_id\":%s,", json_string(group)
        printf "\"risk\":%s,", json_string(risk[group])
        printf "\"reason\":%s,", json_string(reason[group])
        printf "\"priority\":%d,", priority[group]
        printf "\"action\":%s,", json_string(action[group])
        printf "\"budget_status\":%s,", json_string(budget_status[group])
        printf "\"diff_bytes\":%d,", bytes[group]
        printf "\"required_units\":[%s],", units[group]
        printf "\"files\":[%s],", files[group]
        printf "\"review_commands\":[%s],", commands[group]
        printf "\"context_mode\":\"group\","
        printf "\"context_command\":%s,", json_string(context_command)
        printf "\"split_source\":%s,", json_string(split_source[group])
        printf "\"notes\":%s", json_string(notes[group])
        printf "}"
      }
      printf "\n  ],\n"
      printf "  \"coverage_validation\":{"
      printf "\"rule\":\"manifest_units - reviewed_units must be empty before claiming full review\","
      printf "\"blocking_rule\":\"high-risk or needs-split coverage gaps force DO_NOT_COMMIT\""
      printf "}\n"
      printf "}\n"
    }
  ' "$manifest_tmp"
}

emit_reducer_state_snapshot_template() {
  local manifest_tmp="$1"

  echo '## Reducer State Snapshot Template'
  awk -F "$TAB" -v hard="$GROUP_HARD_BYTES" -v source="$mode" '
    function json_escape(value) {
      gsub(/\\/, "\\\\", value)
      gsub(/"/, "\\\"", value)
      gsub(/\r/, "\\r", value)
      gsub(/\t/, "\\t", value)
      return value
    }
    function json_string(value) {
      return "\"" json_escape(value) "\""
    }
    function append_json(array_value, value) {
      if (array_value == "") return json_string(value)
      return array_value "," json_string(value)
    }
    function append_gap(array_value, unit, group, risk, status) {
      gap="{\"unit_id\":" json_string(unit) ",\"group_id\":" json_string(group) ",\"risk_tags\":" json_string(risk) ",\"coverage_status\":" json_string(status) "}"
      if (array_value == "") return gap
      return array_value "," gap
    }
    NR == 1 { next }
    {
      rows[++row_count]=$0
      group=$8
      bytes[group] += $6
      groups[group]=1
      risk[group]=$7
      units[group]=append_json(units[group], $1)
      pending_units=append_json(pending_units, $1)
    }
    END {
      for (i=1; i<=row_count; i++) {
        split(rows[i], fields, "\t")
        status="pending"
        if (bytes[fields[8]] > hard) status="needs-split"
        coverage_gaps=append_gap(coverage_gaps, fields[1], fields[8], fields[7], status)
        if (status == "needs-split") {
          needs_split_units=append_json(needs_split_units, fields[1])
        }
      }
      for (group in groups) group_count += 1
      printf "{"
      printf "\"schema_version\":1,"
      printf "\"state_kind\":\"reducer_state_snapshot\","
      printf "\"source\":%s,", json_string(source)
      printf "\"status\":\"pending_group_reviews\","
      printf "\"manifest_units\":%d,", row_count
      printf "\"review_groups\":%d,", group_count
      printf "\"reviewed_units\":[],"
      printf "\"pending_units\":[%s],", pending_units
      printf "\"needs_split_units\":[%s],", needs_split_units
      printf "\"group_results\":[],"
      printf "\"coverage_gaps\":[%s],", coverage_gaps
      printf "\"finding_merge\":{\"deduplicated_findings\":[],\"blockers\":[],\"notes\":[]},"
      printf "\"dependency_checks\":[],"
      printf "\"test_recommendations\":[],"
      printf "\"final_verdict\":\"blocked_until_coverage_validation_passes\","
      printf "\"persistence_rule\":\"carry this compact state forward after each group result; update reviewed_units, pending_units, group_results, coverage_gaps, and finding_merge before reducer finalization\""
      printf "}\n"
    }
  ' "$manifest_tmp"
}

emit_review_manifest_and_groups() {
  local manifest_tmp
  local split_tmp
  local quoted_script
  local quoted_source

  manifest_tmp="$(mktemp)"
  split_tmp="$(mktemp)"
  register_temp_file "$manifest_tmp"
  register_temp_file "$split_tmp"
  build_review_manifest_tmp "$manifest_tmp"

  if [ "${compact_plan:-no}" = 'yes' ]; then
    echo '## Review Manifest JSONL'
    awk -F "$TAB" '
      function json_escape(value) {
        gsub(/\\/, "\\\\", value)
        gsub(/"/, "\\\"", value)
        gsub(/\r/, "\\r", value)
        gsub(/\t/, "\\t", value)
        return value
      }
      function json_string(value) {
        return "\"" json_escape(value) "\""
      }
      function json_string_array(value, parts, count, i, result) {
        count=split(value, parts, ";")
        result=""
        for (i=1; i<=count; i++) {
          if (parts[i] == "") continue
          if (result != "") result=result ","
          result=result json_string(parts[i])
        }
        return result
      }
      NR == 1 { next }
      {
        printf "{"
        printf "\"unit_id\":%s,", json_string($1)
        printf "\"path\":%s,", json_string($2)
        printf "\"status\":%s,", json_string($3)
        printf "\"additions\":%d,", $4
        printf "\"deletions\":%d,", $5
        printf "\"diff_bytes\":%d,", $6
        printf "\"risk_tags\":[%s],", json_string_array($7)
        printf "\"group_id\":%s,", json_string($8)
        printf "\"review_command\":%s,", json_string($9)
        printf "\"context_command\":%s", json_string($10)
        print "}"
      }
    ' "$manifest_tmp"

    echo
    echo '## Review Groups JSONL'
    awk -F "$TAB" -v target="$GROUP_TARGET_BYTES" -v hard="$GROUP_HARD_BYTES" '
      function json_escape(value) {
        gsub(/\\/, "\\\\", value)
        gsub(/"/, "\\\"", value)
        gsub(/\r/, "\\r", value)
        gsub(/\t/, "\\t", value)
        return value
      }
      function json_string(value) {
        return "\"" json_escape(value) "\""
      }
      NR == 1 { next }
      {
        group=$8
        bytes[group] += $6
        if (files_json[group] == "") files_json[group]=json_string($2)
        else files_json[group]=files_json[group] "," json_string($2)
        if ($7 == "high-risk") {
          risk[group]="high"
          reason[group]="path-or-content-risk"
        } else if ($7 == "generated-like") {
          risk[group]="consistency"
          reason[group]="generated-like"
        } else if ($7 == "lockfile") {
          risk[group]="consistency"
          reason[group]="lockfile"
        } else {
          risk[group]="medium"
          reason[group]="module"
        }
      }
      END {
        for (group in files_json) {
          budget_status="ok"
          if (bytes[group] > hard) budget_status="split-required"
          else if (bytes[group] > target) budget_status="over-target"
          printf "%s\t", group
          printf "{"
          printf "\"group_id\":%s,", json_string(group)
          printf "\"risk\":%s,", json_string(risk[group])
          printf "\"reason\":%s,", json_string(reason[group])
          printf "\"diff_bytes\":%d,", bytes[group]
          printf "\"files\":[%s],", files_json[group]
          printf "\"budget_status\":%s", json_string(budget_status)
          print "}"
        }
      }
    ' "$manifest_tmp" | sort -k1,1 | cut -f2-

    echo
    emit_review_plan_json "$manifest_tmp"

    awk -F "$TAB" -v hard="$GROUP_HARD_BYTES" -v tab="$TAB" '
      NR == 1 { next }
      {
        group=$8
        bytes[group] += $6
        rows[++row_count]=$0
      }
      END {
        for (i=1; i<=row_count; i++) {
          split(rows[i], fields, tab)
          group=fields[8]
          if (bytes[group] > hard) {
            printf "%s\t%s\t%s\n", group, fields[2], fields[9]
          }
        }
      }
    ' "$manifest_tmp" > "$split_tmp"

    echo
    echo '## Split Suggestions'
    echo 'parent_group_id	unit_id	path	split_kind	diff_bytes	hunk_header	review_command'
    if [ -s "$split_tmp" ]; then
      while IFS="$(printf '\t')" read -r parent_group path review_command; do
        emit_hunk_split_suggestions_for_path "$parent_group" "$path" "$review_command"
      done < "$split_tmp"
    else
      echo 'none	none	none	none	0	none	none'
    fi

    echo
    echo '## Coverage Ledger Template'
    echo 'unit_id	group_id	path	coverage_status	coverage_mode	notes'
    awk -F "$TAB" -v hard="$GROUP_HARD_BYTES" -v tab="$TAB" '
      NR == 1 { next }
      {
        rows[++row_count]=$0
        group=$8
        bytes[group] += $6
      }
      END {
        for (i=1; i<=row_count; i++) {
          split(rows[i], fields, tab)
          unit_id=fields[1]
          path=fields[2]
          group=fields[8]
          if (bytes[group] > hard) {
            printf "%s\t%s\t%s\tneeds-split\treplace-with-split-suggestions\tsplit-required group\n", unit_id, group, path
          } else {
            printf "%s\t%s\t%s\tpending\tfile-review\trecord group result before final verdict\n", unit_id, group, path
          }
        }
      }
    ' "$manifest_tmp"

    echo
    emit_reducer_state_snapshot_template "$manifest_tmp"

    echo
    echo '## Coverage Validation Checklist'
    awk -F "$TAB" -v hard="$GROUP_HARD_BYTES" -v tab="$TAB" '
      NR == 1 { next }
      {
        rows[++row_count]=$0
        group=$8
        bytes[group] += $6
        groups[group]=1
        if ($7 == "high-risk") high_risk_units += 1
      }
      END {
        for (group in groups) {
          review_groups += 1
          if (bytes[group] > hard) split_required_groups += 1
        }
        for (i=1; i<=row_count; i++) {
          split(rows[i], fields, tab)
          if (bytes[fields[8]] > hard) needs_split_units += 1
        }
        printf "manifest_units: %d\n", row_count
        printf "review_groups: %d\n", review_groups
        printf "split_required_groups: %d\n", split_required_groups
        printf "needs_split_units: %d\n", needs_split_units
        printf "high_risk_units: %d\n", high_risk_units
        print "validation_rule: manifest_units - reviewed_units must be empty before claiming full review"
        print "blocking_rule: high-risk or needs-split coverage gaps force DO_NOT_COMMIT"
      }
    ' "$manifest_tmp"

    return 0
  fi

  echo '## Review Manifest'
  cat "$manifest_tmp"

  echo
  echo '## Review Manifest JSONL'
  awk -F "$TAB" '
    function json_escape(value) {
      gsub(/\\/, "\\\\", value)
      gsub(/"/, "\\\"", value)
      gsub(/\r/, "\\r", value)
      gsub(/\t/, "\\t", value)
      return value
    }
    function json_string(value) {
      return "\"" json_escape(value) "\""
    }
    function json_string_array(value, parts, count, i, result) {
      count=split(value, parts, ";")
      result=""
      for (i=1; i<=count; i++) {
        if (parts[i] == "") continue
        if (result != "") result=result ","
        result=result json_string(parts[i])
      }
      return result
    }
    NR == 1 { next }
    {
      printf "{"
      printf "\"unit_id\":%s,", json_string($1)
      printf "\"path\":%s,", json_string($2)
      printf "\"status\":%s,", json_string($3)
      printf "\"additions\":%d,", $4
      printf "\"deletions\":%d,", $5
      printf "\"diff_bytes\":%d,", $6
      printf "\"risk_tags\":[%s],", json_string_array($7)
      printf "\"group_id\":%s,", json_string($8)
      printf "\"review_command\":%s,", json_string($9)
      printf "\"context_command\":%s", json_string($10)
      print "}"
    }
  ' "$manifest_tmp"

  echo
  echo '## Review Groups'
  echo 'group_id	risk	reason	diff_bytes	files	budget_status'
  awk -F "$TAB" -v target="$GROUP_TARGET_BYTES" -v hard="$GROUP_HARD_BYTES" '
    NR == 1 { next }
    {
      group=$8
      bytes[group] += $6
      if (files[group] == "") files[group]=$2
      else files[group]=files[group] ";" $2
      if ($7 == "high-risk") {
        risk[group]="high"
        reason[group]="path-or-content-risk"
      } else if ($7 == "generated-like") {
        risk[group]="consistency"
        reason[group]="generated-like"
      } else if ($7 == "lockfile") {
        risk[group]="consistency"
        reason[group]="lockfile"
      } else {
        risk[group]="medium"
        reason[group]="module"
      }
    }
    END {
      for (group in files) {
        budget_status="ok"
        if (bytes[group] > hard) budget_status="split-required"
        else if (bytes[group] > target) budget_status="over-target"
        printf "%s\t%s\t%s\t%d\t%s\t%s\n", group, risk[group], reason[group], bytes[group], files[group], budget_status
      }
    }
  ' "$manifest_tmp" | sort

  echo
  echo '## Review Groups JSONL'
  awk -F "$TAB" -v target="$GROUP_TARGET_BYTES" -v hard="$GROUP_HARD_BYTES" '
    function json_escape(value) {
      gsub(/\\/, "\\\\", value)
      gsub(/"/, "\\\"", value)
      gsub(/\r/, "\\r", value)
      gsub(/\t/, "\\t", value)
      return value
    }
    function json_string(value) {
      return "\"" json_escape(value) "\""
    }
    NR == 1 { next }
    {
      group=$8
      bytes[group] += $6
      if (files_json[group] == "") files_json[group]=json_string($2)
      else files_json[group]=files_json[group] "," json_string($2)
      if ($7 == "high-risk") {
        risk[group]="high"
        reason[group]="path-or-content-risk"
      } else if ($7 == "generated-like") {
        risk[group]="consistency"
        reason[group]="generated-like"
      } else if ($7 == "lockfile") {
        risk[group]="consistency"
        reason[group]="lockfile"
      } else {
        risk[group]="medium"
        reason[group]="module"
      }
    }
    END {
      for (group in files_json) {
        budget_status="ok"
        if (bytes[group] > hard) budget_status="split-required"
        else if (bytes[group] > target) budget_status="over-target"
        printf "%s\t", group
        printf "{"
        printf "\"group_id\":%s,", json_string(group)
        printf "\"risk\":%s,", json_string(risk[group])
        printf "\"reason\":%s,", json_string(reason[group])
        printf "\"diff_bytes\":%d,", bytes[group]
        printf "\"files\":[%s],", files_json[group]
        printf "\"budget_status\":%s", json_string(budget_status)
        print "}"
      }
    }
  ' "$manifest_tmp" | sort -k1,1 | cut -f2-

  echo
  emit_review_plan_json "$manifest_tmp"

  awk -F "$TAB" -v target="$GROUP_TARGET_BYTES" -v hard="$GROUP_HARD_BYTES" -v tab="$TAB" '
    NR == 1 { next }
    {
      group=$8
      bytes[group] += $6
      rows[++row_count]=$0
    }
    END {
      for (i=1; i<=row_count; i++) {
        split(rows[i], fields, tab)
        group=fields[8]
        if (bytes[group] > hard) {
          printf "%s\t%s\t%s\n", group, fields[2], fields[9]
        }
      }
    }
  ' "$manifest_tmp" > "$split_tmp"

  echo
  echo '## Split Suggestions'
  echo 'parent_group_id	unit_id	path	split_kind	diff_bytes	hunk_header	review_command'
  if [ -s "$split_tmp" ]; then
    while IFS="$(printf '\t')" read -r parent_group path review_command; do
      emit_hunk_split_suggestions_for_path "$parent_group" "$path" "$review_command"
    done < "$split_tmp"
  else
    echo 'none	none	none	none	0	none	none'
  fi

  echo
  echo '## Split Unit Diff Preview'
  if [ -s "$split_tmp" ]; then
    while IFS="$(printf '\t')" read -r parent_group path review_command; do
      emit_hunk_split_previews_for_path "$parent_group" "$path"
    done < "$split_tmp"
  else
    echo 'none'
  fi

  echo
  echo '## Coverage Ledger Template'
  echo 'unit_id	group_id	path	coverage_status	coverage_mode	notes'
  awk -F "$TAB" -v target="$GROUP_TARGET_BYTES" -v hard="$GROUP_HARD_BYTES" -v tab="$TAB" '
    NR == 1 { next }
    {
      rows[++row_count]=$0
      group=$8
      bytes[group] += $6
    }
    END {
      for (i=1; i<=row_count; i++) {
        split(rows[i], fields, tab)
        unit_id=fields[1]
        path=fields[2]
        group=fields[8]
        if (bytes[group] > hard) {
          printf "%s\t%s\t%s\tneeds-split\treplace-with-split-suggestions\tsplit-required group\n", unit_id, group, path
        } else {
          printf "%s\t%s\t%s\tpending\tfile-review\trecord group result before final verdict\n", unit_id, group, path
        }
      }
    }
  ' "$manifest_tmp"

  echo
  echo '## Group Review Result Template'
  awk -F "$TAB" -v hard="$GROUP_HARD_BYTES" '
    function json_escape(value) {
      gsub(/\\/, "\\\\", value)
      gsub(/"/, "\\\"", value)
      return value
    }
    NR == 1 { next }
    {
      group=$8
      bytes[group] += $6
      unit=json_escape($1)
      if (units[group] == "") units[group]="\"" unit "\""
      else units[group]=units[group] ", \"" unit "\""
    }
    END {
      for (group in units) {
        coverage="pending"
        if (bytes[group] > hard) coverage="needs-split"
        printf "{"
        printf "\"group_id\":\"%s\",", json_escape(group)
        printf "\"required_units\":[%s],", units[group]
        printf "\"reviewed_units\":[],"
        printf "\"coverage\":\"%s\",", coverage
        printf "\"findings\":[],"
        printf "\"contract_changes\":[],"
        printf "\"dependencies_to_check\":[],"
        printf "\"tests_recommended\":[]"
        printf "}\n"
      }
    }
  ' "$manifest_tmp"

  echo
  emit_reducer_state_snapshot_template "$manifest_tmp"

  echo
  echo '## Coverage Validation Checklist'
  awk -F "$TAB" -v hard="$GROUP_HARD_BYTES" -v tab="$TAB" '
    NR == 1 { next }
    {
      rows[++row_count]=$0
      group=$8
      bytes[group] += $6
      groups[group]=1
      if ($7 == "high-risk") high_risk_units += 1
    }
    END {
      for (group in groups) {
        review_groups += 1
        if (bytes[group] > hard) split_required_groups += 1
      }
      for (i=1; i<=row_count; i++) {
        split(rows[i], fields, tab)
        if (bytes[fields[8]] > hard) needs_split_units += 1
      }
      printf "manifest_units: %d\n", row_count
      printf "review_groups: %d\n", review_groups
      printf "split_required_groups: %d\n", split_required_groups
      printf "needs_split_units: %d\n", needs_split_units
      printf "high_risk_units: %d\n", high_risk_units
      print "validation_rule: manifest_units - reviewed_units must be empty before claiming full review"
      print "blocking_rule: high-risk or needs-split coverage gaps force DO_NOT_COMMIT"
    }
  ' "$manifest_tmp"

  echo
  echo '## Full Review Execution Plan'
  echo 'step	action	group_id	risk	budget_status	units	notes'
  awk -F "$TAB" -v target="$GROUP_TARGET_BYTES" -v hard="$GROUP_HARD_BYTES" '
    NR == 1 { next }
    {
      group=$8
      bytes[group] += $6
      if (units[group] == "") units[group]=$1
      else units[group]=units[group] ";" $1
      if ($7 == "high-risk") {
        risk[group]="high"
      } else if ($7 == "generated-like" || $7 == "lockfile") {
        if (risk[group] != "high") risk[group]="consistency"
      } else if (risk[group] == "") {
        risk[group]="medium"
      }
    }
    END {
      for (group in units) {
        action="review"
        budget_status="ok"
        notes="review-complete-group-before-coverage-validation"
        if (bytes[group] > hard) {
          action="split"
          budget_status="split-required"
          notes="replace-with-split-suggestions-before-review"
          priority=1
        } else if (bytes[group] > target) {
          budget_status="over-target"
          if (risk[group] == "high") {
            priority=2
          } else if (risk[group] == "consistency") {
            priority=3
          } else {
            priority=4
          }
        } else if (risk[group] == "high") {
          priority=2
        } else if (risk[group] == "consistency") {
          priority=3
        } else {
          priority=4
        }
        printf "%d\t%s\t%s\t%s\t%s\t%s\t%s\n", priority, group, action, risk[group], budget_status, units[group], notes
      }
    }
  ' "$manifest_tmp" | sort -k1,1n -k2,2 | awk -F '\t' '
    {
      step += 1
      printf "%d\t%s\t%s\t%s\t%s\t%s\t%s\n", step, $3, $2, $4, $5, $6, $7
    }
  '

  echo
  echo '## Group Review Work Packets'
  quoted_script="$(shell_quote "$SCRIPT_PATH")"
  quoted_source="$(shell_quote "$mode")"
  awk -F "$TAB" -v target="$GROUP_TARGET_BYTES" -v hard="$GROUP_HARD_BYTES" -v quoted_script="$quoted_script" -v quoted_source="$quoted_source" '
    NR == 1 { next }
    {
      group=$8
      bytes[group] += $6
      if (units[group] == "") units[group]=$1
      else units[group]=units[group] ";" $1
      if (commands[group] == "") commands[group]=$9
      else commands[group]=commands[group] " ; " $9
      contexts[group]=quoted_script " --source " quoted_source " --group " group
      if ($7 == "high-risk") {
        risk[group]="high"
      } else if ($7 == "generated-like" || $7 == "lockfile") {
        if (risk[group] != "high") risk[group]="consistency"
      } else if (risk[group] == "") {
        risk[group]="medium"
      }
    }
    END {
      for (group in units) {
        budget_status="ok"
        split_source="none"
        if (bytes[group] > hard) {
          budget_status="split-required"
          split_source="Split Suggestions and Split Unit Diff Preview"
          priority=1
        } else if (bytes[group] > target) {
          budget_status="over-target"
          if (risk[group] == "high") {
            priority=2
          } else if (risk[group] == "consistency") {
            priority=3
          } else {
            priority=4
          }
        } else if (risk[group] == "high") {
          priority=2
        } else if (risk[group] == "consistency") {
          priority=3
        } else {
          priority=4
        }
        printf "%d\t%s\t%s\t%s\t%s\t%s\t%s\t%s\n", priority, group, risk[group], budget_status, units[group], commands[group], contexts[group], split_source
      }
    }
  ' "$manifest_tmp" | sort -k1,1n -k2,2 | awk -F '\t' '
    {
      print "---"
      printf "group_id: %s\n", $2
      printf "risk: %s\n", $3
      printf "budget_status: %s\n", $4
      printf "required_units: %s\n", $5
      printf "review_commands: %s\n", $6
      printf "context_command: %s\n", $7
      printf "split_source: %s\n", $8
    }
  '

  echo
  echo '## Reducer Finalization Template'
  awk -F "$TAB" '
    NR == 1 { next }
    {
      manifest_units += 1
      groups[$8]=1
      if ($7 == "high-risk") high_risk_units += 1
    }
    END {
      for (group in groups) review_groups += 1
      printf "{"
      printf "\"coverage_validation\":\"required\","
      printf "\"manifest_units\":%d,", manifest_units
      printf "\"review_groups\":%d,", review_groups
      printf "\"high_risk_units\":%d,", high_risk_units
      printf "\"coverage_gaps\":[],"
      printf "\"finding_merge\":{\"deduplicated_findings\":[],\"blockers\":[],\"notes\":[]},"
      printf "\"cross_file_reduction\":\"required_after_coverage_validation\","
      printf "\"dependency_checks\":[],"
      printf "\"test_recommendations\":[],"
      printf "\"residual_risks\":[],"
      printf "\"final_verdict\":\"blocked_until_coverage_validation_passes\""
      printf "}\n"
    }
  ' "$manifest_tmp"
  rm -f "$manifest_tmp" "$split_tmp"
}

emit_requested_group_context() {
  local requested_group="$1"
  local manifest_tmp
  local group_diff_tmp
  local summary
  local group_id
  local group_risk
  local group_bytes
  local group_units
  local group_files
  local budget_status
  local path
  local review_command

  manifest_tmp="$(mktemp)"
  group_diff_tmp="$(mktemp)"
  register_temp_file "$manifest_tmp"
  register_temp_file "$group_diff_tmp"
  build_review_manifest_tmp "$manifest_tmp"

  summary="$(
    awk -F "$TAB" -v requested_group="$requested_group" -v target="$GROUP_TARGET_BYTES" -v hard="$GROUP_HARD_BYTES" '
      NR == 1 { next }
      $8 == requested_group {
        found=1
        bytes += $6
        if (units == "") units=$1
        else units=units ";" $1
        if (files == "") files=$2
        else files=files ";" $2
        if ($7 == "high-risk") {
          risk="high"
        } else if ($7 == "generated-like" || $7 == "lockfile") {
          if (risk != "high") risk="consistency"
        } else if (risk == "") {
          risk="medium"
        }
      }
      END {
        if (!found) {
          exit 1
        }
        budget_status="ok"
        if (bytes > hard) budget_status="split-required"
        else if (bytes > target) budget_status="over-target"
        printf "%s\t%s\t%d\t%s\t%s\t%s\n", requested_group, risk, bytes, units, files, budget_status
      }
    ' "$manifest_tmp" || true
  )"

  if [ -z "$summary" ]; then
    echo '## Requested Group Diff'
    print_kv 'group_id' "$requested_group"
    echo
    echo 'No review group found for requested group in the selected diff source.'
    rm -f "$manifest_tmp" "$group_diff_tmp"
    return 0
  fi

  IFS="$TAB" read -r group_id group_risk group_bytes group_units group_files budget_status <<EOF_SUMMARY
$summary
EOF_SUMMARY

  echo '## Requested Group Files'
  echo 'status	path	unit_id	review_command'
  awk -F "$TAB" -v requested_group="$requested_group" '
    NR > 1 && $8 == requested_group {
      printf "%s\t%s\t%s\t%s\n", $3, $2, $1, $9
    }
  ' "$manifest_tmp"

  echo
  echo '## Requested Group Diff'
  print_kv 'group_id' "$group_id"
  print_kv 'risk' "$group_risk"
  print_kv 'budget_status' "$budget_status"
  print_kv 'diff_bytes' "$group_bytes"
  print_kv 'required_units' "$group_units"
  print_kv 'files' "$group_files"
  print_kv 'context_command' "$(context_command_for_group "$group_id")"

  if [ "$budget_status" = 'split-required' ]; then
    echo
    echo 'Group exceeds hard review budget; use split suggestions instead of reviewing it as one group.'
    echo
    echo '## Split Suggestions'
    echo 'parent_group_id	unit_id	path	split_kind	diff_bytes	hunk_header	review_command'
    awk -F "$TAB" -v requested_group="$requested_group" '
      NR > 1 && $8 == requested_group {
        printf "%s\t%s\n", $2, $9
      }
    ' "$manifest_tmp" | while IFS="$(printf '\t')" read -r path review_command; do
      [ -n "$path" ] || continue
      emit_hunk_split_suggestions_for_path "$requested_group" "$path" "$review_command"
    done
    echo
    echo '## Split Unit Diff Preview'
    awk -F "$TAB" -v requested_group="$requested_group" '
      NR > 1 && $8 == requested_group {
        print $2
      }
    ' "$manifest_tmp" | while IFS= read -r path; do
      [ -n "$path" ] || continue
      emit_hunk_split_previews_for_path "$requested_group" "$path"
    done
    rm -f "$manifest_tmp" "$group_diff_tmp"
    return 0
  fi

  awk -F "$TAB" -v requested_group="$requested_group" '
    NR > 1 && $8 == requested_group {
      print $2
    }
  ' "$manifest_tmp" | while IFS= read -r path; do
    [ -n "$path" ] || continue
    file_diff_for_path "$path" >> "$group_diff_tmp"
  done

  if [ ! -s "$group_diff_tmp" ]; then
    echo
    echo 'No diff available for requested group in the selected diff source.'
    rm -f "$manifest_tmp" "$group_diff_tmp"
    return 0
  fi

  emit_diff_limited "$group_diff_tmp"
  rm -f "$manifest_tmp" "$group_diff_tmp"
}

emit_dependency_summary() {
  echo '## Dependency Summary'
  printf 'file\tchange\tkind\tdetail\n'
  awk '
    function trim(line) {
      sub(/^[[:space:]]+/, "", line)
      sub(/[[:space:]]+$/, "", line)
      return line
    }
    function emit(change, kind, detail) {
      detail=trim(detail)
      gsub(/\t/, " ", detail)
      safe_current=current
      gsub(/\t/, " ", safe_current)
      if (current != "" && detail != "") {
        printf "%s\t%s\t%s\t%s\n", safe_current, change, kind, detail
        seen=1
      }
    }
    /^\+\+\+ b\// {
      current=$0
      sub(/^\+\+\+ b\//, "", current)
      next
    }
    /^\+\+\+ / {
      current=""
      next
    }
    /^[+-][^+-]/ {
      if (current == "") next
      change=substr($0, 1, 1) == "+" ? "added" : "removed"
      line=substr($0, 2)
      clean=trim(line)
      lower=tolower(clean)

      if (clean ~ /^(import[[:space:]].*|from[[:space:]].*[[:space:]]import[[:space:]].*|.*require\(.+\).*)$/) {
        emit(change, "import", clean)
      }
      if (clean ~ /^export[[:space:]]/) {
        emit(change, "export", clean)
      }
      if (clean ~ /^(export[[:space:]]+)?(async[[:space:]]+)?function[[:space:]][A-Za-z0-9_$]+[[:space:]]*\(/ ||
          clean ~ /^(export[[:space:]]+)?class[[:space:]][A-Za-z0-9_$]+/ ||
          clean ~ /^(export[[:space:]]+)?interface[[:space:]][A-Za-z0-9_$]+/ ||
          clean ~ /^(export[[:space:]]+)?type[[:space:]][A-Za-z0-9_$]+/ ||
          clean ~ /^def[[:space:]][A-Za-z0-9_]+[[:space:]]*\(/) {
        emit(change, "signature", clean)
      }
      if (lower ~ /^(alter[[:space:]]+table|create[[:space:]]+table|drop[[:space:]]+table|create[[:space:]]+index|drop[[:space:]]+index|grant[[:space:]]+|revoke[[:space:]]+)/) {
        emit(change, "schema", clean)
      }
    }
    END {
      if (!seen) {
        print "none\tnone\tnone\tnone"
      }
    }
  ' "$diff_tmp"
}

emit_semantic_context_queries() {
  local queries_file="$repo_root/.pre-commit-review/context-queries"
  local query
  local output_tmp
  local error_tmp
  local status
  local safe_query

  echo '## Semantic Context Queries'
  printf 'query\tfile\tline\tmatch\n'

  if [ ! -f "$queries_file" ]; then
    echo 'none	none	0	no context queries configured'
    return 0
  fi

  while IFS= read -r query || [ -n "$query" ]; do
    case "$query" in
      ''|'#'*) continue ;;
    esac

    safe_query="$(sanitize_tsv "$query")"
    output_tmp="$(mktemp)"
    error_tmp="$(mktemp)"
    register_temp_file "$output_tmp"
    register_temp_file "$error_tmp"
    set +e
    case "$mode" in
      staged) git grep --cached -n -I -E -e "$query" -- . >"$output_tmp" 2>"$error_tmp" ;;
      unstaged) git grep -n -I -E -e "$query" -- . >"$output_tmp" 2>"$error_tmp" ;;
      branch) git grep -n -I -E -e "$query" HEAD -- . >"$output_tmp" 2>"$error_tmp" ;;
      *) : >"$output_tmp" ;;
    esac
    status=$?
    set -e

    if [ "$status" -gt 1 ]; then
      printf '%s\terror\t0\tinvalid or unsupported git grep ERE\n' "$safe_query"
      rm -f "$output_tmp" "$error_tmp"
      continue
    fi

    if [ "$mode" = 'branch' ]; then
      sed 's/^HEAD://' "$output_tmp" >"${output_tmp}.normalized"
      mv "${output_tmp}.normalized" "$output_tmp"
    fi

    awk -F ':' -v query="$safe_query" -v limit="$CONTEXT_QUERY_LIMIT" '
      NF >= 3 && count < limit {
        file=$1
        if (file == ".pre-commit-review/context-queries") {
          next
        }
        line=$2
        match_text=$0
        sub(/^[^:]*:[^:]*:/, "", match_text)
        gsub(/\t/, " ", file)
        gsub(/\t/, " ", line)
        gsub(/\t/, " ", match_text)
        printf "%s\t%s\t%s\t%s\n", query, file, line, match_text
        count += 1
      }
      END {
        if (count == 0) {
          printf "%s\tnone\t0\tno matches\n", query
        }
      }
    ' "$output_tmp"

    rm -f "$output_tmp" "$error_tmp"
  done < "$queries_file"
}

path_is_test_like() {
  local path="$1"
  local lower
  lower="$(printf '%s' "$path" | tr '[:upper:]' '[:lower:]')"

  # These intentionally overlapping patterns keep common integration-test naming
  # conventions visible in one place for maintainers.
  # shellcheck disable=SC2221,SC2222
  case "$lower" in
    test/*|tests/*|e2e/*|cypress/*|playwright/*|*/test/*|*/tests/*|*/e2e/*|*/cypress/*|*/playwright/*|*/__tests__/*|src/test/*|*/src/test/*|src/it/*|*/src/it/*|src/integrationtest/*|*/src/integrationtest/*|src/integration-test/*|*/src/integration-test/*|*test.java|*tests.java|*it.java|*itcase.java|*integrationtest.java|*spec.java|*test.kt|*tests.kt|*it.kt|*itcase.kt|*integrationtest.kt|*spec.kt|*test.groovy|*spec.groovy|*it.groovy|*integrationtest.groovy|*test.scala|*spec.scala|*it.scala|*integrationtest.scala|*test.ts|*spec.ts|*e2e.ts|*cy.ts|*test.tsx|*spec.tsx|*e2e.tsx|*cy.tsx|*test.js|*spec.js|*e2e.js|*cy.js|*test.jsx|*spec.jsx|*e2e.jsx|*cy.jsx|*_test.go|*_test.py|test_*.py|*/test_*.py|*.spec.py)
      return 0
      ;;
    *)
      return 1
      ;;
  esac
}

path_indicates_jvm_integration() {
  case "$1" in
    src/it/*|*/src/it/*|src/integrationtest/*|*/src/integrationtest/*|src/integration-test/*|*/src/integration-test/*|*it.java|*itcase.java|*integrationtest.java|*it.kt|*itcase.kt|*integrationtest.kt|*it.groovy|*integrationtest.groovy|*it.scala|*integrationtest.scala)
      return 0
      ;;
    *)
      return 1
      ;;
  esac
}

path_indicates_node_e2e_or_integration() {
  case "$1" in
    e2e/*|*/e2e/*|*/integration/*|*.e2e.*)
      return 0
      ;;
    *)
      return 1
      ;;
  esac
}

path_indicates_cypress() {
  case "$1" in
    cypress/*|*/cypress/*|*.cy.ts|*.cy.tsx|*.cy.js|*.cy.jsx)
      return 0
      ;;
    *)
      return 1
      ;;
  esac
}

path_indicates_rust_integration() {
  case "$1" in
    tests/*.rs|*/tests/*.rs|*/integration/*.rs)
      return 0
      ;;
    *)
      return 1
      ;;
  esac
}

configured_test_hint_for_path() {
  local path="$1"
  local content="$2"
  local hints_file="$repo_root/.pre-commit-review/test-hints"

  [ -f "$hints_file" ] || return 1

  awk -F "$TAB" -v path="$path" -v content="$content" '
    function trim(value) {
      sub(/^[[:space:]]+/, "", value)
      sub(/[[:space:]]+$/, "", value)
      return value
    }
    function sanitize(value) {
      gsub(/\t|\r|\n/, " ", value)
      return value
    }
    /^[[:space:]]*($|#)/ { next }
    {
      rule_id=trim($1)
      path_regex=trim($2)
      content_regex=trim($3)
      test_kind=trim($4)
      dependency=trim($5)
      confidence=trim($6)
      hint=$7
      for (i=8; i<=NF; i++) hint=hint " " $i
      hint=trim(hint)
      if (rule_id == "" || test_kind == "" || dependency == "" || confidence == "" || hint == "") next
      path_match=(path_regex != "" && path ~ path_regex)
      content_match=(content_regex != "" && content ~ content_regex)
      if (path_match || content_match) {
        printf "%s\t%s\t%s\t%s\t%s\n", sanitize(rule_id), sanitize(confidence), sanitize(test_kind), sanitize(dependency), sanitize(hint)
        found=1
        exit
      }
    }
    END { if (!found) exit 1 }
  ' "$hints_file"
}

emit_test_selection_hints() {
  local found='no'
  local status
  local path
  local new_path
  local content
  local rule_id
  local confidence
  local kind
  local dependency
  local hint
  local lower_path

  echo '## Test Selection Hints'
  printf 'path\trule_id\tconfidence\ttest_kind\tenvironment_dependency\thint\n'

  while IFS="$(printf '\t')" read -r status path new_path; do
    [ -n "$path" ] || continue
    case "$status" in
      R*|C*) path="$new_path" ;;
    esac
    [ -n "$path" ] || continue
    path_is_test_like "$path" || continue

    content="$(file_content_for_path "$path" 2>/dev/null || true)"
    lower_path="$(printf '%s' "$path" | tr '[:upper:]' '[:lower:]')"
    if hint_line="$(configured_test_hint_for_path "$path" "$content")" && [ -n "$hint_line" ]; then
      printf '%s\t%s\n' "$(sanitize_tsv "$path")" "$hint_line"
      found='yes'
      continue
    fi

    rule_id='no-known-env-heavy-marker'
    confidence='low'
    kind='unit-or-unknown'
    dependency='not-proven-isolated'
    hint='No known env-heavy marker detected; this is not proof of unit-test isolation. Prefer the narrowest focused test command for this file.'

    if printf '%s\n' "$content" | grep -Eiq 'org\.testcontainers|@Testcontainers\b|@Container\b|testcontainers-go'; then
      rule_id='testcontainers'
      confidence='high'
      kind='container-integration'
      dependency='docker-or-testcontainers'
      hint='Requires Docker/Testcontainers; do not treat failure in a sandbox as a pure code failure without environment evidence.'
    elif printf '%s\n' "$content" | grep -Eiq 'DockerComposeContainer|docker-compose|docker compose|compose\.ya?ml'; then
      rule_id='docker-compose-test'
      confidence='high'
      kind='compose-backed-integration'
      dependency='docker-compose-runtime'
      hint='Uses Docker Compose or compose-backed services; verify in an environment with Docker and required service images.'
    elif printf '%s\n' "$content" | grep -Eiq 'WireMockServer|WireMockExtension|@AutoConfigureWireMock|com\.github\.tomakehurst\.wiremock|wiremock\.org'; then
      rule_id='wiremock-test'
      confidence='high'
      kind='http-stub-integration'
      dependency='wiremock-runtime'
      hint='Uses WireMock HTTP stubs; sandbox failures may reflect port/runtime setup rather than the changed code.'
    elif printf '%s\n' "$content" | grep -Eiq 'org\.mockserver|MockServerContainer|ClientAndServer'; then
      rule_id='mockserver-test'
      confidence='high'
      kind='http-stub-integration'
      dependency='mockserver-runtime'
      hint='Uses MockServer or its container runtime; verify with the required local or CI service setup.'
    elif printf '%s\n' "$content" | grep -Eiq '@AutoConfigureStubRunner|StubRunner|spring-cloud-contract|org\.springframework\.cloud\.contract'; then
      rule_id='spring-cloud-contract'
      confidence='high'
      kind='contract-integration'
      dependency='spring-cloud-contract-runtime'
      hint='Uses Spring Cloud Contract or Stub Runner; may require generated stubs, broker settings, or CI contract artifacts.'
    elif printf '%s\n' "$content" | grep -Eiq 'jdbc:|r2dbc:|spring\.datasource\.url|datasource\.url|postgresql|mysql|mariadb|oracle\.jdbc|mongodb://|redis://|spring\.redis|spring\.data\.redis|kafka\.bootstrap|bootstrap\.servers|spring\.kafka|elasticsearch|opensearch|rabbitmq|amqp://|localstack|minio'; then
      rule_id='external-service-config'
      confidence='high'
      kind='service-backed-integration'
      dependency='database-cache-broker-or-search-service'
      hint='References database, cache, broker, search, or object-storage service configuration; run with the expected local profile or CI services.'
    elif printf '%s\n' "$content" | grep -Eiq '@QuarkusTest|@QuarkusIntegrationTest|io\.quarkus\.test'; then
      rule_id='quarkus-test-context'
      confidence='high'
      kind='quarkus-integration'
      dependency='quarkus-test-runtime'
      hint='Loads a Quarkus test context; may require Quarkus profiles, dev services, containers, or CI runtime support.'
    elif printf '%s\n' "$content" | grep -Eiq '@MicronautTest|io\.micronaut\.test'; then
      rule_id='micronaut-test-context'
      confidence='high'
      kind='micronaut-integration'
      dependency='micronaut-test-runtime'
      hint='Loads a Micronaut test context; may require application context configuration or service-backed test resources.'
    elif printf '%s\n' "$content" | grep -Eq '@SpringBootTest\b'; then
      rule_id='spring-boot-context'
      confidence='high'
      kind='spring-boot-integration'
      dependency='spring-context'
      hint='Loads a Spring Boot application context; may require local profiles, DB, middleware, or CI-provided services.'
    elif printf '%s\n' "$content" | grep -Eq '@(DataJpaTest|JdbcTest|JooqTest|MybatisTest)\b'; then
      rule_id='spring-data-slice'
      confidence='high'
      kind='data-slice-integration'
      dependency='database-or-spring-test-slice'
      hint='Loads a data test slice; may require an embedded or configured database.'
    elif printf '%s\n' "$content" | grep -Eq '@(WebMvcTest|AutoConfigureMockMvc)\b'; then
      rule_id='spring-web-slice'
      confidence='high'
      kind='spring-web-slice'
      dependency='spring-test-context'
      hint='Loads a Spring web test slice; usually narrower than full integration but not a pure unit test.'
    elif printf '%s\n' "$content" | grep -Eiq '@ActiveProfiles|SPRING_PROFILES_ACTIVE|quarkus\.test\.profile|micronaut\.environments'; then
      rule_id='jvm-test-profile'
      confidence='high'
      kind='profile-backed-test'
      dependency='maven-gradle-or-framework-profile'
      hint='Selects framework test profiles or environments; use the matching Maven/Gradle profile or CI profile configuration.'
    elif printf '%s\n' "$content" | grep -Eiq '@Tag\("(integration|e2e|contract|slow)"\)|@Category\((IntegrationTest|E2ETest)'; then
      rule_id='junit-integration-tag'
      confidence='high'
      kind='tagged-jvm-integration'
      dependency='junit-tag-or-category-selection'
      hint='Uses JUnit integration/e2e/contract tags; run with the tag expression and environment expected by the project.'
    elif path_indicates_jvm_integration "$lower_path"; then
      rule_id='jvm-integration-naming'
      confidence='medium'
      kind='jvm-integration-by-convention'
      dependency='maven-failsafe-or-gradle-integration-profile'
      hint='Path or class name follows common JVM integration-test conventions such as *IT or src/integrationTest; run the project integration-test profile if available.'
    elif printf '%s\n' "$content" | grep -Eiq 'pytest\.mark\.(integration|e2e|contract|system|django_db|db|redis|kafka|elasticsearch)'; then
      rule_id='pytest-env-marker'
      confidence='high'
      kind='pytest-marked-integration'
      dependency='pytest-marker-or-service-runtime'
      hint='Uses pytest markers that usually select integration/e2e/database/service tests; run with the matching marker and required services.'
    elif printf '%s\n' "$content" | grep -Eiq '@playwright/test|playwright/test' || case "$lower_path" in *.pw.ts|*.pw.js) true ;; *) false ;; esac; then
      rule_id='playwright-e2e'
      confidence='high'
      kind='browser-e2e'
      dependency='browser-runtime-and-app-server'
      hint='Uses Playwright; requires browser runtime and usually a running app server or configured webServer.'
    elif path_indicates_cypress "$lower_path" || printf '%s\n' "$content" | grep -Eiq 'cy\.visit\(|cypress\.'; then
      rule_id='cypress-e2e'
      confidence='high'
      kind='browser-e2e'
      dependency='browser-runtime-and-app-server'
      hint='Uses Cypress; requires browser runtime and usually a running app server.'
    elif path_indicates_node_e2e_or_integration "$lower_path" && printf '%s\n' "$content" | grep -Eiq 'vitest|jest|describe\(|test\('; then
      rule_id='node-e2e-or-integration'
      confidence='medium'
      kind='node-e2e-or-integration'
      dependency='node-runtime-and-possibly-app-server'
      hint='Path/content follows common Node e2e or integration-test conventions; verify with the project test script and required runtime services.'
    elif printf '%s\n' "$content" | grep -Eiq '//go:build (integration|e2e|docker)|// \+build (integration|e2e|docker)'; then
      rule_id='go-integration-build-tag'
      confidence='high'
      kind='go-tagged-integration'
      dependency='go-build-tags-and-service-runtime'
      hint='Uses Go integration/e2e/docker build tags; run go test with the matching tags and required services.'
    elif case "$lower_path" in *_test.go) true ;; *) false ;; esac && case "$lower_path" in *integration*|*/e2e/*) true ;; *) false ;; esac; then
      rule_id='go-integration-naming'
      confidence='medium'
      kind='go-integration-by-convention'
      dependency='go-test-selection-or-service-runtime'
      hint='Go test path suggests integration coverage; check project docs for tags, env vars, or service dependencies.'
    elif printf '%s\n' "$content" | grep -Eq '#\[ignore\]'; then
      rule_id='rust-ignored-test'
      confidence='medium'
      kind='rust-ignored-or-slow-test'
      dependency='cargo-test-ignored-selection'
      # shellcheck disable=SC2016
      hint='Rust ignored tests are not run by default and often need explicit `cargo test -- --ignored` plus external setup.'
    elif path_indicates_rust_integration "$lower_path"; then
      rule_id='rust-integration-path'
      confidence='low'
      kind='rust-integration-by-convention'
      dependency='cargo-test-selection-or-project-specific-runtime'
      hint='Rust test path follows Cargo integration-test layout; treat as a planning hint and verify whether external setup is required.'
    fi

    printf '%s\t%s\t%s\t%s\t%s\t%s\n' \
      "$(sanitize_tsv "$path")" \
      "$(sanitize_tsv "$rule_id")" \
      "$(sanitize_tsv "$confidence")" \
      "$(sanitize_tsv "$kind")" \
      "$(sanitize_tsv "$dependency")" \
      "$(sanitize_tsv "$hint")"
    found='yes'
  done < <(selected_name_status || true)

  if [ "$found" = 'no' ]; then
    echo 'none	none	none	none	none	no changed test files detected'
  fi
}

emit_diff_limited() {
  local tmp_file="$1"
  local size
  size="$(wc -c < "$tmp_file" | tr -d ' ')"

  print_kv 'diff_bytes' "$size"
  print_kv 'max_diff_bytes' "$MAX_DIFF_BYTES"
  print_kv 'inline_diff_bytes' "$INLINE_DIFF_BYTES"
  print_kv 'diff_output' 'inline'

  echo
  echo '## Diff'
  echo '```diff'
  if [ "$MAX_DIFF_BYTES" = '0' ] || [ "$size" -le "$MAX_DIFF_BYTES" ]; then
    cat "$tmp_file"
  else
    head -c "$MAX_DIFF_BYTES" "$tmp_file"
    echo
    printf '[diff truncated after %s bytes; inspect high-risk files with helper-emitted context commands before making safety claims]\n' "$MAX_DIFF_BYTES"
  fi
  echo '```'
}

emit_diff_omitted() {
  local size="$1"
  local reason="$2"

  print_kv 'diff_bytes' "$size"
  print_kv 'max_diff_bytes' "$MAX_DIFF_BYTES"
  print_kv 'inline_diff_bytes' "$INLINE_DIFF_BYTES"
  print_kv 'diff_output' 'omitted'
  print_kv 'diff_omitted_reason' "$reason"
  echo
  echo '## Diff Loading Instructions'
  echo 'Global raw diff omitted from the gateway output so Review Plan JSON, Review Manifest JSONL, and Coverage Ledger Template remain visible to the model.'
  echo 'Use helper-emitted context_command values for group/path loading; do not rebuild review scope with direct git commands.'
}

branch="$(git branch --show-current 2>/dev/null || true)"
head_sha="$(git rev-parse --short HEAD 2>/dev/null || true)"
base="$(detect_base_branch)"

staged='no'
unstaged='no'
if has_staged_changes; then staged='yes'; fi
if has_unstaged_changes; then unstaged='yes'; fi
untracked_names="$(git ls-files --others --exclude-standard || true)"

mode='none'
source='none'
review_limit_note='no diff found in staged, unstaged, or branch-vs-base comparisons'
unreviewed_note='none'
selected_ref=''
diff_truncated='no'

diff_tmp="$(mktemp)"
register_temp_file "$diff_tmp"
staged_files=''
unstaged_files=''
same_files=''
diff_size='0'
diff_output_decision='omitted'
diff_omitted_reason='no diff available'
compact_plan='no'

run_diff() {
  case "$mode" in
    staged) git -c color.ui=false diff --no-ext-diff --find-renames --cached "$@" -- . ;;
    unstaged) git -c color.ui=false diff --no-ext-diff --find-renames "$@" -- . ;;
    branch) git -c color.ui=false diff --no-ext-diff --find-renames "$@" "$selected_ref...HEAD" -- . ;;
    *) return 1 ;;
  esac
}

select_branch_ref() {
  local remote_ref
  remote_ref="origin/$base"

  if git rev-parse --verify --quiet "$remote_ref" >/dev/null; then
    selected_ref="$remote_ref"
    source="branch vs base via git diff ${remote_ref}...HEAD"
    review_limit_note="full diff available from local ${remote_ref}; remote freshness not verified because git fetch was not run"
    return 0
  fi

  if git rev-parse --verify --quiet "$base" >/dev/null; then
    selected_ref="$base"
    source="branch vs local base via git diff ${base}...HEAD"
    review_limit_note='full local branch-vs-base diff available unless truncated by helper output limit'
    return 0
  fi

  return 1
}

write_selected_diff() {
  if [ -n "$REQUEST_PATH" ]; then
    file_diff_for_path "$REQUEST_PATH" > "$diff_tmp"
  else
    run_diff > "$diff_tmp"
  fi
}

selected_numstat() {
  if [ -n "$REQUEST_PATH" ]; then
    file_numstat_for_path "$REQUEST_PATH"
  else
    run_diff --numstat
  fi
}

selected_stat() {
  if [ -n "$REQUEST_PATH" ]; then
    file_stat_for_path "$REQUEST_PATH"
  else
    run_diff --stat
  fi
}

selected_name_status() {
  if [ -n "$REQUEST_PATH" ]; then
    file_name_status_for_path "$REQUEST_PATH"
  else
    run_diff --name-status
  fi
}

if [ "$REQUEST_SOURCE" = 'staged' ]; then
  mode='staged'
  source='staged changes via git diff --cached'
  review_limit_note='full staged diff available unless truncated by helper output limit'
  write_selected_diff
elif [ "$REQUEST_SOURCE" = 'unstaged' ]; then
  mode='unstaged'
  source='unstaged changes via git diff'
  review_limit_note='full unstaged diff available unless truncated by helper output limit'
  write_selected_diff
elif [ "$REQUEST_SOURCE" = 'branch' ]; then
  if select_branch_ref; then
    mode='branch'
    write_selected_diff
  fi
elif [ "$staged" = 'yes' ]; then
  mode='staged'
  source='staged changes via git diff --cached'
  review_limit_note='full staged diff available unless truncated by helper output limit'
  write_selected_diff
fi

if [ "$mode" = 'staged' ]; then
  if [ "$unstaged" = 'yes' ]; then
    unreviewed_note='unstaged changes exist and were not reviewed as part of the staged commit candidate'
    staged_files="$(mktemp)"
    unstaged_files="$(mktemp)"
    same_files="$(mktemp)"
    register_temp_file "$staged_files"
    register_temp_file "$unstaged_files"
    register_temp_file "$same_files"
    git diff --cached --name-only -- . | sort -u > "$staged_files"
    git diff --name-only -- . | sort -u > "$unstaged_files"
    comm -12 "$staged_files" "$unstaged_files" > "$same_files" || true
    if [ -s "$same_files" ]; then
      overlap_csv="$(paste -sd ',' "$same_files")"
      unreviewed_note="unstaged changes touch files also staged for commit; actual working tree behavior may differ from reviewed commit candidate: ${overlap_csv}"
    fi
  fi
fi

if [ "$mode" = 'none' ] && [ "$REQUEST_SOURCE" = '' ] && [ "$unstaged" = 'yes' ]; then
  mode='unstaged'
  source='unstaged changes via git diff'
  review_limit_note='full unstaged diff available unless truncated by helper output limit'
  write_selected_diff
fi

if [ "$mode" = 'none' ] && [ "$REQUEST_SOURCE" = '' ]; then
  remote_ref="origin/$base"
  if git rev-parse --verify --quiet "$remote_ref" >/dev/null && has_diff_for_ref "$remote_ref"; then
    mode='branch'
    selected_ref="$remote_ref"
    source="branch vs base via git diff ${remote_ref}...HEAD"
    review_limit_note="full diff available from local ${remote_ref}; remote freshness not verified because git fetch was not run"
    write_selected_diff
  elif git rev-parse --verify --quiet "$base" >/dev/null && has_diff_for_ref "$base"; then
    mode='branch'
    selected_ref="$base"
    source="branch vs local base via git diff ${base}...HEAD"
    review_limit_note='full local branch-vs-base diff available unless truncated by helper output limit'
    write_selected_diff
  fi
fi

if [ -n "$REQUEST_PATH" ] && [ "$mode" != 'none' ]; then
  review_limit_note='file-specific diff for requested path; no other files included'
fi
if [ -n "$REQUEST_GROUP" ] && [ "$mode" != 'none' ]; then
  review_limit_note='group-specific diff for requested group; no other groups included'
fi

if [ -n "$untracked_names" ]; then
  if [ "$unreviewed_note" = 'none' ]; then
    unreviewed_note='untracked files exist but are not part of git diff; stage them or provide file content to review'
  else
    unreviewed_note="$unreviewed_note; untracked files exist but are not part of git diff"
  fi
fi

if [ -s "$diff_tmp" ]; then
  diff_size="$(wc -c < "$diff_tmp" | tr -d ' ')"
  if [ "$MAX_DIFF_BYTES" != '0' ] && [ "$diff_size" -gt "$MAX_DIFF_BYTES" ]; then
    diff_truncated='yes'
    review_limit_note='partial diff output; inspect file list and prioritize risky files before making safety claims'
  fi
  case "$INCLUDE_DIFF_MODE" in
    always)
      diff_output_decision='inline'
      diff_omitted_reason='none'
      ;;
    never)
      diff_output_decision='omitted'
      diff_omitted_reason='plan-only mode'
      compact_plan='yes'
      ;;
    auto)
      if [ "$INLINE_DIFF_BYTES" = '0' ] || [ "$diff_size" -le "$INLINE_DIFF_BYTES" ]; then
        diff_output_decision='inline'
        diff_omitted_reason='none'
      else
        diff_output_decision='omitted'
        diff_omitted_reason="global diff exceeds inline budget (${diff_size} > ${INLINE_DIFF_BYTES})"
        compact_plan='yes'
      fi
      ;;
    *)
      diff_output_decision='omitted'
      diff_omitted_reason="invalid include-diff mode coerced to plan-only: ${INCLUDE_DIFF_MODE}"
      compact_plan='yes'
      ;;
  esac
fi

files_changed='0 files, 0 insertions(+), 0 deletions(-)'
path_risk_candidate_lines=''
content_risk_candidate_lines=''
configured_path_risk_candidate_lines=''
configured_content_risk_candidate_lines=''
high_risk_candidate_lines=''
generated_like_file_lines=''
lock_file_lines=''
top_churn_file_lines=''
high_risk_candidates='none'
content_risk_candidates='none'
generated_like_files='none'
lock_files='none'
top_churn_files='none'
if [ "$mode" != 'none' ]; then
  files_changed="$(selected_numstat | summarize_numstat)"
  path_risk_candidate_lines="$(selected_name_status | high_risk_paths_from_name_status)"
  content_risk_candidate_lines="$(content_risk_paths_from_diff "$diff_tmp")"
  configured_path_risk_candidate_lines="$(selected_name_status | configured_risk_paths_from_name_status)"
  configured_content_risk_candidate_lines="$(configured_content_risk_paths_from_diff "$diff_tmp")"
  high_risk_candidate_lines="$(
    {
      printf '%s\n' "$path_risk_candidate_lines"
      printf '%s\n' "$content_risk_candidate_lines"
      printf '%s\n' "$configured_path_risk_candidate_lines"
      printf '%s\n' "$configured_content_risk_candidate_lines"
    } | sed '/^$/d' | sort -u
  )"
  generated_like_file_lines="$(selected_name_status | generated_like_paths_from_name_status)"
  lock_file_lines="$(selected_name_status | lock_paths_from_name_status)"
  top_churn_file_lines="$(selected_numstat | top_churn_paths_from_numstat)"
  high_risk_candidates="$(printf '%s\n' "$high_risk_candidate_lines" | join_lines_csv)"
  content_risk_candidates="$(printf '%s\n' "$content_risk_candidate_lines" | join_lines_csv)"
  generated_like_files="$(printf '%s\n' "$generated_like_file_lines" | join_lines_csv)"
  lock_files="$(printf '%s\n' "$lock_file_lines" | join_lines_csv)"
  top_churn_files="$(printf '%s\n' "$top_churn_file_lines" | join_lines_csv)"
fi

echo '# Pre-Commit Review Diff Context'
echo
print_kv 'repository' "$repo_root"
print_kv 'branch' "${branch:-detached-or-unknown}"
print_kv 'head' "${head_sha:-unknown}"
print_kv 'detected_base' "$base"
print_kv 'diff_source' "$source"
if [ -n "$REQUEST_PATH" ]; then
  print_kv 'requested_path' "$REQUEST_PATH"
fi
if [ -n "$REQUEST_GROUP" ]; then
  print_kv 'requested_group' "$REQUEST_GROUP"
fi
if [ -n "$REQUEST_SOURCE" ]; then
  print_kv 'requested_source' "$REQUEST_SOURCE"
fi
print_kv 'review_limits' "$review_limit_note"
print_kv 'diff_truncated' "$diff_truncated"
print_kv 'inline_diff_bytes' "$INLINE_DIFF_BYTES"
print_kv 'diff_output' "$diff_output_decision"
if [ "$diff_output_decision" = 'omitted' ]; then
  print_kv 'diff_omitted_reason' "$diff_omitted_reason"
fi
print_kv 'diff_loading' 'use helper-emitted context_command values; do not rebuild review scope with direct git commands'
print_kv 'group_target_bytes' "$GROUP_TARGET_BYTES"
print_kv 'group_hard_bytes' "$GROUP_HARD_BYTES"
print_kv 'files_changed' "$files_changed"
print_kv 'high_risk_candidates' "$high_risk_candidates"
print_kv 'content_risk_candidates' "$content_risk_candidates"
print_kv 'generated_like_files' "$generated_like_files"
print_kv 'lock_files' "$lock_files"
print_kv 'top_churn_files' "$top_churn_files"
print_kv 'staged_changes' "$staged"
print_kv 'unstaged_changes' "$unstaged"
if [ -n "$untracked_names" ]; then
  print_kv 'untracked_files' 'yes'
else
  print_kv 'untracked_files' 'no'
fi
print_kv 'unreviewed_changes' "$unreviewed_note"
echo

echo '## Status'
if [ -n "$REQUEST_PATH" ]; then
  git status --short -- "$REQUEST_PATH" || true
elif [ -n "$REQUEST_GROUP" ]; then
  echo 'group-specific status is emitted after group resolution'
else
  git status --short || true
fi
echo

if [ "$mode" = 'none' ]; then
  echo 'No diff available. Stage your changes or provide a diff to review.'
  exit 0
fi

if [ -n "$REQUEST_GROUP" ]; then
  echo
  emit_requested_group_context "$REQUEST_GROUP"
  exit 0
fi

echo '## Diff Stat'
selected_stat || true
echo

echo '## File List'
selected_name_status || true
echo

echo '## Numstat'
selected_numstat || true

if [ -n "$REQUEST_PATH" ]; then
  echo
  echo '## Requested File Diff'
  print_kv 'path' "$REQUEST_PATH"
  print_kv 'review_command' "$(review_command_for_path "$REQUEST_PATH")"
  print_kv 'context_command' "$(context_command_for_path "$REQUEST_PATH")"
  if [ ! -s "$diff_tmp" ]; then
    echo
    echo 'No diff available for requested path in the selected diff source.'
    exit 0
  fi
  emit_diff_limited "$diff_tmp"
  exit 0
fi

echo
emit_review_manifest_and_groups

echo
emit_dependency_summary

echo
emit_semantic_context_queries

echo
emit_test_selection_hints

echo
echo '## Suggested Review Queue'
if [ -n "$high_risk_candidate_lines" ]; then
  printf '%s\n' "$high_risk_candidate_lines" | while IFS= read -r path; do
    [ -n "$path" ] && printf 'high-risk: %s\n' "$path"
  done
fi
if [ -n "$top_churn_file_lines" ]; then
  printf '%s\n' "$top_churn_file_lines" | while IFS= read -r path; do
    [ -n "$path" ] && printf 'top-churn: %s\n' "$path"
  done
fi
if [ -n "$generated_like_file_lines" ]; then
  printf '%s\n' "$generated_like_file_lines" | while IFS= read -r path; do
    [ -n "$path" ] && printf 'generated-like consistency check: %s\n' "$path"
  done
fi
if [ -n "$lock_file_lines" ]; then
  printf '%s\n' "$lock_file_lines" | while IFS= read -r path; do
    [ -n "$path" ] && printf 'lockfile consistency check: %s\n' "$path"
  done
fi
if [ -z "$high_risk_candidate_lines" ] && [ -z "$top_churn_file_lines" ] && [ -z "$generated_like_file_lines" ] && [ -z "$lock_file_lines" ]; then
  echo 'none'
fi

if [ -n "$same_files" ] && [ -s "$same_files" ]; then
  echo
  echo '## Staged Files With Unstaged Changes Too'
  cat "$same_files"
fi

if [ "$diff_output_decision" = 'inline' ]; then
  emit_diff_limited "$diff_tmp"
else
  emit_diff_omitted "$diff_size" "$diff_omitted_reason"
fi
