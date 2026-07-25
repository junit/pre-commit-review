# Controlled Static Analysis Competitive Research

## Status and Scope

Research date: 2026-07-25.

This note compares the current `pre-commit-review` controlled static-analysis
design with Semgrep, GitHub CodeQL, SonarQube Server and SonarQube for IDE,
Trunk Check, and MegaLinter. `pre-commit` and reviewdog are included as adjacent
tools because they cover local hook execution and diff-aware result delivery.

The comparison uses only first-party product documentation or official source
repositories. It evaluates execution model, analyzer coverage, changed-code
behavior, SARIF and developer integrations, caching and parallelism, tool
provisioning, authorization boundaries, and failure semantics.

The local design under review is documented in:

- [Static Analysis Evidence Integration](static-analysis-evidence.md)
- [Controlled Static Analysis Execution](static-analysis-execution.md)
- [Rust Multi-Analyzer Orchestration Design](superpowers/specs/2026-07-25-rust-multi-analyzer-orchestration-design.md)

## Executive Conclusion

The current design is high quality for a narrow and real problem: allowing an
AI-assisted review workflow to consume or execute static analyzers without
silently widening the reviewed Git candidate, trusting repository commands by
default, or treating unavailable analysis as successful verification.

It is not a general SAST product and should not claim overall superiority over
Semgrep, CodeQL, or SonarQube. Those products are substantially ahead in rule
quality and coverage, semantic analysis, managed policy, IDE and pull-request
experience, caching, automatic provisioning, and operational deployment.
Trunk and MegaLinter are ahead in zero- or low-friction multi-tool adoption.

The defensible differentiated claim is narrower:

> `pre-commit-review` provides a stronger explicit authorization and evidence
> provenance boundary for agent-triggered local analysis than the compared
> general-purpose products document as part of their normal execution model.

The project should not retain Python as a long-term selectable product runtime.
Python is useful only as a temporary parity oracle while `collect` and `run`
move to Rust. After Rust parity and release-platform validation pass, remove the
Python implementation and preserve compatibility at the Shell and JSON
interfaces instead of maintaining two security-relevant implementations.

## Comparison Summary

