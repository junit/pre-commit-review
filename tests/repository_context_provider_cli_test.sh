#!/usr/bin/env bash
set -euo pipefail

script_dir="$(CDPATH='' cd -- "$(dirname -- "$0")" && pwd -P)"
repo_root="$(CDPATH='' cd -- "$script_dir/.." && pwd -P)"
wrapper="$repo_root/scripts/run_repository_context_provider.sh"
resolver="$repo_root/scripts/lib/repository_context_provider_cli.sh"
validator="$repo_root/scripts/validate_schemas.py"
provider_doc="$repo_root/docs/rust-analyzer-context-provider.md"
capabilities_doc="$repo_root/docs/helper-capabilities.md"
options_doc="$repo_root/docs/call-graph-open-source-options.md"
tmp_dir="$(mktemp -d)"
tmp_dir="$(CDPATH='' cd -- "$tmp_dir" && pwd -P)"
trap 'rm -rf "$tmp_dir"' EXIT

fail() {
  printf 'repository context provider CLI test failed: %s\n' "$*" >&2
  exit 1
}

assert_json_kind() {
  local path="$1"
  local expected="$2"
  python3 - "$path" "$expected" <<'PY'
import json
import pathlib
import sys

payload = json.loads(pathlib.Path(sys.argv[1]).read_text(encoding="utf-8"))
if payload.get("kind") != sys.argv[2]:
    raise SystemExit(f"unexpected JSON kind: {payload.get('kind')!r}")
PY
}

assert_forwarded() {
  local path="$1"
  shift
  python3 - "$path" "$@" <<'PY'
import pathlib
import sys

observed = pathlib.Path(sys.argv[1]).read_text(encoding="utf-8").splitlines()
expected = sys.argv[2:]
if observed != expected:
    raise SystemExit(f"argument mismatch: observed={observed!r} expected={expected!r}")
PY
}

[ -r "$resolver" ] || fail 'resolver is missing'
[ -x "$wrapper" ] || fail 'wrapper is missing or not executable'
[ -r "$provider_doc" ] || fail 'provider documentation is missing'
[ -r "$capabilities_doc" ] || fail 'helper capability documentation is missing'
[ -r "$options_doc" ] || fail 'call-graph options documentation is missing'
grep -Fq "\`repository-context-provider-cli model\`" "$provider_doc" \
  || fail 'provider model command is not documented'
grep -Fq "\`repository-context-provider-cli run\`" "$provider_doc" \
  || fail 'provider run command is not documented'
grep -Fq 'collect-diff-context-cli/schemas/repository-context-provider-registry.schema.json' \
  "$provider_doc" || fail 'provider registry schema is not documented'
grep -Fq 'collect-diff-context-cli/schemas/repository-context-provider-run-request.schema.json' \
  "$provider_doc" || fail 'provider request schema is not documented'
grep -Fq "Delivery 4 does not bundle or download a real \`rust-analyzer\` artifact." \
  "$provider_doc" || fail 'Delivery 4 artifact boundary is not documented'
grep -Fq "\`repository-context-provider-cli\`" "$capabilities_doc" \
  || fail 'explicit provider CLI is not listed in helper capabilities'
grep -Fq 'Delivery 4 explicit CLI' "$options_doc" \
  || fail 'call-graph options do not record the Delivery 4 CLI boundary'

fake_cli="$tmp_dir/fake-provider-cli"
cat >"$fake_cli" <<'EOF_FAKE'
#!/usr/bin/env bash
set -u
: "${FAKE_PROVIDER_LOG:?}"
: "${FAKE_PROVIDER_BINARY_LOG:?}"
printf '%s\n' "$0" >"$FAKE_PROVIDER_BINARY_LOG"
printf '%s\n' "$@" >"$FAKE_PROVIDER_LOG"
case "${FAKE_PROVIDER_MODE:-ok}" in
  stderr)
    printf '%s\n' 'raw-child-stderr-must-not-escape' >&2
    printf '%s\n' '{"kind":"repository_context_provider_report"}'
    exit 0
    ;;
  exit-two)
    printf '%s\n' 'raw-authorization-error-must-not-escape' >&2
    exit 2
    ;;
  exit-three)
    printf '%s\n' 'raw-runtime-error-must-not-escape' >&2
    exit 3
    ;;
  invalid-exit)
    printf '%s\n' 'raw-invalid-exit-must-not-escape' >&2
    exit 17
    ;;
