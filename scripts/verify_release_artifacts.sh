#!/usr/bin/env bash
set -euo pipefail

usage() {
  printf 'usage: %s --fixture /absolute/or/relative/release-fixture\n' "$0" >&2
}

fixture=''
while (($#)); do
  case "$1" in
    --fixture)
      (($# >= 2)) || { usage; exit 2; }
      fixture=$2
      shift 2
      ;;
    --help|-h)
      usage
      exit 0
      ;;
    *)
      usage
      exit 2
      ;;
  esac
done

[[ -n "$fixture" ]] || { usage; exit 2; }

python3 - "$fixture" <<'PY'
import hashlib
import json
import re
import sys
import tarfile
from pathlib import Path


MAX_JSON_BYTES = 1024 * 1024
MAX_ATTESTATION_BYTES = 1024 * 1024
MAX_SIDECAR_BYTES = 4096
MAX_ARCHIVE_BYTES = 512 * 1024 * 1024
MAX_ARCHIVE_MEMBERS = 4096
MAX_EXPANDED_BYTES = 2 * 1024 * 1024 * 1024
MAX_REVOCATION_BYTES = 8 * 1024 * 1024
MAX_REVOCATION_ENTRIES = 16_384
SHA256 = re.compile(r"^[0-9a-f]{64}$")
COMMIT = re.compile(r"^[0-9a-f]{40}$")
RELEASE_TAG = re.compile(r"^v[0-9][A-Za-z0-9._-]*$")
REPOSITORY = "junit/pre-commit-review"
RELEASE_WORKFLOW = ".github/workflows/release.yml"
PACK_WORKFLOW = ".github/workflows/artifact-pack-release.yml"
OIDC_ISSUER = "https://token.actions.githubusercontent.com"
PREDICATE_TYPE = "pre-commit-review.artifact-pack/v1"


class VerificationError(Exception):
    def __init__(self, code, message):
        super().__init__(message)
        self.code = code


def fail(code, message):
    raise VerificationError(code, message)


def read_json(path, limit, code):
    try:
        data = path.read_bytes()
    except OSError as exc:
        fail(code, f"could not read {path.name}: {exc}")
    if len(data) > limit:
        fail(code, f"{path.name} exceeds its byte limit")
    try:
        value = json.loads(data.decode("utf-8"))
    except (UnicodeDecodeError, json.JSONDecodeError) as exc:
        fail(code, f"{path.name} is not valid UTF-8 JSON: {exc}")
    if not isinstance(value, dict):
        fail(code, f"{path.name} must contain an object")
    return value


def digest_file(path):
    try:
        size = path.stat().st_size
    except OSError as exc:
        fail("artifact-open", f"could not stat {path.name}: {exc}")
    if size == 0 or size > MAX_ARCHIVE_BYTES:
        fail("artifact-size", f"{path.name} is outside the archive size policy")
    digest = hashlib.sha256()
    try:
        with path.open("rb") as stream:
            while True:
                chunk = stream.read(1024 * 1024)
                if not chunk:
                    break
                digest.update(chunk)
    except OSError as exc:
        fail("artifact-open", f"could not read {path.name}: {exc}")
    return size, digest.hexdigest()


def require_sha256(value, field):
    if not isinstance(value, str) or not SHA256.fullmatch(value):
        fail("digest-format", f"{field} is not a lower-case SHA256 digest")
    return value


def safe_file_name(value, field):
    if not isinstance(value, str) or not value or len(value) > 255:
        fail("release-identity", f"{field} is not a bounded file name")
    if Path(value).name != value or value in {".", ".."} or "\\" in value:
        fail("release-identity", f"{field} is not a plain file name")


def parse_sidecar(path, archive_name):
    try:
        data = path.read_bytes()
    except OSError as exc:
        fail("sidecar-open", f"could not read {path.name}: {exc}")
    if len(data) > MAX_SIDECAR_BYTES:
        fail("sidecar-size", f"{path.name} exceeds its byte limit")
    try:
        lines = data.decode("ascii").splitlines()
    except UnicodeDecodeError:
        fail("sidecar-format", f"{path.name} is not ASCII")
    if len(lines) != 1:
        fail("sidecar-format", f"{path.name} must contain exactly one checksum line")
    fields = lines[0].split()
    if len(fields) not in {1, 2}:
        fail("sidecar-format", f"{path.name} has an invalid checksum line")
    digest = require_sha256(fields[0], "sidecar digest")
    if len(fields) == 2 and Path(fields[1]).name != archive_name:
        fail("sidecar-subject", f"{path.name} names a different archive")
    return digest


def attestation_signer(attestation):
    signer = attestation.get("signer")
    if isinstance(signer, dict):
        return signer
    predicate = attestation.get("predicate")
    if isinstance(predicate, dict):
        build_definition = predicate.get("buildDefinition")
        if isinstance(build_definition, dict):
            external = build_definition.get("externalParameters")
            if isinstance(external, dict) and isinstance(external.get("signer"), dict):
                return external["signer"]
    fail("attestation-signer", "attestation has no scoped signer identity")


def verify_attestation(path, artifact, release, archive_digest):
    attestation = read_json(path, MAX_ATTESTATION_BYTES, "attestation-json")
    subject = attestation.get("subject")
    if not isinstance(subject, list) or len(subject) != 1 or not isinstance(subject[0], dict):
        fail("attestation-subject", f"{path.name} must contain one subject")
    subject_item = subject[0]
    if subject_item.get("name") != artifact["name"]:
        fail("attestation-subject", f"{path.name} subject name is not the archive")
    subject_digest = subject_item.get("digest")
    if not isinstance(subject_digest, dict) or subject_digest.get("sha256") != archive_digest:
        fail("attestation-subject", f"{path.name} subject digest does not match the archive")
    if attestation.get("predicateType") != PREDICATE_TYPE:
        fail("attestation-predicate", f"{path.name} has an unexpected predicate type")

    signer = attestation_signer(attestation)
    expected_workflow = PACK_WORKFLOW if artifact["kind"] != "core" else RELEASE_WORKFLOW
    expected = {
        "repository": REPOSITORY,
        "workflow": expected_workflow,
        "ref": release["ref"],
        "commit": release["commit"],
        "issuer": OIDC_ISSUER,
    }
    for field, value in expected.items():
        if signer.get(field) != value:
            fail("attestation-signer", f"{path.name} has an unscoped {field}")

    predicate = attestation.get("predicate")
    if not isinstance(predicate, dict):
        fail("attestation-predicate", f"{path.name} has no composition predicate")
    composition = predicate.get("composition")
    if not isinstance(composition, dict):
        fail("attestation-composition", f"{path.name} has no composition inputs")
    required = {"manifest_sha256", "sbom_sha256", "generator_sha256"}
    if artifact["kind"] != "core":
        required |= {"source_lock_sha256", "upstream_archive_sha256"}
    if set(composition) != required:
        fail("attestation-composition", f"{path.name} composition inputs are incomplete")
    for field in required:
        require_sha256(composition[field], f"composition {field}")
    expected_composition = artifact.get("composition")
    if not isinstance(expected_composition, dict) or composition != expected_composition:
        fail("attestation-composition", f"{path.name} composition is not release-bound")


def safe_member_name(name):
    path = Path(name)
    return bool(name) and not path.is_absolute() and "\\" not in name and all(
        part not in {"", ".", ".."} for part in path.parts
    )


def verify_archive(path, artifact):
    internal = artifact.get("internal")
    if not isinstance(internal, dict):
        fail("archive-contract", f"{path.name} has no internal manifest contract")
    required_internal = {"manifest_path", "manifest_sha256", "sbom_path", "sbom_sha256"}
    if set(internal) != required_internal:
        fail("archive-contract", f"{path.name} internal contract is incomplete")
    for field in ("manifest_sha256", "sbom_sha256"):
        require_sha256(internal[field], f"internal {field}")
    try:
        with tarfile.open(path, "r:gz") as archive:
            members = archive.getmembers()
            if not members or len(members) > MAX_ARCHIVE_MEMBERS:
                fail("archive-contract", f"{path.name} has an invalid member count")
            total = 0
            regular = {}
            for member in members:
                if not safe_member_name(member.name):
                    fail("archive-contract", f"{path.name} contains an unsafe member path")
                if member.issym() or member.islnk() or not (member.isdir() or member.isfile()):
                    fail("archive-contract", f"{path.name} contains a link or special member")
                if member.isfile():
                    total += member.size
                    if total > MAX_EXPANDED_BYTES:
                        fail("archive-size", f"{path.name} exceeds its expanded size limit")
                    stream = archive.extractfile(member)
                    if stream is None:
                        fail("archive-contract", f"{path.name} has an unreadable member")
                    regular[member.name] = stream.read(MAX_EXPANDED_BYTES + 1)
                    if len(regular[member.name]) != member.size:
                        fail("archive-contract", f"{path.name} member size is inconsistent")
            for path_key, digest_key in (
                (internal["manifest_path"], "manifest_sha256"),
                (internal["sbom_path"], "sbom_sha256"),
            ):
                if path_key not in regular:
                    fail("archive-contract", f"{path.name} is missing {path_key}")
                if hashlib.sha256(regular[path_key]).hexdigest() != internal[digest_key]:
                    fail("archive-contract", f"{path.name} has a mismatched {path_key}")
    except (tarfile.TarError, OSError) as exc:
        fail("archive-contract", f"{path.name} is not a valid tar-gzip archive: {exc}")


def verify_revocation_index(root, release):
    index = release.get("revocation_index")
    if index is None:
        return
    if not isinstance(index, dict) or set(index) != {"path", "sha256"}:
        fail("revocation-contract", "release revocation index metadata is incomplete")
    safe_file_name(index["path"], "revocation index path")
    expected = require_sha256(index["sha256"], "revocation index digest")
    path = root / index["path"]
    try:
        raw = path.read_bytes()
    except OSError as exc:
        fail("revocation-contract", f"could not read revocation index: {exc}")
    if len(raw) > MAX_REVOCATION_BYTES:
        fail("revocation-size-limit", "revocation index exceeds 8 MiB")
    if hashlib.sha256(raw).hexdigest() != expected:
        fail("revocation-digest", "revocation index digest does not match release metadata")
    try:
        value = json.loads(raw.decode("utf-8"))
    except (UnicodeDecodeError, json.JSONDecodeError) as exc:
        fail("revocation-contract", f"revocation index is not canonical JSON: {exc}")
    entries = value.get("entries") if isinstance(value, dict) else None
    if not isinstance(value, dict) or set(value) != {"schema_version", "kind", "entries"} or value.get("schema_version") != 1 or value.get("kind") != "third_party_artifact_revocations" or not isinstance(entries, list):
        fail("revocation-contract", "revocation index identity is invalid")
    if json.dumps(value, separators=(",", ":")).encode("utf-8") != raw:
        fail("revocation-contract", "revocation index is not compact canonical JSON")
    if len(entries) > MAX_REVOCATION_ENTRIES:
        fail("revocation-entry-limit", "revocation index contains too many entries")
    digests = []
    for entry in entries:
        if not isinstance(entry, dict):
            fail("revocation-contract", "revocation entries must be objects")
        if set(entry) != {"pack_sha256", "artifact_id", "platform_id", "pack_version", "reason", "replacement_pack_version"}:
            fail("revocation-contract", "revocation entry fields are not strict")
        digests.append(require_sha256(entry.get("pack_sha256"), "revocation pack digest"))
    if digests != sorted(set(digests)):
        fail("revocations-not-sorted", "revocation entries must be sorted and unique")


def verify(root):
    metadata = read_json(root / "release.json", MAX_JSON_BYTES, "release-metadata")
    required = {"schema_version", "kind", "repository", "workflow", "ref", "tag", "commit", "issuer", "immutable", "artifacts"}
    if set(metadata) - required - {"revocation_index"}:
        fail("release-metadata", "release metadata contains unknown fields")
    if metadata.get("schema_version") != 1 or metadata.get("kind") != "pre_commit_review_release":
        fail("release-metadata", "release metadata identity is invalid")
    if metadata.get("repository") != REPOSITORY or metadata.get("workflow") != RELEASE_WORKFLOW:
        fail("release-signer", "release metadata is bound to another project or workflow")
    if metadata.get("issuer") != OIDC_ISSUER:
        fail("release-signer", "release metadata has an unexpected OIDC issuer")
    if metadata.get("immutable") is not True:
        fail("immutable-release-unavailable", "release immutability is not enabled")
    tag = metadata.get("tag")
    commit = metadata.get("commit")
    ref = metadata.get("ref")
    if not isinstance(tag, str) or not RELEASE_TAG.fullmatch(tag) or tag.lower() in {"latest", "nightly"}:
        fail("release-signer", "release tag is not an immutable version tag")
    if ref != f"refs/tags/{tag}":
        fail("release-signer", "release ref is not the immutable version tag")
    if not isinstance(commit, str) or not COMMIT.fullmatch(commit):
        fail("release-signer", "release commit is not an immutable commit")
    artifacts = metadata.get("artifacts")
    if not isinstance(artifacts, list) or not artifacts or len(artifacts) > 256:
        fail("release-metadata", "release artifact inventory is outside its bounds")
    names = []
    for artifact in artifacts:
        if not isinstance(artifact, dict):
            fail("release-metadata", "release artifact entries must be objects")
        if set(artifact) - {"name", "kind", "platform_id", "sidecar", "attestation", "internal", "composition"}:
            fail("release-metadata", "release artifact contains unknown fields")
        for field in ("name", "sidecar", "attestation"):
            safe_file_name(artifact.get(field), field)
        if not artifact["name"].endswith(".tar.gz"):
            fail("release-identity", "release artifact is not a tar-gzip archive")
        if artifact["sidecar"] != artifact["name"] + ".sha256":
            fail("release-identity", "release artifact sidecar is not name-bound")
        if artifact["attestation"] != artifact["name"] + ".attestation.json":
            fail("release-identity", "release artifact attestation is not name-bound")
        if artifact["name"] in names:
            fail("release-metadata", "release artifact names must be unique")
        names.append(artifact["name"])
        if artifact.get("kind") not in {"core", "gitleaks", "rust-analyzer"}:
            fail("release-metadata", "release artifact kind is not allowlisted")
        if artifact.get("platform_id") not in {"darwin-amd64", "darwin-arm64", "linux-amd64", "windows-amd64"}:
            fail("release-metadata", "artifact platform is not allowlisted")

        archive = root / artifact["name"]
        sidecar = root / artifact["sidecar"]
        attestation = root / artifact["attestation"]
        size, actual_digest = digest_file(archive)
        sidecar_digest = parse_sidecar(sidecar, archive.name)
        if sidecar_digest != actual_digest:
            fail("sidecar-digest", f"{archive.name} does not match its external sidecar")
        verify_attestation(attestation, artifact, metadata, actual_digest)
        verify_archive(archive, artifact)
        if size <= 0:
            fail("artifact-size", f"{archive.name} is empty")
    verify_revocation_index(root, metadata)
    print(json.dumps({"status": "verified", "artifacts": len(artifacts)}, separators=(",", ":")))


try:
    fixture_root = Path(sys.argv[1]).resolve()
    if not fixture_root.is_dir():
        fail("fixture-root", "release fixture is not a directory")
    verify(fixture_root)
except VerificationError as exc:
    print(f"release verification failed: {exc.code}: {exc}", file=sys.stderr)
    sys.exit(1)
except (KeyError, TypeError, ValueError) as exc:
    print(f"release verification failed: release-metadata: {exc}", file=sys.stderr)
    sys.exit(1)
PY
