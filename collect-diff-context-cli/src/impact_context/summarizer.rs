use crate::impact_context::contracts::{Confidence, DomainSummary, SummaryKind};
use crate::impact_context::normalizer::{stable_id, NormalizedFact, NormalizedUnitFacts};
use std::collections::BTreeMap;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct TestSelectionHint {
    pub rule_id: &'static str,
    pub confidence: &'static str,
    pub test_kind: &'static str,
    pub environment_dependency: &'static str,
    pub hint: &'static str,
}

pub fn summarize_unit(unit: &NormalizedUnitFacts, source: Option<&str>) -> Vec<DomainSummary> {
    let mut summaries = BTreeMap::new();
    for symbol in &unit.changed_symbols {
        if !symbol
            .visibility
            .as_deref()
            .is_some_and(|visibility| visibility.starts_with("pub"))
        {
            continue;
        }
        insert_summary(
            &mut summaries,
            make_summary(
                SummaryKind::InterfaceChange,
                &unit.path,
                Some(&symbol.symbol_id),
                symbol.confidence,
                format!("Public {} {} changed.", symbol.kind, symbol.name),
                vec![symbol.symbol_id.clone()],
                "public-interface",
            ),
        );
    }

    for fact in &unit.facts {
        if fact.kind == "import" {
            insert_summary(
                &mut summaries,
                make_fact_summary(
                    SummaryKind::DependencyChange,
                    fact,
                    format!("Import changed: {}.", fact.text),
                ),
            );
            continue;
        }
        let Some(kind) = text_summary_kind(&fact.kind) else {
            continue;
        };
        let message = match kind {
            SummaryKind::TextQueryMatch => format!(
                "Configured query {} matched {} at line {}: {}.",
                fact.rule_id, fact.path, fact.range.start_line, fact.text
            ),
            SummaryKind::TestSelection => format!(
                "Configured test selection rule {} matched {}: test kind {}, environment dependency {}, hint {}.",
                fact.rule_id,
                fact.path,
                fact.details
                    .get("test_kind")
                    .map(String::as_str)
                    .unwrap_or("unknown"),
                fact.details
                    .get("environment_dependency")
                    .map(String::as_str)
                    .unwrap_or("unknown"),
                fact.text
            ),
            _ => format!(
                "{} evidence changed at line {}: {}.",
                fact.kind, fact.range.start_line, fact.text
            ),
        };
        let summary = if kind == SummaryKind::TestSelection {
            make_summary(
                kind,
                &fact.path,
                None,
                fact.details
                    .get("confidence")
                    .map(String::as_str)
                    .map(confidence_from_text)
                    .unwrap_or(fact.confidence),
                message,
                vec![fact.fact_id.clone()],
                &fact.rule_id,
            )
        } else {
            make_fact_summary(kind, fact, message)
        };
        insert_summary(&mut summaries, summary);
    }

    if is_test_like_path(&unit.path) {
        if let Some(source) = source {
            let selection = classify_test_hint(&unit.path, source);
            let mut evidence = unit
                .facts
                .iter()
                .filter(|fact| fact.kind == "text:test-marker" || fact.kind == "text:test-hint")
                .map(|fact| fact.fact_id.clone())
                .collect::<Vec<_>>();
            if evidence.is_empty() {
                evidence.extend(
                    unit.changed_symbols
                        .iter()
                        .map(|symbol| symbol.symbol_id.clone()),
                );
            }
            evidence.sort();
            evidence.dedup();
            insert_summary(
                &mut summaries,
                make_summary(
                    SummaryKind::TestSelection,
                    &unit.path,
                    None,
                    confidence_from_text(selection.confidence),
                    format!(
                        "Test selection {} indicates {} with environment {}.",
                        selection.rule_id, selection.test_kind, selection.environment_dependency
                    ),
                    evidence,
                    selection.rule_id,
                ),
            );
        }
    }

    summaries.into_values().collect()
}

