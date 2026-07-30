#!/usr/bin/env python3
import argparse
import hashlib
import json
import os
import re
import sys
from pathlib import Path

MAX_JSON_BYTES = 1024 * 1024
MAX_COMPRESSED_BYTES = 512 * 1024 * 1024
MAX_EXPANDED_BYTES = 2 * 1024 * 1024 * 1024
SOURCE_LOCK_SHA256 = (
    "298bc6c0339fe2c58fd35bfbd53db285ea7ff34e40734a4f0c36ccb3fe60d862"
)
PACK_VERSION = "2026.07.27-pcr.3"
TOOL_VERSION = "2026-07-27"
RELEASE_TAG = "artifact-rust-analyzer-2026.07.27-pcr.3"
REPOSITORY = "junit/pre-commit-review"
WORKFLOW = ".github/workflows/artifact-pack-release.yml"
ISSUER = "https://token.actions.githubusercontent.com"
PREDICATE_TYPE = "pre-commit-review.artifact-pack/v1"
PLATFORMS = [
    "darwin-amd64",
    "darwin-arm64",
    "linux-amd64",
    "windows-amd64",
]
SHA256 = re.compile(r"^[0-9a-f]{64}$")
COMMIT = re.compile(r"^[0-9a-f]{40}$")
IDENTIFIER = re.compile(r"^[a-z0-9][a-z0-9-]{0,127}$")


class GenerationError(Exception):
    def __init__(self, code, message):
        super().__init__(message)
        self.code = code


def fail(code, message):
    raise GenerationError(code, message)


def canonical_bytes(value):
    return json.dumps(value, ensure_ascii=False, separators=(",", ":")).encode("utf-8")


def canonical_output_bytes(value):
    return json.dumps(
        value, ensure_ascii=False, separators=(",", ":"), sort_keys=True
    ).encode("utf-8")


def read_canonical(path, code="canonical-json"):
    try:
        if path.is_symlink() or not path.is_file():
            fail(code, f"{path.name} is not a regular file")
        raw = path.read_bytes()
    except OSError as exc:
        fail(code, f"could not read {path.name}: {exc}")
    if not raw or len(raw) > MAX_JSON_BYTES:
        fail(code, f"{path.name} is outside its byte limit")
    try:
        value = json.loads(raw.decode("utf-8"))
    except (UnicodeDecodeError, json.JSONDecodeError) as exc:
        fail(code, f"{path.name} is not valid JSON: {exc}")
    if not isinstance(value, dict) or canonical_bytes(value) != raw:
        fail(code, f"{path.name} is not compact canonical JSON")
    return value, raw


def require_fields(value, expected, code, label):
    if not isinstance(value, dict) or set(value) != set(expected):
        fail(code, f"{label} fields are incomplete or unexpected")


def require_sha256(value, code, label):
    if not isinstance(value, str) or not SHA256.fullmatch(value):
        fail(code, f"{label} is not a lower-case SHA256 digest")
    return value


def require_identifier(value, code, label):
    if not isinstance(value, str) or not IDENTIFIER.fullmatch(value):
        fail(code, f"{label} is not a bounded identifier")
    return value


def require_positive_integer(value, maximum, code, label):
    if isinstance(value, bool) or not isinstance(value, int) or not 0 < value <= maximum:
        fail(code, f"{label} is outside its authorized range")
    return value


def digest(raw):
    return hashlib.sha256(raw).hexdigest()


def core_release_context():
    marker = os.environ.get("PCR_CORE_RELEASE_JOB", "").lower()
    if marker not in {"", "0", "false"}:
        return True
    workflow_ref = os.environ.get("GITHUB_WORKFLOW_REF", "")
    if "/.github/workflows/release.yml@" in workflow_ref:
        return True
    workflow_name = os.environ.get("GITHUB_WORKFLOW", "").lower()
    return os.environ.get("GITHUB_ACTIONS", "").lower() == "true" and workflow_name in {
        "release",
        "release multi-platform packs",
    }


