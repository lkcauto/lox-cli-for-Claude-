//! MCP (Model Context Protocol) stdio server.
//!
//! Exposes a curated set of `lox` operations as MCP tools over newline-delimited
//! JSON-RPC on stdin/stdout, so AI assistants (Claude Desktop, etc.) can control
//! and inspect a Loxone Miniserver directly. Each tool call spawns the `lox`
//! binary itself as a subprocess (reusing all existing auth/config/resolution
//! logic) and captures its `-o json` output, so tool behavior always matches
//! the CLI exactly.

use anyhow::{Context, Result, bail};
use serde_json::{Value, json};
use std::io::{self, BufRead, Write};
use std::process::Command;

use crate::commands::RunContext;

const PROTOCOL_VERSION: &str = "2024-11-05";

/// First-argument (or first two-argument) combinations blocked from the
/// generic `run_lox` escape hatch because they are destructive, irreversible,
/// or long-running/blocking (which would hang a tool call indefinitely).
const BLOCKED_RAW_COMMANDS: &[&[&str]] = &[
    &["reboot"],
    &["update", "install"],
    &["watch"],
    &["stream"],
    &["discover"],
    &["otel", "serve"],
    &["mcp"],
];

pub fn cmd_mcp(_ctx: &RunContext) -> Result<()> {
    let stdin = io::stdin();
    let mut stdout = io::stdout();

    for line in stdin.lock().lines() {
        let line = line.context("reading stdin")?;
        let line = line.trim();
        if line.is_empty() {
            continue;
        }

        let request: Value = match serde_json::from_str(line) {
            Ok(v) => v,
            Err(_) => continue, // ignore malformed lines rather than crashing the loop
        };

        let id = request.get("id").cloned();
        let method = request.get("method").and_then(|m| m.as_str()).unwrap_or("");

        let response = match method {
            "initialize" => id.map(|id| success(id, initialize_result())),
            "notifications/initialized" | "notifications/cancelled" => None,
            "ping" => id.map(|id| success(id, json!({}))),
            "tools/list" => id.map(|id| success(id, json!({ "tools": tool_defs() }))),
            "tools/call" => id.map(|id| handle_tools_call(id, &request)),
            _ => id.map(|id| error(id, -32601, "Method not found")),
        };

        if let Some(resp) = response {
            writeln!(stdout, "{}", serde_json::to_string(&resp)?)?;
            stdout.flush()?;
        }
    }
    Ok(())
}

fn success(id: Value, result: Value) -> Value {
    json!({ "jsonrpc": "2.0", "id": id, "result": result })
}

fn error(id: Value, code: i64, message: &str) -> Value {
    json!({ "jsonrpc": "2.0", "id": id, "error": { "code": code, "message": message } })
}

fn initialize_result() -> Value {
    json!({
        "protocolVersion": PROTOCOL_VERSION,
        "capabilities": { "tools": {} },
        "serverInfo": { "name": "lox", "version": env!("CARGO_PKG_VERSION") },
    })
}

fn handle_tools_call(id: Value, request: &Value) -> Value {
    let params = request.get("params").cloned().unwrap_or(Value::Null);
    let name = params.get("name").and_then(|v| v.as_str()).unwrap_or("");
    let arguments = params
        .get("arguments")
        .cloned()
        .unwrap_or_else(|| json!({}));

    let outcome = build_args(name, &arguments).and_then(run_lox);
    match outcome {
        Ok(result) => success(id, tool_result(result, false)),
        Err(e) => success(
            id,
            tool_result(json!({ "error": format!("{:#}", e) }), true),
        ),
    }
}

fn tool_result(value: Value, is_error: bool) -> Value {
    json!({
        "content": [{
            "type": "text",
            "text": serde_json::to_string_pretty(&value).unwrap_or_default(),
        }],
        "isError": is_error,
    })
}

