#!/usr/bin/env python3
import argparse
import hashlib
import json
import os
import platform as host_platform
import re
import shutil
import signal
import stat
import subprocess
import sys
import tempfile
from pathlib import Path

MAX_JSON_BYTES = 1024 * 1024
MAX_RUNNER_BYTES = 512 * 1024 * 1024
MAX_SAMPLE_MS = 30_000
MAX_RSS_BYTES = 2 * 1024 * 1024 * 1024
SOURCE_LOCK_SHA256 = (
    "298bc6c0339fe2c58fd35bfbd53db285ea7ff34e40734a4f0c36ccb3fe60d862"
)
PACK_VERSION = "2026.07.27-pcr.3"
TOOLCHAIN = "rust-1.95.0-locked"
TIMING_SCOPE = "provider-run-only-v1"
SHA256 = re.compile(r"^[0-9a-f]{64}$")
IDENTIFIER = re.compile(r"^[a-z0-9][a-z0-9-]{0,127}$")
ENVIRONMENT_NAME = re.compile(r"^[A-Za-z_][A-Za-z0-9_]*$")
EXPECTED_RUNNER_SHA256 = "PCR_PROVIDER_BASELINE_EXPECTED_RUNNER_SHA256"
RUNNER_CLASSES = {
    "darwin-amd64": "github-hosted-macos-15-intel",
    "darwin-arm64": "github-hosted-macos-14-arm64",
    "linux-amd64": "github-hosted-ubuntu-24-x64",
    "windows-amd64": "github-hosted-windows-2025-x64",
}
HOSTED_RUNNER_METADATA = {
    "darwin-amd64": {
        "GITHUB_ACTIONS": "true",
        "GITHUB_REPOSITORY": "junit/pre-commit-review",
        "RUNNER_OS": "macOS",
        "RUNNER_ARCH": "X64",
        "ImageOS": "macos15",
    },
    "darwin-arm64": {
        "GITHUB_ACTIONS": "true",
        "GITHUB_REPOSITORY": "junit/pre-commit-review",
        "RUNNER_OS": "macOS",
        "RUNNER_ARCH": "ARM64",
        "ImageOS": "macos14",
    },
    "linux-amd64": {
        "GITHUB_ACTIONS": "true",
        "GITHUB_REPOSITORY": "junit/pre-commit-review",
        "RUNNER_OS": "Linux",
        "RUNNER_ARCH": "X64",
        "ImageOS": "ubuntu24",
    },
    "windows-amd64": {
        "GITHUB_ACTIONS": "true",
        "GITHUB_REPOSITORY": "junit/pre-commit-review",
        "RUNNER_OS": "Windows",
        "RUNNER_ARCH": "X64",
        "ImageOS": "win25",
    },
}
IDENTITY_FIELDS = {
    "platform_id",
    "pack_version",
    "pack_sha256",
    "executable_sha256",
    "source_lock_sha256",
    "profile_sha256",
    "fixture_id",
    "fixture_sha256",
    "request_sha256",
    "runner_class",
    "toolchain",
    "timing_scope",
    "provisioning_included",
}
SAMPLE_FIELDS = IDENTITY_FIELDS | {
    "schema_version",
    "kind",
    "elapsed_ms",
    "peak_process_tree_rss_bytes",
}
RUNNER_FIELDS = {
    "schema_version",
    "kind",
    "command",
    "current_directory",
    "environment",
    "expected",
}


class MeasurementError(Exception):
    def __init__(self, code, message):
        super().__init__(message)
        self.code = code


def fail(code, message):
    raise MeasurementError(code, message)


def canonical_bytes(value):
    return json.dumps(value, ensure_ascii=False, separators=(",", ":")).encode(
        "utf-8"
    )


def canonical_output_bytes(value):
    return json.dumps(
        value, ensure_ascii=False, separators=(",", ":"), sort_keys=True
    ).encode("utf-8")


