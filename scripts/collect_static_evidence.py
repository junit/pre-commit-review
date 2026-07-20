#!/usr/bin/env python3
"""Normalize explicit SARIF/JSON reports into snapshot-bound review evidence."""

from __future__ import annotations

import argparse
import hashlib
import json
import os
import pathlib
import re
import subprocess
import sys
import urllib.parse
from dataclasses import dataclass
from typing import Any


FINGERPRINT_RE = re.compile(r"^[0-9a-f]{40}(?:[0-9a-f]{24})?$")
HUNK_RE = re.compile(r"^@@ -(?P<old>\d+)(?:,(?P<old_count>\d+))? \+(?P<new>\d+)(?:,(?P<new_count>\d+))? @@")
MAX_INPUT_BYTES = int(os.environ.get("PRE_COMMIT_REVIEW_STATIC_MAX_INPUT_BYTES", "10000000"))
MAX_INPUT_FINDINGS = 10000
MATERIAL_CATEGORIES = {
    "security",
    "privacy",
    "build",
    "correctness",
    "data",
    "compatibility",
    "reliability",
}
SEVERITY_ORDER = {"unknown": 0, "none": 1, "note": 2, "warning": 3, "error": 4, "critical": 5}
CONFIDENCE_ORDER = {"unknown": 0, "low": 1, "medium": 2, "high": 3, "very-high": 4}


class EvidenceError(Exception):
    """Expected, actionable evidence-ingestion failure."""


@dataclass
class ParsedReport:
    report_id: str
    format: str
    tool_name: str
    tool_version: str | None
    status: str
    scope_binding: str
    finding_count: int
    findings: list[dict[str, Any]]


def compact_hash(*parts: object) -> str:
    digest = hashlib.sha256()
    for part in parts:
        if isinstance(part, bytes):
            digest.update(part)
        else:
            digest.update(str(part).encode("utf-8", errors="replace"))
        digest.update(b"\0")
    return digest.hexdigest()[:16]


def clean_text(value: object, *, fallback: str, limit: int = 1000) -> str:
    text = str(value or fallback).replace("\x00", "")
    text = " ".join(text.split())
    if not text:
        text = fallback
    return text[:limit]


def require_fingerprint(value: object, label: str) -> str:
    fingerprint = str(value or "")
    if not FINGERPRINT_RE.fullmatch(fingerprint):
        raise EvidenceError(f"{label} is missing or invalid")
    return fingerprint


def load_json_file(path: pathlib.Path) -> tuple[dict[str, Any], bytes]:
    try:
        size = path.stat().st_size
    except OSError as exc:
        raise EvidenceError(f"cannot read static result {path.name}: {exc}") from exc
    if size > MAX_INPUT_BYTES:
        raise EvidenceError(
            f"static result {path.name} exceeds the {MAX_INPUT_BYTES}-byte input limit"
        )
    try:
        raw = path.read_bytes()
        payload = json.loads(raw.decode("utf-8"))
    except (OSError, UnicodeDecodeError, json.JSONDecodeError) as exc:
        raise EvidenceError(f"static result {path.name} is not valid UTF-8 JSON: {exc}") from exc
    if not isinstance(payload, dict):
        raise EvidenceError(f"static result {path.name} must contain a JSON object")
    return payload, raw


def extract_section_json(output: str, marker: str) -> dict[str, Any]:
    lines = output.splitlines()
    try:
        index = lines.index(marker)
    except ValueError as exc:
        raise EvidenceError(f"helper output is missing {marker}") from exc
    payload_lines = [line for line in lines[index + 1 :] if line.strip()]
    if len(payload_lines) != 1:
        raise EvidenceError(f"helper section {marker} must contain exactly one JSON value")
    try:
        payload = json.loads(payload_lines[0])
    except json.JSONDecodeError as exc:
        raise EvidenceError(f"helper section {marker} contains invalid JSON") from exc
    if not isinstance(payload, dict):
        raise EvidenceError(f"helper section {marker} must contain a JSON object")
    return payload


