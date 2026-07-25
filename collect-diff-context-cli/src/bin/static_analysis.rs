use collect_diff_context_cli::review_scope::ReviewSource;
use collect_diff_context_cli::static_analysis::contracts::EvidenceTrust;
use collect_diff_context_cli::static_analysis::evidence::{collect_evidence, CollectRequest};
use collect_diff_context_cli::static_analysis::output::render_collect;
use std::env;
use std::path::PathBuf;

const COLLECT_HELP: &str = "Usage: static-analysis-cli collect --result <path> [--result <path> ...] --expect-scope <fingerprint> [options]\n\nOptions:\n  --source <staged|unstaged|branch>\n  --result-scope <fingerprint>\n  --max-findings <1..5000>\n  --trust <explicit-input|controlled-execution>\n  --execution-id <16-hex>\n  --helper <path>\n  -h, --help\n";

#[derive(Debug)]
struct CollectArgs {
    result_paths: Vec<PathBuf>,
    source: Option<ReviewSource>,
    expected_scope: Option<String>,
    asserted_result_scope: Option<String>,
    max_findings: usize,
    trust: EvidenceTrust,
    execution_id: Option<String>,
}

impl Default for CollectArgs {
    fn default() -> Self {
        Self {
            result_paths: Vec::new(),
            source: None,
            expected_scope: None,
            asserted_result_scope: None,
            max_findings: 500,
            trust: EvidenceTrust::ExplicitInput,
            execution_id: None,
        }
    }
}

enum ParseOutcome {
    Help,
    Collect(CollectArgs),
}

fn main() {
    let exit_code = main_entry();
    if exit_code != 0 {
        std::process::exit(exit_code);
    }
}

fn main_entry() -> i32 {
    let mut arguments = env::args().skip(1);
    match arguments.next().as_deref() {
        Some("collect") => match parse_collect(arguments.collect()) {
            Ok(ParseOutcome::Help) => {
                print!("{COLLECT_HELP}");
                0
            }
            Ok(ParseOutcome::Collect(arguments)) => run_collect(arguments),
            Err(error) => collect_error(&error),
        },
        Some("--help" | "-h") => {
            println!("Usage: static-analysis-cli <collect|run> [options]");
            0
        }
        Some("run") => {
            eprintln!("static-analysis-cli: run subcommand is not implemented yet");
            2
        }
        _ => {
            eprintln!("static-analysis-cli: expected collect or run subcommand");
            2
        }
    }
}

fn parse_collect(arguments: Vec<String>) -> Result<ParseOutcome, String> {
    if arguments
        .iter()
        .any(|argument| argument == "--help" || argument == "-h")
    {
        return Ok(ParseOutcome::Help);
    }

    let mut parsed = CollectArgs::default();
    let mut index = 0;
    while index < arguments.len() {
        let argument = &arguments[index];
        let (flag, inline_value) = argument
            .split_once('=')
            .map_or((argument.as_str(), None), |(flag, value)| {
                (flag, Some(value))
            });
        let value = |name: &str| -> Result<String, String> {
            if let Some(value) = inline_value {
                return Ok(value.to_string());
            }
            arguments
                .get(index + 1)
                .cloned()
                .ok_or_else(|| format!("{name} requires a value"))
        };
        let consumed_value = inline_value.is_none();

        match flag {
            "--result" => parsed.result_paths.push(PathBuf::from(value("--result")?)),
            "--source" => {
                parsed.source = Some(match value("--source")?.as_str() {
                    "staged" => ReviewSource::Staged,
                    "unstaged" => ReviewSource::Unstaged,
                    "branch" => ReviewSource::Branch,
                    observed => {
                        return Err(format!(
                            "--source must be staged, unstaged, or branch; received {observed}"
                        ));
                    }
                });
            }
            "--expect-scope" => parsed.expected_scope = Some(value("--expect-scope")?),
            "--result-scope" => {
                parsed.asserted_result_scope = Some(value("--result-scope")?);
            }
            "--max-findings" => {
                let raw = value("--max-findings")?;
                parsed.max_findings = raw
                    .parse::<usize>()
                    .map_err(|_| "--max-findings must be an integer".to_string())?;
            }
            "--trust" => {
                parsed.trust = match value("--trust")?.as_str() {
                    "explicit-input" => EvidenceTrust::ExplicitInput,
                    "controlled-execution" => EvidenceTrust::ControlledExecution,
                    observed => {
                        return Err(format!(
                            "--trust must be explicit-input or controlled-execution; received {observed}"
                        ));
                    }
                };
            }
            "--execution-id" => parsed.execution_id = Some(value("--execution-id")?),
            "--helper" => {
                let _legacy_helper = value("--helper")?;
            }
            observed => return Err(format!("unsupported argument: {observed}")),
        }
        index += if consumed_value { 2 } else { 1 };
    }

    if parsed.result_paths.is_empty() {
        return Err("at least one --result is required".to_string());
    }
    if parsed.expected_scope.is_none() {
        return Err("--expect-scope is required".to_string());
    }
    Ok(ParseOutcome::Collect(parsed))
}

fn run_collect(arguments: CollectArgs) -> i32 {
    let repository = match env::current_dir() {
        Ok(path) => path,
        Err(error) => return collect_error(&format!("cannot resolve current directory: {error}")),
    };
    let evidence = match collect_evidence(CollectRequest {
        repository,
        source: arguments.source,
        expected_scope: arguments
            .expected_scope
            .expect("validated by parse_collect"),
        result_paths: arguments.result_paths,
        asserted_result_scope: arguments.asserted_result_scope,
        max_findings: arguments.max_findings,
        trust: arguments.trust,
        execution_id: arguments.execution_id,
    }) {
        Ok(evidence) => evidence,
        Err(error) => return collect_error(&error.to_string()),
    };
    match render_collect(&evidence) {
        Ok(output) => {
            print!("{output}");
            0
        }
        Err(error) => collect_error(&format!("cannot serialize static evidence: {error}")),
    }
}

fn collect_error(message: &str) -> i32 {
    eprintln!("collect_static_evidence: {message}");
    2
}
