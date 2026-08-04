use super::contracts::StaticAnalysisEvidence;
use super::executor::RunArtifact;
use super::orchestration::OrchestrationOutput;

pub fn render_collect(evidence: &StaticAnalysisEvidence) -> Result<String, serde_json::Error> {
    Ok(format!(
        "# Pre-Commit Review Static Analysis Evidence\n\n## Static Analysis Evidence JSON\n{}\n",
        serde_json::to_string(evidence)?
    ))
}

pub fn render_run(artifact: &RunArtifact) -> Result<String, serde_json::Error> {
    Ok(format!(
        "# Pre-Commit Review Controlled Static Analysis\n\n## Static Analysis Execution JSON\n{}\n\n## Static Analysis Evidence JSON\n{}\n",
        serde_json::to_string(&artifact.execution)?,
        serde_json::to_string(&artifact.evidence)?
    ))
}

pub fn render_orchestration(output: &OrchestrationOutput) -> Result<String, serde_json::Error> {
    Ok(format!(
        "# Pre-Commit Review Static Analysis Orchestration\n\n## Static Analysis Orchestration JSON\n{}\n\n## Static Analysis Evidence JSON\n{}\n",
        serde_json::to_string(&output.orchestration)?,
        serde_json::to_string(&output.evidence)?
    ))
}
