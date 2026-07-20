#!/usr/bin/env python3
"""Run one explicitly authorized static analyzer in a bounded candidate snapshot."""

from __future__ import annotations

import argparse
import hashlib
import json
import os
import pathlib
import re
import shutil
import signal
import stat
import subprocess
import sys
import tempfile
import threading
import time
from dataclasses import dataclass
from typing import Any, BinaryIO


FINGERPRINT_RE = re.compile(r"^[0-9a-f]{40}(?:[0-9a-f]{24})?$")
SHA256_RE = re.compile(r"^[0-9a-f]{64}$")
MAX_PROFILE_BYTES = 1_000_000


class RunnerError(Exception):
    """Expected controlled-execution failure that invalidates authoritative output."""


@dataclass(frozen=True)
class SnapshotInfo:
    sha256: str
    files: int
    bytes: int


@dataclass(frozen=True)
class ProcessResult:
    status: str
    exit_code: int | None
    duration_ms: int
    stdout_path: pathlib.Path
    stdout_bytes: int
    stdout_sha256: str
    stderr_bytes: int
    stderr_sha256: str
    failure_reason: str | None


@dataclass
class StreamCapture:
    path: pathlib.Path
    limit: int
    written: int = 0
    error: OSError | None = None

    def consume(self, stream: BinaryIO, overflow: threading.Event) -> None:
        try:
            with self.path.open("wb") as destination:
                while True:
                    chunk = stream.read(64 * 1024)
                    if not chunk:
                        break
                    remaining = self.limit + 1 - self.written
                    if remaining > 0:
                        saved = chunk[:remaining]
                        destination.write(saved)
                        self.written += len(saved)
                    if len(chunk) > remaining or self.written > self.limit:
                        overflow.set()
        except OSError as exc:
            self.error = exc
            overflow.set()
        finally:
            try:
                stream.close()
            except OSError:
                pass


def sha256_file(path: pathlib.Path) -> tuple[str, int]:
    digest = hashlib.sha256()
    total = 0
    try:
        with path.open("rb") as stream:
            while True:
                chunk = stream.read(1024 * 1024)
                if not chunk:
                    break
                total += len(chunk)
                digest.update(chunk)
    except OSError as exc:
        raise RunnerError(f"cannot hash {path.name}: {exc}") from exc
    return digest.hexdigest(), total


def compact_hash(*values: object) -> str:
    digest = hashlib.sha256()
    for value in values:
        digest.update(str(value).encode("utf-8", errors="replace"))
        digest.update(b"\0")
    return digest.hexdigest()[:16]


def require_exact_keys(value: dict[str, Any], expected: set[str], label: str) -> None:
    missing = sorted(expected - set(value))
    extra = sorted(set(value) - expected)
    if missing:
        raise RunnerError(f"{label} is missing required fields: {', '.join(missing)}")
    if extra:
        raise RunnerError(f"{label} has unsupported fields: {', '.join(extra)}")


def require_string(value: object, label: str, maximum: int) -> str:
    if not isinstance(value, str) or not value or "\x00" in value or len(value) > maximum:
        raise RunnerError(f"{label} must be a non-empty string of at most {maximum} characters")
    return value


def require_integer(value: object, label: str, minimum: int, maximum: int) -> int:
    if type(value) is not int or value < minimum or value > maximum:
        raise RunnerError(f"{label} must be an integer between {minimum} and {maximum}")
    return value


