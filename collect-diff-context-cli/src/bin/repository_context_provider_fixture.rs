#![cfg(feature = "test-fixture")]

use serde_json::{json, Value};
use std::env;
use std::io::{self, Read, Write};
use std::process::Command;
use std::thread;
use std::time::Duration;

fn main() {
    let mut arguments = env::args().skip(1);
    let scenario = arguments.next().unwrap_or_else(|| "lifecycle".to_string());
    let log_path = arguments.next();
    if let Some(path) = log_path.as_deref() {
        let _ = std::fs::File::create(path);
    }
    let result = match scenario.as_str() {
        "lifecycle" => lifecycle(log_path.as_deref(), false),
        "config-requests" => lifecycle(log_path.as_deref(), true),
        "split-frame" => split_frame(),
        "stderr-flood" => stderr_flood(),
        "hang" => hang(),
        "malformed-frame" => malformed_frame(),
        "unknown-id" => unknown_id(),
        "crash" => std::process::exit(9),
        "spawn-descendant" => spawn_descendant(arguments.next()),
        _ => Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "unknown fixture scenario",
        )),
    };
    if result.is_err() {
        std::process::exit(2);
    }
}

fn lifecycle(log_path: Option<&str>, configuration_request: bool) -> io::Result<()> {
    let mut input = io::stdin().lock();
    let mut output = io::stdout().lock();
    let initialize = read_frame(&mut input)?;
    let initialize: Value = serde_json::from_slice(&initialize)
        .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?;
    log_method(log_path, initialize.get("method").and_then(Value::as_str))?;
    if configuration_request {
        write_frame(
            &mut output,
            &json!({
                "jsonrpc": "2.0",
                "id": 42,
                "method": "workspace/configuration",
                "params": {"items": [{"section": "rust-analyzer.cargo"}]}
            }),
        )?;
        let _ = read_frame(&mut input)?;
    }
    write_frame(
        &mut output,
        &json!({"jsonrpc":"2.0","id":initialize.get("id").cloned().unwrap_or(Value::Null),"result":{"capabilities":{}}}),
    )?;
    loop {
        let message = read_frame(&mut input)?;
        let message: Value = serde_json::from_slice(&message)
            .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?;
        let method = message.get("method").and_then(Value::as_str);
        log_method(log_path, method)?;
        match method {
            Some("shutdown") => write_frame(
                &mut output,
                &json!({"jsonrpc":"2.0","id":message.get("id").cloned().unwrap_or(Value::Null),"result":null}),
            )?,
            Some("exit") => break,
            _ => {}
        }
    }
    Ok(())
}

fn stderr_flood() -> io::Result<()> {
    let mut stderr = io::stderr().lock();
    stderr.write_all(&vec![b'e'; 1_048_577])?;
    stderr.flush()?;
    thread::sleep(Duration::from_secs(30));
    Ok(())
}

fn split_frame() -> io::Result<()> {
    let mut input = io::stdin().lock();
    let mut output = io::stdout().lock();
    let body = read_frame(&mut input)?;
    let message: Value = serde_json::from_slice(&body)
        .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?;
    write_frame_split(
        &mut output,
        &json!({"jsonrpc":"2.0","id":message.get("id").cloned().unwrap_or(Value::Null),"result":{}}),
    )
}

fn hang() -> io::Result<()> {
    thread::sleep(Duration::from_secs(30));
    Ok(())
}

fn malformed_frame() -> io::Result<()> {
    let mut stdout = io::stdout().lock();
    stdout.write_all(b"Content-Length: nope\r\n\r\n")?;
    stdout.flush()
}

fn unknown_id() -> io::Result<()> {
    let mut input = io::stdin().lock();
    let mut output = io::stdout().lock();
    let _ = read_frame(&mut input)?;
    write_frame(
        &mut output,
        &json!({"jsonrpc":"2.0","id":999,"result":null}),
    )
}

fn spawn_descendant(marker: Option<String>) -> io::Result<()> {
    if let Some(marker) = marker {
        let _ = Command::new("/bin/sh")
            .args(["-c", &format!("sleep 30; touch '{}'", marker)])
            .spawn()?;
    }
    thread::sleep(Duration::from_secs(30));
    Ok(())
}

fn log_method(path: Option<&str>, method: Option<&str>) -> io::Result<()> {
    if let (Some(path), Some(method)) = (path, method) {
        let mut file = std::fs::OpenOptions::new().append(true).open(path)?;
        writeln!(file, "{method}")?;
    }
    Ok(())
}

fn read_frame(reader: &mut impl Read) -> io::Result<Vec<u8>> {
    let mut header = Vec::new();
    let mut byte = [0_u8; 1];
    loop {
        reader.read_exact(&mut byte)?;
        header.push(byte[0]);
        if header.ends_with(b"\r\n\r\n") {
            break;
        }
        if header.len() > 16 * 1024 {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "header too large",
            ));
        }
    }
    let header_text = std::str::from_utf8(&header).map_err(|_| io::ErrorKind::InvalidData)?;
    let length = header_text
        .lines()
        .find_map(|line| line.strip_prefix("Content-Length:"))
        .and_then(|value| value.trim().parse::<usize>().ok())
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "missing length"))?;
    let mut body = vec![0_u8; length];
    reader.read_exact(&mut body)?;
    Ok(body)
}

fn write_frame(writer: &mut impl Write, value: &Value) -> io::Result<()> {
    let body = serde_json::to_vec(value).map_err(io::Error::other)?;
    write!(writer, "Content-Length: {}\r\n\r\n", body.len())?;
    writer.write_all(&body)?;
    writer.flush()
}

fn write_frame_split(writer: &mut impl Write, value: &Value) -> io::Result<()> {
    let body = serde_json::to_vec(value).map_err(io::Error::other)?;
    let mut frame = format!("Content-Length: {}\r\n\r\n", body.len()).into_bytes();
    frame.extend_from_slice(&body);
    for byte in frame {
        writer.write_all(&[byte])?;
        writer.flush()?;
    }
    Ok(())
}
