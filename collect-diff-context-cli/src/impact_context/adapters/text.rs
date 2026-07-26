use crate::candidate::{CandidateContent, CandidatePresence, RepoPath};
use crate::impact_context::budget::{BudgetResource, BudgetTracker};
use crate::impact_context::contracts::{SourceRange, UnitStatus};
use regex::Regex;
use serde::Serialize;
use std::collections::BTreeMap;

const CONTEXT_QUERIES_PATH: &str = ".pre-commit-review/context-queries";
const TEST_HINTS_PATH: &str = ".pre-commit-review/test-hints";

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum TextFactKind {
    ConfiguredQuery,
    Configuration,
    Framework,
    TestMarker,
    TestHint,
    Endpoint,
    Authorization,
    Storage,
    Network,
    Cache,
    Broker,
    Search,
    Lifecycle,
}

impl TextFactKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::ConfiguredQuery => "configured-query",
            Self::Configuration => "configuration",
            Self::Framework => "framework",
            Self::TestMarker => "test-marker",
            Self::TestHint => "test-hint",
            Self::Endpoint => "endpoint",
            Self::Authorization => "authorization",
            Self::Storage => "storage",
            Self::Network => "network",
            Self::Cache => "cache",
            Self::Broker => "broker",
            Self::Search => "search",
            Self::Lifecycle => "lifecycle",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum TextProvenance {
    Textual,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct TextFact {
    pub rule_id: String,
    pub kind: TextFactKind,
    pub match_text: String,
    pub range: SourceRange,
    pub provenance: TextProvenance,
    pub resolved_target: Option<String>,
    pub details: BTreeMap<String, String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct TextOutput {
    pub status: UnitStatus,
    pub facts: Vec<TextFact>,
    pub limitation_codes: Vec<String>,
}

#[derive(Debug, Clone)]
pub struct TextConfiguration {
    queries: Vec<ConfiguredQuery>,
    test_hints: Vec<TestHint>,
    pub limitation_codes: Vec<String>,
}

#[derive(Debug, Clone)]
struct ConfiguredQuery {
    rule_id: String,
    regex: Regex,
}

#[derive(Debug, Clone)]
struct TestHint {
    rule_id: String,
    path_regex: Option<Regex>,
    content_regex: Option<Regex>,
    test_kind: String,
    environment_dependency: String,
    confidence: String,
    hint: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TextAdapterError {
    message: String,
}

impl TextAdapterError {
    fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
        }
    }
}

impl std::fmt::Display for TextAdapterError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl std::error::Error for TextAdapterError {}

pub struct TextAdapter;

impl TextAdapter {
    pub fn load_configuration(
        candidate: &dyn CandidateContent,
        budget: &mut BudgetTracker,
    ) -> Result<TextConfiguration, TextAdapterError> {
        let mut limitation_codes = Vec::new();
        let mut queries = Vec::new();
        if let Some(bytes) = read_optional_candidate(candidate, CONTEXT_QUERIES_PATH)? {
            if bytes.iter().take(8192).any(|byte| *byte == 0) {
                push_unique(&mut limitation_codes, "binary-context-query-config");
            } else {
                for (index, line) in String::from_utf8_lossy(&bytes).lines().enumerate() {
                    let pattern = line.trim();
                    if pattern.is_empty() || pattern.starts_with('#') {
                        continue;
                    }
                    if budget.consume(BudgetResource::QueryPatterns, 1).is_err() {
                        push_unique(&mut limitation_codes, "query-pattern-budget-exhausted");
                        break;
                    }
                    if pattern.chars().count() > 500 {
                        push_unique(&mut limitation_codes, "text-query-too-long");
                        continue;
                    }
                    match Regex::new(pattern) {
                        Ok(regex) => queries.push(ConfiguredQuery {
                            rule_id: format!("context-query-{:03}", index + 1),
                            regex,
                        }),
                        Err(_) => push_unique(&mut limitation_codes, "invalid-text-query"),
                    }
                }
                push_unique(&mut limitation_codes, "text-query-scope-changed-files");
            }
        }

        let mut test_hints = Vec::new();
        if let Some(bytes) = read_optional_candidate(candidate, TEST_HINTS_PATH)? {
            if bytes.iter().take(8192).any(|byte| *byte == 0) {
                push_unique(&mut limitation_codes, "binary-test-hint-config");
            } else {
                for line in String::from_utf8_lossy(&bytes).lines() {
                    let trimmed = line.trim();
                    if trimmed.is_empty() || trimmed.starts_with('#') {
                        continue;
                    }
                    if budget.consume(BudgetResource::QueryPatterns, 1).is_err() {
                        push_unique(&mut limitation_codes, "query-pattern-budget-exhausted");
                        break;
                    }
                    let parts = line.split('\t').collect::<Vec<_>>();
                    if parts.len() < 7 {
                        push_unique(&mut limitation_codes, "invalid-test-hint");
                        continue;
                    }
                    let path_regex = compile_optional_regex(parts[1].trim());
                    let content_regex = compile_optional_regex(parts[2].trim());
                    if path_regex.is_err() || content_regex.is_err() {
                        push_unique(&mut limitation_codes, "invalid-test-hint");
                        continue;
                    }
                    let hint = parts[6..].join(" ");
                    if parts[0].trim().is_empty()
                        || parts[3].trim().is_empty()
                        || parts[4].trim().is_empty()
                        || parts[5].trim().is_empty()
                        || hint.trim().is_empty()
                    {
                        push_unique(&mut limitation_codes, "invalid-test-hint");
                        continue;
                    }
                    test_hints.push(TestHint {
                        rule_id: bounded_text(parts[0].trim()),
                        path_regex: path_regex.unwrap(),
                        content_regex: content_regex.unwrap(),
                        test_kind: bounded_text(parts[3].trim()),
                        environment_dependency: bounded_text(parts[4].trim()),
                        confidence: bounded_text(parts[5].trim()),
                        hint: bounded_text(hint.trim()),
                    });
                }
            }
        }
        limitation_codes.sort();
        Ok(TextConfiguration {
            queries,
            test_hints,
            limitation_codes,
        })
    }

    pub fn scan(
        path: &RepoPath,
        source: &[u8],
        binary: bool,
        configuration: &TextConfiguration,
        budget: &mut BudgetTracker,
    ) -> TextOutput {
        if binary || source.iter().take(8192).any(|byte| *byte == 0) {
            return TextOutput {
                status: UnitStatus::Unsupported,
                facts: Vec::new(),
                limitation_codes: vec!["binary-text-unavailable".to_string()],
            };
        }

        let text = String::from_utf8_lossy(source);
        let mut facts = Vec::new();
        let mut limitations = Vec::new();
        let mut status = UnitStatus::Completed;

        for query in &configuration.queries {
            let mut matches = query.regex.find_iter(&text);
            let maximum = budget.budget().max_matches_per_pattern;
            for index in 0..=maximum {
                let Some(found) = matches.next() else {
                    break;
                };
                if index == maximum {
                    push_unique(&mut limitations, "query-match-budget-exhausted");
                    status = UnitStatus::Partial;
                    break;
                }
                if !push_fact(
                    &mut facts,
                    fact_from_span(
                        &text,
                        found.start(),
                        found.end(),
                        &query.rule_id,
                        TextFactKind::ConfiguredQuery,
                        BTreeMap::new(),
                    ),
                    budget,
                ) {
                    push_unique(&mut limitations, "fact-budget-exhausted");
                    status = UnitStatus::BudgetExhausted;
                    break;
                }
            }
        }

        let lower_path = path.as_str().to_ascii_lowercase();
        if lower_path.ends_with(".toml") {
            scan_key_values(
                &text,
                r"(?m)^[ \t]*([A-Za-z_][A-Za-z0-9_.-]*)[ \t]*=[ \t]*([^\r\n]*)",
                "toml-key",
                &mut facts,
                &mut limitations,
                &mut status,
                budget,
            );
        } else if lower_path.ends_with(".yaml") || lower_path.ends_with(".yml") {
            scan_key_values(
                &text,
                r"(?m)^[ \t]*([A-Za-z_][A-Za-z0-9_.-]*)[ \t]*:[ \t]*([^\r\n]*)",
                "yaml-key",
                &mut facts,
                &mut limitations,
                &mut status,
                budget,
            );
        } else if lower_path.ends_with("dockerfile") || lower_path.contains("dockerfile.") {
            scan_key_values(
                &text,
                r"(?mi)^[ \t]*(FROM|ENV|ARG|RUN|EXPOSE|HEALTHCHECK|ENTRYPOINT|CMD)\b([^\r\n]*)",
                "docker-instruction",
                &mut facts,
                &mut limitations,
                &mut status,
                budget,
            );
        } else if lower_path.ends_with(".sql") {
            scan_key_values(
                &text,
                r"(?mi)\b(CREATE|ALTER|DROP|SELECT|INSERT|UPDATE|DELETE)\b[^;\r\n]*",
                "sql-statement",
                &mut facts,
                &mut limitations,
                &mut status,
                budget,
            );
        }

        scan_markers(&text, &mut facts, &mut limitations, &mut status, budget);
        for hint in &configuration.test_hints {
            let path_match = hint
                .path_regex
                .as_ref()
                .is_some_and(|regex| regex.is_match(path.as_str()));
            let content_match = hint
                .content_regex
                .as_ref()
                .is_some_and(|regex| regex.is_match(&text));
            if !path_match && !content_match {
                continue;
            }
            let mut details = BTreeMap::new();
            details.insert("confidence".to_string(), hint.confidence.clone());
            details.insert(
                "environment_dependency".to_string(),
                hint.environment_dependency.clone(),
            );
            details.insert("test_kind".to_string(), hint.test_kind.clone());
            let fact = TextFact {
                rule_id: hint.rule_id.clone(),
                kind: TextFactKind::TestHint,
                match_text: hint.hint.clone(),
                range: SourceRange {
                    start_line: 1,
                    start_column: 1,
                    end_line: 1,
                    end_column: 1,
                    start_byte: 0,
                    end_byte: 0,
                },
                provenance: TextProvenance::Textual,
                resolved_target: None,
                details,
            };
            if !push_fact(&mut facts, fact, budget) {
                push_unique(&mut limitations, "fact-budget-exhausted");
                status = UnitStatus::BudgetExhausted;
                break;
            }
        }

        facts.sort_by(|left, right| {
            (
                left.range.start_byte,
                left.range.end_byte,
                left.kind,
                &left.rule_id,
            )
                .cmp(&(
                    right.range.start_byte,
                    right.range.end_byte,
                    right.kind,
                    &right.rule_id,
                ))
        });
        facts.dedup_by(|left, right| {
            left.kind == right.kind
                && left.rule_id == right.rule_id
                && left.range == right.range
                && left.match_text == right.match_text
        });
        limitations.sort();
        TextOutput {
            status,
            facts,
            limitation_codes: limitations,
        }
    }
}

fn read_optional_candidate(
    candidate: &dyn CandidateContent,
    path: &str,
) -> Result<Option<Vec<u8>>, TextAdapterError> {
    let repo_path = RepoPath::new(path)
        .map_err(|error| TextAdapterError::new(format!("invalid config path: {error}")))?;
    let present = candidate
        .files()
        .iter()
        .any(|file| file.path == repo_path && file.presence == CandidatePresence::Present);
    if !present {
        return Ok(None);
    }
    candidate
        .read(&repo_path)
        .map(|content| Some(content.bytes))
        .map_err(|error| TextAdapterError::new(format!("cannot read {path}: {error}")))
}

fn compile_optional_regex(pattern: &str) -> Result<Option<Regex>, regex::Error> {
    if pattern.is_empty() {
        Ok(None)
    } else {
        Regex::new(pattern).map(Some)
    }
}

fn scan_key_values(
    text: &str,
    pattern: &str,
    rule_prefix: &str,
    facts: &mut Vec<TextFact>,
    limitations: &mut Vec<String>,
    status: &mut UnitStatus,
    budget: &mut BudgetTracker,
) {
    let regex = Regex::new(pattern).expect("built-in configuration regex must compile");
    for captures in regex.captures_iter(text) {
        let Some(complete) = captures.get(0) else {
            continue;
        };
        let key = captures
            .get(1)
            .map(|value| value.as_str())
            .unwrap_or("item");
        let mut details = BTreeMap::new();
        details.insert("key".to_string(), bounded_text(key));
        let fact = fact_from_span(
            text,
            complete.start(),
            complete.end(),
            &format!("{rule_prefix}:{}", key.to_ascii_lowercase()),
            TextFactKind::Configuration,
            details,
        );
        if !push_fact(facts, fact, budget) {
            push_unique(limitations, "fact-budget-exhausted");
            *status = UnitStatus::BudgetExhausted;
            break;
        }
    }
}

fn scan_markers(
    text: &str,
    facts: &mut Vec<TextFact>,
    limitations: &mut Vec<String>,
    status: &mut UnitStatus,
    budget: &mut BudgetTracker,
) {
    let markers: &[(TextFactKind, &str, &[&str])] = &[
        (
            TextFactKind::Framework,
            "framework",
            &["spring", "tokio", "actix", "react", "django", "fastapi"],
        ),
        (
            TextFactKind::TestMarker,
            "test-marker",
            &["#[test]", "@test", "describe(", "test(", "pytest", "junit"],
        ),
        (
            TextFactKind::Endpoint,
            "endpoint",
            &["/api/", "endpoint", "route", "http://", "https://"],
        ),
        (
            TextFactKind::Authorization,
            "authorization",
            &[
                "authorization",
                "permission",
                "bearer",
                "jwt",
                "token",
                "role",
            ],
        ),
        (
            TextFactKind::Storage,
            "storage",
            &[
                "postgres",
                "mysql",
                "sqlite",
                "database",
                "repository",
                "s3",
            ],
        ),
        (
            TextFactKind::Network,
            "network",
            &["http://", "https://", "grpc", "socket", "network"],
        ),
        (
            TextFactKind::Cache,
            "cache",
            &["cache", "redis", "memcached"],
        ),
        (
            TextFactKind::Broker,
            "broker",
            &["kafka", "rabbitmq", "broker", "queue"],
        ),
        (
            TextFactKind::Search,
            "search",
            &["elasticsearch", "opensearch", "search"],
        ),
        (
            TextFactKind::Lifecycle,
            "lifecycle",
            &["startup", "shutdown", "healthcheck", "migration", "cleanup"],
        ),
    ];
    let lower = text.to_ascii_lowercase();
    for (kind, rule_id, candidates) in markers {
        let Some((start, marker)) = candidates
            .iter()
            .filter_map(|marker| lower.find(marker).map(|start| (start, *marker)))
            .min_by_key(|(start, _)| *start)
        else {
            continue;
        };
        let end = start + marker.len();
        let fact = fact_from_span(text, start, end, rule_id, *kind, BTreeMap::new());
        if !push_fact(facts, fact, budget) {
            push_unique(limitations, "fact-budget-exhausted");
            *status = UnitStatus::BudgetExhausted;
            break;
        }
    }
}

fn fact_from_span(
    text: &str,
    start: usize,
    end: usize,
    rule_id: &str,
    kind: TextFactKind,
    details: BTreeMap<String, String>,
) -> TextFact {
    TextFact {
        rule_id: bounded_text(rule_id),
        kind,
        match_text: bounded_text(&text[start.min(text.len())..end.min(text.len()).max(start)]),
        range: range_for_span(text, start, end),
        provenance: TextProvenance::Textual,
        resolved_target: None,
        details,
    }
}

fn range_for_span(text: &str, start: usize, end: usize) -> SourceRange {
    let start = start.min(text.len());
    let end = end.min(text.len()).max(start);
    let (start_line, start_column) = line_column(text, start);
    let (end_line, end_column) = line_column(text, end);
    SourceRange {
        start_line,
        start_column,
        end_line,
        end_column,
        start_byte: start,
        end_byte: end,
    }
}

fn line_column(text: &str, byte: usize) -> (u32, u32) {
    let prefix = &text[..byte.min(text.len())];
    let line = prefix.bytes().filter(|value| *value == b'\n').count() as u32 + 1;
    let line_start = prefix.rfind('\n').map(|index| index + 1).unwrap_or(0);
    let column = prefix[line_start..].chars().count() as u32 + 1;
    (line, column)
}

fn push_fact(facts: &mut Vec<TextFact>, fact: TextFact, budget: &mut BudgetTracker) -> bool {
    if budget.consume(BudgetResource::Facts, 1).is_err() {
        false
    } else {
        facts.push(fact);
        true
    }
}

fn bounded_text(value: &str) -> String {
    value
        .chars()
        .filter(|character| !character.is_control() || *character == '\t')
        .take(1_000)
        .collect::<String>()
        .trim()
        .to_string()
}

fn push_unique(values: &mut Vec<String>, value: &str) {
    if !values.iter().any(|existing| existing == value) {
        values.push(value.to_string());
    }
}