def run_control_plane(helper: pathlib.Path, source: str | None, expected: str) -> dict[str, Any]:
    command = [str(helper)]
    if source:
        command.extend(["--source", source])
    command.extend(["--control-plane", "--expect-scope", expected])
    completed = subprocess.run(
        command,
        check=False,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        text=True,
        encoding="utf-8",
        errors="replace",
    )
    if completed.returncode != 0:
        detail = clean_text(completed.stderr, fallback="helper failed", limit=500)
        raise EvidenceError(f"control-plane helper failed: {detail}")
    payload = extract_section_json(completed.stdout, "## Review Control Plane JSON")
    if payload.get("authoritative") is not True:
        reason = clean_text(payload.get("reason"), fallback="non-authoritative scope", limit=200)
        raise EvidenceError(f"control-plane scope is not authoritative: {reason}")
    observed = require_fingerprint(payload.get("scope_fingerprint"), "control-plane fingerprint")
    if observed != expected:
        raise EvidenceError("control-plane scope fingerprint does not match --expect-scope")
    return payload


def unquote_git_path(value: str) -> str:
    if len(value) < 2 or value[0] != '"' or value[-1] != '"':
        return value
    data = value[1:-1]
    output = bytearray()
    index = 0
    escapes = {
        "a": 7,
        "b": 8,
        "t": 9,
        "n": 10,
        "v": 11,
        "f": 12,
        "r": 13,
        '"': 34,
        "\\": 92,
        "?": 63,
    }
    while index < len(data):
        character = data[index]
        if character != "\\" or index + 1 >= len(data):
            output.extend(character.encode("utf-8"))
            index += 1
            continue
        escaped = data[index + 1]
        if escaped in escapes:
            output.append(escapes[escaped])
            index += 2
            continue
        if escaped in "01234567":
            end = index + 1
            while end < len(data) and end < index + 4 and data[end] in "01234567":
                end += 1
            output.append(int(data[index + 1 : end], 8))
            index = end
            continue
        output.extend(escaped.encode("utf-8"))
        index += 2
    return output.decode("utf-8", errors="replace")


def normalize_path(value: object, repo_root: pathlib.Path) -> str:
    path = urllib.parse.unquote(str(value or "").strip())
    if path.startswith("file://"):
        path = urllib.parse.urlparse(path).path
    path = path.replace("\\", "/")
    if re.match(r"^/[A-Za-z]:/", path):
        path = path[1:]
    candidate = pathlib.Path(path)
    if candidate.is_absolute():
        try:
            path = candidate.resolve().relative_to(repo_root.resolve()).as_posix()
        except (OSError, ValueError):
            return candidate.as_posix()
    while path.startswith("./"):
        path = path[2:]
    return pathlib.PurePosixPath(path).as_posix() if path else "unknown"


def normalize_severity(value: object) -> str:
    severity = str(value or "unknown").lower().replace("_", "-")
    aliases = {
        "fatal": "critical",
        "high": "error",
        "medium": "warning",
        "low": "note",
        "info": "note",
        "information": "note",
    }
    severity = aliases.get(severity, severity)
    return severity if severity in SEVERITY_ORDER else "unknown"


def normalize_confidence(value: object) -> str:
    confidence = str(value or "unknown").lower().replace("_", "-")
    aliases = {"veryhigh": "very-high", "moderate": "medium"}
    confidence = aliases.get(confidence, confidence)
    return confidence if confidence in CONFIDENCE_ORDER else "unknown"


def infer_category(rule_id: str, message: str, tool: str, tags: list[str]) -> str:
    corpus = " ".join([rule_id, message, tool, *tags]).lower()
    classifiers = [
        ("privacy", ("privacy", "pii", "personal-data")),
        ("security", ("security", "cwe-", "owasp", "injection", "xss", "ssrf", "auth", "vulnerability")),
        ("build", ("compiler", "compile", "type-check", "typecheck", "type-error", "type error", "rustc", "tsc", "mypy", "pyright", "javac")),
        ("data", ("data-loss", "migration", "database", "corruption")),
        ("compatibility", ("compatibility", "breaking", "api-contract")),
        ("reliability", ("reliability", "deadlock", "race-condition", "resource-leak")),
        ("performance", ("performance", "complexity", "n+1")),
        ("correctness", ("correctness", "null-deref", "use-after-free", "logic-error", "bug")),
        ("maintainability", ("maintainability", "style", "format", "documentation")),
    ]
    for category, needles in classifiers:
        if any(needle in corpus for needle in needles):
            return category
    return "unknown"