esac
case "${1:-}" in
  --help|-h)
    printf '%s\n' 'fixture provider CLI help'
    ;;
  model)
    printf '%s\n' '{"schema_version":1,"kind":"repository_context_project_model"}'
    ;;
  run)
    printf '%s\n' '{"schema_version":1,"kind":"repository_context_provider_report"}'
    ;;
  *)
    exit 2
    ;;
esac
EOF_FAKE
chmod +x "$fake_cli"

export FAKE_PROVIDER_LOG="$tmp_dir/fake-arguments.log"
export FAKE_PROVIDER_BINARY_LOG="$tmp_dir/fake-binary.log"

PRE_COMMIT_REVIEW_REPOSITORY_CONTEXT_PROVIDER_BIN="$fake_cli" \
  "$wrapper" --help >"$tmp_dir/help.out"
grep -Fq 'fixture provider CLI help' "$tmp_dir/help.out" \
  || fail 'wrapper did not forward --help'
assert_forwarded "$FAKE_PROVIDER_LOG" --help

model_args=(
  model
  --source staged
  --expect-scope aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa
  --max-model-files 64
  --max-model-bytes 65536
)
PRE_COMMIT_REVIEW_REPOSITORY_CONTEXT_PROVIDER_BIN="$fake_cli" \
  "$wrapper" "${model_args[@]}" >"$tmp_dir/model.json"
assert_json_kind "$tmp_dir/model.json" repository_context_project_model
assert_forwarded "$FAKE_PROVIDER_LOG" "${model_args[@]}"

run_args=(
  run
  --source staged
  --expect-scope aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa
  --registry /fixtures/provider-registry.json
  --expect-registry-sha256 bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb
  --provider-id fixture-local
  --model /fixtures/project-model.json
  --expect-model-sha256 cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc
  --request /fixtures/provider-request.json
)
PRE_COMMIT_REVIEW_REPOSITORY_CONTEXT_PROVIDER_BIN="$fake_cli" \
  "$wrapper" "${run_args[@]}" >"$tmp_dir/run.json"
assert_json_kind "$tmp_dir/run.json" repository_context_provider_report
assert_forwarded "$FAKE_PROVIDER_LOG" "${run_args[@]}"

provider_report="$tmp_dir/provider-report.json"
cat >"$provider_report" <<'EOF_REPORT'
{
  "schema_version": 1,
  "kind": "repository_context_provider_report",
  "candidate": {
    "source": "staged",
    "scope_fingerprint": "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
    "candidate_digest": "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb",
    "snapshot_sha256": "cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc",
    "snapshot_files": 1,
    "snapshot_bytes": 32,
    "project_model_digest": "dddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddd"
  },
  "provider": {
    "kind": "rust-analyzer",
    "version": "fixture-1",
    "profile_sha256": "eeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeee",
    "executable_sha256": "ffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff",
    "configuration_sha256": "1111111111111111111111111111111111111111111111111111111111111111",
    "target_triple": "x86_64-unknown-linux-gnu",
    "toolchain_mode": "none",
    "project_model_algorithm": "rust-analyzer-linked-project-v1",
    "negotiated_encoding": null
  },
  "status": "unavailable",
  "index_completeness": "unknown",
  "query_completeness": "unavailable",
  "seed_symbols": [],
  "related_symbols": [],
  "edges": [],
  "limitations": [
    {
      "code": "provider-unavailable",
      "message": "Provider capability is unavailable",
      "changed_symbol_id": null,
      "path": null
    }
  ],
  "isolation": {
    "network": "best-effort-offline",
    "shell_enabled": false,
    "original_repository_access": false
  },
  "metrics": {
    "requests": 0,
    "messages": 0,
    "notifications": 0,
    "server_requests": 0,
    "invalid_messages": 0,
    "call_ranges": 0,
    "protocol_bytes": 0,
    "stderr_bytes": 0,
    "source_bytes": 0,
    "nodes": 0,
    "edges": 0,
    "report_bytes": 0,
    "elapsed_ms": 0,
    "process_tree_peak_rss_bytes": 0,
    "process_tree_sample_interval_ms": 100,
    "process_tree_accounting": "available"
  }
}
EOF_REPORT
python3 "$validator" --repository-context-provider-report "$provider_report" \
  >"$tmp_dir/provider-report-valid.out"