fn text_summary_kind(kind: &str) -> Option<SummaryKind> {
    match kind {
        "text:configured-query" => Some(SummaryKind::TextQueryMatch),
        "text:test-hint" => Some(SummaryKind::TestSelection),
        "text:framework" => Some(SummaryKind::FrameworkEffect),
        "text:configuration" => Some(SummaryKind::ConfigurationEffect),
        "text:authorization" => Some(SummaryKind::AuthorizationEffect),
        "text:storage" | "text:cache" | "text:search" => Some(SummaryKind::StorageEffect),
        "text:network" | "text:broker" | "text:endpoint" => Some(SummaryKind::NetworkEffect),
        "text:lifecycle" => Some(SummaryKind::LifecycleEffect),
        _ => None,
    }
}

fn make_fact_summary(kind: SummaryKind, fact: &NormalizedFact, message: String) -> DomainSummary {
    make_summary(
        kind,
        &fact.path,
        None,
        fact.confidence,
        message,
        vec![fact.fact_id.clone()],
        &fact.rule_id,
    )
}

#[allow(clippy::too_many_arguments)]
fn make_summary(
    kind: SummaryKind,
    path: &str,
    symbol_id: Option<&str>,
    confidence: Confidence,
    message: String,
    mut evidence_fact_ids: Vec<String>,
    rule_id: &str,
) -> DomainSummary {
    evidence_fact_ids.sort();
    evidence_fact_ids.dedup();
    let summary_id = stable_id(
        "impact-summary/v1",
        &[
            summary_kind_name(kind),
            path,
            symbol_id.unwrap_or(""),
            rule_id,
            evidence_fact_ids.first().map(String::as_str).unwrap_or(""),
        ],
    );
    DomainSummary {
        summary_id,
        summary_kind: kind,
        path: path.to_string(),
        symbol_id: symbol_id.map(str::to_string),
        confidence,
        message: bounded_message(message),
        evidence_fact_ids,
    }
}

fn insert_summary(summaries: &mut BTreeMap<String, DomainSummary>, summary: DomainSummary) {
    summaries
        .entry(summary.summary_id.clone())
        .or_insert(summary);
}

fn confidence_from_text(value: &str) -> Confidence {
    match value {
        "high" => Confidence::High,
        "medium" => Confidence::Medium,
        _ => Confidence::Low,
    }
}

fn bounded_message(message: String) -> String {
    message
        .chars()
        .filter(|character| !character.is_control())
        .take(1_000)
        .collect::<String>()
}

fn summary_kind_name(kind: SummaryKind) -> &'static str {
    match kind {
        SummaryKind::DependencyChange => "dependency-change",
        SummaryKind::InterfaceChange => "interface-change",
        SummaryKind::TextQueryMatch => "text-query-match",
        SummaryKind::TestSelection => "test-selection",
        SummaryKind::FrameworkEffect => "framework-effect",
        SummaryKind::ConfigurationEffect => "configuration-effect",
        SummaryKind::AuthorizationEffect => "authorization-effect",
        SummaryKind::StorageEffect => "storage-effect",
        SummaryKind::NetworkEffect => "network-effect",
        SummaryKind::LifecycleEffect => "lifecycle-effect",
    }
}