def embedded_sarif_scope(payload: dict[str, Any], run: dict[str, Any]) -> str | None:
    property_sources = [
        payload.get("properties"),
        run.get("properties"),
        (run.get("automationDetails") or {}).get("properties")
        if isinstance(run.get("automationDetails"), dict)
        else None,
    ]
    keys = (
        "preCommitReviewScopeFingerprint",
        "pre-commit-review/scopeFingerprint",
        "scope_fingerprint",
    )
    for properties in property_sources:
        if not isinstance(properties, dict):
            continue
        for key in keys:
            if properties.get(key):
                return str(properties[key])
    return None


def resolve_scope_binding(
    embedded: str | None, asserted: str | None, expected: str, report_label: str
) -> str:
    if embedded:
        observed = require_fingerprint(embedded, f"{report_label} embedded scope fingerprint")
        if observed != expected:
            raise EvidenceError(f"{report_label} scope fingerprint does not match the review scope")
        return "embedded"
    if asserted:
        observed = require_fingerprint(asserted, "--result-scope")
        if observed != expected:
            raise EvidenceError("--result-scope fingerprint does not match the review scope")
        return "explicit-assertion"
    raise EvidenceError(
        f"{report_label} has no embedded scope fingerprint; pass --result-scope only when you can assert its snapshot"
    )


def normalized_finding(
    finding: dict[str, Any], tool_name: str, tool_version: str | None, repo_root: pathlib.Path
) -> dict[str, Any]:
    required = ("rule_id", "message", "path", "severity", "category", "confidence")
    missing = [key for key in required if key not in finding]
    if missing:
        raise EvidenceError(f"normalized finding is missing required fields: {', '.join(missing)}")
    allowed_keys = set(required) | {"start_line", "end_line", "baseline_state"}
    unknown_keys = sorted(set(finding) - allowed_keys)
    if unknown_keys:
        raise EvidenceError(
            f"normalized finding has unsupported fields: {', '.join(unknown_keys)}"
        )
    for key in ("rule_id", "message", "path", "severity", "category", "confidence"):
        if not isinstance(finding[key], str) or not finding[key]:
            raise EvidenceError(f"normalized finding {key} must be a non-empty string")
    category = str(finding["category"])
    allowed_categories = MATERIAL_CATEGORIES | {"performance", "maintainability", "unknown"}
    if category not in allowed_categories:
        raise EvidenceError(f"normalized finding has unsupported category: {category}")
    severity = str(finding["severity"])
    if severity not in SEVERITY_ORDER:
        raise EvidenceError(f"normalized finding has unsupported severity: {severity}")
    confidence = str(finding["confidence"])
    if confidence not in CONFIDENCE_ORDER:
        raise EvidenceError(f"normalized finding has unsupported confidence: {confidence}")
    start_line = finding.get("start_line")
    end_line = finding.get("end_line", start_line)
    if start_line is not None and (type(start_line) is not int or start_line < 1):
        raise EvidenceError("normalized finding start_line must be a positive integer or null")
    if end_line is not None and (type(end_line) is not int or end_line < 1):
        raise EvidenceError("normalized finding end_line must be a positive integer or null")
    if start_line is not None and end_line is not None and end_line < start_line:
        raise EvidenceError("normalized finding end_line cannot precede start_line")
    baseline_value = finding.get("baseline_state", "unknown")
    if not isinstance(baseline_value, str):
        raise EvidenceError("normalized finding baseline_state must be a string")
    baseline = baseline_value
    if baseline not in {"new", "existing", "unknown"}:
        raise EvidenceError(f"normalized finding has unsupported baseline_state: {baseline}")
    return {
        "tool": {"name": tool_name, "version": tool_version},
        "rule_id": clean_text(finding["rule_id"], fallback="unknown-rule", limit=200),
        "message": clean_text(finding["message"], fallback="Static analyzer finding."),
        "path": normalize_path(finding["path"], repo_root),
        "start_line": start_line,
        "end_line": end_line,
        "severity": severity,
        "category": category,
        "confidence": confidence,
        "baseline_state": baseline,
    }