raw_protocol_report="$tmp_dir/provider-report-raw-protocol.json"
python3 - "$provider_report" "$raw_protocol_report" <<'PY'
import json
import pathlib
import sys

payload = json.loads(pathlib.Path(sys.argv[1]).read_text(encoding="utf-8"))
payload["limitations"][0]["message"] = "Content-Length: 42"
pathlib.Path(sys.argv[2]).write_text(json.dumps(payload), encoding="utf-8")
PY
if python3 "$validator" \
  --repository-context-provider-report "$raw_protocol_report" \
  >"$tmp_dir/provider-report-raw.out" 2>"$tmp_dir/provider-report-raw.err"; then
  fail 'provider report validator accepted raw JSON-RPC framing text'
fi
grep -Fq 'raw JSON-RPC framing' "$tmp_dir/provider-report-raw.err" \
  || fail 'raw JSON-RPC rejection was not actionable'

python3 - "$validator" "$provider_report" <<'PY'
import copy
import importlib.util
import json
import pathlib
import sys

spec = importlib.util.spec_from_file_location("provider_schema_validator", sys.argv[1])
module = importlib.util.module_from_spec(spec)
spec.loader.exec_module(module)
valid = json.loads(pathlib.Path(sys.argv[2]).read_text(encoding="utf-8"))

def local_snapshot_root(payload):
    payload["candidate"]["snapshot_root"] = "/private/tmp/provider-snapshot"

def raw_stderr(payload):
    payload["limitations"][0]["stderr"] = "private child text"

def raw_json_rpc(payload):
    payload["limitations"][0]["jsonrpc"] = "2.0"

def unknown_top_level(payload):
    payload["unknown"] = True

def empty_identity(payload):
    payload["provider"]["version"] = ""

def missing_digest(payload):
    del payload["provider"]["profile_sha256"]

def local_file_uri(payload):
    payload["limitations"][0]["message"] = "file:///private/tmp/provider-snapshot/src/lib.rs"

for name, mutate in (
    ("local snapshot root", local_snapshot_root),
    ("raw stderr", raw_stderr),
    ("raw JSON-RPC", raw_json_rpc),
    ("unknown top-level field", unknown_top_level),
    ("empty identity", empty_identity),
    ("missing digest", missing_digest),
    ("local file URI", local_file_uri),
):
    payload = copy.deepcopy(valid)
    mutate(payload)
    try:
        module.validate_provider_report_invariants(payload)
    except ValueError:
        continue
    raise SystemExit(f"provider report invariant accepted {name}")
PY

if PRE_COMMIT_REVIEW_REPOSITORY_CONTEXT_PROVIDER_BIN='relative-provider-cli' \
  "$wrapper" --help >"$tmp_dir/relative.out" 2>"$tmp_dir/relative.err"; then
  fail 'relative provider CLI override was accepted'
fi
grep -Fq 'run_repository_context_provider: provider CLI override is invalid' \
  "$tmp_dir/relative.err" || fail 'relative override error was not stable'

isolated_root="$tmp_dir/isolated"
isolated_scripts="$isolated_root/scripts"
mkdir -p "$isolated_scripts/lib" "$isolated_scripts/bin" \
  "$isolated_root/collect-diff-context-cli/target/release"