pub(crate) fn is_test_like_path(path: &str) -> bool {
    let lower = path.to_ascii_lowercase();
    lower.starts_with("test/")
        || lower.starts_with("tests/")
        || lower.starts_with("e2e/")
        || lower.starts_with("cypress/")
        || lower.starts_with("playwright/")
        || lower.starts_with("src/test/")
        || lower.contains("/test/")
        || lower.contains("/tests/")
        || lower.contains("/e2e/")
        || lower.contains("/cypress/")
        || lower.contains("/playwright/")
        || lower.contains("/__tests__/")
        || lower.contains("/src/test/")
        || lower.contains("/src/it/")
        || lower.contains("/src/integrationtest/")
        || lower.contains("/src/integration-test/")
        || lower.ends_with("test.java")
        || lower.ends_with("tests.java")
        || lower.ends_with("it.java")
        || lower.ends_with("itcase.java")
        || lower.ends_with("integrationtest.java")
        || lower.ends_with("spec.java")
        || lower.ends_with("test.kt")
        || lower.ends_with("tests.kt")
        || lower.ends_with("it.kt")
        || lower.ends_with("itcase.kt")
        || lower.ends_with("integrationtest.kt")
        || lower.ends_with("spec.kt")
        || lower.ends_with("test.groovy")
        || lower.ends_with("spec.groovy")
        || lower.ends_with("it.groovy")
        || lower.ends_with("integrationtest.groovy")
        || lower.ends_with("test.scala")
        || lower.ends_with("spec.scala")
        || lower.ends_with("it.scala")
        || lower.ends_with("integrationtest.scala")
        || lower.ends_with("test.ts")
        || lower.ends_with("spec.ts")
        || lower.ends_with("e2e.ts")
        || lower.ends_with("cy.ts")
        || lower.ends_with("test.tsx")
        || lower.ends_with("spec.tsx")
        || lower.ends_with("e2e.tsx")
        || lower.ends_with("cy.tsx")
        || lower.ends_with("test.js")
        || lower.ends_with("spec.js")
        || lower.ends_with("e2e.js")
        || lower.ends_with("cy.js")
        || lower.ends_with("test.jsx")
        || lower.ends_with("spec.jsx")
        || lower.ends_with("e2e.jsx")
        || lower.ends_with("cy.jsx")
        || lower.ends_with("_test.go")
        || lower.ends_with("_test.py")
        || lower.ends_with(".spec.py")
        || lower.starts_with("test_")
        || lower.contains("/test_")
}

fn contains_any(haystack: &str, needles: &[&str]) -> bool {
    needles.iter().any(|needle| haystack.contains(needle))
}

fn path_indicates_jvm_integration(lower_path: &str) -> bool {
    lower_path.contains("/src/it/")
        || lower_path.contains("/src/integrationtest/")
        || lower_path.contains("/src/integration-test/")
        || lower_path.ends_with("it.java")
        || lower_path.ends_with("itcase.java")
        || lower_path.ends_with("integrationtest.java")
        || lower_path.ends_with("it.kt")
        || lower_path.ends_with("itcase.kt")
        || lower_path.ends_with("integrationtest.kt")
        || lower_path.ends_with("it.groovy")
        || lower_path.ends_with("integrationtest.groovy")
        || lower_path.ends_with("it.scala")
        || lower_path.ends_with("integrationtest.scala")
}

fn test_hint(
    rule_id: &'static str,
    confidence: &'static str,
    test_kind: &'static str,
    environment_dependency: &'static str,
    hint: &'static str,
) -> TestSelectionHint {
    TestSelectionHint {
        rule_id,
        confidence,
        test_kind,
        environment_dependency,
        hint,
    }
}