def parse_normalized(
    payload: dict[str, Any], raw: bytes, path: pathlib.Path, asserted_scope: str | None,
    expected_scope: str, repo_root: pathlib.Path
) -> list[ParsedReport]:
    if (
        type(payload.get("schema_version")) is not int
        or payload.get("schema_version") != 1
        or payload.get("kind") != "static_analysis_input"
    ):
        raise EvidenceError(f"{path.name} is neither SARIF 2.1.0 nor static_analysis_input/v1")
    allowed_payload_keys = {
        "schema_version",
        "kind",
        "scope_fingerprint",
        "tool",
        "status",
        "findings",
    }
    unknown_payload_keys = sorted(set(payload) - allowed_payload_keys)
    if unknown_payload_keys:
        raise EvidenceError(
            f"{path.name} normalized input has unsupported fields: {', '.join(unknown_payload_keys)}"
        )
    tool = payload.get("tool")
    if (
        not isinstance(tool, dict)
        or not isinstance(tool.get("name"), str)
        or not tool.get("name")
    ):
        raise EvidenceError(f"{path.name} normalized input is missing tool.name")
    unknown_tool_keys = sorted(set(tool) - {"name", "version"})
    if unknown_tool_keys:
        raise EvidenceError(
            f"{path.name} normalized tool has unsupported fields: {', '.join(unknown_tool_keys)}"
        )
    if tool.get("version") is not None and not isinstance(tool.get("version"), str):
        raise EvidenceError(f"{path.name} normalized tool.version must be a string or null")
    tool_name = clean_text(tool["name"], fallback="unknown-tool", limit=200)
    tool_version = clean_text(tool.get("version"), fallback="", limit=100) or None
    status_value = payload.get("status", "")
    if not isinstance(status_value, str):
        raise EvidenceError(f"{path.name} normalized input status must be a string")
    status = status_value
    if status not in {"completed", "failed", "timeout", "unavailable"}:
        raise EvidenceError(f"{path.name} normalized input has unsupported status: {status}")
    findings_value = payload.get("findings")
    if not isinstance(findings_value, list):
        raise EvidenceError(f"{path.name} normalized input findings must be an array")
    embedded_scope = str(payload.get("scope_fingerprint") or "") or None
    if not embedded_scope:
        raise EvidenceError(f"{path.name} normalized input must embed scope_fingerprint")
    binding = resolve_scope_binding(embedded_scope, None, expected_scope, path.name)
    findings = []
    for finding in findings_value:
        if not isinstance(finding, dict):
            raise EvidenceError(f"{path.name} contains a non-object normalized finding")
        findings.append(normalized_finding(finding, tool_name, tool_version, repo_root))
    report_id = compact_hash(raw, 0, tool_name)
    return [
        ParsedReport(
            report_id=report_id,
            format="normalized-json",
            tool_name=tool_name,
            tool_version=tool_version,
            status=status,
            scope_binding=binding,
            finding_count=len(findings_value),
            findings=findings,
        )
    ]


def sarif_rule_maps(driver: dict[str, Any]) -> tuple[dict[str, dict[str, Any]], dict[int, dict[str, Any]]]:
    by_id: dict[str, dict[str, Any]] = {}
    by_index: dict[int, dict[str, Any]] = {}
    rules = driver.get("rules")
    if not isinstance(rules, list):
        return by_id, by_index
    for index, rule in enumerate(rules):
        if not isinstance(rule, dict):
            continue
        by_index[index] = rule
        if rule.get("id"):
            by_id[str(rule["id"])] = rule
    return by_id, by_index


def sarif_result_locations(result: dict[str, Any]) -> list[dict[str, Any] | None]:
    locations = result.get("locations")
    if not isinstance(locations, list) or not locations:
        return [None]
    return [location if isinstance(location, dict) else None for location in locations]