def require_fields(value, fields, code, label):
    if not isinstance(value, dict) or set(value) != fields:
        fail(code, f"{label} fields are incomplete or unexpected")


def read_canonical(path, code, label):
    raw = read_regular_bytes(path, MAX_JSON_BYTES, code, label)
    try:
        value = json.loads(raw.decode("utf-8"))
    except (UnicodeDecodeError, json.JSONDecodeError) as exc:
        fail(code, f"{label} is not valid JSON: {exc}")
    if canonical_bytes(value) != raw:
        fail(code, f"{label} is not compact canonical JSON")
    return value


def read_regular_bytes(path, maximum_bytes, code, label):
    try:
        descriptor = open_regular_file_no_follow(path)
        try:
            before = os.fstat(descriptor)
            validate_regular_stat(before, code, label)
            if before.st_size <= 0 or before.st_size > maximum_bytes:
                fail(code, f"{label} is outside its byte limit")
            before_fingerprint = file_fingerprint(descriptor, before)
            raw = read_bounded(descriptor, maximum_bytes)
            after = os.fstat(descriptor)
            after_fingerprint = file_fingerprint(descriptor, after)
        finally:
            os.close(descriptor)
    except OSError as exc:
        fail(code, f"could not read {label}: {exc}")
    if before_fingerprint != after_fingerprint:
        fail(code, f"{label} changed while it was read")
    if not raw or len(raw) > maximum_bytes or len(raw) != before.st_size:
        fail(code, f"{label} is outside its byte limit")
    return raw


def open_regular_file_no_follow(path):
    if os.name != "nt":
        flags = os.O_RDONLY | getattr(os, "O_CLOEXEC", 0)
        flags |= getattr(os, "O_NOFOLLOW", 0) | getattr(os, "O_NONBLOCK", 0)
        return os.open(path, flags)

    import ctypes
    import msvcrt

    create_file = ctypes.windll.kernel32.CreateFileW
    create_file.argtypes = [
        ctypes.c_wchar_p,
        ctypes.c_uint32,
        ctypes.c_uint32,
        ctypes.c_void_p,
        ctypes.c_uint32,
        ctypes.c_uint32,
        ctypes.c_void_p,
    ]
    create_file.restype = ctypes.c_void_p
    handle = create_file(
        str(path),
        0x80000000,  # GENERIC_READ
        0x00000001 | 0x00000002 | 0x00000004,  # FILE_SHARE_READ|WRITE|DELETE
        None,
        3,  # OPEN_EXISTING
        0x00200000,  # FILE_FLAG_OPEN_REPARSE_POINT
        None,
    )
    invalid_handle = ctypes.c_void_p(-1).value
    if handle in (None, invalid_handle):
        raise ctypes.WinError()
    try:
        return msvcrt.open_osfhandle(handle, os.O_RDONLY | os.O_BINARY)
    except BaseException:
        ctypes.windll.kernel32.CloseHandle(handle)
        raise


def validate_regular_stat(value, code, label):
    reparse_attribute = getattr(stat, "FILE_ATTRIBUTE_REPARSE_POINT", 0x400)
    if not stat.S_ISREG(value.st_mode) or (
        getattr(value, "st_file_attributes", 0) & reparse_attribute
    ):
        fail(code, f"{label} is not a regular file")


def file_fingerprint(descriptor, value):
    change_time = (
        windows_file_change_time(descriptor) if os.name == "nt" else value.st_ctime_ns
    )
    return (
        value.st_dev,
        value.st_ino,
        value.st_mode,
        value.st_nlink,
        value.st_size,
        value.st_mtime_ns,
        value.st_ctime_ns,
        getattr(value, "st_file_attributes", 0),
        change_time,
    )