def load_source_lock(repo_root):
    path = repo_root / "third_party_artifacts/sources/rust-analyzer-2026-07-27.json"
    source_lock, raw = read_canonical(path, "source-lock-binding")
    if digest(raw) != SOURCE_LOCK_SHA256:
        fail("source-lock-binding", "reviewed source-lock bytes have drifted")
    required = {
        "schema_version",
        "kind",
        "artifact_id",
        "tool_version",
        "upstream_repository",
        "upstream_tag",
        "upstream_commit",
        "assets",
    }
    require_fields(source_lock, required, "source-lock-binding", "source lock")
    if (
        source_lock["schema_version"] != 1
        or source_lock["kind"] != "third_party_sources"
        or source_lock["artifact_id"] != "rust-analyzer"
        or source_lock["tool_version"] != TOOL_VERSION
        or source_lock["upstream_repository"] != "rust-lang/rust-analyzer"
        or source_lock["upstream_tag"] != TOOL_VERSION
        or not COMMIT.fullmatch(source_lock.get("upstream_commit", ""))
    ):
        fail("source-lock-binding", "source-lock identity is not reviewed")
    assets = source_lock.get("assets")
    if (
        not isinstance(assets, list)
        or any(not isinstance(item, dict) for item in assets)
        or [item.get("platform_id") for item in assets] != PLATFORMS
    ):
        fail("source-lock-binding", "source-lock platform inventory is incomplete")
    return source_lock, {item["platform_id"]: item for item in assets}


def validate_publication_identity(publication):
    required = {
        "schema_version",
        "kind",
        "verification_status",
        "repository",
        "workflow",
        "ref",
        "commit",
        "issuer",
        "artifact_id",
        "tool_version",
        "pack_version",
        "source_lock_sha256",
        "platforms",
    }
    require_fields(publication, required, "publication-contract", "publication")
    if (
        publication["schema_version"] != 1
        or publication["kind"] != "verified_provider_publication"
        or publication["verification_status"] != "verified"
        or publication["repository"] != REPOSITORY
        or publication["workflow"] != WORKFLOW
        or publication["ref"] != f"refs/tags/{RELEASE_TAG}"
        or publication["issuer"] != ISSUER
        or publication["artifact_id"] != "rust-analyzer"
        or publication["tool_version"] != TOOL_VERSION
        or publication["pack_version"] != PACK_VERSION
        or not COMMIT.fullmatch(publication.get("commit", ""))
    ):
        fail("publication-contract", "publication identity is not reviewed")
    if publication["source_lock_sha256"] != SOURCE_LOCK_SHA256:
        fail("source-lock-binding", "publication does not bind the reviewed source lock")


def validate_file_binding(value, expected_path=None):
    require_fields(value, {"path", "size", "sha256"}, "publication-contract", "file binding")
    path = value["path"]
    if not isinstance(path, str) or not path or path.startswith(("/", "../")) or "\\" in path:
        fail("publication-contract", "file binding path is not relative")
    if expected_path is not None and path != expected_path:
        fail("publication-contract", "file binding path does not match the reviewed path")
    require_positive_integer(value["size"], MAX_EXPANDED_BYTES, "publication-contract", "file size")
    require_sha256(value["sha256"], "publication-digest", "file digest")


def expected_subject_names(platform):
    return [
        f"pre-commit-review-rust-analyzer-{PACK_VERSION}-{platform}.tar.gz",
        f"rust-analyzer-{platform}.pack-manifest.json",
        f"rust-analyzer-{platform}.sbom.cdx.json",
    ]


def validate_attestation(subject, composition):
    attestation = subject.get("attestation")
    require_fields(
        attestation,
        {"verification_status", "predicate_type", "subject", "composition"},
        "attestation-contract",
        "attestation",
    )
    require_fields(
        attestation["subject"],
        {"name", "sha256"},
        "attestation-contract",
        "attestation subject",
    )
    if (
        attestation["verification_status"] != "verified"
        or attestation["predicate_type"] != PREDICATE_TYPE
        or attestation["subject"]
        != {"name": subject["name"], "sha256": subject["sha256"]}
        or attestation["composition"] != composition
    ):
        fail("attestation-contract", "attestation does not bind its exact subject and composition")


def validate_composition(value, publication, asset):
    fields = {
        "source_lock_sha256",
        "upstream_archive_sha256",
        "pack_builder_commit",
        "pack_manifest_sha256",
        "sbom_sha256",
        "generator_configuration_sha256",
    }
    require_fields(value, fields, "attestation-composition", "composition")
    for field in fields - {"pack_builder_commit"}:
        require_sha256(value[field], "publication-digest", field)
    if (
        value["source_lock_sha256"] != SOURCE_LOCK_SHA256
        or value["upstream_archive_sha256"] != asset["archive_sha256"]
        or value["pack_builder_commit"] != publication["commit"]
    ):
        fail("attestation-composition", "composition materials are not release-bound")