pub(crate) fn classify_test_hint(path: &str, content: &str) -> TestSelectionHint {
    let lower_path = path.to_ascii_lowercase();
    let lower_content = content.to_ascii_lowercase();

    if contains_any(
        &lower_content,
        &[
            "org.testcontainers",
            "@testcontainers",
            "@container",
            "testcontainers-go",
        ],
    ) {
        test_hint(
            "testcontainers",
            "high",
            "container-integration",
            "docker-or-testcontainers",
            "Requires Docker/Testcontainers; do not treat failure in a sandbox as a pure code failure without environment evidence.",
        )
    } else if contains_any(
        &lower_content,
        &[
            "dockercomposecontainer",
            "docker-compose",
            "docker compose",
            "compose.yml",
            "compose.yaml",
        ],
    ) {
        test_hint(
            "docker-compose-test",
            "high",
            "compose-backed-integration",
            "docker-compose-runtime",
            "Uses Docker Compose or compose-backed services; verify in an environment with Docker and required service images.",
        )
    } else if contains_any(
        &lower_content,
        &[
            "wiremockserver",
            "wiremockextension",
            "@autoconfigurewiremock",
            "com.github.tomakehurst.wiremock",
            "wiremock.org",
        ],
    ) {
        test_hint(
            "wiremock-test",
            "high",
            "http-stub-integration",
            "wiremock-runtime",
            "Uses WireMock HTTP stubs; sandbox failures may reflect port/runtime setup rather than the changed code.",
        )
    } else if contains_any(
        &lower_content,
        &["org.mockserver", "mockservercontainer", "clientandserver"],
    ) {
        test_hint(
            "mockserver-test",
            "high",
            "http-stub-integration",
            "mockserver-runtime",
            "Uses MockServer or its container runtime; verify with the required local or CI service setup.",
        )
    } else if contains_any(
        &lower_content,
        &[
            "@autoconfigurestubrunner",
            "stubrunner",
            "spring-cloud-contract",
            "org.springframework.cloud.contract",
        ],
    ) {
        test_hint(
            "spring-cloud-contract",
            "high",
            "contract-integration",
            "spring-cloud-contract-runtime",
            "Uses Spring Cloud Contract or Stub Runner; may require generated stubs, broker settings, or CI contract artifacts.",
        )
    } else if contains_any(
        &lower_content,
        &[
            "jdbc:",
            "r2dbc:",
            "spring.datasource.url",
            "datasource.url",
            "postgresql",
            "mysql",
            "mariadb",
            "oracle.jdbc",
            "mongodb://",
            "redis://",
            "spring.redis",
            "spring.data.redis",
            "kafka.bootstrap",
            "bootstrap.servers",
            "spring.kafka",
            "elasticsearch",
            "opensearch",
            "rabbitmq",
            "amqp://",
            "localstack",
            "minio",
        ],
    ) {
        test_hint(
            "external-service-config",
            "high",
            "service-backed-integration",
            "database-cache-broker-or-search-service",
            "References database, cache, broker, search, or object-storage service configuration; run with the expected local profile or CI services.",
        )
    } else if contains_any(
        &lower_content,
        &["@quarkustest", "@quarkusintegrationtest", "io.quarkus.test"],
    ) {
        test_hint(
            "quarkus-test-context",
            "high",
            "quarkus-integration",
            "quarkus-test-runtime",
            "Loads a Quarkus test context; may require Quarkus profiles, dev services, containers, or CI runtime support.",
        )
    } else if contains_any(&lower_content, &["@micronauttest", "io.micronaut.test"]) {
        test_hint(
            "micronaut-test-context",
            "high",
            "micronaut-integration",
            "micronaut-test-runtime",
            "Loads a Micronaut test context; may require application context configuration or service-backed test resources.",
        )
    } else if content.contains("@SpringBootTest") {
        test_hint(
            "spring-boot-context",
            "high",
            "spring-boot-integration",
            "spring-context",
            "Loads a Spring Boot application context; may require local profiles, DB, middleware, or CI-provided services.",
        )
    } else if content.contains("@DataJpaTest")
        || content.contains("@JdbcTest")
        || content.contains("@JooqTest")
        || content.contains("@MybatisTest")
    {
        test_hint(
            "spring-data-slice",
            "high",
            "data-slice-integration",
            "database-or-spring-test-slice",
            "Loads a data test slice; may require an embedded or configured database.",
        )
    } else if content.contains("@WebMvcTest") || content.contains("@AutoConfigureMockMvc") {
        test_hint(
            "spring-web-slice",
            "high",
            "spring-web-slice",
            "spring-test-context",
            "Loads a Spring web test slice; usually narrower than full integration but not a pure unit test.",
        )
    } else if contains_any(
        &lower_content,
        &[
            "@activeprofiles",
            "spring_profiles_active",
            "quarkus.test.profile",
            "micronaut.environments",
        ],
    ) {
        test_hint(
            "jvm-test-profile",
            "high",
            "profile-backed-test",
            "maven-gradle-or-framework-profile",
            "Selects framework test profiles or environments; use the matching Maven/Gradle profile or CI profile configuration.",
        )
    } else if contains_any(
        &lower_content,
        &[
            "@tag(\"integration\")",
            "@tag(\"e2e\")",
            "@tag(\"contract\")",
            "@tag(\"slow\")",
            "@category(integrationtest",
            "@category(e2etest",
        ],
    ) {
        test_hint(
            "junit-integration-tag",
            "high",
            "tagged-jvm-integration",
            "junit-tag-or-category-selection",
            "Uses JUnit integration/e2e/contract tags; run with the tag expression and environment expected by the project.",
        )
    } else if path_indicates_jvm_integration(&lower_path) {
        test_hint(
            "jvm-integration-naming",
            "medium",
            "jvm-integration-by-convention",
            "maven-failsafe-or-gradle-integration-profile",
            "Path or class name follows common JVM integration-test conventions such as *IT or src/integrationTest; run the project integration-test profile if available.",
        )
    } else if contains_any(
        &lower_content,
        &[
            "pytest.mark.integration",
            "pytest.mark.e2e",
            "pytest.mark.contract",
            "pytest.mark.system",
            "pytest.mark.django_db",
            "pytest.mark.db",
            "pytest.mark.redis",
            "pytest.mark.kafka",
            "pytest.mark.elasticsearch",
        ],
    ) {
        test_hint(
            "pytest-env-marker",
            "high",
            "pytest-marked-integration",
            "pytest-marker-or-service-runtime",
            "Uses pytest markers that usually select integration/e2e/database/service tests; run with the matching marker and required services.",
        )
    } else if contains_any(&lower_content, &["@playwright/test", "playwright/test"])
        || lower_path.ends_with(".pw.ts")
        || lower_path.ends_with(".pw.js")
    {
        test_hint(
            "playwright-e2e",
            "high",
            "browser-e2e",
            "browser-runtime-and-app-server",
            "Uses Playwright; requires browser runtime and usually a running app server or configured webServer.",
        )
    } else if lower_path.contains("/cypress/")
        || lower_path.ends_with(".cy.ts")
        || lower_path.ends_with(".cy.tsx")
        || lower_path.ends_with(".cy.js")
        || lower_path.ends_with(".cy.jsx")
        || contains_any(&lower_content, &["cy.visit(", "cypress."])
    {
        test_hint(
            "cypress-e2e",
            "high",
            "browser-e2e",
            "browser-runtime-and-app-server",
            "Uses Cypress; requires browser runtime and usually a running app server.",
        )
    } else if (lower_path.contains("/e2e/")
        || lower_path.contains(".e2e.")
        || lower_path.contains("/integration/"))
        && contains_any(&lower_content, &["vitest", "jest", "describe(", "test("])
    {
        test_hint(
            "node-e2e-or-integration",
            "medium",
            "node-e2e-or-integration",
            "node-runtime-and-possibly-app-server",
            "Path/content follows common Node e2e or integration-test conventions; verify with the project test script and required runtime services.",
        )
    } else if contains_any(
        &lower_content,
        &[
            "//go:build integration",
            "//go:build e2e",
            "//go:build docker",
            "// +build integration",
            "// +build e2e",
            "// +build docker",
        ],
    ) {
        test_hint(
            "go-integration-build-tag",
            "high",
            "go-tagged-integration",
            "go-build-tags-and-service-runtime",
            "Uses Go integration/e2e/docker build tags; run go test with the matching tags and required services.",
        )
    } else if lower_path.ends_with("_test.go")
        && (lower_path.contains("integration") || lower_path.contains("/e2e/"))
    {
        test_hint(
            "go-integration-naming",
            "medium",
            "go-integration-by-convention",
            "go-test-selection-or-service-runtime",
            "Go test path suggests integration coverage; check project docs for tags, env vars, or service dependencies.",
        )
    } else if lower_content.contains("#[ignore]") {
        test_hint(
            "rust-ignored-test",
            "medium",
            "rust-ignored-or-slow-test",
            "cargo-test-ignored-selection",
            "Rust ignored tests are not run by default and often need explicit `cargo test -- --ignored` plus external setup.",
        )
    } else if lower_path.ends_with(".rs")
        && (lower_path.starts_with("tests/")
            || lower_path.contains("/tests/")
            || lower_path.contains("/integration/"))
    {
        test_hint(
            "rust-integration-path",
            "low",
            "rust-integration-by-convention",
            "cargo-test-selection-or-project-specific-runtime",
            "Rust test path follows Cargo integration-test layout; treat as a planning hint and verify whether external setup is required.",
        )
    } else {
        test_hint(
            "no-known-env-heavy-marker",
            "low",
            "unit-or-unknown",
            "not-proven-isolated",
            "No known env-heavy marker detected; this is not proof of unit-test isolation. Prefer the narrowest focused test command for this file.",
        )
    }
}
