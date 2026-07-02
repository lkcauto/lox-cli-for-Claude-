use assert_cmd::Command;
use serde_json::Value;

fn lox() -> Command {
    Command::cargo_bin("lox").unwrap()
}

fn requests(reqs: &[Value]) -> String {
    reqs.iter()
        .map(|r| r.to_string())
        .collect::<Vec<_>>()
        .join("\n")
        + "\n"
}

fn run(reqs: &[Value]) -> Vec<Value> {
    let output = lox()
        .arg("mcp")
        .write_stdin(requests(reqs))
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "mcp server exited non-zero: {output:?}"
    );
    String::from_utf8(output.stdout)
        .unwrap()
        .lines()
        .map(|l| serde_json::from_str(l).unwrap_or_else(|e| panic!("invalid JSON line {l:?}: {e}")))
        .collect()
}

#[test]
fn initialize_and_tools_list() {
    let responses = run(&[
        serde_json::json!({"jsonrpc": "2.0", "id": 1, "method": "initialize", "params": {}}),
        serde_json::json!({"jsonrpc": "2.0", "method": "notifications/initialized"}),
        serde_json::json!({"jsonrpc": "2.0", "id": 2, "method": "tools/list"}),
    ]);

    // The notification has no id and must not produce a response line.
    assert_eq!(responses.len(), 2);

    assert_eq!(responses[0]["id"], 1);
    assert_eq!(responses[0]["result"]["serverInfo"]["name"], "lox");

    assert_eq!(responses[1]["id"], 2);
    let tools = responses[1]["result"]["tools"].as_array().unwrap();
    let names: Vec<&str> = tools.iter().map(|t| t["name"].as_str().unwrap()).collect();
    assert!(names.contains(&"list_rooms"));
    assert!(names.contains(&"turn_on"));
    assert!(names.contains(&"set_blind"));
    assert!(names.contains(&"run_lox"));
    // Every tool must carry a usable JSON Schema for its arguments.
    for tool in tools {
        assert_eq!(tool["inputSchema"]["type"], "object");
    }
}

#[test]
fn unknown_method_is_json_rpc_error() {
    let responses =
        run(&[serde_json::json!({"jsonrpc": "2.0", "id": 1, "method": "not/a/method"})]);
    assert_eq!(responses[0]["error"]["code"], -32601);
}

#[test]
fn tools_call_get_schema_needs_no_miniserver_config() {
    let responses = run(&[serde_json::json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "tools/call",
        "params": {"name": "get_schema", "arguments": {"command": "blind"}},
    })]);

    let content = responses[0]["result"]["content"][0]["text"]
        .as_str()
        .unwrap();
    let inner: Value = serde_json::from_str(content).unwrap();
    assert_eq!(inner["exit_code"], 0);
    assert_eq!(inner["result"]["name"], "blind");
    assert_eq!(responses[0]["result"]["isError"], false);
}

#[test]
fn tools_call_unknown_tool_reports_tool_error() {
    let responses = run(&[serde_json::json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "tools/call",
        "params": {"name": "does_not_exist", "arguments": {}},
    })]);

    // Unknown tools are a tool-level error (isError:true), not a JSON-RPC
    // protocol error, per the MCP spec.
    assert!(responses[0]["error"].is_null());
    assert_eq!(responses[0]["result"]["isError"], true);
}

#[test]
fn tools_call_missing_required_argument_reports_tool_error() {
    let responses = run(&[serde_json::json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "tools/call",
        "params": {"name": "get_control", "arguments": {}},
    })]);

    assert_eq!(responses[0]["result"]["isError"], true);
    let text = responses[0]["result"]["content"][0]["text"]
        .as_str()
        .unwrap();
    assert!(text.contains("name_or_uuid"));
}

#[test]
fn run_lox_blocks_destructive_commands() {
    for blocked in [
        vec!["reboot"],
        vec!["update", "install"],
        vec!["watch", "SomeLight"],
    ] {
        let responses = run(&[serde_json::json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "tools/call",
            "params": {"name": "run_lox", "arguments": {"args": blocked}},
        })]);
        assert_eq!(
            responses[0]["result"]["isError"], true,
            "expected {blocked:?} to be blocked"
        );
    }
}

#[test]
fn run_lox_allows_non_blocked_commands() {
    let responses = run(&[serde_json::json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "tools/call",
        "params": {"name": "run_lox", "arguments": {"args": ["schema", "light"]}},
    })]);
    assert_eq!(responses[0]["result"]["isError"], false);
    let content = responses[0]["result"]["content"][0]["text"]
        .as_str()
        .unwrap();
    let inner: Value = serde_json::from_str(content).unwrap();
    assert_eq!(inner["exit_code"], 0);
}