def validate_subjects(platform, composition):
    subjects = platform.get("subjects")
    if (
        not isinstance(subjects, list)
        or any(not isinstance(item, dict) for item in subjects)
        or [item.get("role") for item in subjects] != ["pack", "manifest", "sbom"]
    ):
        fail("attestation-contract", "platform must contain three ordered subject attestations")
    names = expected_subject_names(platform["platform_id"])
    for subject, expected_name in zip(subjects, names):
        if not isinstance(subject, dict) or "sha256" not in subject:
            fail("publication-digest", "publication subject digest is missing")
        require_fields(
            subject,
            {"role", "name", "sha256", "attestation"},
            "attestation-contract",
            "publication subject",
        )
        if subject["name"] != expected_name:
            fail("publication-state", "publication subject does not use its final asset name")
        require_sha256(subject.get("sha256"), "publication-digest", "publication subject digest")
        validate_attestation(subject, composition)
    if (
        subjects[1]["sha256"] != composition["pack_manifest_sha256"]
        or subjects[2]["sha256"] != composition["sbom_sha256"]
    ):
        fail("attestation-composition", "manifest or SBOM subject is not composition-bound")
    return subjects


def validate_platform(platform, publication, asset):
    fields = {
        "platform_id",
        "target_triple",
        "published",
        "expected_compressed_size",
        "max_compressed_size",
        "executable",
        "license_files",
        "baseline_binding",
        "composition",
        "subjects",
    }
    require_fields(platform, fields, "publication-contract", "platform publication")
    if platform["published"] is not True:
        fail("publication-state", "provider pack is not published")
    if platform["target_triple"] != asset["target_triple"]:
        fail("publication-contract", "publication target does not match the source lock")
    expected_size = require_positive_integer(
        platform["expected_compressed_size"],
        MAX_COMPRESSED_BYTES,
        "publication-contract",
        "pack size",
    )
    maximum = require_positive_integer(
        platform["max_compressed_size"],
        MAX_COMPRESSED_BYTES,
        "publication-contract",
        "maximum pack size",
    )
    if maximum < expected_size:
        fail("publication-contract", "pack size exceeds its reviewed maximum")
    expected_executable = f"bin/{asset['executable_name']}"
    validate_file_binding(platform["executable"], expected_executable)
    if (
        platform["executable"]["size"] != asset["executable_size"]
        or platform["executable"]["sha256"] != asset["executable_sha256"]
    ):
        fail("publication-contract", "executable binding differs from the source lock")
    validate_license_files(platform.get("license_files"))
    validate_baseline_binding(platform.get("baseline_binding"))
    validate_composition(platform.get("composition"), publication, asset)
    return validate_subjects(platform, platform["composition"])


def validate_license_files(licenses):
    if not isinstance(licenses, list) or len(licenses) != 2:
        fail("publication-contract", "provider publication must contain two license files")
    expected = ["licenses/LICENSE-APACHE", "licenses/LICENSE-MIT"]
    for license_file, expected_path in zip(licenses, expected):
        validate_file_binding(license_file, expected_path)


def validate_baseline_binding(binding):
    fields = {
        "profile_sha256",
        "fixture_id",
        "fixture_sha256",
        "request_sha256",
        "runner_class",
    }
    require_fields(binding, fields, "publication-contract", "baseline binding")
    for field in ["profile_sha256", "fixture_sha256", "request_sha256"]:
        require_sha256(binding[field], "publication-digest", field)
    require_identifier(binding["fixture_id"], "publication-contract", "fixture id")
    require_identifier(binding["runner_class"], "publication-contract", "runner class")


def validate_publication(publication, assets):
    validate_publication_identity(publication)
    platforms = publication.get("platforms")
    if not isinstance(platforms, list) or any(
        not isinstance(item, dict) for item in platforms
    ):
        fail("publication-contract", "publication platforms must be objects")
    if [item.get("platform_id") for item in platforms] != PLATFORMS:
        fail("publication-state", "publication must contain the sorted four-platform set")
    pack_digests = set()
    for platform in platforms:
        subjects = validate_platform(platform, publication, assets[platform["platform_id"]])
        pack_digests.add(subjects[0]["sha256"])
    if len(pack_digests) != len(PLATFORMS):
        fail("publication-state", "platform pack digests must be independent")
    return platforms