/// Spawn the `lox` binary itself with the given args, always forcing JSON
/// output and non-interactive mode, and return its parsed result.
fn run_lox(mut args: Vec<String>) -> Result<Value> {
    let exe = std::env::current_exe().context("resolving lox executable path")?;
    args.push("--non-interactive".into());
    args.push("-o".into());
    args.push("json".into());

    let output = Command::new(exe)
        .args(&args)
        .output()
        .context("spawning lox subprocess")?;

    let stdout = String::from_utf8_lossy(&output.stdout).trim().to_string();
    let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
    let parsed = serde_json::from_str::<Value>(&stdout).ok();

    Ok(json!({
        "exit_code": output.status.code(),
        "result": parsed.clone().unwrap_or(Value::Null),
        "raw_stdout": if parsed.is_none() { Value::String(stdout) } else { Value::Null },
        "stderr": stderr,
    }))
}

// ── Argument helpers ─────────────────────────────────────────────────────────

fn str_arg(args: &Value, key: &str) -> Option<String> {
    args.get(key)
        .and_then(|v| v.as_str())
        .map(|s| s.to_string())
}

fn bool_arg(args: &Value, key: &str) -> bool {
    args.get(key).and_then(|v| v.as_bool()).unwrap_or(false)
}

fn require_str(args: &Value, key: &str) -> Result<String> {
    str_arg(args, key).with_context(|| format!("missing required argument '{key}'"))
}

fn push_opt(argv: &mut Vec<String>, flag: &str, value: Option<String>) {
    if let Some(v) = value {
        argv.push(flag.to_string());
        argv.push(v);
    }
}

fn push_flag(argv: &mut Vec<String>, flag: &str, value: bool) {
    if value {
        argv.push(flag.to_string());
    }
}

fn push_room(argv: &mut Vec<String>, args: &Value) {
    push_opt(argv, "--room", str_arg(args, "room"));
}

// ── Tool -> argv mapping ─────────────────────────────────────────────────────