def parse_sarif(
    payload: dict[str, Any], raw: bytes, path: pathlib.Path, asserted_scope: str | None,
    expected_scope: str, repo_root: pathlib.Path
) -> list[ParsedReport]:
    if payload.get("version") != "2.1.0" or not isinstance(payload.get("runs"), list):
        raise EvidenceError(f"{path.name} is neither SARIF 2.1.0 nor static_analysis_input/v1")
    reports: list[ParsedReport] = []
    for run_index, run_value in enumerate(payload["runs"]):
        if not isinstance(run_value, dict):
            raise EvidenceError(f"{path.name} SARIF run {run_index} must be an object")
        run = run_value
        binding = resolve_scope_binding(
            embedded_sarif_scope(payload, run),
            asserted_scope,
            expected_scope,
            f"{path.name} SARIF run {run_index}",
        )
        driver = ((run.get("tool") or {}).get("driver") or {}) if isinstance(run.get("tool"), dict) else {}
        if not isinstance(driver, dict):
            driver = {}
        tool_name = clean_text(driver.get("name"), fallback="unknown-sarif-tool", limit=200)
        tool_version = clean_text(
            driver.get("semanticVersion") or driver.get("version"), fallback="", limit=100
        ) or None
        by_id, by_index = sarif_rule_maps(driver)
        invocations = run.get("invocations")
        status = "completed"
        if isinstance(invocations, list) and any(
            isinstance(item, dict) and item.get("executionSuccessful") is False
            for item in invocations
        ):
            status = "failed"
        results = run.get("results")
        if not isinstance(results, list):
            results = []
        findings: list[dict[str, Any]] = []
        for result_index, result_value in enumerate(results):
            if not isinstance(result_value, dict):
                continue
            result = result_value
            if result.get("baselineState") == "absent":
                continue
            rule_id = clean_text(result.get("ruleId"), fallback=f"result-{result_index}", limit=200)
            rule = by_id.get(rule_id, {})
            rule_index = result.get("ruleIndex")
            if not rule and isinstance(rule_index, int):
                rule = by_index.get(rule_index, {})
            rule_properties = rule.get("properties") if isinstance(rule.get("properties"), dict) else {}
            result_properties = result.get("properties") if isinstance(result.get("properties"), dict) else {}
            tags_value = result_properties.get("tags", rule_properties.get("tags", []))
            tags = [str(tag) for tag in tags_value] if isinstance(tags_value, list) else []
            message_value = result.get("message")
            if isinstance(message_value, dict):
                message = message_value.get("text") or message_value.get("markdown")
            else:
                message = message_value
            message_text = clean_text(message, fallback="Static analyzer finding.")
            default_configuration = rule.get("defaultConfiguration") if isinstance(rule.get("defaultConfiguration"), dict) else {}
            severity = normalize_severity(
                result_properties.get("severity")
                or result.get("level")
                or default_configuration.get("level")
            )
            confidence = normalize_confidence(
                result_properties.get("precision") or rule_properties.get("precision")
            )
            category = infer_category(rule_id, message_text, tool_name, tags)
            baseline_raw = str(result.get("baselineState") or "unknown")
            baseline = {
                "new": "new",
                "updated": "new",
                "unchanged": "existing",
            }.get(baseline_raw, "unknown")
            for location in sarif_result_locations(result):
                path_value: object = "unknown"
                start_line: int | None = None
                end_line: int | None = None
                if location:
                    physical = location.get("physicalLocation")
                    if isinstance(physical, dict):
                        artifact = physical.get("artifactLocation")
                        if isinstance(artifact, dict):
                            path_value = artifact.get("uri") or artifact.get("uriBaseId") or "unknown"
                        region = physical.get("region")
                        if isinstance(region, dict):
                            if isinstance(region.get("startLine"), int) and region["startLine"] > 0:
                                start_line = region["startLine"]
                            if isinstance(region.get("endLine"), int) and region["endLine"] > 0:
                                end_line = region["endLine"]
                if start_line is not None and end_line is None:
                    end_line = start_line
                if start_line is not None and end_line is not None and end_line < start_line:
                    end_line = start_line
                findings.append(
                    {
                        "tool": {"name": tool_name, "version": tool_version},
                        "rule_id": rule_id,
                        "message": message_text,
                        "path": normalize_path(path_value, repo_root),
                        "start_line": start_line,
                        "end_line": end_line,
                        "severity": severity,
                        "category": category,
                        "confidence": confidence,
                        "baseline_state": baseline,
                    }
                )
        report_id = compact_hash(raw, run_index, tool_name)
        reports.append(
            ParsedReport(
                report_id=report_id,
                format="sarif",
                tool_name=tool_name,
                tool_version=tool_version,
                status=status,
                scope_binding=binding,
                finding_count=len(findings),
                findings=findings,
            )
        )
    if not reports:
        raise EvidenceError(f"{path.name} SARIF input contains no runs")
    return reports