def validate_samples(measurement):
    samples = measurement.get("samples_ms")
    if (
        not isinstance(samples, list)
        or not 20 <= len(samples) <= 100
        or any(
            isinstance(sample, bool) or not isinstance(sample, int) or not 0 < sample <= 30_000
            for sample in samples
        )
    ):
        fail("baseline-binding", "baseline samples are outside the authorized range")
    ordered = sorted(samples)
    rank = (len(ordered) * 95 + 99) // 100
    p95_ms = require_positive_integer(
        measurement.get("p95_ms"), 30_000, "baseline-binding", "baseline p95"
    )
    if p95_ms != ordered[rank - 1]:
        fail("baseline-binding", "baseline p95 does not use nearest-rank selection")
    require_positive_integer(
        measurement.get("peak_process_tree_rss_bytes"),
        MAX_EXPANDED_BYTES,
        "baseline-binding",
        "baseline RSS",
    )


def validate_measurement(measurement, platform, subjects):
    fields = {
        "platform_id",
        "pack_sha256",
        "executable_sha256",
        "profile_sha256",
        "fixture_id",
        "fixture_sha256",
        "request_sha256",
        "runner_class",
        "samples_ms",
        "p95_ms",
        "peak_process_tree_rss_bytes",
    }
    require_fields(measurement, fields, "baseline-binding", "baseline measurement")
    expected = {
        "platform_id": platform["platform_id"],
        "pack_sha256": subjects[0]["sha256"],
        "executable_sha256": platform["executable"]["sha256"],
        **platform["baseline_binding"],
    }
    if any(measurement.get(field) != value for field, value in expected.items()):
        fail("baseline-binding", "baseline measurement differs from its publication binding")
    for field in [
        "pack_sha256",
        "executable_sha256",
        "profile_sha256",
        "fixture_sha256",
        "request_sha256",
    ]:
        require_sha256(measurement[field], "baseline-binding", field)
    validate_samples(measurement)


def validate_baseline(baseline, publication, platforms):
    fields = {
        "schema_version",
        "kind",
        "artifact_id",
        "pack_version",
        "source_lock_sha256",
        "measurements",
    }
    require_fields(baseline, fields, "baseline-binding", "baseline")
    if (
        baseline["schema_version"] != 1
        or baseline["kind"] != "third_party_artifact_baseline"
        or baseline["artifact_id"] != publication["artifact_id"]
        or baseline["pack_version"] != publication["pack_version"]
        or baseline["source_lock_sha256"] != publication["source_lock_sha256"]
    ):
        fail("baseline-binding", "baseline identity differs from the publication")
    measurements = baseline.get("measurements")
    if (
        not isinstance(measurements, list)
        or any(not isinstance(item, dict) for item in measurements)
        or [item.get("platform_id") for item in measurements] != PLATFORMS
    ):
        fail("baseline-binding", "baseline must contain one sorted measurement per platform")
    for measurement, platform in zip(measurements, platforms):
        validate_measurement(measurement, platform, platform["subjects"])


def build_manifest_record(source_lock, platform, quality_baseline_sha256):
    subjects = platform["subjects"]
    return {
        "artifact_id": "rust-analyzer",
        "artifact_role": "repository-context-provider",
        "tool_version": TOOL_VERSION,
        "upstream_repository": source_lock["upstream_repository"],
        "upstream_tag": source_lock["upstream_tag"],
        "upstream_commit": source_lock["upstream_commit"],
        "source_lock_sha256": SOURCE_LOCK_SHA256,
        "platform_id": platform["platform_id"],
        "target_triple": platform["target_triple"],
        "state": "active",
        "pack_version": PACK_VERSION,
        "project_release_tag": RELEASE_TAG,
        "project_asset_name": subjects[0]["name"],
        "expected_compressed_size": platform["expected_compressed_size"],
        "max_compressed_size": platform["max_compressed_size"],
        "pack_sha256": subjects[0]["sha256"],
        "pack_manifest_sha256": subjects[1]["sha256"],
        "sbom_sha256": subjects[2]["sha256"],
        "pack_format": "normalized-tar-gzip-v1",
        "executable": platform["executable"],
        "version_probe": "rust-analyzer-version-v1",
        "capability_probe": "rust-analyzer-stdio-v1",
        "expected_version": next(
            asset["expected_version_output"]
            for asset in source_lock["assets"]
            if asset["platform_id"] == platform["platform_id"]
        ),
        "license_component": "rust-analyzer",
        "license_files": platform["license_files"],
        "sbom_component": "pkg:github/rust-lang/rust-analyzer@2026-07-27",
        "default_configuration_sha256": None,
        "quality_baseline_sha256": quality_baseline_sha256,
        "revoked_reason": None,
        "replacement_pack_version": None,
    }


