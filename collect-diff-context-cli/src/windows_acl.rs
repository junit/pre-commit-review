#![cfg(windows)]

use std::path::{Path, PathBuf};
use std::process::Command;

pub(crate) fn restrict_tree_read_execute(path: &Path) -> Result<(), String> {
    let sid = current_user_sid()?;
    apply_current_user_grant(path, &sid, "RX")?;
    apply_current_user_deny(path, &sid, "(WD,AD,WEA,WA,DE,DC)")
}

pub(crate) fn restrict_tree_private(path: &Path) -> Result<(), String> {
    let sid = current_user_sid()?;
    apply_current_user_grant(path, &sid, "(OI)(CI)F")
}

pub(crate) fn grant_tree_full_control(path: &Path) -> Result<(), String> {
    apply_current_user_full_control(path)
}

fn apply_current_user_full_control(path: &Path) -> Result<(), String> {
    let sid = current_user_sid()?;
    remove_current_user_denies(path, &sid)?;
    apply_current_user_grant(path, &sid, "F")
}

fn apply_current_user_grant(path: &Path, sid: &str, permissions: &str) -> Result<(), String> {
    let identity = format!("*{sid}:{permissions}");
    run_icacls(
        path,
        &["/inheritance:r", "/grant:r", &identity, "/T", "/C", "/Q"],
    )
}

fn apply_current_user_deny(path: &Path, sid: &str, permissions: &str) -> Result<(), String> {
    let identity = format!("*{sid}:{permissions}");
    run_icacls(path, &["/deny", &identity, "/T", "/C", "/Q"])
}

fn remove_current_user_denies(path: &Path, sid: &str) -> Result<(), String> {
    let identity = format!("*{sid}");
    run_icacls(path, &["/remove:d", &identity, "/T", "/C", "/Q"])
}

fn run_icacls(path: &Path, arguments: &[&str]) -> Result<(), String> {
    let output = Command::new(system_binary("icacls.exe")?)
        .arg(path)
        .args(arguments)
        .output()
        .map_err(|error| format!("cannot start icacls.exe: {error}"))?;
    if output.status.success() {
        return Ok(());
    }
    Err(format!(
        "icacls.exe failed: {}",
        bounded_detail(&output.stderr)
    ))
}

fn current_user_sid() -> Result<String, String> {
    let output = Command::new(system_binary("whoami.exe")?)
        .args(["/user", "/fo", "csv", "/nh"])
        .output()
        .map_err(|error| format!("cannot start whoami.exe: {error}"))?;
    if !output.status.success() {
        return Err(format!(
            "whoami.exe failed: {}",
            bounded_detail(&output.stderr)
        ));
    }
    let text = String::from_utf8_lossy(&output.stdout);
    let sid = text
        .trim()
        .rsplit_once(',')
        .map(|(_, sid)| sid.trim().trim_matches('"'))
        .filter(|sid| {
            sid.starts_with("S-1-")
                && sid
                    .bytes()
                    .all(|byte| byte.is_ascii_digit() || byte == b'-' || byte == b'S')
        })
        .ok_or_else(|| "whoami.exe returned an invalid current-user SID".to_string())?;
    Ok(sid.to_string())
}

fn system_binary(name: &str) -> Result<PathBuf, String> {
    let system_root = std::env::var_os("SystemRoot")
        .or_else(|| std::env::var_os("WINDIR"))
        .map(PathBuf::from)
        .filter(|path| path.is_absolute())
        .ok_or_else(|| "Windows system root is unavailable".to_string())?;
    Ok(system_root.join("System32").join(name))
}

fn bounded_detail(value: &[u8]) -> String {
    let detail = String::from_utf8_lossy(value)
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ");
    let detail = detail.chars().take(500).collect::<String>();
    if detail.is_empty() {
        "unknown Windows ACL error".to_string()
    } else {
        detail
    }
}