def parse_report_file(
    path: pathlib.Path, asserted_scope: str | None, expected_scope: str, repo_root: pathlib.Path
) -> list[ParsedReport]:
    payload, raw = load_json_file(path)
    if payload.get("version") == "2.1.0" and isinstance(payload.get("runs"), list):
        return parse_sarif(payload, raw, path, asserted_scope, expected_scope, repo_root)
    return parse_normalized(payload, raw, path, asserted_scope, expected_scope, repo_root)


def git_added_lines(source: str, selected_ref: str, path: str) -> set[int]:
    command = [
        "git",
        "-c",
        "color.ui=false",
        "diff",
        "--no-ext-diff",
        "--no-textconv",
        "--find-renames",
        "--unified=0",
    ]
    if source == "staged":
        command.append("--cached")
    elif source == "branch":
        if not selected_ref:
            raise EvidenceError("branch scope is missing selected_ref")
        command.append(f"{selected_ref}...HEAD")
    command.extend(["--", path])
    completed = subprocess.run(
        command,
        check=False,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
    )
    if completed.returncode != 0:
        detail = clean_text(
            completed.stderr.decode("utf-8", errors="replace"), fallback="git diff failed", limit=500
        )
        raise EvidenceError(f"cannot map changed lines for {path}: {detail}")
    text = completed.stdout.decode("utf-8", errors="replace")
    added: set[int] = set()
    current_new: int | None = None
    for line in text.splitlines():
        match = HUNK_RE.match(line)
        if match:
            current_new = int(match.group("new"))
            continue
        if current_new is None:
            continue
        if line.startswith("+") and not line.startswith("+++"):
            added.add(current_new)
            current_new += 1
        elif line.startswith("-") and not line.startswith("---"):
            continue
        elif line.startswith("\\ No newline at end of file"):
            continue
        else:
            current_new += 1
    return added


def merge_findings(reports: list[ParsedReport]) -> tuple[list[dict[str, Any]], int]:
    merged: dict[tuple[object, ...], dict[str, Any]] = {}
    input_count = 0
    for report in reports:
        input_count += report.finding_count
        for finding in report.findings:
            key = (
                finding["tool"]["name"],
                finding["rule_id"],
                finding["message"],
                finding["path"],
                finding["start_line"],
                finding["end_line"],
            )
            if key not in merged:
                item = dict(finding)
                item["report_ids"] = [report.report_id]
                item["_completed"] = report.status == "completed"
                merged[key] = item
                continue
            item = merged[key]
            if report.report_id not in item["report_ids"]:
                item["report_ids"].append(report.report_id)
            if SEVERITY_ORDER[finding["severity"]] > SEVERITY_ORDER[item["severity"]]:
                item["severity"] = finding["severity"]
            if CONFIDENCE_ORDER[finding["confidence"]] > CONFIDENCE_ORDER[item["confidence"]]:
                item["confidence"] = finding["confidence"]
            if item["category"] == "unknown" and finding["category"] != "unknown":
                item["category"] = finding["category"]
            if finding["baseline_state"] == "new":
                item["baseline_state"] = "new"
            elif item["baseline_state"] == "unknown" and finding["baseline_state"] == "existing":
                item["baseline_state"] = "existing"
            item["_completed"] = item["_completed"] or report.status == "completed"
    values = list(merged.values())
    values.sort(
        key=lambda item: (
            item["path"],
            item["start_line"] or 0,
            item["tool"]["name"],
            item["rule_id"],
            item["message"],
        )
    )
    return values, input_count


def deduplicate_reports(reports: list[ParsedReport]) -> list[ParsedReport]:
    unique: dict[str, ParsedReport] = {}
    for report in reports:
        existing = unique.get(report.report_id)
        if existing is None:
            unique[report.report_id] = report
            continue
        if (
            existing.format != report.format
            or existing.tool_name != report.tool_name
            or existing.tool_version != report.tool_version
            or existing.status != report.status
            or existing.findings != report.findings
        ):
            raise EvidenceError(f"report identifier collision: {report.report_id}")
    return list(unique.values())


