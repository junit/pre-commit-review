mod support;

use collect_diff_context_cli::candidate::{
    CandidateBytes, CandidateContent, CandidateError, CandidateFile, CandidateOpenLimits,
    CandidatePresence, ChangedRange, GitCandidateContent, RepoPath,
};
use collect_diff_context_cli::impact_context::engine::{build_impact_context, ImpactRequest};
use collect_diff_context_cli::review_scope::ReviewSource;
use sha2::{Digest, Sha256};
use std::alloc::{GlobalAlloc, Layout, System};
use std::collections::BTreeMap;
use std::error::Error;
use std::hint::black_box;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant};
use support::GitRepo;

const P95_LIMIT: Duration = Duration::from_millis(200);
const P99_LIMIT: Duration = Duration::from_millis(500);
const PEAK_MEMORY_LIMIT: usize = 128 * 1024 * 1024;

struct TrackingAllocator;

static CURRENT_BYTES: AtomicUsize = AtomicUsize::new(0);
static PEAK_BYTES: AtomicUsize = AtomicUsize::new(0);

#[global_allocator]
static ALLOCATOR: TrackingAllocator = TrackingAllocator;

unsafe impl GlobalAlloc for TrackingAllocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        let pointer = unsafe { System.alloc(layout) };
        if !pointer.is_null() {
            record_allocation(layout.size());
        }
        pointer
    }

    unsafe fn dealloc(&self, pointer: *mut u8, layout: Layout) {
        unsafe { System.dealloc(pointer, layout) };
        CURRENT_BYTES.fetch_sub(layout.size(), Ordering::Relaxed);
    }

    unsafe fn realloc(&self, pointer: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        let replacement = unsafe { System.realloc(pointer, layout, new_size) };
        if !replacement.is_null() {
            if new_size >= layout.size() {
                record_allocation(new_size - layout.size());
            } else {
                CURRENT_BYTES.fetch_sub(layout.size() - new_size, Ordering::Relaxed);
            }
        }
        replacement
    }
}

fn record_allocation(bytes: usize) {
    let current = CURRENT_BYTES.fetch_add(bytes, Ordering::Relaxed) + bytes;
    PEAK_BYTES.fetch_max(current, Ordering::Relaxed);
}

struct PerformanceCandidate {
    files: Vec<CandidateFile>,
    contents: BTreeMap<String, Vec<u8>>,
}

impl PerformanceCandidate {
    fn rust_files(count: usize) -> Self {
        let source = b"pub fn changed() { helper(); }\nfn helper() {}\n";
        let mut files = Vec::with_capacity(count);
        let mut contents = BTreeMap::new();
        for index in 0..count {
            let path = format!("src/file_{index}.rs");
            contents.insert(path.clone(), source.to_vec());
            files.push(CandidateFile {
                path: RepoPath::new(&path).unwrap(),
                mode: "100644".to_string(),
                content_identity: Some(format!("sha256:{:x}", Sha256::digest(source))),
                presence: CandidatePresence::Present,
                manifest_unit_id: Some(format!("file:{path}")),
                change_status: Some("M".to_string()),
                changed_ranges: vec![ChangedRange {
                    start_line: 1,
                    end_line: 1,
                    deletion_anchor: false,
                }],
            });
        }
        Self { files, contents }
    }
}

impl CandidateContent for PerformanceCandidate {
    fn scope_fingerprint(&self) -> &str {
        "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
    }

    fn candidate_digest(&self) -> &str {
        "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb"
    }

    fn source(&self) -> ReviewSource {
        ReviewSource::Staged
    }

    fn files(&self) -> &[CandidateFile] {
        &self.files
    }

    fn read_bounded(
        &self,
        path: &RepoPath,
        max_bytes: usize,
    ) -> Result<CandidateBytes, CandidateError> {
        let source = &self.contents[path.as_str()];
        if source.len() > max_bytes {
            return Err(CandidateError::byte_limit_exceeded(path, max_bytes));
        }
        let bytes = source.clone();
        Ok(CandidateBytes {
            sha256: format!("{:x}", Sha256::digest(&bytes)),
            binary: false,
            bytes,
        })
    }
}

