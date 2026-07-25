use super::contracts::StaticAnalysisEvidence;

pub fn render_collect(evidence: &StaticAnalysisEvidence) -> Result<String, serde_json::Error> {
    Ok(format!(
        "# Pre-Commit Review Static Analysis Evidence\n\n## Static Analysis Evidence JSON\n{}\n",
        serde_json::to_string(evidence)?
    ))
}
