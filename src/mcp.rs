//! A small MCP server so an AI app (Claude, Cursor and others) can read your meetings.
//!
//! Run as `Notary mcp`: it speaks JSON-RPC over stdin and stdout, one message per line, and only reads.
//! It is off until you turn it on in Settings, because it hands your notes to whichever app you connect.

use crate::data::Meeting;
use crate::export;
use crate::search;
use crate::settings::Settings;
use crate::store::Store;
use serde_json::{Value, json};
use std::io::{BufRead, Write};

const OFF: &str = "The Notary MCP server is switched off. Open Notary, go to Settings and turn on \"Let AI apps read your meetings\".";

fn tools() -> Value {
    let id = json!({"type": "object", "properties": {"id": {"type": "integer", "description": "Meeting id from list_meetings"}}, "required": ["id"]});
    json!([
        {"name": "list_meetings", "description": "Lists saved meetings, newest first, with id, title, date and number of lines.",
         "inputSchema": {"type": "object", "properties": {"limit": {"type": "integer", "description": "How many, default 20"}}}},
        {"name": "get_meeting", "description": "The notes of one meeting: summary, decisions, next steps and who was there.", "inputSchema": id},
        {"name": "get_transcript", "description": "The full transcript of one meeting with speaker names and times.", "inputSchema": id},
        {"name": "search_meetings", "description": "Finds meetings where every word of the query appears in the title, transcript, summary or next steps.",
         "inputSchema": {"type": "object", "properties": {"query": {"type": "string"}}, "required": ["query"]}},
        {"name": "list_action_items", "description": "Next steps across all meetings.",
         "inputSchema": {"type": "object", "properties": {"open_only": {"type": "boolean", "description": "Only items not ticked off, default true"}}}},
        {"name": "meeting_count", "description": "How many meetings are saved.", "inputSchema": {"type": "object", "properties": {}}}
    ])
}

fn day(m: &Meeting) -> String {
    chrono::DateTime::from_timestamp(m.started, 0)
        .map(|d| d.with_timezone(&chrono::Local).format("%Y-%m-%d %H:%M").to_string())
        .unwrap_or_default()
}

fn text(s: String) -> Value {
    json!({"content": [{"type": "text", "text": s}]})
}

fn failure(s: &str) -> Value {
    json!({"content": [{"type": "text", "text": s}], "isError": true})
}

fn call(store: &Store, name: &str, args: &Value) -> Value {
    let meetings = match store.load_meetings() {
        Ok(m) => m,
        Err(e) => return failure(&format!("Could not read the meetings: {e:#}")),
    };
    let find = |args: &Value| args.get("id").and_then(Value::as_i64).and_then(|id| meetings.iter().find(|m| m.id == id));
    match name {
        "meeting_count" => text(meetings.len().to_string()),
        "list_meetings" => {
            let limit = args.get("limit").and_then(Value::as_u64).unwrap_or(20) as usize;
            let rows: Vec<String> = meetings
                .iter()
                .take(limit)
                .map(|m| format!("{} | {} | {} | {} lines", m.id, day(m), m.title, m.lines.len()))
                .collect();
            text(if rows.is_empty() { "No meetings yet.".into() } else { format!("id | date | title | lines\n{}", rows.join("\n")) })
        }
        "get_meeting" => match find(args) {
            Some(m) => text(export::notes_markdown(m)),
            None => failure("No meeting with that id."),
        },
        "get_transcript" => match find(args) {
            Some(m) if m.lines.is_empty() => text("This meeting has no transcript.".into()),
            Some(m) => text(export::transcript_text(m)),
            None => failure("No meeting with that id."),
        },
        "search_meetings" => {
            let terms = search::terms(args.get("query").and_then(Value::as_str).unwrap_or(""));
            if terms.is_empty() {
                return failure("Give a query.");
            }
            let rows: Vec<String> = meetings
                .iter()
                .filter_map(|m| search::search(m, &terms).map(|h| format!("{} | {} | {}{}", m.id, day(m), m.title, h.snippet.map(|s| format!(" | \"{s}\"")).unwrap_or_default())))
                .take(20)
                .collect();
            text(if rows.is_empty() { "Nothing found.".into() } else { rows.join("\n") })
        }
        "list_action_items" => {
            let open_only = args.get("open_only").and_then(Value::as_bool).unwrap_or(true);
            let rows: Vec<String> = meetings
                .iter()
                .flat_map(|m| {
                    m.actions
                        .iter()
                        .filter(move |_| true)
                        .filter(|a| !open_only || !a.done)
                        .map(move |a| format!("[{}] {} ({}) , {} (meeting {})", if a.done { "x" } else { " " }, a.label, a.owner, m.title, m.id))
                })
                .collect();
            text(if rows.is_empty() { "No next steps.".into() } else { rows.join("\n") })
        }
        _ => failure("Unknown tool."),
    }
}