cp "$wrapper" "$isolated_scripts/run_repository_context_provider.sh"
cp "$resolver" "$isolated_scripts/lib/repository_context_provider_cli.sh"
chmod +x "$isolated_scripts/run_repository_context_provider.sh"

local_cli="$isolated_root/collect-diff-context-cli/target/release/repository-context-provider-cli"
cp "$fake_cli" "$local_cli"
chmod +x "$local_cli"
env -u PRE_COMMIT_REVIEW_REPOSITORY_CONTEXT_PROVIDER_BIN \
  "$isolated_scripts/run_repository_context_provider.sh" --help \
  >"$tmp_dir/local.out"
observed_binary="$(cat "$FAKE_PROVIDER_BINARY_LOG")"
[ "$observed_binary" = "$local_cli" ] \
  || fail "local release provider CLI did not precede packaged binary: $observed_binary"

os_name="$(uname -s | tr '[:upper:]' '[:lower:]')"
arch_name="$(uname -m)"
case "$os_name" in
  darwin) os_name=darwin ;;
  linux) os_name=linux ;;
  msys*|mingw*|cygwin*) os_name=windows ;;
  *) fail 'unsupported host OS for provider resolver test' ;;
esac
case "$arch_name" in
  x86_64|amd64) arch_name=amd64 ;;
  arm64|aarch64) arch_name=arm64 ;;
  *) fail 'unsupported host architecture for provider resolver test' ;;
esac
packaged_name="repository_context_provider-${os_name}-${arch_name}"
[ "$os_name" = windows ] && packaged_name="${packaged_name}.exe"
packaged_cli="$isolated_scripts/bin/$packaged_name"
cp "$fake_cli" "$packaged_cli"
chmod +x "$packaged_cli"
rm -f "$local_cli"
env -u PRE_COMMIT_REVIEW_REPOSITORY_CONTEXT_PROVIDER_BIN \
  "$isolated_scripts/run_repository_context_provider.sh" --help \
  >"$tmp_dir/packaged.out"
observed_binary="$(cat "$FAKE_PROVIDER_BINARY_LOG")"
[ "$observed_binary" = "$packaged_cli" ] \
  || fail "packaged provider CLI was not resolved: $observed_binary"

rm -f "$packaged_cli"
ambient_dir="$tmp_dir/ambient"
mkdir -p "$ambient_dir"
cp "$fake_cli" "$ambient_dir/repository-context-provider-cli"
chmod +x "$ambient_dir/repository-context-provider-cli"
missing_status=0
env -u PRE_COMMIT_REVIEW_REPOSITORY_CONTEXT_PROVIDER_BIN \
  PATH="$ambient_dir:/usr/bin:/bin:/usr/sbin:/sbin" \
  "$isolated_scripts/run_repository_context_provider.sh" --help \
  >"$tmp_dir/missing.out" 2>"$tmp_dir/missing.err" || missing_status=$?
[ "$missing_status" -eq 2 ] || fail 'missing provider CLI did not return exit 2'
[ ! -s "$tmp_dir/missing.out" ] || fail 'missing provider CLI emitted stdout'
grep -Fq 'run_repository_context_provider: provider CLI is unavailable' \
  "$tmp_dir/missing.err" || fail 'missing provider CLI error was not stable'

(
  # shellcheck source=scripts/lib/repository_context_provider_cli.sh
  source "$resolver"
  unset PRE_COMMIT_REVIEW_REPOSITORY_CONTEXT_PROVIDER_BIN
  uname() {
    case "$1" in
      -s) printf '%s\n' Plan9 ;;
      -m) printf '%s\n' mystery ;;
    esac
  }
  if resolve_repository_context_provider_cli "$isolated_scripts" >/dev/null; then
    fail 'unknown OS and architecture were accepted'
  fi
)