def load_profile(path: pathlib.Path, expected_hash: str) -> tuple[dict[str, Any], str]:
    if not path.is_absolute():
        raise RunnerError("--profile must be an absolute path")
    try:
        profile_stat = path.stat()
    except OSError as exc:
        raise RunnerError(f"cannot read static-analysis profile: {exc}") from exc
    if not stat.S_ISREG(profile_stat.st_mode):
        raise RunnerError("static-analysis profile must be a regular file")
    if profile_stat.st_size > MAX_PROFILE_BYTES:
        raise RunnerError(f"static-analysis profile exceeds {MAX_PROFILE_BYTES} bytes")
    try:
        with path.open("rb") as stream:
            raw_profile = stream.read(MAX_PROFILE_BYTES + 1)
    except OSError as exc:
        raise RunnerError(f"cannot read static-analysis profile: {exc}") from exc
    if len(raw_profile) > MAX_PROFILE_BYTES:
        raise RunnerError(f"static-analysis profile exceeds {MAX_PROFILE_BYTES} bytes")
    observed_hash = hashlib.sha256(raw_profile).hexdigest()
    if observed_hash != expected_hash:
        raise RunnerError("profile SHA256 does not match --expect-profile-sha256")
    try:
        payload = json.loads(raw_profile.decode("utf-8"))
    except (UnicodeDecodeError, json.JSONDecodeError) as exc:
        raise RunnerError(f"static-analysis profile is not valid UTF-8 JSON: {exc}") from exc
    if not isinstance(payload, dict):
        raise RunnerError("static-analysis profile must be a JSON object")
    required = {
        "schema_version",
        "kind",
        "name",
        "tool",
        "executable",
        "arguments",
        "output_format",
        "success_exit_codes",
        "limits",
        "repository_configuration",
        "network_access",
    }
    require_exact_keys(payload, required, "static-analysis profile")
    if type(payload["schema_version"]) is not int or payload["schema_version"] != 1:
        raise RunnerError("static-analysis profile schema_version must be 1")
    if payload["kind"] != "static_analysis_profile":
        raise RunnerError("static-analysis profile kind must be static_analysis_profile")
    require_string(payload["name"], "profile name", 200)

    tool = payload["tool"]
    if not isinstance(tool, dict):
        raise RunnerError("profile tool must be an object")
    require_exact_keys(tool, {"name", "version"}, "profile tool")
    require_string(tool["name"], "profile tool.name", 200)
    require_string(tool["version"], "profile tool.version", 100)

    executable = payload["executable"]
    if not isinstance(executable, dict):
        raise RunnerError("profile executable must be an object")
    require_exact_keys(executable, {"path", "sha256"}, "profile executable")
    require_string(executable["path"], "profile executable.path", 4096)
    if not isinstance(executable["sha256"], str) or not SHA256_RE.fullmatch(executable["sha256"]):
        raise RunnerError("profile executable.sha256 must be 64 lowercase hexadecimal characters")

    arguments = payload["arguments"]
    if not isinstance(arguments, list) or len(arguments) > 128:
        raise RunnerError("profile arguments must be an array of at most 128 strings")
    for index, argument in enumerate(arguments):
        if not isinstance(argument, str) or "\x00" in argument or len(argument) > 4096:
            raise RunnerError(f"profile arguments[{index}] must be a string of at most 4096 characters")

    if payload["output_format"] not in {"sarif", "normalized-json"}:
        raise RunnerError("profile output_format must be sarif or normalized-json")
    exit_codes = payload["success_exit_codes"]
    if (
        not isinstance(exit_codes, list)
        or not exit_codes
        or len(exit_codes) > 16
        or len(set(exit_codes)) != len(exit_codes)
    ):
        raise RunnerError("profile success_exit_codes must contain 1 to 16 unique exit codes")
    for index, code in enumerate(exit_codes):
        require_integer(code, f"profile success_exit_codes[{index}]", 0, 255)

    limits = payload["limits"]
    if not isinstance(limits, dict):
        raise RunnerError("profile limits must be an object")
    require_exact_keys(
        limits,
        {"timeout_seconds", "max_output_bytes", "max_snapshot_bytes", "max_snapshot_files"},
        "profile limits",
    )
    require_integer(limits["timeout_seconds"], "profile limits.timeout_seconds", 1, 600)
    require_integer(limits["max_output_bytes"], "profile limits.max_output_bytes", 1024, 10_000_000)
    require_integer(
        limits["max_snapshot_bytes"],
        "profile limits.max_snapshot_bytes",
        1_048_576,
        2_147_483_648,
    )
    require_integer(limits["max_snapshot_files"], "profile limits.max_snapshot_files", 1, 200_000)
    if payload["repository_configuration"] not in {"disabled", "explicitly-trusted"}:
        raise RunnerError(
            "profile repository_configuration must be disabled or explicitly-trusted"
        )
    if payload["network_access"] != "offline-required":
        raise RunnerError("profile network_access must be offline-required")
    return payload, observed_hash


def path_is_within(path: pathlib.Path, parent: pathlib.Path) -> bool:
    try:
        path.relative_to(parent)
        return True
    except ValueError:
        return False


def resolve_executable(profile: dict[str, Any], repo_root: pathlib.Path) -> tuple[pathlib.Path, str]:
    configured = pathlib.Path(profile["executable"]["path"])
    if not configured.is_absolute():
        raise RunnerError("profile executable.path must be absolute")
    try:
        resolved = configured.resolve(strict=True)
        executable_stat = resolved.stat()
    except OSError as exc:
        raise RunnerError(f"cannot resolve profile executable: {exc}") from exc
    if path_is_within(resolved, repo_root):
        raise RunnerError("executable must be outside the reviewed repository")
    if not stat.S_ISREG(executable_stat.st_mode) or not os.access(resolved, os.X_OK):
        raise RunnerError("profile executable must be an executable regular file")
    observed_hash, _ = sha256_file(resolved)
    if observed_hash != profile["executable"]["sha256"]:
        raise RunnerError("executable SHA256 does not match the profile")
    repo_text = str(repo_root)
    for argument in profile["arguments"]:
        if repo_text in argument:
            raise RunnerError("profile arguments must not expose the reviewed repository path")
        candidate = pathlib.Path(argument)
        if candidate.is_absolute():
            try:
                if path_is_within(candidate.resolve(strict=False), repo_root):
                    raise RunnerError(
                        "profile arguments must not reference paths inside the reviewed repository"
                    )
            except OSError as exc:
                raise RunnerError(f"cannot validate profile argument path: {exc}") from exc
    return resolved, observed_hash