/// Answers one JSON-RPC message. Notifications get no answer.
pub fn handle(store: &Store, settings: &Settings, request: &Value) -> Option<Value> {
    let id = request.get("id")?.clone();
    let method = request.get("method").and_then(Value::as_str).unwrap_or("");
    let params = request.get("params").cloned().unwrap_or(Value::Null);
    let result = match method {
        "initialize" => json!({
            "protocolVersion": params.get("protocolVersion").and_then(Value::as_str).unwrap_or("2024-11-05"),
            "capabilities": {"tools": {}},
            "serverInfo": {"name": "notary", "version": env!("CARGO_PKG_VERSION")},
            "instructions": if settings.mcp_enabled { "Read-only access to the user's Notary meetings: notes, transcripts, search and next steps." } else { OFF },
        }),
        "ping" => json!({}),
        "tools/list" => json!({"tools": if settings.mcp_enabled { tools() } else { json!([]) }}),
        "tools/call" => {
            if !settings.mcp_enabled {
                failure(OFF)
            } else {
                let name = params.get("name").and_then(Value::as_str).unwrap_or("");
                call(store, name, params.get("arguments").unwrap_or(&Value::Null))
            }
        }
        _ => return Some(json!({"jsonrpc": "2.0", "id": id, "error": {"code": -32601, "message": "Method not found"}})),
    };
    Some(json!({"jsonrpc": "2.0", "id": id, "result": result}))
}

/// Serves MCP on stdin and stdout until the client closes the pipe.
pub fn serve() {
    let store = match Store::open() {
        Ok(s) => s,
        Err(e) => {
            eprintln!("Notary MCP: {e:#}");
            std::process::exit(1);
        }
    };
    let stdout = std::io::stdout();
    for line in std::io::stdin().lock().lines() {
        let Ok(line) = line else { break };
        let Ok(request) = serde_json::from_str::<Value>(&line) else { continue };
        // Read the settings every time, so switching the server on or off applies at once.
        let settings = Settings::load();
        if let Some(answer) = handle(&store, &settings, &request) {
            let mut out = stdout.lock();
            if writeln!(out, "{answer}").and_then(|_| out.flush()).is_err() {
                break;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::data::{Speaker, line};

    fn store_with_meeting() -> Store {
        let s = Store::open_at(std::path::Path::new(":memory:")).unwrap();
        let id = s.create_meeting("Launch planning", 1_795_000_000).unwrap();
        s.add_line(id, &line("00:05", "Lena", Speaker::Other, "We ship on Friday.")).unwrap();
        s
    }

    fn ask(store: &Store, on: bool, method: &str, params: Value) -> Value {
        let settings = Settings { mcp_enabled: on, ..Settings::default() };
        handle(store, &settings, &json!({"jsonrpc": "2.0", "id": 1, "method": method, "params": params})).unwrap()
    }

    #[test]
    fn off_by_default_hides_the_tools_and_refuses_calls() {
        let s = store_with_meeting();
        assert_eq!(ask(&s, false, "tools/list", json!({}))["result"]["tools"], json!([]));
        let r = ask(&s, false, "tools/call", json!({"name": "list_meetings", "arguments": {}}));
        assert_eq!(r["result"]["isError"], true);
    }

    #[test]
    fn lists_reads_and_searches_when_on() {
        let s = store_with_meeting();
        assert_eq!(ask(&s, true, "tools/list", json!({}))["result"]["tools"].as_array().unwrap().len(), 6);
        let list = ask(&s, true, "tools/call", json!({"name": "list_meetings", "arguments": {}}));
        assert!(list["result"]["content"][0]["text"].as_str().unwrap().contains("Launch planning"));
        let t = ask(&s, true, "tools/call", json!({"name": "get_transcript", "arguments": {"id": 1}}));
        assert!(t["result"]["content"][0]["text"].as_str().unwrap().contains("Lena: We ship on Friday."));
        let found = ask(&s, true, "tools/call", json!({"name": "search_meetings", "arguments": {"query": "friday"}}));
        assert!(found["result"]["content"][0]["text"].as_str().unwrap().contains("Launch planning"));
        let missing = ask(&s, true, "tools/call", json!({"name": "get_meeting", "arguments": {"id": 99}}));
        assert_eq!(missing["result"]["isError"], true);
    }

    #[test]
    fn notifications_get_no_answer_and_unknown_methods_an_error() {
        let s = store_with_meeting();
        let settings = Settings::default();
        assert!(handle(&s, &settings, &json!({"jsonrpc": "2.0", "method": "notifications/initialized"})).is_none());
        assert_eq!(ask(&s, true, "nope", json!({}))["error"]["code"], -32601);
    }
}