stderr_status=0
FAKE_PROVIDER_MODE=stderr \
PRE_COMMIT_REVIEW_REPOSITORY_CONTEXT_PROVIDER_BIN="$fake_cli" \
  "$wrapper" run >"$tmp_dir/stderr.out" 2>"$tmp_dir/stderr.err" \
  || stderr_status=$?
[ "$stderr_status" -eq 3 ] || fail 'child stderr violation did not return exit 3'
[ ! -s "$tmp_dir/stderr.out" ] || fail 'child stderr violation emitted stdout'
grep -Fq 'run_repository_context_provider: provider CLI violated its stderr contract' \
  "$tmp_dir/stderr.err" || fail 'child stderr error was not stable'
if grep -Fq 'raw-child-stderr-must-not-escape' "$tmp_dir/stderr.err"; then
  fail 'raw child stderr escaped the wrapper'
fi

authorization_status=0
FAKE_PROVIDER_MODE=exit-two \
PRE_COMMIT_REVIEW_REPOSITORY_CONTEXT_PROVIDER_BIN="$fake_cli" \
  "$wrapper" run >"$tmp_dir/exit-two.out" 2>"$tmp_dir/exit-two.err" \
  || authorization_status=$?
[ "$authorization_status" -eq 2 ] || fail 'provider exit 2 was not preserved'
[ ! -s "$tmp_dir/exit-two.out" ] || fail 'provider exit 2 emitted stdout'
grep -Fq 'run_repository_context_provider: provider CLI rejected the invocation' \
  "$tmp_dir/exit-two.err" || fail 'provider exit 2 error was not stable'
if grep -Fq 'raw-authorization-error-must-not-escape' "$tmp_dir/exit-two.err"; then
  fail 'raw provider authorization stderr escaped the wrapper'
fi

runtime_status=0
FAKE_PROVIDER_MODE=exit-three \
PRE_COMMIT_REVIEW_REPOSITORY_CONTEXT_PROVIDER_BIN="$fake_cli" \
  "$wrapper" run >"$tmp_dir/exit-three.out" 2>"$tmp_dir/exit-three.err" \
  || runtime_status=$?
[ "$runtime_status" -eq 3 ] || fail 'provider exit 3 was not preserved'
[ ! -s "$tmp_dir/exit-three.out" ] || fail 'provider exit 3 emitted stdout'
grep -Fq 'run_repository_context_provider: provider CLI execution failed' \
  "$tmp_dir/exit-three.err" || fail 'provider exit 3 error was not stable'
if grep -Fq 'raw-runtime-error-must-not-escape' "$tmp_dir/exit-three.err"; then
  fail 'raw provider runtime stderr escaped the wrapper'
fi

invalid_status=0
FAKE_PROVIDER_MODE=invalid-exit \
PRE_COMMIT_REVIEW_REPOSITORY_CONTEXT_PROVIDER_BIN="$fake_cli" \
  "$wrapper" run >"$tmp_dir/invalid.out" 2>"$tmp_dir/invalid.err" \
  || invalid_status=$?
[ "$invalid_status" -eq 3 ] || fail 'invalid provider exit was not mapped to exit 3'
[ ! -s "$tmp_dir/invalid.out" ] || fail 'invalid provider exit emitted stdout'
grep -Fq 'run_repository_context_provider: provider CLI returned an invalid exit code' \
  "$tmp_dir/invalid.err" || fail 'invalid provider exit error was not stable'
if grep -Fq 'raw-invalid-exit-must-not-escape' "$tmp_dir/invalid.err"; then
  fail 'raw invalid-exit stderr escaped the wrapper'
fi

for error_file in \
  "$tmp_dir/relative.err" \
  "$tmp_dir/missing.err" \
  "$tmp_dir/stderr.err" \
  "$tmp_dir/exit-two.err" \
  "$tmp_dir/exit-three.err" \
  "$tmp_dir/invalid.err"; do
  [ "$(wc -c <"$error_file")" -le 512 ] || fail "unbounded stderr: $error_file"
done

printf '%s\n' 'repository context provider CLI tests passed'
