use serde_json::json;
use std::env;
use std::fs;
use std::process::{Command, Stdio};
use std::thread;
use std::time::Duration;

fn main() {
    let arguments = env::args().skip(1).collect::<Vec<_>>();
    let result = match arguments.first().map(String::as_str) {
        Some("normalized") => emit_normalized(),
        Some("spawn-descendant") => spawn_descendant(&arguments[1..]),
        Some("write-after-delay") => write_after_delay(&arguments[1..]),
        _ => Err("unknown fixture mode".to_string()),
    };
    if let Err(error) = result {
        eprintln!("{error}");
        std::process::exit(2);
    }
}

fn emit_normalized() -> Result<(), String> {
    let scope = env::var("PRE_COMMIT_REVIEW_SCOPE_FINGERPRINT")
        .map_err(|_| "scope fingerprint is missing".to_string())?;
    let payload = json!({
        "schema_version": 1,
        "kind": "static_analysis_input",
        "scope_fingerprint": scope,
        "tool": {"name": "platform-fixture", "version": "1.0"},
        "status": "completed",
        "findings": []
    });
    print!("{payload}");
    Ok(())
}

fn spawn_descendant(arguments: &[String]) -> Result<(), String> {
    let marker = arguments
        .first()
        .ok_or_else(|| "descendant marker is missing".to_string())?;
    let delay_ms = parse_delay(arguments.get(1))?;
    let executable = env::current_exe().map_err(|error| error.to_string())?;
    Command::new(executable)
        .args(["write-after-delay", marker, &delay_ms.to_string()])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|error| format!("cannot start fixture descendant: {error}"))?;
    thread::sleep(Duration::from_secs(30));
    Ok(())
}

fn write_after_delay(arguments: &[String]) -> Result<(), String> {
    let marker = arguments
        .first()
        .ok_or_else(|| "descendant marker is missing".to_string())?;
    let delay_ms = parse_delay(arguments.get(1))?;
    thread::sleep(Duration::from_millis(delay_ms));
    fs::write(marker, b"descendant survived\n").map_err(|error| error.to_string())
}

fn parse_delay(value: Option<&String>) -> Result<u64, String> {
    value
        .ok_or_else(|| "delay is missing".to_string())?
        .parse::<u64>()
        .map_err(|_| "delay is invalid".to_string())
}
