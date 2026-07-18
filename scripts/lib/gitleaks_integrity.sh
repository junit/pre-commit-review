#!/usr/bin/env bash

gitleaks_path_is_absolute() {
  case "$1" in
    /*|[A-Za-z]:[\\/]*) return 0 ;;
    *) return 1 ;;
  esac
}

gitleaks_sha256_file() {
  local file="$1"
  if command -v sha256sum >/dev/null 2>&1; then
    sha256sum "$file" | awk '{print $1}'
  elif command -v shasum >/dev/null 2>&1; then
    shasum -a 256 "$file" | awk '{print $1}'
  else
    return 1
  fi
}

gitleaks_manifest_hash() {
  local manifest="$1"
  local name="$2"
  [ -f "$manifest" ] || return 1
  awk -v name="$name" '$2 == name {print $1}' "$manifest"
}

gitleaks_hash_matches() {
  local executable="$1"
  local manifest="$2"
  local name="$3"
  local expected
  local actual

  [ -f "$executable" ] || return 1
  expected="$(gitleaks_manifest_hash "$manifest" "$name")"
  [ -n "$expected" ] || return 1
  actual="$(gitleaks_sha256_file "$executable")" || return 1
  [ "$actual" = "$expected" ]
}

gitleaks_version_matches() {
  local executable="$1"
  local version_file="$2"
  local expected
  local actual

  [ -x "$executable" ] && [ -f "$version_file" ] || return 1
  expected="$(tr -d '[:space:]' < "$version_file")"
  actual="$("$executable" version 2>/dev/null | tr -d '[:space:]')" || return 1
  [ -n "$expected" ] && [ "$actual" = "$expected" ]
}

gitleaks_smoke_scan() {
  local executable="$1"
  local config="$2"
  local tmp_dir
  local report_file
  local error_file
  local config_dir
  local compact_report

  [ -x "$executable" ] && [ -f "$config" ] || return 1
  tmp_dir="$(mktemp -d)" || return 1
  report_file="$tmp_dir/report.json"
  error_file="$tmp_dir/error.log"
  config_dir="$(CDPATH='' cd -- "$(dirname -- "$config")" && pwd -P)" || {
    rm -rf "$tmp_dir"
    return 1
  }

  if ! (
    cd "$config_dir"
    printf '' | "$executable" \
      --config "$config" \
      --ignore-gitleaks-allow \
      --redact=100 \
      --exit-code=42 \
      --no-banner \
      --no-color \
      --log-level=error \
      --max-decode-depth=5 \
      --report-format=json \
      --report-path=- \
      stdin > "$report_file" 2> "$error_file"
  ); then
    rm -rf "$tmp_dir"
    return 1
  fi

  compact_report="$(tr -d '[:space:]' < "$report_file")"
  rm -rf "$tmp_dir"
  [ "$compact_report" = '[]' ]
}