def git_environment() -> dict[str, str]:
    environment = os.environ.copy()
    environment["GIT_OPTIONAL_LOCKS"] = "0"
    environment["GIT_NO_LAZY_FETCH"] = "1"
    environment["GIT_CONFIG_NOSYSTEM"] = "1"
    if os.name != "nt":
        environment["GIT_CONFIG_GLOBAL"] = "/dev/null"
    return environment


def run_git(repo_root: pathlib.Path, arguments: list[str]) -> bytes:
    completed = subprocess.run(
        ["git", *arguments],
        cwd=repo_root,
        env=git_environment(),
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        check=False,
    )
    if completed.returncode != 0:
        detail = completed.stderr.decode("utf-8", errors="replace").strip()[:500]
        raise RunnerError(f"Git snapshot command failed: {detail or 'unknown Git error'}")
    return completed.stdout


def update_digest_from_git(
    repo_root: pathlib.Path, arguments: list[str], digest: Any
) -> None:
    with tempfile.TemporaryFile() as stderr_stream:
        process = subprocess.Popen(
            ["git", *arguments],
            cwd=repo_root,
            env=git_environment(),
            stdin=subprocess.DEVNULL,
            stdout=subprocess.PIPE,
            stderr=stderr_stream,
        )
        if process.stdout is None:
            process.kill()
            process.wait()
            raise RunnerError("cannot hash Git repository state")
        while True:
            chunk = process.stdout.read(1024 * 1024)
            if not chunk:
                break
            digest.update(chunk)
        return_code = process.wait()
        if return_code != 0:
            stderr_stream.seek(0)
            detail = stderr_stream.read(500).decode("utf-8", errors="replace").strip()
            raise RunnerError(
                f"Git repository-state command failed: {detail or 'unknown Git error'}"
            )


def extract_section_json(output: str, marker: str) -> dict[str, Any]:
    lines = output.splitlines()
    try:
        marker_index = lines.index(marker)
    except ValueError as exc:
        raise RunnerError(f"output is missing {marker}") from exc
    values = [line for line in lines[marker_index + 1 :] if line.strip()]
    if len(values) != 1:
        raise RunnerError(f"{marker} must contain exactly one JSON object")
    try:
        payload = json.loads(values[0])
    except json.JSONDecodeError as exc:
        raise RunnerError(f"{marker} contains invalid JSON") from exc
    if not isinstance(payload, dict):
        raise RunnerError(f"{marker} must contain a JSON object")
    return payload


def run_control_plane(
    helper: pathlib.Path, repo_root: pathlib.Path, source: str, expected_scope: str
) -> dict[str, Any]:
    completed = subprocess.run(
        [str(helper), "--source", source, "--control-plane", "--expect-scope", expected_scope],
        cwd=repo_root,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        text=True,
        encoding="utf-8",
        errors="replace",
        check=False,
    )
    if completed.returncode != 0:
        detail = " ".join(completed.stderr.split())[:500]
        raise RunnerError(f"control-plane helper failed: {detail or 'scope mismatch'}")
    control = extract_section_json(completed.stdout, "## Review Control Plane JSON")
    if control.get("authoritative") is not True:
        raise RunnerError("control-plane scope is not authoritative")
    if control.get("scope_fingerprint") != expected_scope or control.get("source") != source:
        raise RunnerError("control-plane source or fingerprint does not match the requested scope")
    return control


def safe_relative_path(raw_path: bytes) -> pathlib.PurePath:
    decoded = os.fsdecode(raw_path)
    candidate = pathlib.PurePath(decoded)
    if candidate.is_absolute() or not candidate.parts or ".." in candidate.parts:
        raise RunnerError("Git contains a path that escapes the temporary snapshot")
    return candidate


def parse_index_entries(raw: bytes) -> list[tuple[bytes, str, str]]:
    entries: list[tuple[bytes, str, str]] = []
    for record in raw.split(b"\0"):
        if not record:
            continue
        try:
            metadata, path = record.split(b"\t", 1)
            mode_raw, object_raw, stage_raw = metadata.split(b" ", 2)
            mode = mode_raw.decode("ascii")
            object_id = object_raw.decode("ascii")
            stage = stage_raw.decode("ascii")
        except (ValueError, UnicodeDecodeError) as exc:
            raise RunnerError("cannot parse staged Git index entry") from exc
        if stage != "0":
            raise RunnerError("cannot analyze an index with unmerged entries")
        entries.append((path, mode, object_id))
    return entries


def parse_tree_entries(raw: bytes) -> list[tuple[bytes, str, str]]:
    entries: list[tuple[bytes, str, str]] = []
    for record in raw.split(b"\0"):
        if not record:
            continue
        try:
            metadata, path = record.split(b"\t", 1)
            mode_raw, object_type_raw, object_raw = metadata.split(b" ", 2)
            mode = mode_raw.decode("ascii")
            object_type = object_type_raw.decode("ascii")
            object_id = object_raw.decode("ascii")
        except (ValueError, UnicodeDecodeError) as exc:
            raise RunnerError("cannot parse branch Git tree entry") from exc
        if object_type == "blob":
            entries.append((path, mode, object_id))
    return entries