def classify_findings(
    findings: list[dict[str, Any]], control: dict[str, Any], repo_root: pathlib.Path
) -> None:
    units: dict[str, tuple[str, str]] = {}
    for unit in control["units"]:
        display_path = str(unit[0])
        raw_path = unquote_git_path(display_path)
        units[normalize_path(raw_path, repo_root)] = (display_path, f"file:{display_path}")
    needed_paths = sorted({item["path"] for item in findings if item["path"] in units})
    added_by_path = {
        path: git_added_lines(control["source"], str(control.get("selected_ref") or ""), unquote_git_path(units[path][0]))
        for path in needed_paths
    }
    for item in findings:
        unit = units.get(item["path"])
        start_line = item["start_line"]
        end_line = item["end_line"]
        if unit is None:
            line_scope = "outside-scope"
            manifest_unit_id = None
        else:
            manifest_unit_id = unit[1]
            if start_line is None:
                line_scope = "unknown"
            else:
                end = end_line or start_line
                line_scope = (
                    "added"
                    if any(start_line <= line <= end for line in added_by_path[item["path"]])
                    else "unchanged"
                )
        if line_scope == "added":
            item["baseline_state"] = "new"
        blocking = (
            item["_completed"]
            and line_scope == "added"
            and item["baseline_state"] == "new"
            and item["category"] in MATERIAL_CATEGORIES
            and item["severity"] in {"critical", "error"}
            and item["confidence"] in {"high", "very-high"}
        )
        if line_scope == "outside-scope":
            disposition = "outside-scope"
        elif blocking:
            disposition = "blocking-candidate"
        elif (
            item["_completed"]
            and item["category"] in MATERIAL_CATEGORIES
            and item["severity"] in {"critical", "error", "warning"}
            and (
                line_scope == "added"
                or item["baseline_state"] == "new"
                or (line_scope == "unknown" and manifest_unit_id is not None)
            )
        ):
            disposition = "priority-candidate"
        else:
            disposition = "note"
        item["manifest_unit_id"] = manifest_unit_id
        item["line_scope"] = line_scope
        item["disposition"] = disposition
        item["blocking_candidate"] = blocking
        item["finding_id"] = compact_hash(
            item["tool"]["name"],
            item["rule_id"],
            item["message"],
            item["path"],
            start_line,
            end_line,
        )
        item["report_ids"].sort()
        del item["_completed"]
    disposition_order = {
        "blocking-candidate": 0,
        "priority-candidate": 1,
        "note": 2,
        "outside-scope": 3,
    }
    findings.sort(
        key=lambda item: (
            disposition_order[item["disposition"]],
            -SEVERITY_ORDER[item["severity"]],
            -CONFIDENCE_ORDER[item["confidence"]],
            item["path"],
            item["start_line"] or 0,
            item["tool"]["name"],
            item["rule_id"],
        )
    )


def evidence_payload(
    reports: list[ParsedReport], findings: list[dict[str, Any]], input_count: int,
    control: dict[str, Any], max_findings: int, trust: str, execution_id: str | None
) -> dict[str, Any]:
    counts = {
        "reports": len(reports),
        "input_findings": input_count,
        "deduplicated_findings": len(findings),
        "mapped_to_units": sum(item["manifest_unit_id"] is not None for item in findings),
        "added_line": sum(item["line_scope"] == "added" for item in findings),
        "blocking_candidates": sum(item["disposition"] == "blocking-candidate" for item in findings),
        "priority_candidates": sum(item["disposition"] == "priority-candidate" for item in findings),
        "notes": sum(item["disposition"] == "note" for item in findings),
        "outside_scope": sum(item["disposition"] == "outside-scope" for item in findings),
    }
    report_values = [
        {
            "report_id": report.report_id,
            "format": report.format,
            "tool": {"name": report.tool_name, "version": report.tool_version},
            "status": report.status,
            "trust": trust,
            "scope_binding": (
                "controlled-execution" if trust == "controlled-execution" else report.scope_binding
            ),
            "execution_id": execution_id,
            "finding_count": report.finding_count,
        }
        for report in reports
    ]
    report_values.sort(key=lambda item: item["report_id"])
    return {
        "schema_version": 1,
        "kind": "static_analysis_evidence",
        "authoritative": True,
        "scope": {
            "source": control["source"],
            "head": control["head"],
            "fingerprint": control["scope_fingerprint"],
        },
        "reports": report_values,
        "counts": counts,
        "findings": findings[:max_findings],
        "truncated": len(findings) > max_findings,
        "decision_contract": {
            "blocking": "blocking-candidate findings require independent finding verification and normally force DO_NOT_COMMIT when confirmed",
            "non_blocking": "historical, unbaselined unchanged, maintainability-only, failed-report, and outside-scope findings cannot block by themselves",
            "verification": "trace every blocking or priority candidate to the changed execution point before final severity and verdict selection",
            "finalization": "expand truncated evidence before claiming complete static review, disposition every material candidate, and require the final control-plane fingerprint to match this evidence scope",
        },
    }


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser(
        description="Normalize explicit SARIF/JSON reports against an authoritative review scope."
    )
    parser.add_argument("--result", action="append", required=True, help="SARIF or static_analysis_input/v1 JSON file; repeatable")
    parser.add_argument("--source", choices=("staged", "unstaged", "branch"), help="explicit review source; defaults to helper resolution")
    parser.add_argument("--expect-scope", required=True, help="opening control-plane scope fingerprint")
    parser.add_argument("--result-scope", help="explicitly assert the snapshot for reports without an embedded fingerprint")
    parser.add_argument("--helper", help="path to collect_diff_context.sh")
    parser.add_argument("--max-findings", type=int, default=500, help="maximum normalized findings emitted; default 500")
    parser.add_argument(
        "--trust",
        choices=("explicit-input", "controlled-execution"),
        default="explicit-input",
        help="evidence provenance; controlled-execution is reserved for run_static_analysis.py",
    )
    parser.add_argument(
        "--execution-id",
        help="16-character controlled execution identifier",
    )
    return parser.parse_args()