def windows_file_change_time(descriptor):
    import ctypes
    import msvcrt

    class FileBasicInfo(ctypes.Structure):
        _fields_ = [
            ("creation_time", ctypes.c_longlong),
            ("last_access_time", ctypes.c_longlong),
            ("last_write_time", ctypes.c_longlong),
            ("change_time", ctypes.c_longlong),
            ("file_attributes", ctypes.c_uint32),
        ]

    get_file_information = ctypes.windll.kernel32.GetFileInformationByHandleEx
    get_file_information.argtypes = [
        ctypes.c_void_p,
        ctypes.c_int,
        ctypes.c_void_p,
        ctypes.c_uint32,
    ]
    get_file_information.restype = ctypes.c_int
    information = FileBasicInfo()
    handle = msvcrt.get_osfhandle(descriptor)
    if not get_file_information(
        ctypes.c_void_p(handle),
        0,  # FileBasicInfo
        ctypes.byref(information),
        ctypes.sizeof(information),
    ):
        raise ctypes.WinError()
    return information.change_time


def read_bounded(descriptor, maximum_bytes):
    remaining = maximum_bytes + 1
    chunks = []
    while remaining:
        chunk = os.read(descriptor, min(64 * 1024, remaining))
        if not chunk:
            break
        chunks.append(chunk)
        remaining -= len(chunk)
    return b"".join(chunks)


def materialize_validated_runner(temporary_root, source_path, runner_bytes):
    directory = Path(temporary_root) / "validated-runner"
    directory.mkdir(mode=0o700)
    if os.name != "nt":
        os.chmod(directory, 0o700)
    path = directory / Path(source_path).name
    flags = os.O_WRONLY | os.O_CREAT | os.O_EXCL | getattr(os, "O_CLOEXEC", 0)
    flags |= getattr(os, "O_BINARY", 0)
    try:
        descriptor = os.open(path, flags, 0o700)
        try:
            offset = 0
            while offset < len(runner_bytes):
                written = os.write(descriptor, runner_bytes[offset : offset + 64 * 1024])
                if written <= 0:
                    fail("runner-provenance", "validated runner copy could not be written")
                offset += written
            os.fsync(descriptor)
        finally:
            os.close(descriptor)
        if os.name != "nt":
            os.chmod(path, 0o500)
    except OSError as exc:
        fail("runner-provenance", f"validated runner copy could not be created: {exc}")
    copied_bytes = read_regular_bytes(
        path, MAX_RUNNER_BYTES, "runner-provenance", "validated runner copy"
    )
    if copied_bytes != runner_bytes:
        fail("runner-provenance", "validated runner copy differs from its source bytes")
    return path


def validate_identity(identity, code, evidence_only_local=False):
    require_fields(identity, IDENTITY_FIELDS, code, "measurement identity")
    platform = identity["platform_id"]
    if platform not in RUNNER_CLASSES:
        fail(code, "measurement platform is unsupported")
    if identity["pack_version"] != PACK_VERSION:
        fail(code, "measurement pack version differs")
    for field in [
        "pack_sha256",
        "executable_sha256",
        "source_lock_sha256",
        "profile_sha256",
        "fixture_sha256",
        "request_sha256",
    ]:
        if not isinstance(identity[field], str) or not SHA256.fullmatch(identity[field]):
            fail(code, f"{field} is not a lower-case SHA256 digest")
    if identity["source_lock_sha256"] != SOURCE_LOCK_SHA256:
        fail(code, "measurement source lock differs")
    if not isinstance(identity["fixture_id"], str) or not IDENTIFIER.fullmatch(
        identity["fixture_id"]
    ):
        fail(code, "measurement fixture id is invalid")
    expected_runner_class = (
        f"local-{platform}" if evidence_only_local else RUNNER_CLASSES[platform]
    )
    if identity["runner_class"] != expected_runner_class:
        fail(code, "measurement runner class differs from its platform")
    if (
        identity["toolchain"] != TOOLCHAIN
        or identity["timing_scope"] != TIMING_SCOPE
        or identity["provisioning_included"] is not False
    ):
        fail(code, "measurement timing or toolchain policy differs")


