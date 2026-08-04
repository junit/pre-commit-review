use collect_diff_context_cli::candidate::{
    CandidateBytes, CandidateContent, CandidateError, CandidateFile, CandidatePresence,
    ChangedRange, GitCandidateContent, RepoPath,
};
use collect_diff_context_cli::impact_context::adapters::tree_sitter_rust::TreeSitterRustAdapter;
use collect_diff_context_cli::impact_context::budget::{BudgetTracker, ImpactBudget};
use collect_diff_context_cli::impact_context::engine::{
    build_impact_context, detect_language, ImpactRequest,
};
use collect_diff_context_cli::impact_context::normalizer::normalize_unit;
use collect_diff_context_cli::impact_context::summarizer::summarize_unit;
use collect_diff_context_cli::review_scope::{
    open_authoritative_scope, ReviewSource, ScopeRequest,
};
use criterion::{black_box, criterion_group, criterion_main, BenchmarkId, Criterion};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::fs;
use std::process::Command;
use tempfile::TempDir;

struct BenchCandidate {
    files: Vec<CandidateFile>,
    contents: BTreeMap<String, Vec<u8>>,
}

impl BenchCandidate {
    fn rust_files(count: usize, source: &[u8], prefix: &str) -> Self {
        let mut files = Vec::with_capacity(count);
        let mut contents = BTreeMap::new();
        for index in 0..count {
            let path = format!("{prefix}/file_{index}.rs");
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

impl CandidateContent for BenchCandidate {
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
            binary: bytes.iter().take(8192).any(|byte| *byte == 0),
            bytes,
        })
    }
}

fn sources() -> Vec<(&'static str, Vec<u8>)> {
    let clean = b"pub fn changed() { helper(); }\nfn helper() {}\n".to_vec();
    let malformed = b"pub fn changed( { let next = @;\n".to_vec();
    let mut deeply_nested = b"pub fn changed() {".to_vec();
    deeply_nested.extend(std::iter::repeat_n(b'{', 600));
    deeply_nested.extend(std::iter::repeat_n(b'}', 600));
    deeply_nested.push(b'}');
    let mut two_mib = b"pub fn changed() { let payload = \"".to_vec();
    two_mib.resize(2 * 1024 * 1024 - 4, b'x');
    two_mib.extend_from_slice(b"\"; }\n");
    vec![
        ("clean", clean),
        ("malformed", malformed),
        ("deeply_nested", deeply_nested),
        ("two_mib", two_mib),
    ]
}

fn staged_git_candidate() -> (TempDir, GitCandidateContent, RepoPath) {
    let repository = TempDir::new().unwrap();
    let git = |arguments: &[&str]| {
        let output = Command::new("git")
            .args(arguments)
            .current_dir(repository.path())
            .output()
            .unwrap();
        assert!(output.status.success());
    };
    git(&["init", "-q"]);
    git(&["config", "user.email", "bench@example.test"]);
    git(&["config", "user.name", "Benchmark"]);
    fs::create_dir_all(repository.path().join("src")).unwrap();
    fs::write(repository.path().join("src/lib.rs"), b"pub fn base() {}\n").unwrap();
    git(&["add", "--", "src/lib.rs"]);
    git(&["commit", "-qm", "base"]);
    fs::write(
        repository.path().join("src/lib.rs"),
        b"pub fn changed() {}\n",
    )
    .unwrap();
    git(&["add", "--", "src/lib.rs"]);
    let scope = open_authoritative_scope(ScopeRequest {
        repository: repository.path().to_path_buf(),
        source: Some(ReviewSource::Staged),
        expected_fingerprint: None,
    })
    .unwrap();
    let candidate = GitCandidateContent::open(&scope).unwrap();
    (repository, candidate, RepoPath::new("src/lib.rs").unwrap())
}