| Dimension | `pre-commit-review` | Semgrep | GitHub CodeQL | SonarQube | Trunk Check | MegaLinter |
|---|---|---|---|---|---|---|
| Primary role | Agent review evidence and controlled analyzer orchestration | SAST/SCA/secrets analyzer and platform | Semantic security analyzer and GitHub code-scanning platform | Central code-quality/security platform plus IDE analysis | Hermetic metalinter and static-analysis manager | Containerized CI metalinter |
| Analyzer coverage | No built-in rules; depends on authorized tools | 35+ SAST languages, with varying analysis depth | Ten listed language groups, deep query-based analysis | 40+ languages advertised, edition-dependent | Broad third-party linter/security plugin catalog | 69 languages and 137 linters in the default image as currently documented |
| Candidate model | Exact staged, unstaged, or branch tracked-file snapshot with scope and content identity | Working tree or CI checkout; full or Git-baseline diff-aware scan | CodeQL database from checkout/build; PR presentation and incremental analysis | Scanner checkout/build; PR new-code comparison against target | Git-aware hold-the-line filtering, changed files/lines | Full repository by default; new/edited files when configured, with project-mode exceptions |
| Multi-tool model | Explicit ordered manifest; serial; shared immutable snapshot | One product engine with multiple rules/products | CodeQL plus separately uploaded third-party SARIF categories | Native analyzers plus imported external issues | Downloads and orchestrates enabled tools | Bundles and runs many linters, parallel by default |
| Provisioning | Never auto-discovers or downloads analyzers | CLI/container install; rules can be local, URL, registry, or automatic | Default setup provisions through GitHub Actions; external CI installs CLI bundle | Scanners can auto-download JRE, engine, and analyzers from server | Hermetically downloads and caches pinned CLI, runtimes, and linters | Linters preinstalled in versioned Docker flavors; plugins and commands are configurable |
| SARIF | Explicit SARIF 2.1.0 ingestion and controlled output normalization | Native SARIF output | Native SARIF and third-party SARIF upload | Imports SARIF external issues | Plugin definitions may normalize tool output as SARIF | Optional aggregate SARIF for SARIF-capable linters only |
| PR / IDE / governance | No first-class PR comments, IDE, or central policy service | PR/MR comments, IDE extensions, AppSec Platform | GitHub alerts/checks, organization security configuration; VS Code tooling is mainly query/model oriented | Strong PR decoration, quality gates/profiles, organization server, connected IDE mode | PR checks, VS Code/Neovim, repository configuration | PR comments/checks; no integrated central policy or IDE analysis layer |
| Performance | Serial MVP; no result cache | Parallel scan jobs; diff-aware scanning | Language matrix parallelism, query/dependency caches, incremental overlay analysis | Analysis cache and some unchanged-file skipping | Daemon background precomputation and persistent cache | Parallel linters by default; optimized Docker flavors |
| Authorization boundary | Manifest, profile, and entrypoint executable exact SHA256; explicit repository-config trust; external rules, plugins, interpreters, and build assets are not yet a pinned execution closure | Trusts installed engine and selected rules/config; optional remote rules, builds, validators, and platform connection | Trusts workflow, CodeQL bundle/action, query packs, build, checkout, and GitHub permissions | Trusts scanner/server analyzers, project/CI configuration, checkout/build, and access token | Trusts repository `trunk.yaml`, imported plugin ref, downloaded runtimes/tools, and custom definitions | Trusts image and repo/remote configuration; supports plugins and arbitrary pre/post commands |
| Failure meaning | `completed`, `partial`, `failed`, per-tool terminal states, `not-run`; failed tools cannot create blocking candidates | Findings and configuration errors affect exit status; timeouts can skip targets | Analysis errors and security findings are distinct workflow/check outcomes | Scanner failure and quality-gate failure are distinct; invalid SARIF may be ignored with logs | Tool success/error codes are configured; PR check can be explicitly skipped | Global/per-linter blocking controls; missing linters and updated sources can separately fail |

## Product Findings

### Semgrep