def validate_runner(value, evidence_only_local=False):
    require_fields(value, RUNNER_FIELDS, "runner-contract", "runner contract")
    if value["schema_version"] != 1 or value["kind"] != "provider_baseline_runner":
        fail("runner-contract", "runner contract identity differs")
    command = value["command"]
    if (
        not isinstance(command, list)
        or not 1 <= len(command) <= 64
        or any(not isinstance(item, str) or not item or len(item) > 4096 for item in command)
        or not Path(command[0]).is_absolute()
    ):
        fail("runner-contract", "runner command is invalid")
    executable = Path(command[0])
    runner_bytes = read_regular_bytes(
        executable, MAX_RUNNER_BYTES, "runner-contract", "runner executable"
    )
    runner_sha256 = hashlib.sha256(runner_bytes).hexdigest()
    current_directory = Path(value["current_directory"])
    if (
        not current_directory.is_absolute()
        or current_directory.is_symlink()
        or not current_directory.is_dir()
    ):
        fail("runner-contract", "runner current directory is invalid")
    environment = value["environment"]
    if (
        not isinstance(environment, dict)
        or len(environment) > 64
        or any(
            not isinstance(key, str)
            or not key
            or not ENVIRONMENT_NAME.fullmatch(key)
            or not isinstance(item, str)
            or len(key) > 128
            or len(item) > 16 * 1024
            or "\0" in item
            for key, item in environment.items()
        )
    ):
        fail("runner-contract", "runner environment is invalid")
    folded_environment_names = [key.casefold() for key in environment]
    if len(folded_environment_names) != len(set(folded_environment_names)):
        fail("runner-contract", "runner environment contains case-folded duplicate names")
    validate_identity(value["expected"], "baseline-binding", evidence_only_local)
    if not evidence_only_local:
        validate_hosted_runner_provenance(
            command, environment, value["expected"], runner_sha256
        )
    return (
        command,
        current_directory,
        environment,
        value["expected"],
        runner_sha256,
        runner_bytes,
    )


def current_platform():
    machine = host_platform.machine().lower()
    if sys.platform == "darwin" and machine in {"x86_64", "amd64"}:
        return "darwin-amd64"
    if sys.platform == "darwin" and machine in {"arm64", "aarch64"}:
        return "darwin-arm64"
    if sys.platform.startswith("linux") and machine in {"x86_64", "amd64"}:
        return "linux-amd64"
    if sys.platform == "win32" and machine in {"x86_64", "amd64"}:
        return "windows-amd64"
    fail("runner-provenance", "measurement host platform is unsupported")


def validate_hosted_runner_provenance(command, environment, expected, runner_sha256):
    platform_id = expected["platform_id"]
    if current_platform() != platform_id:
        fail("runner-provenance", "measurement host platform differs from the contract")
    metadata = HOSTED_RUNNER_METADATA[platform_id]
    if any(
        os.environ.get(name) != value or environment.get(name) != value
        for name, value in metadata.items()
    ):
        fail("runner-provenance", "GitHub runner metadata is not process-bound")
    trusted_digest_name = EXPECTED_RUNNER_SHA256.casefold()
    if any(name.casefold() == trusted_digest_name for name in environment):
        fail("runner-provenance", "runner contract cannot declare its trusted digest")
    validate_hosted_environment(environment, metadata)
    trusted_runner_sha256 = os.environ.get(EXPECTED_RUNNER_SHA256)
    if not isinstance(trusted_runner_sha256, str) or not SHA256.fullmatch(
        trusted_runner_sha256
    ):
        fail("runner-provenance", "trusted runner digest is missing or invalid")
    if runner_sha256 != trusted_runner_sha256:
        fail("runner-provenance", "runner executable differs from the trusted build digest")
    executable_name = Path(command[0]).name
    expected_name = (
        "provider-baseline-sample-runner.exe"
        if sys.platform == "win32"
        else "provider-baseline-sample-runner"
    )
    expected_flags = ["sample", "--target-root", "--source-lock", "--fixture-root", "--runner-class"]
    if (
        executable_name != expected_name
        or len(command) != 10
        or [command[index] for index in [1, 2, 4, 6, 8]] != expected_flags
        or any(not Path(command[index]).is_absolute() for index in [3, 5, 7])
        or command[9] != expected["runner_class"]
    ):
        fail("runner-provenance", "hosted measurement command is not the authorized Rust runner")