def read_batch_blob(
    stream: BinaryIO, expected_object: str, remaining_snapshot_bytes: int
) -> bytes:
    header = stream.readline()
    if not header:
        raise RunnerError("git cat-file ended before returning a requested blob")
    parts = header.rstrip(b"\n").split(b" ")
    if len(parts) == 2 and parts[1] == b"missing":
        raise RunnerError("a Git blob needed for the analysis snapshot is missing locally")
    if len(parts) != 3:
        raise RunnerError("git cat-file returned an invalid batch header")
    object_id = parts[0].decode("ascii", errors="replace")
    object_type = parts[1].decode("ascii", errors="replace")
    try:
        size = int(parts[2])
    except ValueError as exc:
        raise RunnerError("git cat-file returned an invalid blob size") from exc
    if object_id != expected_object or object_type != "blob" or size < 0:
        raise RunnerError("git cat-file returned a different object than requested")
    if size > remaining_snapshot_bytes:
        raise RunnerError("Git blob exceeds the remaining snapshot byte limit")
    content = stream.read(size)
    terminator = stream.read(1)
    if len(content) != size or terminator != b"\n":
        raise RunnerError("git cat-file returned a truncated blob")
    return content


def materialize_blobs(
    repo_root: pathlib.Path,
    snapshot_root: pathlib.Path,
    entries: list[tuple[bytes, str, str]],
    max_files: int,
    max_bytes: int,
) -> None:
    if len(entries) > max_files:
        raise RunnerError(f"analysis snapshot exceeds the {max_files}-file profile limit")
    process = subprocess.Popen(
        ["git", "cat-file", "--batch"],
        cwd=repo_root,
        env=git_environment(),
        stdin=subprocess.PIPE,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
    )
    if process.stdin is None or process.stdout is None:
        process.kill()
        raise RunnerError("cannot open git cat-file batch streams")
    total_bytes = 0
    try:
        for raw_path, mode, object_id in entries:
            relative = safe_relative_path(raw_path)
            destination = snapshot_root.joinpath(*relative.parts)
            if mode == "160000":
                continue
            destination.parent.mkdir(parents=True, exist_ok=True)
            process.stdin.write(object_id.encode("ascii") + b"\n")
            process.stdin.flush()
            content = read_batch_blob(process.stdout, object_id, max_bytes - total_bytes)
            total_bytes += len(content)
            if total_bytes > max_bytes:
                raise RunnerError(
                    f"analysis snapshot exceeds the {max_bytes}-byte profile limit"
                )
            if mode == "120000":
                target = os.fsdecode(content)
                os.symlink(target, destination)
            elif mode in {"100644", "100755"}:
                destination.write_bytes(content)
                destination.chmod(0o755 if mode == "100755" else 0o644)
            else:
                raise RunnerError(f"unsupported tracked file mode in snapshot: {mode}")
        process.stdin.close()
        return_code = process.wait(timeout=10)
        if return_code != 0:
            detail = (process.stderr.read() if process.stderr else b"").decode(
                "utf-8", errors="replace"
            )[:500]
            raise RunnerError(f"git cat-file failed while building snapshot: {detail}")
    except Exception:
        if process.poll() is None:
            process.kill()
            process.wait()
        raise


def materialize_unstaged(
    repo_root: pathlib.Path,
    snapshot_root: pathlib.Path,
    raw_paths: bytes,
    max_files: int,
    max_bytes: int,
) -> None:
    paths = [path for path in raw_paths.split(b"\0") if path]
    if len(paths) > max_files:
        raise RunnerError(f"analysis snapshot exceeds the {max_files}-file profile limit")
    total_bytes = 0
    for raw_path in paths:
        relative = safe_relative_path(raw_path)
        source = repo_root.joinpath(*relative.parts)
        destination = snapshot_root.joinpath(*relative.parts)
        try:
            source_stat = source.lstat()
        except FileNotFoundError:
            continue
        except OSError as exc:
            raise RunnerError(f"cannot inspect tracked working-tree path: {exc}") from exc
        if stat.S_ISDIR(source_stat.st_mode):
            continue
        destination.parent.mkdir(parents=True, exist_ok=True)
        if stat.S_ISLNK(source_stat.st_mode):
            target = os.readlink(source)
            total_bytes += len(os.fsencode(target))
            os.symlink(target, destination)
        elif stat.S_ISREG(source_stat.st_mode):
            total_bytes += source_stat.st_size
            if total_bytes > max_bytes:
                raise RunnerError(
                    f"analysis snapshot exceeds the {max_bytes}-byte profile limit"
                )
            shutil.copyfile(source, destination, follow_symlinks=False)
            destination.chmod(stat.S_IMODE(source_stat.st_mode))
        else:
            raise RunnerError("tracked working-tree path is not a regular file or symlink")


def validate_symlink(path: pathlib.Path, snapshot_root: pathlib.Path) -> bytes:
    target = os.readlink(path)
    target_path = pathlib.Path(target)
    if target_path.is_absolute():
        raise RunnerError("analysis snapshot contains an absolute symlink")
    resolved = pathlib.Path(os.path.realpath(path.parent / target_path))
    if not path_is_within(resolved, snapshot_root.resolve()):
        raise RunnerError("analysis snapshot contains a symlink that escapes the snapshot")
    return os.fsencode(target)


