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
        "readiness-ok" => handshake(log_path.as_deref(), "ok", Some("utf-8")),
        "readiness-warning" => handshake(log_path.as_deref(), "warning", Some("utf-8")),
        "readiness-error" => handshake(log_path.as_deref(), "error", Some("utf-8")),
        "readiness-default-encoding" => handshake(log_path.as_deref(), "ok", None),
        "readiness-config-requests" => handshake_config_requests(log_path.as_deref()),
        "registration-disallowed" => handshake_registration(log_path.as_deref()),
        "readiness-hang" => handshake_hang(log_path.as_deref()),
        "missing-capability" => handshake_missing_capability(log_path.as_deref()),
        "initialize-error" => handshake_initialize_error(log_path.as_deref()),
        "unknown-encoding" => handshake(log_path.as_deref(), "ok", Some("utf-32")),
        "graph" => graph(log_path.as_deref()),
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

fn handshake(log_path: Option<&str>, health: &str, encoding: Option<&str>) -> io::Result<()> {
    let mut input = io::stdin().lock();
    let mut output = io::stdout().lock();
    let initialize = read_json_frame(&mut input)?;
    log_method(log_path, initialize.get("method").and_then(Value::as_str))?;
    validate_initialize_request(&initialize)?;
    let mut capabilities = json!({"callHierarchyProvider": true});
    if let Some(encoding) = encoding {
        capabilities["positionEncoding"] = Value::String(encoding.to_string());
    }
    write_frame(
        &mut output,
        &json!({"jsonrpc":"2.0","id":initialize.get("id").cloned().unwrap_or(Value::Null),"result":{"capabilities":capabilities}}),
    )?;
    let initialized = read_json_frame(&mut input)?;
    log_method(log_path, initialized.get("method").and_then(Value::as_str))?;
    write_frame(
        &mut output,
        &json!({"jsonrpc":"2.0","method":"experimental/serverStatus","params":{"health":health,"quiescent":true}}),
    )?;
    finish_lifecycle(&mut input, &mut output, log_path)
}

fn handshake_config_requests(log_path: Option<&str>) -> io::Result<()> {
    let mut input = io::stdin().lock();
    let mut output = io::stdout().lock();
    let initialize = read_json_frame(&mut input)?;
    log_method(log_path, initialize.get("method").and_then(Value::as_str))?;
    validate_initialize_request(&initialize)?;
    write_frame(
        &mut output,
        &json!({"jsonrpc":"2.0","id":42,"method":"workspace/configuration","params":{"items":[{"section":"one"},{"section":"two"}]}}),
    )?;
    let configuration_response = read_json_frame(&mut input)?;
    let values = configuration_response
        .get("result")
        .and_then(Value::as_array)
        .ok_or_else(|| {
            io::Error::new(io::ErrorKind::InvalidData, "configuration response invalid")
        })?;
    if values != &[Value::Null, Value::Null] {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "configuration response not positional",
        ));
    }
    write_frame(
        &mut output,
        &json!({"jsonrpc":"2.0","id":initialize.get("id").cloned().unwrap_or(Value::Null),"result":{"capabilities":{"callHierarchyProvider":true,"positionEncoding":"utf-8"}}}),
    )?;
    let initialized = read_json_frame(&mut input)?;
    log_method(log_path, initialized.get("method").and_then(Value::as_str))?;
    write_frame(
        &mut output,
        &json!({"jsonrpc":"2.0","method":"experimental/serverStatus","params":{"health":"ok","quiescent":true}}),
    )?;
    finish_lifecycle(&mut input, &mut output, log_path)
}

fn handshake_registration(log_path: Option<&str>) -> io::Result<()> {
    let mut input = io::stdin().lock();
    let mut output = io::stdout().lock();
    let initialize = read_json_frame(&mut input)?;
    log_method(log_path, initialize.get("method").and_then(Value::as_str))?;
    validate_initialize_request(&initialize)?;
    write_frame(
        &mut output,
        &json!({"jsonrpc":"2.0","id":42,"method":"client/registerCapability","params":{"registrations":[{"id":"ok","method":"workspace/didChangeConfiguration"},{"id":"bad","method":"workspace/executeCommand"}]}}),
    )?;
    let registration_response = read_json_frame(&mut input)?;
    if registration_response.get("error").is_none() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "disallowed registration accepted",
        ));
    }
    write_frame(
        &mut output,
        &json!({"jsonrpc":"2.0","id":initialize.get("id").cloned().unwrap_or(Value::Null),"result":{"capabilities":{"callHierarchyProvider":true,"positionEncoding":"utf-8"}}}),
    )?;
    let initialized = read_json_frame(&mut input)?;
    log_method(log_path, initialized.get("method").and_then(Value::as_str))?;
    write_frame(
        &mut output,
        &json!({"jsonrpc":"2.0","method":"experimental/serverStatus","params":{"health":"ok","quiescent":true}}),
    )?;
    finish_lifecycle(&mut input, &mut output, log_path)
}