fn build_args(tool: &str, args: &Value) -> Result<Vec<String>> {
    match tool {
        "get_schema" => {
            let mut argv = vec!["schema".to_string()];
            if let Some(cmd) = str_arg(args, "command") {
                argv.push(cmd);
            }
            Ok(argv)
        }
        "list_rooms" => Ok(vec!["rooms".to_string()]),
        "list_categories" => Ok(vec!["categories".to_string()]),
        "list_controls" => {
            let mut argv = vec!["ls".to_string()];
            push_opt(&mut argv, "--type", str_arg(args, "type"));
            push_room(&mut argv, args);
            push_opt(&mut argv, "--cat", str_arg(args, "category"));
            push_flag(&mut argv, "--favorites", bool_arg(args, "favorites"));
            push_flag(&mut argv, "--values", bool_arg(args, "values"));
            Ok(argv)
        }
        "get_control" => {
            let mut argv = vec!["get".to_string(), require_str(args, "name_or_uuid")?];
            push_room(&mut argv, args);
            Ok(argv)
        }
        "control_info" => {
            let mut argv = vec!["info".to_string(), require_str(args, "name_or_uuid")?];
            push_room(&mut argv, args);
            Ok(argv)
        }
        "turn_on" | "turn_off" => {
            let mut argv = vec![if tool == "turn_on" { "on" } else { "off" }.to_string()];
            if let Some(name) = str_arg(args, "name_or_uuid") {
                argv.push(name);
            }
            push_room(&mut argv, args);
            push_opt(&mut argv, "--all-in-room", str_arg(args, "all_in_room"));
            Ok(argv)
        }
        "set_blind" => {
            let mut argv = vec![
                "blind".to_string(),
                require_str(args, "name_or_uuid")?,
                require_str(args, "action")?,
            ];
            if let Some(pos) = args.get("position").and_then(|v| v.as_f64()) {
                argv.push(pos.to_string());
            }
            push_room(&mut argv, args);
            Ok(argv)
        }
        "set_light" => {
            let mode = require_str(args, "mode")?;
            if !matches!(mode.as_str(), "mood" | "dim" | "color") {
                bail!("'mode' must be one of: mood, dim, color");
            }
            let mut argv = vec![
                "light".to_string(),
                mode,
                require_str(args, "name_or_uuid")?,
                require_str(args, "value")?,
            ];
            push_room(&mut argv, args);
            Ok(argv)
        }
        "set_thermostat" => {
            let name = require_str(args, "name_or_uuid")?;
            let action = str_arg(args, "action");
            let value = str_arg(args, "value");
            let duration = args.get("duration_minutes").and_then(|v| v.as_u64());
            if duration.is_some() && value.is_none() {
                bail!("'duration_minutes' requires 'value' to also be set");
            }
            if value.is_some() && action.is_none() {
                bail!("'value' requires 'action' to also be set");
            }
            let mut argv = vec!["thermostat".to_string(), name];
            if let Some(a) = action {
                argv.push(a);
            }
            if let Some(v) = value {
                argv.push(v);
            }
            if let Some(d) = duration {
                argv.push(d.to_string());
            }
            push_room(&mut argv, args);
            Ok(argv)
        }
        "set_alarm" => {
            let mut argv = vec![
                "alarm".to_string(),
                require_str(args, "name_or_uuid")?,
                require_str(args, "action")?,
            ];
            push_flag(&mut argv, "--no-motion", bool_arg(args, "no_motion"));
            push_opt(&mut argv, "--code", str_arg(args, "code"));
            push_room(&mut argv, args);
            Ok(argv)
        }
        "set_door_lock" => {
            let mut argv = vec![
                "door".to_string(),
                require_str(args, "name_or_uuid")?,
                require_str(args, "action")?,
            ];
            push_room(&mut argv, args);
            Ok(argv)
        }
        "send_command" => {
            let mut argv = vec![
                "send".to_string(),
                require_str(args, "name_or_uuid")?,
                require_str(args, "command")?,
            ];
            push_room(&mut argv, args);
            push_opt(&mut argv, "--secured", str_arg(args, "secured"));
            Ok(argv)
        }
        "list_sensors" => {
            let mut argv = vec!["sensors".to_string()];
            push_opt(&mut argv, "--type", str_arg(args, "type"));
            push_room(&mut argv, args);
            Ok(argv)
        }
        "get_status" => {
            let mut argv = vec!["status".to_string()];
            push_flag(&mut argv, "--diag", bool_arg(args, "diag"));
            push_flag(&mut argv, "--net", bool_arg(args, "net"));
            push_flag(&mut argv, "--bus", bool_arg(args, "bus"));
            push_flag(&mut argv, "--lan", bool_arg(args, "lan"));
            push_flag(&mut argv, "--all", bool_arg(args, "all"));
            Ok(argv)
        }
        "get_weather" => {
            let mut argv = vec!["weather".to_string()];
            push_flag(&mut argv, "--forecast", bool_arg(args, "forecast"));
            Ok(argv)
        }
        "get_energy" => {
            let mut argv = vec!["energy".to_string()];
            push_room(&mut argv, args);
            Ok(argv)
        }
        "list_extensions" => Ok(vec!["extensions".to_string()]),
        "get_health" => {
            let mut argv = vec!["health".to_string()];
            push_opt(&mut argv, "--type", str_arg(args, "device_type"));
            push_flag(&mut argv, "--problems", bool_arg(args, "problems"));
            Ok(argv)
        }
        "run_scene" => {
            let mut argv = vec!["run".to_string(), require_str(args, "scene")?];
            push_flag(&mut argv, "--dry-run", bool_arg(args, "dry_run"));
            Ok(argv)
        }
        "list_scenes" => Ok(vec!["scene".to_string(), "ls".to_string()]),
        "run_lox" => {
            let raw: Vec<String> = args
                .get("args")
                .and_then(|v| v.as_array())
                .map(|arr| {
                    arr.iter()
                        .filter_map(|v| v.as_str().map(str::to_string))
                        .collect()
                })
                .unwrap_or_default();
            if raw.is_empty() {
                bail!("'args' must be a non-empty array of CLI arguments");
            }
            if BLOCKED_RAW_COMMANDS
                .iter()
                .any(|blocked| blocked.len() <= raw.len() && raw[..blocked.len()] == **blocked)
            {
                bail!(
                    "command '{}' is blocked in the MCP server (destructive, irreversible, or long-running); run it directly via the lox CLI instead",
                    raw.join(" ")
                );
            }
            Ok(raw)
        }
        _ => bail!("unknown tool '{tool}'"),
    }
}