#[test]
fn fast_mode_meets_release_latency_and_memory_gates() {
    if cfg!(debug_assertions) {
        return;
    }

    let candidate = PerformanceCandidate::rust_files(10);
    for _ in 0..10 {
        black_box(build_impact_context(
            &candidate,
            ImpactRequest::fast_defaults(),
        ))
        .unwrap();
    }

    let mut samples = Vec::with_capacity(100);
    for _ in 0..100 {
        let started = Instant::now();
        let context = build_impact_context(&candidate, ImpactRequest::fast_defaults()).unwrap();
        black_box(context);
        samples.push(started.elapsed());
    }
    samples.sort_unstable();
    let p95 = percentile(&samples, 95);
    let p99 = percentile(&samples, 99);

    let baseline = CURRENT_BYTES.load(Ordering::Relaxed);
    PEAK_BYTES.store(baseline, Ordering::Relaxed);
    let context = build_impact_context(&candidate, ImpactRequest::fast_defaults()).unwrap();
    black_box(&context);
    let peak_increment = PEAK_BYTES.load(Ordering::Relaxed).saturating_sub(baseline);

    assert!(
        p95 <= P95_LIMIT,
        "fast-mode P95 {p95:?} exceeds {P95_LIMIT:?}"
    );
    assert!(
        p99 <= P99_LIMIT,
        "fast-mode P99 {p99:?} exceeds {P99_LIMIT:?}"
    );
    assert!(
        peak_increment <= PEAK_MEMORY_LIMIT,
        "fast-mode incremental peak memory {peak_increment} exceeds {PEAK_MEMORY_LIMIT} bytes"
    );
}

#[test]
fn git_candidate_preparation_is_included_in_the_release_latency_gate() -> Result<(), Box<dyn Error>>
{
    if cfg!(debug_assertions) {
        return Ok(());
    }

    let repository = GitRepo::new()?;
    repository.commit_file("src/file_0.rs", b"pub fn value() -> u8 { 1 }\n")?;
    for index in 1..10 {
        repository.write(
            &format!("src/file_{index}.rs"),
            b"pub fn value() -> u8 { 1 }\n",
        )?;
    }
    repository.git(["add", "--", "."])?;
    repository.git(["commit", "-qm", "remaining fixture files"])?;
    for index in 0..10 {
        repository.write(
            &format!("src/file_{index}.rs"),
            b"pub fn value() -> u8 { 2 }\n",
        )?;
    }
    let scope = repository.scope(ReviewSource::Unstaged)?;
    let budget = collect_diff_context_cli::impact_context::budget::ImpactBudget::fast_defaults();

    for _ in 0..3 {
        black_box(run_fast_pipeline(&scope, &budget)?);
    }
    let mut samples = Vec::with_capacity(30);
    for _ in 0..30 {
        let started = Instant::now();
        black_box(run_fast_pipeline(&scope, &budget)?);
        samples.push(started.elapsed());
    }
    samples.sort_unstable();
    let p95 = percentile(&samples, 95);
    let p99 = percentile(&samples, 99);

    assert!(
        p95 <= P95_LIMIT,
        "candidate-plus-engine P95 {p95:?} exceeds {P95_LIMIT:?}"
    );
    assert!(
        p99 <= P99_LIMIT,
        "candidate-plus-engine P99 {p99:?} exceeds {P99_LIMIT:?}"
    );
    Ok(())
}

fn run_fast_pipeline(
    scope: &collect_diff_context_cli::review_scope::AuthoritativeScope,
    budget: &collect_diff_context_cli::impact_context::budget::ImpactBudget,
) -> Result<collect_diff_context_cli::impact_context::contracts::ImpactContext, Box<dyn Error>> {
    let started = Instant::now();
    let candidate = GitCandidateContent::open_bounded(
        scope,
        CandidateOpenLimits {
            deadline: budget.deadline,
            max_changed_files: budget.max_changed_files,
            max_file_bytes: budget.max_file_bytes,
            max_total_bytes: budget.max_total_bytes,
        },
    )?;
    let mut request = ImpactRequest::fast_defaults();
    request.budget = budget.clone();
    request.budget.deadline = request.budget.deadline.saturating_sub(started.elapsed());
    Ok(build_impact_context(&candidate, request)?)
}

fn percentile(samples: &[Duration], percentile: usize) -> Duration {
    let rank = samples.len().saturating_mul(percentile).div_ceil(100);
    samples[rank.saturating_sub(1).min(samples.len() - 1)]
}