fn handshake_missing_capability(log_path: Option<&str>) -> io::Result<()> {
    let mut input = io::stdin().lock();
    let mut output = io::stdout().lock();
    let initialize = read_json_frame(&mut input)?;
    log_method(log_path, initialize.get("method").and_then(Value::as_str))?;
    write_frame(
        &mut output,
        &json!({"jsonrpc":"2.0","id":initialize.get("id").cloned().unwrap_or(Value::Null),"result":{"capabilities":{}}}),
    )?;
    finish_lifecycle(&mut input, &mut output, log_path)
}

fn handshake_initialize_error(log_path: Option<&str>) -> io::Result<()> {
    let mut input = io::stdin().lock();
    let mut output = io::stdout().lock();
    let initialize = read_json_frame(&mut input)?;
    log_method(log_path, initialize.get("method").and_then(Value::as_str))?;
    write_frame(
        &mut output,
        &json!({"jsonrpc":"2.0","id":initialize.get("id").cloned().unwrap_or(Value::Null),"error":{"code":-32603,"message":"fixture initialize failure"}}),
    )
}

fn handshake_hang(log_path: Option<&str>) -> io::Result<()> {
    let mut input = io::stdin().lock();
    let initialize = read_json_frame(&mut input)?;
    log_method(log_path, initialize.get("method").and_then(Value::as_str))?;
    thread::sleep(Duration::from_secs(30));
    Ok(())
}

fn graph(log_path: Option<&str>) -> io::Result<()> {
    let mut input = io::stdin().lock();
    let mut output = io::stdout().lock();
    let initialize = read_json_frame(&mut input)?;
    log_method(log_path, initialize.get("method").and_then(Value::as_str))?;
    validate_initialize_request(&initialize)?;
    let root_uri = initialize
        .get("params")
        .and_then(|params| params.get("rootUri"))
        .and_then(Value::as_str)
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "root URI missing"))?;
    write_frame(
        &mut output,
        &json!({
            "jsonrpc": "2.0",
            "id": initialize.get("id").cloned().unwrap_or(Value::Null),
            "result": {"capabilities": {"callHierarchyProvider": true, "positionEncoding": "utf-8"}}
        }),
    )?;
    let initialized = read_json_frame(&mut input)?;
    log_method(log_path, initialized.get("method").and_then(Value::as_str))?;
    write_frame(
        &mut output,
        &json!({"jsonrpc":"2.0","method":"experimental/serverStatus","params":{"health":"ok","quiescent":true}}),
    )?;
    let uri = format!("{root_uri}src/lib.rs");
    loop {
        let message = read_json_frame(&mut input)?;
        let method = message.get("method").and_then(Value::as_str);
        log_method(log_path, method)?;
        let id = message.get("id").cloned().unwrap_or(Value::Null);
        match method {
            Some("textDocument/prepareCallHierarchy") => write_frame(
                &mut output,
                &json!({"jsonrpc":"2.0","id":id,"result":[graph_item(&uri, "seed")]}),
            )?,
            Some("callHierarchy/incomingCalls") => {
                let name = message
                    .get("params")
                    .and_then(|params| params.get("item"))
                    .and_then(|item| item.get("name"))
                    .and_then(Value::as_str)
                    .unwrap_or_default();
                write_frame(
                    &mut output,
                    &json!({"jsonrpc":"2.0","id":id,"result":graph_incoming(&uri, name)}),
                )?;
            }
            Some("callHierarchy/outgoingCalls") => {
                let name = message
                    .get("params")
                    .and_then(|params| params.get("item"))
                    .and_then(|item| item.get("name"))
                    .and_then(Value::as_str)
                    .unwrap_or_default();
                write_frame(
                    &mut output,
                    &json!({"jsonrpc":"2.0","id":id,"result":graph_outgoing(&uri, name)}),
                )?;
            }
            Some("shutdown") => {
                write_frame(&mut output, &json!({"jsonrpc":"2.0","id":id,"result":null}))?
            }
            Some("exit") => break,
            _ => {}
        }
    }
    Ok(())
}