// ── Tool definitions (JSON Schema for MCP `tools/list`) ─────────────────────

fn tool_defs() -> Vec<Value> {
    let room_prop = json!({ "type": "string", "description": "Room name to disambiguate controls with the same name" });

    vec![
        json!({
            "name": "get_schema",
            "description": "Get the full lox CLI command schema (all commands, args, valid actions), or the schema for one command.",
            "inputSchema": {
                "type": "object",
                "properties": { "command": { "type": "string", "description": "Command name to filter to, e.g. 'blind' or 'light'" } },
            },
        }),
        json!({
            "name": "list_rooms",
            "description": "List all rooms defined on the Miniserver.",
            "inputSchema": { "type": "object", "properties": {} },
        }),
        json!({
            "name": "list_categories",
            "description": "List all categories defined on the Miniserver.",
            "inputSchema": { "type": "object", "properties": {} },
        }),
        json!({
            "name": "list_controls",
            "description": "List controls (lights, blinds, sensors, etc.), optionally filtered by type, room, or category.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "type": { "type": "string", "description": "Control type filter, e.g. 'Jalousie', 'LightControllerV2'" },
                    "room": room_prop,
                    "category": { "type": "string" },
                    "favorites": { "type": "boolean", "description": "Only show favorites" },
                    "values": { "type": "boolean", "description": "Include current values" },
                },
            },
        }),
        json!({
            "name": "get_control",
            "description": "Get the full current state of a control by name or UUID.",
            "inputSchema": {
                "type": "object",
                "properties": { "name_or_uuid": { "type": "string" }, "room": room_prop },
                "required": ["name_or_uuid"],
            },
        }),
        json!({
            "name": "control_info",
            "description": "Get detailed info about a control: sub-controls, states, moods, flags.",
            "inputSchema": {
                "type": "object",
                "properties": { "name_or_uuid": { "type": "string" }, "room": room_prop },
                "required": ["name_or_uuid"],
            },
        }),
        json!({
            "name": "turn_on",
            "description": "Turn on a control by name or UUID, or all controls in a room.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "name_or_uuid": { "type": "string" },
                    "room": room_prop,
                    "all_in_room": { "type": "string", "description": "Room name to turn on every control in" },
                },
            },
        }),
        json!({
            "name": "turn_off",
            "description": "Turn off a control by name or UUID, or all controls in a room.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "name_or_uuid": { "type": "string" },
                    "room": room_prop,
                    "all_in_room": { "type": "string", "description": "Room name to turn off every control in" },
                },
            },
        }),
        json!({
            "name": "set_blind",
            "description": "Control a blind/shade/awning: action is one of up, down, stop, shade, pos.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "name_or_uuid": { "type": "string" },
                    "action": { "type": "string", "enum": ["up", "down", "stop", "shade", "pos"] },
                    "position": { "type": "number", "description": "0-100, required for the 'pos' action" },
                    "room": room_prop,
                },
                "required": ["name_or_uuid", "action"],
            },
        }),
        json!({
            "name": "set_light",
            "description": "Control a light: mode 'mood' (value: plus|minus|off|<mood-id>), 'dim' (value: 0-100), or 'color' (value: hex '#RRGGBB' or 'hsv(h,s,v)').",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "name_or_uuid": { "type": "string" },
                    "mode": { "type": "string", "enum": ["mood", "dim", "color"] },
                    "value": { "type": "string" },
                    "room": room_prop,
                },
                "required": ["name_or_uuid", "mode", "value"],
            },
        }),
        json!({
            "name": "set_thermostat",
            "description": "Control or read a thermostat. Omit action/value to just read current state. action: temp|mode|override; value: temperature or mode name (auto|eco|comfort|manual); duration_minutes applies to override.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "name_or_uuid": { "type": "string" },
                    "action": { "type": "string", "enum": ["temp", "mode", "override"] },
                    "value": { "type": "string" },
                    "duration_minutes": { "type": "integer" },
                    "room": room_prop,
                },
                "required": ["name_or_uuid"],
            },
        }),
        json!({
            "name": "set_alarm",
            "description": "Control an alarm panel: action is one of arm, arm-home, disarm, quit.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "name_or_uuid": { "type": "string" },
                    "action": { "type": "string", "enum": ["arm", "arm-home", "disarm", "quit"] },
                    "no_motion": { "type": "boolean", "description": "Arm without motion detection" },
                    "code": { "type": "string", "description": "PIN code for arm/disarm" },
                    "room": room_prop,
                },
                "required": ["name_or_uuid", "action"],
            },
        }),
        json!({
            "name": "set_door_lock",
            "description": "Control a door lock: action is one of lock, unlock, open.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "name_or_uuid": { "type": "string" },
                    "action": { "type": "string", "enum": ["lock", "unlock", "open"] },
                    "room": room_prop,
                },
                "required": ["name_or_uuid", "action"],
            },
        }),
        json!({
            "name": "send_command",
            "description": "Send a raw Loxone command string to a control (escape hatch for actions not covered by other tools).",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "name_or_uuid": { "type": "string" },
                    "command": { "type": "string" },
                    "room": room_prop,
                    "secured": { "type": "string", "description": "Visualization password hash, for secured commands" },
                },
                "required": ["name_or_uuid", "command"],
            },
        }),
        json!({
            "name": "list_sensors",
            "description": "List sensor readings: temperature, door/window, motion, smoke.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "type": { "type": "string", "enum": ["temperature", "door-window", "motion", "smoke", "all"] },
                    "room": room_prop,
                },
            },
        }),
        json!({
            "name": "get_status",
            "description": "Show Miniserver health. Set diag/net/bus/lan/all for extra diagnostic sections.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "diag": { "type": "boolean" },
                    "net": { "type": "boolean" },
                    "bus": { "type": "boolean" },
                    "lan": { "type": "boolean" },
                    "all": { "type": "boolean" },
                },
            },
        }),
        json!({
            "name": "get_weather",
            "description": "Show current weather data, optionally with a 7-day forecast.",
            "inputSchema": { "type": "object", "properties": { "forecast": { "type": "boolean" } } },
        }),
        json!({
            "name": "get_energy",
            "description": "Show energy meter readings, optionally filtered by room.",
            "inputSchema": { "type": "object", "properties": { "room": room_prop } },
        }),
        json!({
            "name": "list_extensions",
            "description": "List connected Loxone extensions and devices.",
            "inputSchema": { "type": "object", "properties": {} },
        }),
        json!({
            "name": "get_health",
            "description": "Device health dashboard: battery, signal, offline, bus errors.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "device_type": { "type": "string", "enum": ["tree", "air"] },
                    "problems": { "type": "boolean", "description": "Only show devices with problems" },
                },
            },
        }),
        json!({
            "name": "run_scene",
            "description": "Run a saved multi-step scene by name.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "scene": { "type": "string" },
                    "dry_run": { "type": "boolean", "description": "Preview without executing" },
                },
                "required": ["scene"],
            },
        }),
        json!({
            "name": "list_scenes",
            "description": "List all saved scenes.",
            "inputSchema": { "type": "object", "properties": {} },
        }),
        json!({
            "name": "run_lox",
            "description": "Escape hatch: run any `lox` CLI subcommand not covered by the other tools (e.g. autopilot, files, ctx, config). Pass the arguments exactly as you would on the command line, without the leading 'lox'. Destructive or long-running commands (reboot, update install, watch, stream, discover, otel serve) are blocked.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "args": {
                        "type": "array",
                        "items": { "type": "string" },
                        "description": "e.g. [\"autopilot\", \"ls\"]",
                    },
                },
                "required": ["args"],
            },
        }),
    ]
}
