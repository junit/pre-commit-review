# Windows Artifact Verify Stack Overflow Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make the debug artifact verifier's trusted-runtime probe path fit within the 1 MiB Windows main-thread stack without changing verification behavior or I/O chunk size.

**Architecture:** Keep the existing 1 MiB copy/hash chunk and all digest checks, but allocate the reusable byte buffers on the heap instead of in each function's stack frame. Lock the bug down at the trusted-runtime seam by creating and reverifying a native executable from a worker configured with the Windows PE main-thread stack size.

**Tech Stack:** Rust 1.95, `std::thread`, SHA-256, existing trusted-runtime unit tests

---

### Task 1: Bound Trusted Runtime Stack Use

**Files:**
- Modify: `collect-diff-context-cli/src/trusted_runtime.rs`
- Test: `collect-diff-context-cli/src/trusted_runtime.rs`

- [x] **Step 1: Write the failing 1 MiB stack regression test**

Add a unit test that uses the current native test executable, computes its digest outside the constrained worker, and calls `PrivateRuntime::create` from a worker with the Windows 1 MiB stack reserve:

```rust
#[test]
fn private_runtime_creation_fits_windows_main_thread_stack() {
    const WINDOWS_MAIN_THREAD_STACK_BYTES: usize = 1024 * 1024;

    let source = std::env::current_exe().unwrap();
    let expected_sha256 = format!("{:x}", Sha256::digest(std::fs::read(&source).unwrap()));

    let worker = std::thread::Builder::new()
        .name("trusted-runtime-stack-regression".to_string())
        .stack_size(WINDOWS_MAIN_THREAD_STACK_BYTES)
        .spawn(move || PrivateRuntime::create(&source, &expected_sha256))
        .unwrap();
    let runtime = worker.join().unwrap().unwrap();
    runtime.verify().unwrap();
}
```

- [x] **Step 2: Run the regression test and verify RED**

Run:

```bash
rtk cargo +1.95.0 test --manifest-path collect-diff-context-cli/Cargo.toml --locked --lib trusted_runtime::tests::private_runtime_creation_fits_windows_main_thread_stack -- --exact --nocapture
```

Expected before the fix: the worker reports `has overflowed its stack` because `copy_and_hash` or `hash_file` retains a 1 MiB local array.

- [x] **Step 3: Move the trusted-runtime copy/hash buffers to the heap**

Define one authoritative chunk size and use a heap-backed vector in both paths:

```rust
const COPY_BUFFER_BYTES: usize = 1024 * 1024;

// In copy_and_hash and hash_file:
let mut buffer = vec![0_u8; COPY_BUFFER_BYTES];
```

Do not reduce the chunk size, change hashing, merge trust stages, or weaken post-copy/postflight verification.

- [x] **Step 4: Run the regression and focused trusted-runtime tests and verify GREEN**

Run:

```bash
rtk cargo +1.95.0 test --manifest-path collect-diff-context-cli/Cargo.toml --locked --lib trusted_runtime::tests -- --nocapture
rtk cargo +1.95.0 test --manifest-path collect-diff-context-cli/Cargo.toml --locked --test artifact_cli
```

Expected: all trusted-runtime and artifact CLI tests pass; the constrained worker no longer overflows.

- [x] **Step 5: Run formatting, Clippy, and diff checks**

Run:

```bash
rtk cargo +1.95.0 fmt --all --manifest-path collect-diff-context-cli/Cargo.toml -- --check
rtk cargo +1.95.0 clippy --manifest-path collect-diff-context-cli/Cargo.toml --locked --all-targets --all-features -- -D warnings
rtk git diff --check
```

Expected: all commands exit zero with no warnings or whitespace errors.

- [x] **Step 6: Record the local fix commit**

```bash
rtk git add collect-diff-context-cli/src/trusted_runtime.rs
rtk git commit -m "fix(artifacts): bound trusted runtime stack use"
```

Do not push or dispatch a hosted workflow without separate user approval.

Implementation commit: `0b39b6797e62c54329ba7f1821b155aa2dceddd7`.

- [x] **Step 7: Preserve the implementation plan**

Force-add this ignored workflow artifact in a separate documentation commit so
the reviewed implementation commit remains unchanged.