def validate_hosted_environment(environment, metadata):
    process_bound_names = {"SystemRoot", "TMPDIR", "TMP", "TEMP"}
    fixed_values = {
        "GIT_CONFIG_GLOBAL": "NUL" if os.name == "nt" else "/dev/null",
        "GIT_CONFIG_NOSYSTEM": "1",
        "GIT_TERMINAL_PROMPT": "0",
        "LC_ALL": "C",
    }
    required_process_bound_names = {
        name for name in process_bound_names if os.environ.get(name) is not None
    }
    expected_names = (
        set(metadata) | required_process_bound_names | set(fixed_values) | {"PATH"}
    )
    if set(environment) != expected_names:
        fail("runner-provenance", "runner environment policy names differ")
    if any(environment[name] != value for name, value in fixed_values.items()):
        fail("runner-provenance", "runner environment policy value differs")
    if any(
        os.environ.get(name) != environment[name]
        for name in required_process_bound_names
    ):
        fail("runner-provenance", "runner environment is not process-bound")
    validate_hosted_git_path(environment["PATH"])


def validate_hosted_git_path(value):
    git = shutil.which("git.exe" if os.name == "nt" else "git")
    if git is None:
        fail("runner-provenance", "measurement process Git executable is unavailable")
    trusted_directory = str(Path(git).resolve(strict=True).parent)
    paths = value.split(os.pathsep)
    if (
        len(paths) != 1
        or not Path(paths[0]).is_absolute()
        or os.path.normcase(os.path.normpath(paths[0]))
        != os.path.normcase(os.path.normpath(trusted_directory))
    ):
        fail("runner-provenance", "runner Git PATH is not process-bound")


def run_sample(
    command,
    current_directory,
    environment,
    expected,
    output_path,
    evidence_only_local=False,
    runner_timeout_seconds=35.0,
):
    output_path.unlink(missing_ok=True)
    run_environment = dict(environment)
    run_environment["PCR_PROVIDER_BASELINE_SAMPLE_OUTPUT"] = str(output_path)
    process = None
    try:
        process = subprocess.Popen(
            command,
            cwd=current_directory,
            env=run_environment,
            stdin=subprocess.DEVNULL,
            stdout=subprocess.DEVNULL,
            stderr=subprocess.PIPE,
            start_new_session=os.name != "nt",
            creationflags=(
                subprocess.CREATE_NEW_PROCESS_GROUP if os.name == "nt" else 0
            ),
        )
        _, stderr = process.communicate(timeout=runner_timeout_seconds)
    except subprocess.TimeoutExpired:
        terminate_process_tree(process)
        fail("runner-timeout", "provider baseline runner exceeded its outer timeout")
    except (OSError, subprocess.SubprocessError) as exc:
        if process is not None:
            terminate_process_tree(process)
        fail("runner-execution", f"provider baseline runner failed: {exc}")
    if process.returncode != 0:
        detail = stderr[:4096].decode("utf-8", errors="replace")
        fail("runner-execution", f"provider baseline runner exited unsuccessfully: {detail}")
    sample = read_canonical(output_path, "sample-output", "provider baseline sample")
    require_fields(sample, SAMPLE_FIELDS, "sample-output", "provider baseline sample")
    if sample["schema_version"] != 1 or sample["kind"] != "provider_baseline_sample":
        fail("sample-output", "provider baseline sample identity differs")
    identity = {field: sample[field] for field in IDENTITY_FIELDS}
    validate_identity(identity, "baseline-binding", evidence_only_local)
    if identity != expected:
        fail("baseline-binding", "provider baseline sample differs from expected bindings")
    elapsed = sample["elapsed_ms"]
    if isinstance(elapsed, bool) or not isinstance(elapsed, int) or not 0 < elapsed <= MAX_SAMPLE_MS:
        fail("measurement-deadline", "provider baseline sample exceeded its deadline")
    rss = sample["peak_process_tree_rss_bytes"]
    if isinstance(rss, bool) or not isinstance(rss, int) or not 0 < rss <= MAX_RSS_BYTES:
        fail("measurement-rss", "provider baseline RSS is outside its authorized range")
    return elapsed, rss