fn impact_context_benchmarks(criterion: &mut Criterion) {
    let clean = b"pub fn changed() { helper(); }\nfn helper() {}\n";
    let one = BenchCandidate::rust_files(1, clean, "src");
    let path = RepoPath::new("src/file_0.rs").unwrap();
    criterion.bench_function("candidate_bytes/read_one", |bencher| {
        bencher.iter(|| black_box(one.read(black_box(&path)).unwrap()))
    });
    let (_repository, git_candidate, git_path) = staged_git_candidate();
    criterion.bench_function("candidate_bytes/git_staged_blob", |bencher| {
        bencher.iter(|| black_box(git_candidate.read(black_box(&git_path)).unwrap()))
    });

    criterion.bench_function("language_detection/rust", |bencher| {
        bencher.iter(|| black_box(detect_language(black_box("src/service.rs"))))
    });

    let changed_ranges = [ChangedRange {
        start_line: 1,
        end_line: 1,
        deletion_anchor: false,
    }];
    let mut parse_group = criterion.benchmark_group("tree_sitter_parse_query");
    for (name, source) in sources() {
        parse_group.bench_with_input(
            BenchmarkId::from_parameter(name),
            &source,
            |bencher, source| {
                bencher.iter(|| {
                    let mut tracker = BudgetTracker::new(ImpactBudget::fast_defaults());
                    black_box(
                        TreeSitterRustAdapter::analyze(
                            black_box(source),
                            &changed_ranges,
                            &mut tracker,
                        )
                        .unwrap(),
                    )
                })
            },
        );
    }
    parse_group.finish();

    let mut tracker = BudgetTracker::new(ImpactBudget::fast_defaults());
    let syntax = TreeSitterRustAdapter::analyze(clean, &changed_ranges, &mut tracker).unwrap();
    criterion.bench_function("normalization/clean_rust", |bencher| {
        bencher.iter(|| {
            black_box(normalize_unit(
                "src/lib.rs",
                "rust",
                "1111111111111111",
                "2222222222222222",
                Some(black_box(&syntax)),
                None,
            ))
        })
    });

    let normalized = normalize_unit(
        "src/lib.rs",
        "rust",
        "1111111111111111",
        "2222222222222222",
        Some(&syntax),
        None,
    );
    criterion.bench_function("summarization/clean_rust", |bencher| {
        bencher.iter(|| {
            black_box(summarize_unit(
                black_box(&normalized),
                Some("pub fn changed() {}"),
            ))
        })
    });

    let context = build_impact_context(&one, ImpactRequest::fast_defaults()).unwrap();
    criterion.bench_function("serialization/impact_context", |bencher| {
        bencher.iter(|| black_box(serde_json::to_vec(black_box(&context)).unwrap()))
    });

    let ten = BenchCandidate::rust_files(10, clean, "src");
    let generated = BenchCandidate::rust_files(10, clean, "generated");
    let hundred = (0..10)
        .map(|batch| BenchCandidate::rust_files(10, clean, &format!("batch_{batch}")))
        .collect::<Vec<_>>();
    let mut end_to_end = criterion.benchmark_group("end_to_end");
    end_to_end.bench_function("one_file", |bencher| {
        bencher
            .iter(|| black_box(build_impact_context(&one, ImpactRequest::fast_defaults()).unwrap()))
    });
    end_to_end.bench_function("ten_files", |bencher| {
        bencher
            .iter(|| black_box(build_impact_context(&ten, ImpactRequest::fast_defaults()).unwrap()))
    });
    end_to_end.bench_function("generated_like_ten_files", |bencher| {
        bencher.iter(|| {
            black_box(build_impact_context(&generated, ImpactRequest::fast_defaults()).unwrap())
        })
    });
    end_to_end.bench_function("one_hundred_files_in_fast_batches", |bencher| {
        bencher.iter(|| {
            for candidate in &hundred {
                black_box(build_impact_context(candidate, ImpactRequest::fast_defaults()).unwrap());
            }
        })
    });
    end_to_end.finish();
}

criterion_group!(benches, impact_context_benchmarks);
criterion_main!(benches);