def main() -> int:
    args = parse_args()
    expected = require_fingerprint(args.expect_scope, "--expect-scope")
    if args.result_scope:
        require_fingerprint(args.result_scope, "--result-scope")
    if args.max_findings < 1 or args.max_findings > 5000:
        raise EvidenceError("--max-findings must be between 1 and 5000")
    if args.trust == "controlled-execution":
        if not args.execution_id or not re.fullmatch(r"[0-9a-f]{16}", args.execution_id):
            raise EvidenceError("controlled-execution trust requires a valid --execution-id")
    elif args.execution_id:
        raise EvidenceError("--execution-id is valid only with --trust controlled-execution")
    script_dir = pathlib.Path(__file__).resolve().parent
    helper = pathlib.Path(args.helper).resolve() if args.helper else script_dir / "collect_diff_context.sh"
    if not helper.is_file():
        raise EvidenceError(f"helper does not exist: {helper}")
    repo_root_result = subprocess.run(
        ["git", "rev-parse", "--show-toplevel"],
        check=False,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        text=True,
        encoding="utf-8",
        errors="replace",
    )
    if repo_root_result.returncode != 0:
        raise EvidenceError("current directory is not a Git repository")
    repo_root = pathlib.Path(repo_root_result.stdout.strip()).resolve()
    control = run_control_plane(helper, args.source, expected)
    reports: list[ParsedReport] = []
    for result_path in args.result:
        reports.extend(
            parse_report_file(
                pathlib.Path(result_path).resolve(),
                args.result_scope,
                expected,
                repo_root,
            )
        )
    reports = deduplicate_reports(reports)
    if sum(report.finding_count for report in reports) > MAX_INPUT_FINDINGS:
        raise EvidenceError(f"static results exceed the {MAX_INPUT_FINDINGS}-finding processing limit")
    findings, input_count = merge_findings(reports)
    classify_findings(findings, control, repo_root)
    final_control = run_control_plane(helper, control["source"], expected)
    for key in ("scope_fingerprint", "units", "groups", "work_order"):
        if final_control.get(key) != control.get(key):
            raise EvidenceError(f"review scope changed while collecting static evidence: {key}")
    payload = evidence_payload(
        reports,
        findings,
        input_count,
        final_control,
        args.max_findings,
        args.trust,
        args.execution_id,
    )
    print("# Pre-Commit Review Static Analysis Evidence\n")
    print("## Static Analysis Evidence JSON")
    print(json.dumps(payload, ensure_ascii=False, separators=(",", ":")))
    return 0


if __name__ == "__main__":
    try:
        raise SystemExit(main())
    except EvidenceError as exc:
        print(f"collect_static_evidence: {exc}", file=sys.stderr)
        raise SystemExit(2)