def terminate_process_tree(process):
    if process is None:
        return
    if os.name == "nt":
        terminate_windows_process_tree(process)
    else:
        terminate_unix_process_tree(process)
    try:
        process.communicate(timeout=5)
    except subprocess.TimeoutExpired:
        if process.poll() is None:
            process.kill()
        if process.stderr is not None:
            process.stderr.close()
        try:
            process.wait(timeout=1)
        except subprocess.TimeoutExpired:
            pass


def terminate_windows_process_tree(process):
    try:
        subprocess.run(
            ["taskkill", "/PID", str(process.pid), "/T", "/F"],
            stdin=subprocess.DEVNULL,
            stdout=subprocess.DEVNULL,
            stderr=subprocess.DEVNULL,
            timeout=5,
            check=False,
        )
    except (OSError, subprocess.SubprocessError):
        pass
    if process.poll() is None:
        process.kill()


def terminate_unix_process_tree(process):
    own_group = os.getpgrp()
    try:
        root_group = os.getpgid(process.pid)
    except (ProcessLookupError, PermissionError):
        root_group = process.pid
    if root_group > 0 and root_group != own_group:
        try:
            os.killpg(root_group, signal.SIGSTOP)
        except (ProcessLookupError, PermissionError):
            pass
    processes = unix_process_snapshot()
    descendants = descendant_processes(process.pid, processes)
    for _, group_id in descendants:
        if group_id > 0 and group_id != own_group:
            try:
                os.killpg(group_id, signal.SIGSTOP)
            except (ProcessLookupError, PermissionError):
                pass
    if descendants:
        processes = unix_process_snapshot()
        descendants = descendant_processes(process.pid, processes)
    groups = {
        group_id
        for _, group_id in [(process.pid, root_group), *descendants]
        if group_id > 0 and group_id != own_group
    }
    for group_id in groups:
        try:
            os.killpg(group_id, signal.SIGKILL)
        except (ProcessLookupError, PermissionError):
            pass
    for process_id, _ in descendants:
        try:
            os.kill(process_id, signal.SIGKILL)
        except (ProcessLookupError, PermissionError):
            pass
    if process.poll() is None:
        process.kill()


def unix_process_snapshot():
    try:
        result = subprocess.run(
            ["ps", "-axo", "pid=,ppid=,pgid="],
            stdin=subprocess.DEVNULL,
            stdout=subprocess.PIPE,
            stderr=subprocess.DEVNULL,
            timeout=2,
            check=False,
        )
    except (OSError, subprocess.SubprocessError):
        return {}
    if result.returncode != 0 or len(result.stdout) > 16 * 1024 * 1024:
        return {}
    processes = {}
    for line in result.stdout.decode("ascii", errors="ignore").splitlines():
        fields = line.split()
        if len(fields) != 3 or not all(field.isdigit() for field in fields):
            continue
        process_id, parent_id, group_id = map(int, fields)
        processes[process_id] = (parent_id, group_id)
    return processes


def descendant_processes(root_process_id, processes):
    descendants = []
    frontier = [root_process_id]
    seen = {root_process_id}
    while frontier:
        parent = frontier.pop()
        for process_id, (parent_id, group_id) in processes.items():
            if parent_id == parent and process_id not in seen:
                seen.add(process_id)
                frontier.append(process_id)
                descendants.append((process_id, group_id))
    return descendants