def snapshot_info(
    snapshot_root: pathlib.Path, max_files: int, max_bytes: int
) -> SnapshotInfo:
    digest = hashlib.sha256()
    file_count = 0
    total_bytes = 0
    for current, directories, files in os.walk(snapshot_root, topdown=True, followlinks=False):
        directories.sort()
        files.sort()
        current_path = pathlib.Path(current)
        symlink_directories = [name for name in directories if (current_path / name).is_symlink()]
        directories[:] = [name for name in directories if name not in symlink_directories]
        for name in [*symlink_directories, *files]:
            path = current_path / name
            relative = path.relative_to(snapshot_root).as_posix()
            mode = path.lstat().st_mode
            file_count += 1
            if file_count > max_files:
                raise RunnerError(f"analysis snapshot exceeds the {max_files}-file profile limit")
            digest.update(relative.encode("utf-8", errors="surrogateescape"))
            digest.update(b"\0")
            digest.update(str(stat.S_IMODE(mode)).encode("ascii"))
            digest.update(b"\0")
            if stat.S_ISLNK(mode):
                content = validate_symlink(path, snapshot_root)
                total_bytes += len(content)
                digest.update(b"symlink\0")
                digest.update(content)
            elif stat.S_ISREG(mode):
                digest.update(b"file\0")
                try:
                    with path.open("rb") as stream:
                        while True:
                            chunk = stream.read(1024 * 1024)
                            if not chunk:
                                break
                            total_bytes += len(chunk)
                            if total_bytes > max_bytes:
                                raise RunnerError(
                                    f"analysis snapshot exceeds the {max_bytes}-byte profile limit"
                                )
                            digest.update(chunk)
                except OSError as exc:
                    raise RunnerError(f"cannot hash analysis snapshot file: {exc}") from exc
            else:
                raise RunnerError("analysis snapshot contains an unsupported file type")
            digest.update(b"\0")
    return SnapshotInfo(digest.hexdigest(), file_count, total_bytes)


def make_snapshot_read_only(snapshot_root: pathlib.Path) -> None:
    directories: list[pathlib.Path] = []
    for current, directory_names, file_names in os.walk(
        snapshot_root, topdown=True, followlinks=False
    ):
        current_path = pathlib.Path(current)
        directories.append(current_path)
        for name in file_names:
            path = current_path / name
            if not path.is_symlink():
                mode = stat.S_IMODE(path.stat().st_mode)
                path.chmod(mode & ~0o222)
        directory_names[:] = [
            name for name in directory_names if not (current_path / name).is_symlink()
        ]
    for directory in reversed(directories):
        directory.chmod(0o555)


def make_snapshot_writable(snapshot_root: pathlib.Path) -> None:
    if not snapshot_root.exists():
        return
    for current, directory_names, file_names in os.walk(
        snapshot_root, topdown=True, followlinks=False
    ):
        current_path = pathlib.Path(current)
        try:
            current_path.chmod(0o755)
        except OSError:
            pass
        for name in file_names:
            path = current_path / name
            if not path.is_symlink():
                try:
                    path.chmod(0o644)
                except OSError:
                    pass
        directory_names[:] = [
            name for name in directory_names if not (current_path / name).is_symlink()
        ]


def materialize_snapshot(
    repo_root: pathlib.Path,
    source: str,
    snapshot_root: pathlib.Path,
    limits: dict[str, int],
) -> SnapshotInfo:
    max_files = limits["max_snapshot_files"]
    max_bytes = limits["max_snapshot_bytes"]
    if source == "staged":
        entries = parse_index_entries(run_git(repo_root, ["ls-files", "--stage", "-z"]))
        materialize_blobs(repo_root, snapshot_root, entries, max_files, max_bytes)
    elif source == "branch":
        entries = parse_tree_entries(
            run_git(repo_root, ["ls-tree", "-rz", "--full-tree", "HEAD"])
        )
        materialize_blobs(repo_root, snapshot_root, entries, max_files, max_bytes)
    else:
        paths = run_git(repo_root, ["ls-files", "--cached", "-z"])
        materialize_unstaged(repo_root, snapshot_root, paths, max_files, max_bytes)
    info = snapshot_info(snapshot_root, max_files, max_bytes)
    make_snapshot_read_only(snapshot_root)
    return info


