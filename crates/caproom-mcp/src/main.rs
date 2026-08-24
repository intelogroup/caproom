use std::io::{BufRead, Write};

fn main() {
    let stdin = std::io::stdin();
    let mut stdout = std::io::stdout();
    for line in stdin.lock().lines() {
        let line = match line { Ok(l) => l, Err(_) => break };
        if line.trim().is_empty() { continue; }
        let v: serde_json::Value = match serde_json::from_str(&line) {
            Ok(j) => j,
            Err(e) => {
                let err = serde_json::json!({"jsonrpc":"2.0","id":null,"error":{"code":-32700,"message":e.to_string()}});
                writeln!(stdout, "{}", err).unwrap(); stdout.flush().unwrap(); continue;
            }
        };
        let id = v.get("id").cloned().unwrap_or(serde_json::Value::Null);
        let method = v.get("method").and_then(|m| m.as_str()).unwrap_or("");
        let resp = match method {
            "initialize" => serde_json::json!({
                "jsonrpc":"2.0","id":id,
                "result":{
                    "protocolVersion":"2024-11-05",
                    "serverInfo":{"name":"caproom-mcp","version":env!("CARGO_PKG_VERSION")},
                    "capabilities":{"tools":{"listChanged":false}}
                }
            }),
            "notifications/initialized" => continue,
            "tools/list" => serde_json::json!({
                "jsonrpc":"2.0","id":id,
                "result":{
                    "tools":[
                        {"name":"top","description":"tree inventory — typed TopResponse","inputSchema":{"type":"object","properties":{"pid":{"type":"integer"}},"additionalProperties":false}},
                        {"name":"park","description":"SIGSTOP single PID","inputSchema":{"type":"object","properties":{"pid":{"type":"integer"}},"required":["pid"]}},
                        {"name":"park_tree","description":"SIGSTOP whole tree — PID reuse guarded","inputSchema":{"type":"object","properties":{"pid":{"type":"integer"}},"required":["pid"]}},
                        {"name":"wake","description":"SIGCONT single PID","inputSchema":{"type":"object","properties":{"pid":{"type":"integer"}},"required":["pid"]}},
                        {"name":"wake_tree","description":"SIGCONT whole tree","inputSchema":{"type":"object","properties":{"pid":{"type":"integer"}},"required":["pid"]}},
                        {"name":"run","description":"execute command under caproom run --limit","inputSchema":{"type":"object","properties":{"command":{"type":"array","items":{"type":"string"}},"limit_mb":{"type":"integer"}},"required":["command"]}},
                        {"name":"freemem","description":"free memory %","inputSchema":{"type":"object","properties":{}}},
                        {"name":"status","description":"ps stat for pid","inputSchema":{"type":"object","properties":{"pid":{"type":"integer"}},"required":["pid"]}}
                    ]
                }
            }),
            "tools/call" => {
                let params = v.get("params").cloned().unwrap_or(serde_json::Value::Null);
                let name = params.get("name").and_then(|n| n.as_str()).unwrap_or("");
                let args = params.get("arguments").cloned().unwrap_or(serde_json::Value::Null);
                let result_text = match name {
                    "top" => {
                        let pid = args.get("pid").and_then(|p| p.as_i64()).map(|p| p as i32);
                        let r = caproom_mcp::handle_top(pid);
                        serde_json::to_string(&r).unwrap()
                    },
                    "park" => {
                        let pid = args.get("pid").and_then(|p| p.as_i64()).unwrap_or(0) as i32;
                        serde_json::to_string(&caproom_mcp::handle_park(pid)).unwrap()
                    },
                    "park_tree" => {
                        let pid = args.get("pid").and_then(|p| p.as_i64()).unwrap_or(0) as i32;
                        serde_json::to_string(&caproom_mcp::handle_park_tree(pid)).unwrap()
                    },
                    "wake" => {
                        let pid = args.get("pid").and_then(|p| p.as_i64()).unwrap_or(0) as i32;
                        serde_json::to_string(&caproom_mcp::handle_wake(pid)).unwrap()
                    },
                    "wake_tree" => {
                        let pid = args.get("pid").and_then(|p| p.as_i64()).unwrap_or(0) as i32;
                        serde_json::to_string(&caproom_mcp::handle_wake_tree(pid)).unwrap()
                    },
                    "run" => {
                        let cmd = args.get("command").and_then(|c| c.as_array()).map(|a| a.iter().filter_map(|v| v.as_str().map(|s| s.to_string())).collect::<Vec<_>>()).unwrap_or_default();
                        let limit = args.get("limit_mb").and_then(|l| l.as_u64()).unwrap_or(4096);
                        serde_json::to_string(&caproom_mcp::handle_run(cmd, limit)).unwrap()
                    },
                    "freemem" => {
                        let pct = caproom_core::pressure::free_mem_pct();
                        serde_json::to_string(&serde_json::json!({"free_pct": pct})).unwrap()
                    },
                    "status" => {
                        let pid = args.get("pid").and_then(|p| p.as_i64()).unwrap_or(0) as i32;
                        let snap = caproom_core::collector::snapshot_current_user();
                        if let Some(pr) = snap.by_pid(pid) {
                            serde_json::to_string(&serde_json::json!({"pid": pr.pid, "state": pr.state.to_string(), "footprint_kb": pr.footprint_kb, "cmd": pr.cmd})).unwrap()
                        } else {
                            serde_json::to_string(&serde_json::json!({"error": format!("no such pid {}", pid)})).unwrap()
                        }
                    },
                    _ => serde_json::to_string(&serde_json::json!({"error": format!("unknown tool {}", name)})).unwrap()
                };
                serde_json::json!({
                    "jsonrpc":"2.0","id":id,
                    "result":{"content":[{"type":"text","text": result_text}]}
                })
            },
            _ => serde_json::json!({"jsonrpc":"2.0","id":id,"error":{"code":-32601,"message": format!("unknown method {}", method)}})
        };
        writeln!(stdout, "{}", resp).unwrap();
        stdout.flush().unwrap();
    }
}