def measure(
    runner_path,
    sample_count,
    evidence_only_local=False,
    runner_timeout_seconds=35.0,
):
    if not 20 <= sample_count <= 100:
        fail("measurement-samples", "sample count must be between 20 and 100")
    runner = read_canonical(runner_path, "runner-contract", "runner contract")
    (
        command,
        current_directory,
        environment,
        expected,
        runner_sha256,
        runner_bytes,
    ) = validate_runner(runner, evidence_only_local)
    with tempfile.TemporaryDirectory(prefix="provider-baseline-") as temporary:
        validated_runner = materialize_validated_runner(
            temporary, command[0], runner_bytes
        )
        command = [str(validated_runner), *command[1:]]
        output_path = Path(temporary) / "sample.json"
        run_sample(
            command,
            current_directory,
            environment,
            expected,
            output_path,
            evidence_only_local,
            runner_timeout_seconds,
        )
        samples = []
        peak_rss = 0
        for _ in range(sample_count):
            elapsed, rss = run_sample(
                command,
                current_directory,
                environment,
                expected,
                output_path,
                evidence_only_local,
                runner_timeout_seconds,
            )
            samples.append(elapsed)
            peak_rss = max(peak_rss, rss)
    ordered = sorted(samples)
    rank = (len(ordered) * 95 + 99) // 100
    return {
        "platform_id": expected["platform_id"],
        "pack_version": expected["pack_version"],
        "pack_sha256": expected["pack_sha256"],
        "executable_sha256": expected["executable_sha256"],
        "runner_sha256": runner_sha256,
        "source_lock_sha256": expected["source_lock_sha256"],
        "profile_sha256": expected["profile_sha256"],
        "fixture_id": expected["fixture_id"],
        "fixture_sha256": expected["fixture_sha256"],
        "request_sha256": expected["request_sha256"],
        "runner_class": expected["runner_class"],
        "toolchain": expected["toolchain"],
        "timing_scope": expected["timing_scope"],
        "provisioning_included": expected["provisioning_included"],
        "samples_ms": samples,
        "p95_ms": ordered[rank - 1],
        "peak_process_tree_rss_bytes": peak_rss,
    }


def parse_args():
    parser = argparse.ArgumentParser(
        description="Measure one provisioned rust-analyzer provider baseline"
    )
    parser.add_argument("--runner", required=True, type=Path)
    parser.add_argument("--samples", required=True, type=int)
    parser.add_argument("--evidence-only-local", action="store_true")
    parser.add_argument("--runner-timeout-seconds", type=float)
    return parser.parse_args()


def main():
    args = parse_args()
    if args.runner_timeout_seconds is not None and not args.evidence_only_local:
        fail(
            "runner-timeout-policy",
            "a custom runner timeout is permitted only for local evidence",
        )
    runner_timeout_seconds = (
        35.0 if args.runner_timeout_seconds is None else args.runner_timeout_seconds
    )
    if not 0.1 <= runner_timeout_seconds <= 35.0:
        fail("runner-timeout-policy", "runner timeout is outside its authorized range")
    measurement = measure(
        args.runner,
        args.samples,
        args.evidence_only_local,
        runner_timeout_seconds,
    )
    if args.evidence_only_local:
        measurement = {
            "schema_version": 1,
            "kind": "provider_baseline_local_evidence",
            "baseline_eligible": False,
            "reason": "non-hosted-runner",
            "measurement": measurement,
        }
    sys.stdout.buffer.write(canonical_output_bytes(measurement))


if __name__ == "__main__":
    try:
        main()
    except MeasurementError as exc:
        print(f"provider baseline measurement failed: {exc.code}: {exc}", file=sys.stderr)
        sys.exit(1)
    except (KeyError, TypeError, ValueError) as exc:
        print(f"provider baseline measurement failed: runner-contract: {exc}", file=sys.stderr)
        sys.exit(1)