def child_environment(
    runtime_root: pathlib.Path, source: str, expected_scope: str
) -> dict[str, str]:
    runtime_home = runtime_root / "home"
    runtime_tmp = runtime_root / "tmp"
    runtime_home.mkdir(mode=0o700)
    runtime_tmp.mkdir(mode=0o700)
    environment = {
        "PATH": os.defpath,
        "LANG": "C.UTF-8",
        "LC_ALL": "C.UTF-8",
        "HOME": str(runtime_home),
        "TMPDIR": str(runtime_tmp),
        "TMP": str(runtime_tmp),
        "TEMP": str(runtime_tmp),
        "NO_COLOR": "1",
        "PRE_COMMIT_REVIEW_SCOPE_FINGERPRINT": expected_scope,
        "PRE_COMMIT_REVIEW_SOURCE": source,
        "HTTP_PROXY": "http://127.0.0.1:9",
        "HTTPS_PROXY": "http://127.0.0.1:9",
        "ALL_PROXY": "http://127.0.0.1:9",
        "NO_PROXY": "",
    }
    if os.name == "nt":
        for name in ("SystemRoot", "WINDIR"):
            if os.environ.get(name):
                environment[name] = os.environ[name]
    return environment


def terminate_process_group(process: subprocess.Popen[bytes]) -> None:
    if process.poll() is not None:
        if os.name != "nt":
            try:
                os.killpg(process.pid, signal.SIGKILL)
            except ProcessLookupError:
                pass
        return
    if os.name == "nt":
        process.kill()
    else:
        try:
            os.killpg(process.pid, signal.SIGKILL)
        except ProcessLookupError:
            process.kill()
    try:
        process.wait(timeout=5)
    except subprocess.TimeoutExpired:
        process.kill()
        process.wait()


def execute_analyzer(
    executable: pathlib.Path,
    arguments: list[str],
    snapshot_root: pathlib.Path,
    runtime_root: pathlib.Path,
    profile: dict[str, Any],
    source: str,
    expected_scope: str,
) -> ProcessResult:
    stdout_path = runtime_root / "analyzer.stdout"
    stderr_path = runtime_root / "analyzer.stderr"
    start = time.monotonic()
    creation_flags = 0
    start_new_session = os.name != "nt"
    if os.name == "nt":
        creation_flags = getattr(subprocess, "CREATE_NEW_PROCESS_GROUP", 0)
    try:
        process = subprocess.Popen(
            [str(executable), *arguments],
            cwd=snapshot_root,
            env=child_environment(runtime_root, source, expected_scope),
            stdin=subprocess.DEVNULL,
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
            shell=False,
            start_new_session=start_new_session,
            creationflags=creation_flags,
        )
    except OSError as exc:
        raise RunnerError(f"cannot start trusted analyzer: {exc}") from exc
    if process.stdout is None or process.stderr is None:
        terminate_process_group(process)
        raise RunnerError("cannot capture trusted analyzer output")
    output_limit = profile["limits"]["max_output_bytes"]
    overflow = threading.Event()
    stdout_capture = StreamCapture(stdout_path, output_limit)
    stderr_capture = StreamCapture(stderr_path, output_limit)
    capture_threads = [
        threading.Thread(
            target=stdout_capture.consume,
            args=(process.stdout, overflow),
            name="static-analysis-stdout",
            daemon=True,
        ),
        threading.Thread(
            target=stderr_capture.consume,
            args=(process.stderr, overflow),
            name="static-analysis-stderr",
            daemon=True,
        ),
    ]
    for capture_thread in capture_threads:
        capture_thread.start()
    forced_status: str | None = None
    timeout_seconds = profile["limits"]["timeout_seconds"]
    while process.poll() is None:
        elapsed = time.monotonic() - start
        if overflow.is_set():
            forced_status = "output-limit"
            terminate_process_group(process)
            break
        if elapsed >= timeout_seconds:
            forced_status = "timeout"
            terminate_process_group(process)
            break
        time.sleep(0.02)
    if process.poll() is None:
        process.wait()
    if forced_status is None and os.name != "nt":
        terminate_process_group(process)
    for capture_thread in capture_threads:
        capture_thread.join(timeout=5)
    if any(capture_thread.is_alive() for capture_thread in capture_threads):
        raise RunnerError("analyzer output capture did not terminate")
    capture_error = stdout_capture.error or stderr_capture.error
    if capture_error is not None:
        raise RunnerError(f"cannot capture trusted analyzer output: {capture_error}")
    if overflow.is_set() and forced_status is None:
        forced_status = "output-limit"
    duration_ms = max(0, int((time.monotonic() - start) * 1000))
    stdout_hash, stdout_bytes = sha256_file(stdout_path)
    stderr_hash, stderr_bytes = sha256_file(stderr_path)
    if forced_status == "timeout":
        status = "timeout"
        exit_code = None
        failure_reason = "timeout"
    elif forced_status == "output-limit":
        status = "output-limit"
        exit_code = None
        failure_reason = "output-limit"
    elif process.returncode not in profile["success_exit_codes"]:
        status = "failed"
        exit_code = process.returncode
        failure_reason = "non-success-exit"
    else:
        status = "completed"
        exit_code = process.returncode
        failure_reason = None
    return ProcessResult(
        status=status,
        exit_code=exit_code,
        duration_ms=duration_ms,
        stdout_path=stdout_path,
        stdout_bytes=stdout_bytes,
        stdout_sha256=stdout_hash,
        stderr_bytes=stderr_bytes,
        stderr_sha256=stderr_hash,
        failure_reason=failure_reason,
    )