Semgrep is an analyzer product, not only an orchestrator. Its current support
table advertises more than 35 Semgrep Code languages, with cross-file analysis
for the strongest-supported languages and lower analysis maturity for others.
It also includes separate SCA and secrets products. This is a fundamentally
larger detection and rule-maintenance surface than this project intends to
build. [Supported languages](https://semgrep.dev/docs/supported-languages)

Its CI model supports push, pull-request, merge-request, scheduled, and manual
events. A full scan reports the full codebase; a diff-aware scan compares the
candidate before and after a Git baseline and reports newly introduced
findings. That is a strong practical new-code workflow, but the documented
contract is not an exact external scope fingerprint or a cryptographic binding
between an agent review manifest and every accepted result.
[Semgrep CI overview and scan scope](https://semgrep.dev/docs/semgrep-ci/overview)

The CLI can fetch automatic project-tailored rules, load local files or URLs,
emit SARIF, run scan workers in parallel, fail on findings with `--error`, and
fail on configuration warnings with `--strict`. It also documents explicit
security-sensitive flags for repository builds and untrusted validators. These
are useful controls, but they leave rule selection and tool execution inside
the ordinary CLI trust model rather than requiring a separate hash-pinned
authorization chain.
[Semgrep CLI reference](https://semgrep.dev/docs/cli-reference)

Semgrep has official VS Code and IntelliJ extensions, a pre-commit integration,
PR/MR comments, and centralized finding triage in Semgrep AppSec Platform.
These are mature developer and governance capabilities absent from the current
project. Semgrep also states that CI scans run in the CI environment and code is
not sent to Semgrep unless code access is explicitly granted, while finding
metadata is sent to the platform.
[IDE extensions](https://semgrep.dev/docs/extensions/overview),
[CI data handling](https://semgrep.dev/docs/semgrep-ci/overview)

Assessment: the project does not surpass Semgrep as SAST. It can surpass the
documented Semgrep execution path only in exact candidate binding, external
profile and entrypoint authorization, and preservation of analysis-unavailable
states for an agent review reducer. It does not yet pin Semgrep rule/config
assets as a complete execution closure.

### GitHub CodeQL and Code Scanning

CodeQL creates a database representing the codebase and runs queries over it.
It supports C/C++, C#, Go, Java/Kotlin, JavaScript/TypeScript, Python, Ruby,
Rust, Swift, and GitHub Actions workflows. GitHub explicitly warns that
unsupported languages can produce no alerts and incomplete analysis. Default
setup chooses languages, query suite, and scan events automatically; advanced
setup exposes a workflow; external CI can run the CLI and upload results.
[Code scanning with CodeQL](https://docs.github.com/en/code-security/code-scanning/introduction-to-code-scanning/about-code-scanning-with-codeql)

CodeQL query packs include transitive dependencies and a compilation cache,
which improves performance and fixes the effective query dependency set until
the pack or CLI is upgraded. Advanced workflows can use a language matrix so
language analyses run in parallel. GitHub also documents incremental overlay
analysis and states that default setup and `codeql-action` handle incremental
analysis automatically.
[Workflow configuration](https://docs.github.com/en/code-security/reference/code-scanning/workflow-configuration-options),
[Incremental analysis](https://docs.github.com/en/code-security/how-tos/find-and-fix-code-vulnerabilities/scan-from-the-command-line/incremental-analysis)

Code scanning accepts SARIF 2.1.0 from third-party tools. Multiple result sets
for one commit are separated by categories; otherwise a later upload replaces
the earlier set. Alerts from multiple tools are displayed together. Pull
request alerts appear as checks and annotations, and an alert appears in a PR
only when all lines identified by the alert exist in the PR diff.
[SARIF support](https://docs.github.com/en/code-security/reference/code-scanning/sarif-files/sarif-support),
[External CI and result categories](https://docs.github.com/en/code-security/how-tos/find-and-fix-code-vulnerabilities/integrate-with-existing-tools/use-with-existing-ci-system),
[Code scanning alerts](https://docs.github.com/en/code-security/code-scanning/managing-code-scanning-alerts/about-code-scanning-alerts)

GitHub can apply default setup across an organization and configure eligible
repositories centrally. For an external CI system, each server must install the
CodeQL bundle, prepare dependencies and builds, and use a token or GitHub App
with `security_events` write permission. This is strong operational governance,
but it is a different security boundary from exact executable and profile
hashes supplied by the authorizing review context.
[Code scanning at scale](https://docs.github.com/en/code-security/how-tos/secure-at-scale/configure-organization-security/configure-specific-tools/code-scanning-at-scale),
[External CI setup](https://docs.github.com/en/code-security/how-tos/find-and-fix-code-vulnerabilities/integrate-with-existing-tools/use-with-existing-ci-system)

The code-scanning results check fails for `error`, `critical`, or `high`
findings and succeeds for lower severities, subject to configuration and merge
protection. Analysis workflow failures remain observable separately from alert
severity. This is a clear CI policy, but not the same as an aggregate artifact
that records which other tools succeeded, failed, timed out, or were not run.
[PR alert triage and check failures](https://docs.github.com/en/code-security/how-tos/manage-security-alerts/manage-code-scanning-alerts/triage-alerts-in-pull-requests)

Assessment: CodeQL is ahead in deep semantic security analysis, incremental
query execution, caching, GitHub integration, and organization rollout. The
project's advantage is a local, tool-neutral, fail-closed authorization and
scope-provenance layer for agent-triggered execution.

### SonarQube Server and SonarQube for IDE

SonarQube Server is a centralized code quality and security platform. Its
product overview advertises analysis for more than 40 languages, frameworks,
and infrastructure-as-code platforms, with exact availability depending on
edition. Its core governance primitives are centrally managed quality profiles
(rules), new-code definitions, and quality gates.
[SonarQube Server overview](https://docs.sonarsource.com/sonarqube-server),
[supported languages](https://docs.sonarsource.com/sonarqube-server/analyzing-source-code/languages/overview)

For pull requests, the scanner runs in CI against a checkout containing the
source branch, target branch, and valid Git metadata. SonarQube defines new code
as the code changed relative to the target branch and reports only issues on
new code. It can decorate the pull request with the quality-gate result, and a
quality gate can block merging or fail the CI pipeline.
[Pull-request analysis](https://docs.sonarsource.com/sonarqube-server/analyzing-source-code/pull-request-analysis/setting-up-the-pull-request-analysis),
[new-code model](https://docs.sonarsource.com/sonarqube-server/user-guide/about-new-code),
[quality gates](https://docs.sonarsource.com/sonarqube-server/quality-standards-administration/managing-quality-gates/introduction-to-quality-gates)

SonarQube has an analysis cache enabled by default and supports unchanged-file
skipping for some analyzers, including Java and Kotlin. SonarScanner can also
auto-provision the required JRE from SonarQube; the scanner engine and analyzers
are downloaded at analysis time in the standard server model. These features
reduce adoption and repeat-analysis cost, but add networked server and
provisioning trust that the current project's offline, externally pinned model
deliberately avoids.
[Incremental analysis controls](https://docs.sonarsource.com/sonarqube-server/analyzing-source-code/managing-incremental-analysis),
[JRE auto-provisioning](https://docs.sonarsource.com/sonarqube-server/analyzing-source-code/scanners/scanner-environment/managing-jre-auto-provisioning)

Connected mode synchronizes server quality profiles, analyzer settings,
accepted/false-positive issue state, branch awareness, quality-gate changes,
and new issues into the IDE. SonarQube also imports SARIF external issues, but
the third-party rules remain managed by the producing tool; malformed reports
with missing mandatory fields are ignored and noted in scanner logs.
[Connected mode](https://docs.sonarsource.com/sonarqube-for-vs-code/connect-your-ide/connected-mode),
[SARIF import](https://docs.sonarsource.com/sonarqube-server/analyzing-source-code/importing-external-issues/importing-issues-from-sarif-reports)

Assessment: SonarQube is substantially ahead in central governance, quality
metrics, IDE consistency, historical state, and enterprise deployment. The
current project is stronger only when the required property is an auditable
one-shot authorization for a precise local Git candidate and honest propagation
of unavailable evidence into an agent review.

### Trunk Check

Trunk describes Code Quality as a C++ CLI and daemon that orchestrate downloads,
installation, and execution of third-party analysis tools. It manages tool and
runtime versions hermetically, caches them, and isolates them from host runtime
versions. Its official plugin repository contains a broad catalog of linters,
formatters, and security tools and imports the default plugin definitions at a
versioned Git ref.
[Trunk Code Quality overview](https://docs.trunk.io/code-quality/overview),
[official plugins repository](https://github.com/trunk-io/plugins)

Its defining incremental feature is hold-the-line. Trunk filters to modified
files or lines using Git and states that line-level hold-the-line works even for
linters that do not natively support line-level execution. The daemon monitors
file changes, performs background work, and caches results for later checks.
In CI, Trunk caches its CLI, tools, formatters, and lint results under
`~/.cache/trunk` and can seed that cache on ephemeral runners.
[Hold-the-line and daemon](https://docs.trunk.io/code-quality/overview),
[CI and caching](https://docs.trunk.io/code-quality/overview/prevent-new-issues)

Trunk supports PR checks and VS Code/Neovim integrations. The VS Code extension
can suggest applicable tools and can initialize a local single-player
configuration; a shared repository configuration pins the Trunk CLI, runtimes,
and linters for reproducible team and CI execution.
[VS Code integration](https://docs.trunk.io/code-quality/overview/ide-integration/vscode),
[configuration](https://docs.trunk.io/code-quality/overview/getting-started/configuration)

The normal trust boundary is repository `trunk.yaml`, imported plugin
definitions, downloaded tool packages, and any custom linter overrides. The
plugin schema defines commands and success/error codes, and official plugin
definitions may normalize tool output to SARIF. The cited official docs do not
establish a mandatory external SHA256 authorization chain for the complete
configuration and executable set, or a shared content-addressed review snapshot.

Assessment: Trunk is ahead in developer ergonomics, tool discovery,
installation, caching, background execution, and changed-line adoption. The
project is ahead only for restrictive Agent execution authorization, snapshot
identity, and explicit partial-evidence semantics. Those strengths come with a
material usability and latency cost.

### MegaLinter

MegaLinter is a CI-oriented metalinter distributed as Docker images. Its current
documentation advertises 69 languages and a default image containing 137
linters, plus smaller language- or domain-specific flavors. Linters are already
installed in the images, making broad coverage easy to adopt at the cost of
large images and trusting the bundled toolchain.
[MegaLinter overview](https://megalinter.io/latest/),
[flavors](https://megalinter.io/latest/flavors/)

MegaLinter validates the whole repository by default. Setting
`VALIDATE_ALL_CODEBASE=false` limits file selection to new or edited files, but
the documentation explicitly notes that repository/project-mode linters may
not honor file-list filters because they are invoked at project root without a
file list. This is a practical limitation absent from the current project's
tracked-file snapshot model, although the current snapshot model has its own
compatibility problem with analyzers that require generated files, ignored
dependencies, or full repository metadata.
[Configuration and CLI lint modes](https://megalinter.io/latest/configuration/)

MegaLinter runs linters in parallel by default, grouping tools that might
modify the same sources to reduce lock conflicts. Its SARIF reporter is
disabled by default and aggregates only linters that support SARIF. It also has
GitHub PR comments and per-linter GitHub checks.
[Configuration](https://megalinter.io/latest/configuration/),
[SARIF reporter](https://megalinter.io/latest/reporters/SarifReporter/),
[GitHub reporter](https://megalinter.io/latest/reporters/GitHubCommentReporter/),
[GitHub installation](https://megalinter.io/latest/install-github/)

Failure behavior is highly configurable: findings can be globally
non-blocking, selected linters can be non-blocking, and missing linters or
updated sources can be configured to fail. MegaLinter also allows remote
configuration and rules, plugins, and arbitrary pre/post commands. It hides a
large default set of sensitive environment variables from linter child
processes, while explicitly documenting that this is not full sandboxing and
that files, command arguments, network services, and unmatched variables remain
available attack channels.
[Configuration and security boundary](https://megalinter.io/latest/configuration/)

Assessment: MegaLinter is ahead in coverage, installation convenience,
parallel throughput, CI integrations, and configurability. The project is
stronger in explicit executable authorization, repository-command avoidance,
candidate immutability, and machine-readable distinction between accepted,
failed, invalidated, and not-run evidence.

## Adjacent Tools

### `pre-commit`

`pre-commit` is a multi-language hook package manager. It downloads and builds
hook environments, reuses installed environments, normally passes changed files,
and runs all hooks unless `fail_fast` is enabled. Hook repositories are pinned
by a configured revision or tag, and hooks can execute arbitrary entry points.
It provides much better ecosystem ergonomics than this project but does not
provide the same exact executable hashing, candidate snapshot, SARIF provenance,
or agent evidence contract.
[Official documentation](https://pre-commit.com/)

### reviewdog

reviewdog is an analysis-result adapter and reporter rather than an analyzer
manager. It accepts SARIF 2.1.0 and other formats, filters diagnostics through a
Git diff to identify newly introduced findings, and posts GitHub, GitLab,
Gerrit, or Bitbucket annotations and review comments. It demonstrates a mature
delivery layer that this project could integrate with later, but it does not
authorize or isolate the analyzer that produced the diagnostics.
[Official repository](https://github.com/reviewdog/reviewdog)

## Where the Current Design Is Stronger

The following claims are supported by the compared official execution models
and are narrow enough to be credible:

1. **Exact entrypoint authorization chain.** One externally supplied manifest
   hash pins ordered profiles, and profiles pin exact entrypoint executable
   bytes before any tool starts. The compared products generally pin versions,
   Git refs, images, query packs, or centrally managed configuration rather
   than requiring this external byte-level entrypoint authorization. This claim
   applies only to self-contained analyzers; it is not yet a complete execution
   closure for tools that load mutable rules, plugins, interpreters, query
   packs, generated inputs, or build dependencies.
2. **Candidate identity.** Staged, unstaged, and branch candidates have explicit
   semantics and one shared tracked-file snapshot identity. Accepted evidence
   is bound to the same review scope and revalidated before release.
3. **Agent-safe default posture.** Repository commands, package scripts,
   plugins, profiles, and analyzers are never discovered and executed merely
   because the repository contains them.
4. **Evidence honesty.** Timeout, invalid output, output overflow, tool failure,
   snapshot mutation, budget exhaustion, and never-started tools remain
   distinct. Failed evidence cannot independently become a blocking finding.
5. **Review integration.** Static findings are mapped to manifest units and
   added lines, retain source provenance, and still pass independent finding
   verification rather than automatically becoming a review verdict.
6. **Documented limits.** The design states that read-only permissions and proxy
   poisoning are not an OS sandbox or guaranteed network isolation. This avoids
   claiming protections the implementation does not provide.

These strengths matter most for AI agents because the caller may otherwise
confuse permission to inspect a change with permission to execute repository
code, download tools, trust mutable rules, or treat an incomplete scan as clean.

## Where the Project Must Not Claim Leadership

The project is not ahead in these dimensions:

1. **Detection quality or language coverage.** It owns no analyzer or rule
   corpus. Coverage and precision come entirely from configured third parties.
2. **Deep semantic analysis.** It does not replace CodeQL databases and queries,
   Semgrep interfile analysis, or Sonar analyzers.
3. **Low-friction adoption.** Exact path and multiple SHA256 requirements are
   operationally expensive compared with default setup, versioned tool
   manifests, container flavors, or automatic tool discovery.
4. **Incremental performance.** There is no analyzer result cache, background
   daemon, overlay database, or unchanged-file cache. Serial orchestration will
   be slower on large polyglot repositories.
5. **Developer workflow.** There are no first-class IDE diagnostics, PR inline
   comments, autofix suggestions, baseline triage UI, central finding history,
   or organization dashboard.
6. **Central governance.** There is no server-managed rule policy, quality gate,
   fleet rollout, audit UI, alert ownership, suppression workflow, or historical
   trend reporting.
7. **Tool lifecycle.** The project does not install, update, select, or validate
   analyzer compatibility. Operators must create profiles and distribute
   binaries themselves.
8. **Full-build compatibility.** A tracked-file read-only snapshot intentionally
   excludes ignored dependencies, untracked generated code, Git metadata, and
   other checkout state. Many build-coupled analyzers need a prepared build
   environment that this model does not provide.

## Design Quality and Over-Design Risks

The architecture is correct if its primary product requirement is **trusted,
auditable Agent-assisted pre-commit and CI evidence**, not a replacement for a
developer metalinter or enterprise SAST platform.

Several choices are proportionate to that threat model:

- preflight authorization of the full tool set;
- direct process execution and environment allowlisting;
- one shared snapshot and final scope revalidation;
- separate per-tool and cumulative budgets;
- `partial` results instead of fail-open or all-or-nothing loss of accepted
  evidence;
- conservative treatment of corroborating tools.

There are also real over-design risks:

1. **The execution closure is not fully pinned.** Hashing the profile and one
   executable does not fix external rule files, plugins, query packs,
   interpreters, dynamic tool assets, or multi-stage build inputs. The MVP must
   either limit controlled execution to self-contained source-only analyzers or
   add a separately designed resource-bundle contract; it must not claim
   complete authorization for arbitrary analyzers.
2. **Manifest/profile/executable three-level authorization is expensive.** It is
   justified for centrally curated CI or high-trust Agent execution, but too
   cumbersome for ordinary developer onboarding. Tooling must eventually
   generate, verify, and rotate these artifacts, or adoption will remain small.
3. **Rust migration and orchestration in one delivery increases risk.** Porting
   `collect` and `run`, adding multi-tool scheduling, changing packaging, and
   introducing aggregation at once combines compatibility and new-feature
   risk.
4. **Semantic cross-tool aggregation is premature.** `problem_key`,
   `remediation_key`, and a new input v2 contract add producer obligations
   before there is evidence that duplicate findings are a dominant user
   problem. Keeping source findings separate is safer for an MVP.
5. **Serial execution is a sound deterministic MVP, not an end-state advantage.**
   It simplifies budgets and integrity checks but will lose badly to cached or
   parallel products on large repositories.
6. **One snapshot model cannot fit every analyzer.** The design needs an explicit
   supported-analyzer class, such as source-only/offline analyzers, instead of
   implying compatibility with build-coupled SAST tools.

## Python Runtime Recommendation

Do not ship a long-lived
`PRE_COMMIT_REVIEW_STATIC_IMPL=rust|python|shadow` compatibility surface.

Recommended migration contract:

1. Keep Python privately during development as a parity oracle for existing
   `collect` and `run` behavior.
2. Keep `shadow` as a development and CI diagnostic only; do not document it as
   a supported production runtime selection.
3. When deterministic parity, schema, installer, and Linux/macOS/Windows tests
   pass, switch the Shell wrappers to Rust and delete the Python implementation
   in the same cutover branch. Do not publish a release with a public Python or
   shadow runtime selector.
4. Preserve only the public Shell entrypoints, JSON schemas, exit semantics,
   and normalized nondeterministic fields.
5. Do not add new orchestration behavior to Python and do not automatically
   fall back from Rust to Python.

Long-term dual implementation would duplicate Git candidate construction,
snapshot safety, process supervision, schema interpretation, hashing, failure
semantics, release packaging, tests, and security fixes. The compared products
gain compatibility through stable configuration and result contracts, not by
maintaining two authoritative implementations of the same local control plane.

## Recommended Phase-Three Scope

Split the approved phase into two consecutive deliveries.

### Delivery A: Rust consolidation

- Move Phase 1 `collect` and Phase 2 `run` to the Rust library and CLI.
- Preserve existing Shell and JSON contracts.
- Use Python only for parity comparison during development.
- Switch the default to Rust and remove the Python production path after the
  parity and platform gates pass.

### Delivery B: orchestration MVP

- Explicitly support only self-contained, source-only, offline analyzers that
  emit SARIF or normalized JSON on stdout. Keep build-coupled and multi-stage
  tools on the precomputed evidence path.
- Add the hash-pinned manifest and preflight authorization for that supported
  analyzer class without claiming a general execution closure.
- Reuse one snapshot identity.
- Run profiles serially with per-tool and cumulative budgets.
- Preserve `completed`, `partial`, `failed`, invalidated, and not-run states.
- Emit every tool's findings independently with source provenance.
- Defer semantic cross-tool grouping and `static_analysis_input/v2` until real
  result sets demonstrate enough duplicate volume to justify the contract.

After the MVP is used on representative repositories, prioritize measured
product gaps in this order:

1. analyzer compatibility profiles, a clear source-only support class, and a
   decision on whether pinned resource bundles are justified;
2. deterministic result caching keyed by snapshot, executable, profile, and
   tool inputs;
3. PR delivery through SARIF upload or reviewdog-style annotations;
4. baseline/new-code triage and policy bundles;
5. bounded parallel execution only after resource and snapshot integrity rules
   are proven under concurrency.

## Final Assessment

The design is a high-quality, differentiated control plane for trustworthy
static-analysis evidence in Agent-assisted review. It fits security-sensitive
teams that value auditability, exact candidate identity, offline operation, and
honest partial results more than zero-configuration adoption or minimum latency.

It does not yet fit teams primarily seeking a turnkey language-wide linter,
enterprise SAST dashboard, IDE-first feedback loop, or automatic tool manager.
Positioning it as a secure evidence and authorization layer that integrates
existing analyzers is accurate. Positioning it as broadly superior to Semgrep,
CodeQL, SonarQube, Trunk, or MegaLinter is not.