def build_candidate(repo_root, publication_raw, baseline_raw, platforms, source_lock):
    manifest_path = repo_root / "third_party_artifacts/manifest.json"
    manifest, manifest_raw = read_canonical(manifest_path, "manifest-state")
    if any(pack.get("artifact_id") == "rust-analyzer" for pack in manifest.get("packs", [])):
        fail("manifest-state", "canonical manifest already contains a rust-analyzer record")
    baseline_sha256 = digest(baseline_raw)
    records = [build_manifest_record(source_lock, platform, baseline_sha256) for platform in platforms]
    packs = list(manifest.get("packs", [])) + records
    packs.sort(key=lambda pack: (pack["artifact_id"], pack["platform_id"], pack["pack_version"]))
    manifest_candidate = {**manifest, "packs": packs}
    summaries = []
    for platform in platforms:
        subjects = platform["subjects"]
        summaries.append(
            {
                "platform_id": platform["platform_id"],
                "pack_asset_name": subjects[0]["name"],
                "pack_sha256": subjects[0]["sha256"],
                "pack_manifest_asset_name": subjects[1]["name"],
                "pack_manifest_sha256": subjects[1]["sha256"],
                "sbom_asset_name": subjects[2]["name"],
                "sbom_sha256": subjects[2]["sha256"],
                "executable_sha256": platform["executable"]["sha256"],
                "source_lock_sha256": SOURCE_LOCK_SHA256,
                "quality_baseline_sha256": baseline_sha256,
            }
        )
    return {
        "schema_version": 1,
        "kind": "provider_manifest_update_candidate",
        "synthetic_fixture_only": True,
        "source_publication_sha256": digest(publication_raw),
        "quality_baseline_sha256": baseline_sha256,
        "base_manifest_sha256": digest(manifest_raw),
        "platforms": summaries,
        "manifest_candidate": manifest_candidate,
    }


def parse_args():
    parser = argparse.ArgumentParser(
        description="Generate a review-only rust-analyzer manifest update candidate"
    )
    parser.add_argument("--fixture", required=True, type=Path)
    parser.add_argument("--baseline", type=Path)
    return parser.parse_args()


def main():
    if core_release_context():
        fail("core-release-boundary", "core release jobs cannot generate manifest updates")
    args = parse_args()
    fixture_root = args.fixture.resolve()
    if not fixture_root.is_dir():
        fail("publication-contract", "provider publication fixture root is not a directory")
    repo_root = Path(__file__).resolve().parent.parent
    publication, publication_raw = read_canonical(
        fixture_root / "verified-publication.json"
    )
    baseline_path = args.baseline.resolve() if args.baseline else fixture_root / "reviewed-baseline.json"
    baseline, baseline_raw = read_canonical(baseline_path)
    source_lock, assets = load_source_lock(repo_root)
    platforms = validate_publication(publication, assets)
    validate_baseline(baseline, publication, platforms)
    candidate = build_candidate(
        repo_root,
        publication_raw,
        baseline_raw,
        platforms,
        source_lock,
    )
    sys.stdout.buffer.write(canonical_output_bytes(candidate))


if __name__ == "__main__":
    try:
        main()
    except GenerationError as exc:
        print(f"provider manifest update failed: {exc.code}: {exc}", file=sys.stderr)
        sys.exit(1)
    except (KeyError, TypeError, ValueError) as exc:
        print(f"provider manifest update failed: publication-contract: {exc}", file=sys.stderr)
        sys.exit(1)