def failure_report(
    path: pathlib.Path,
    expected_scope: str,
    tool: dict[str, str],
    status: str,
) -> None:
    normalized_status = "timeout" if status == "timeout" else "failed"
    payload = {
        "schema_version": 1,
        "kind": "static_analysis_input",
        "scope_fingerprint": expected_scope,
        "tool": {"name": tool["name"], "version": tool["version"]},
        "status": normalized_status,
        "findings": [],
    }
    path.write_text(json.dumps(payload, separators=(",", ":")), encoding="utf-8")


def run_evidence_collector(
    collector: pathlib.Path,
    helper: pathlib.Path,
    repo_root: pathlib.Path,
    source: str,
    expected_scope: str,
    result_path: pathlib.Path,
    result_format: str,
    execution_id: str,
    max_findings: int,
) -> tuple[dict[str, Any] | None, str]:
    command = [
        sys.executable,
        str(collector),
        "--source",
        source,
        "--expect-scope",
        expected_scope,
        "--result",
        str(result_path),
        "--helper",
        str(helper),
        "--max-findings",
        str(max_findings),
        "--trust",
        "controlled-execution",
        "--execution-id",
        execution_id,
    ]
    if result_format == "sarif":
        command.extend(["--result-scope", expected_scope])
    completed = subprocess.run(
        command,
        cwd=repo_root,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        text=True,
        encoding="utf-8",
        errors="replace",
        check=False,
    )
    if completed.returncode != 0:
        return None, "collector-rejected-result"
    try:
        return extract_section_json(completed.stdout, "## Static Analysis Evidence JSON"), ""
    except RunnerError:
        return None, "collector-returned-invalid-evidence"


def evidence_matches_profile(evidence: dict[str, Any], profile: dict[str, Any]) -> bool:
    reports = evidence.get("reports")
    if not isinstance(reports, list) or not reports:
        return False
    expected_tool = profile["tool"]
    for report in reports:
        if not isinstance(report, dict):
            return False
        if report.get("tool") != expected_tool or report.get("status") != "completed":
            return False
    return True


def repository_state_digest(repo_root: pathlib.Path) -> str:
    digest = hashlib.sha256()
    commands = [
        ["status", "--porcelain=v2", "-z", "--untracked-files=all"],
        ["diff", "--no-ext-diff", "--no-textconv", "--binary"],
        ["diff", "--cached", "--no-ext-diff", "--no-textconv", "--binary"],
    ]
    for command in commands:
        update_digest_from_git(repo_root, command, digest)
        digest.update(b"\0")
    return digest.hexdigest()


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser(
        description="Run one hash-pinned static analyzer in a bounded tracked-file snapshot."
    )
    parser.add_argument("--source", required=True, choices=("staged", "unstaged", "branch"))
    parser.add_argument("--expect-scope", required=True, help="opening authoritative scope fingerprint")
    parser.add_argument("--profile", required=True, help="absolute static_analysis_profile/v1 path")
    parser.add_argument(
        "--expect-profile-sha256",
        required=True,
        help="exact lowercase SHA256 of the authorized profile bytes",
    )
    parser.add_argument(
        "--allow-repository-configuration",
        action="store_true",
        help="separately authorize an explicitly-trusted repository configuration",
    )
    parser.add_argument("--max-findings", type=int, default=500)
    return parser.parse_args()