fn graph_item(uri: &str, name: &str) -> Value {
    let (line, start, end, full_end) = match name {
        "seed" => (0, 7, 11, 26),
        "caller" => (1, 7, 13, 26),
        "callee" => (2, 7, 13, 18),
        _ => (0, 7, 11, 26),
    };
    json!({
        "name": name,
        "kind": 12,
        "detail": "fixture",
        "uri": uri,
        "range": {"start": {"line": line, "character": 0}, "end": {"line": line, "character": full_end}},
        "selectionRange": {"start": {"line": line, "character": start}, "end": {"line": line, "character": end}},
        "data": {"fixture": name}
    })
}

fn graph_call(uri: &str, name: &str, start: u32, end: u32) -> Value {
    json!({
        "from": graph_item(uri, name),
        "fromRanges": [{"start": {"line": if name == "caller" {1} else {0}, "character": start}, "end": {"line": if name == "caller" {1} else {0}, "character": end}}]
    })
}

fn graph_incoming(uri: &str, name: &str) -> Value {
    match name {
        "seed" => json!([
            graph_call(uri, "caller", 18, 22),
            graph_call(uri, "caller", 18, 22)
        ]),
        "caller" => json!([graph_call(uri, "seed", 16, 22)]),
        _ => json!([]),
    }
}

fn graph_outgoing(uri: &str, name: &str) -> Value {
    match name {
        "seed" => json!([
            {"to": graph_item(uri, "caller"), "fromRanges": [{"start": {"line": 0, "character": 16}, "end": {"line": 0, "character": 22}}]},
            {"to": graph_item(uri, "caller"), "fromRanges": [{"start": {"line": 0, "character": 16}, "end": {"line": 0, "character": 22}}]},
            {"to": graph_item(uri, "callee"), "fromRanges": [{"start": {"line": 0, "character": 7}, "end": {"line": 0, "character": 11}}]}
        ]),
        "caller" => json!([
            {"to": graph_item(uri, "seed"), "fromRanges": [{"start": {"line": 1, "character": 18}, "end": {"line": 1, "character": 22}}]}
        ]),
        _ => json!([]),
    }
}

fn finish_lifecycle(
    input: &mut impl Read,
    output: &mut impl Write,
    log_path: Option<&str>,
) -> io::Result<()> {
    loop {
        let message = read_json_frame(input)?;
        let method = message.get("method").and_then(Value::as_str);
        log_method(log_path, method)?;
        match method {
            Some("shutdown") => write_frame(
                output,
                &json!({"jsonrpc":"2.0","id":message.get("id").cloned().unwrap_or(Value::Null),"result":null}),
            )?,
            Some("exit") => return Ok(()),
            _ => {}
        }
    }
}

fn read_json_frame(reader: &mut impl Read) -> io::Result<Value> {
    let body = read_frame(reader)?;
    serde_json::from_slice(&body).map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))
}

fn validate_initialize_request(value: &Value) -> io::Result<()> {
    let value = value.get("params").unwrap_or(value);
    let capabilities = value.get("capabilities").ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::InvalidData,
            "initialize capabilities missing",
        )
    })?;
    let encodings = capabilities
        .get("general")
        .and_then(|value| value.get("positionEncodings"))
        .and_then(Value::as_array)
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "position encodings missing"))?;
    if encodings != &[json!("utf-8"), json!("utf-16")] {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "position encodings invalid",
        ));
    }
    if capabilities
        .get("workspace")
        .and_then(|value| value.get("configuration"))
        != Some(&Value::Bool(true))
        || capabilities
            .get("textDocument")
            .and_then(|value| value.get("callHierarchy"))
            .and_then(|value| value.get("dynamicRegistration"))
            != Some(&Value::Bool(false))
        || capabilities
            .get("experimental")
            .and_then(|value| value.get("serverStatusNotification"))
            != Some(&Value::Bool(true))
    {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "initialize capabilities invalid",
        ));
    }
    let linked_projects = value
        .get("initializationOptions")
        .and_then(|value| value.get("linkedProjects"))
        .and_then(Value::as_array)
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "linked projects missing"))?;
    if linked_projects.len() != 1 {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "linked projects must be single",
        ));
    }
    let options = value.get("initializationOptions").unwrap();
    if options
        .get("cargo")
        .and_then(|value| value.get("buildScripts"))
        .and_then(|value| value.get("enable"))
        != Some(&Value::Bool(false))
        || options.get("cargo").and_then(|value| value.get("noDeps")) != Some(&Value::Bool(true))
        || options
            .get("procMacro")
            .and_then(|value| value.get("enable"))
            != Some(&Value::Bool(false))
        || options.get("checkOnSave") != Some(&Value::Bool(false))
    {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "initialize hardening invalid",
        ));
    }
    Ok(())
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