def main() -> int:
    args = parse_args()
    if not FINGERPRINT_RE.fullmatch(args.expect_scope):
        raise RunnerError("--expect-scope is missing or invalid")
    if not SHA256_RE.fullmatch(args.expect_profile_sha256):
        raise RunnerError("--expect-profile-sha256 must be 64 lowercase hexadecimal characters")
    if args.max_findings < 1 or args.max_findings > 5000:
        raise RunnerError("--max-findings must be between 1 and 5000")
    script_dir = pathlib.Path(__file__).resolve().parent
    helper = script_dir / "collect_diff_context.sh"
    collector = script_dir / "collect_static_evidence.py"
    if not helper.is_file() or not collector.is_file():
        raise RunnerError("skill-owned control-plane or evidence collector is unavailable")
    repo_root_raw = run_git(pathlib.Path.cwd(), ["rev-parse", "--show-toplevel"])
    repo_root = pathlib.Path(os.fsdecode(repo_root_raw.rstrip(b"\r\n"))).resolve()
    profile_path = pathlib.Path(args.profile)
    profile, profile_hash = load_profile(profile_path, args.expect_profile_sha256)
    if profile["repository_configuration"] == "explicitly-trusted":
        if not args.allow_repository_configuration:
            raise RunnerError(
                "profile requires separate --allow-repository-configuration authorization"
            )
    elif args.allow_repository_configuration:
        raise RunnerError(
            "--allow-repository-configuration is valid only for an explicitly-trusted profile"
        )
    executable, executable_hash = resolve_executable(profile, repo_root)
    control = run_control_plane(helper, repo_root, args.source, args.expect_scope)
    state_before = repository_state_digest(repo_root)

    with tempfile.TemporaryDirectory(prefix="pre-commit-review-static-") as temporary:
        temporary_root = pathlib.Path(temporary)
        snapshot_root = temporary_root / "snapshot"
        runtime_root = temporary_root / "runtime"
        snapshot_root.mkdir(mode=0o700)
        runtime_root.mkdir(mode=0o700)
        try:
            snapshot = materialize_snapshot(
                repo_root, args.source, snapshot_root, profile["limits"]
            )
            process_result = execute_analyzer(
                executable,
                profile["arguments"],
                snapshot_root,
                runtime_root,
                profile,
                args.source,
                args.expect_scope,
            )
            final_status = process_result.status
            execution_id = compact_hash(
                args.expect_scope,
                profile_hash,
                executable_hash,
                process_result.stdout_sha256,
                final_status,
            )
            evidence: dict[str, Any] | None = None
            if final_status == "completed":
                evidence, _ = run_evidence_collector(
                    collector,
                    helper,
                    repo_root,
                    args.source,
                    args.expect_scope,
                    process_result.stdout_path,
                    profile["output_format"],
                    execution_id,
                    args.max_findings,
                )
                if evidence is None or not evidence_matches_profile(evidence, profile):
                    final_status = "invalid-output"
                    execution_id = compact_hash(
                        args.expect_scope,
                        profile_hash,
                        executable_hash,
                        process_result.stdout_sha256,
                        final_status,
                    )
                    evidence = None
            if evidence is None:
                failed_result = runtime_root / "failed-result.json"
                failure_report(failed_result, args.expect_scope, profile["tool"], final_status)
                evidence, detail = run_evidence_collector(
                    collector,
                    helper,
                    repo_root,
                    args.source,
                    args.expect_scope,
                    failed_result,
                    "normalized-json",
                    execution_id,
                    args.max_findings,
                )
                if evidence is None:
                    raise RunnerError(f"cannot create bounded failure evidence: {detail}")

            observed_profile_hash, _ = sha256_file(profile_path)
            if observed_profile_hash != profile_hash:
                raise RunnerError("static-analysis profile changed during execution")
            observed_executable_hash, _ = sha256_file(executable)
            if observed_executable_hash != executable_hash:
                raise RunnerError("trusted analyzer executable changed during execution")
            if repository_state_digest(repo_root) != state_before:
                raise RunnerError("reviewed repository state changed during controlled execution")
            if evidence.get("scope") != {
                "source": control["source"],
                "head": control["head"],
                "fingerprint": control["scope_fingerprint"],
            }:
                raise RunnerError("controlled evidence scope does not match the opening control plane")
            report_ids = sorted(report["report_id"] for report in evidence["reports"])
            failure_reason = process_result.failure_reason
            if final_status == "invalid-output":
                failure_reason = "invalid-output"
            execution = {
                "schema_version": 1,
                "kind": "static_analysis_execution",
                "authoritative": True,
                "execution_id": execution_id,
                "scope": evidence["scope"],
                "profile": {
                    "profile_id": profile_hash[:16],
                    "sha256": profile_hash,
                    "name": profile["name"],
                    "output_format": profile["output_format"],
                    "success_exit_codes": profile["success_exit_codes"],
                    "limits": profile["limits"],
                    "repository_configuration": profile["repository_configuration"],
                    "network_access": profile["network_access"],
                },
                "tool": profile["tool"],
                "executable": {
                    "name": executable.name,
                    "sha256": executable_hash,
                    "path_policy": "absolute-explicit-outside-repository",
                },
                "snapshot": {
                    "kind": "temporary-tracked-files",
                    "sha256": snapshot.sha256,
                    "files": snapshot.files,
                    "bytes": snapshot.bytes,
                },
                "isolation": {
                    "shell": False,
                    "vcs_metadata": False,
                    "environment": "allowlist",
                    "source_tree": "read-only-temporary-snapshot",
                    "original_repository_path": "not-exposed",
                    "network": "best-effort-offline-profile-required",
                },
                "execution": {
                    "status": final_status,
                    "exit_code": process_result.exit_code,
                    "duration_ms": process_result.duration_ms,
                    "stdout_bytes": process_result.stdout_bytes,
                    "stdout_sha256": process_result.stdout_sha256,
                    "stderr_bytes": process_result.stderr_bytes,
                    "stderr_sha256": process_result.stderr_sha256,
                    "result_accepted": final_status == "completed",
                    "failure_reason": failure_reason,
                },
                "evidence": {"report_ids": report_ids},
            }
            print("# Pre-Commit Review Controlled Static Analysis\n")
            print("## Static Analysis Execution JSON")
            print(json.dumps(execution, ensure_ascii=False, separators=(",", ":")))
            print("\n## Static Analysis Evidence JSON")
            print(json.dumps(evidence, ensure_ascii=False, separators=(",", ":")))
        finally:
            make_snapshot_writable(snapshot_root)
    return 0


if __name__ == "__main__":
    try:
        raise SystemExit(main())
    except RunnerError as exc:
        print(f"run_static_analysis: {exc}", file=sys.stderr)
        raise SystemExit(2)
