use std::io::{BufRead, BufReader, Write};
use std::process::{Command, Stdio};

use serde_json::Value;
use tempfile::tempdir;

#[test]
fn test_mcp_stdio_end_to_end() {
    let dir = tempdir().unwrap();
    let cfg_path = dir.path().join("academic-agent.toml");
    let db_path = dir.path().join("state.redb");

    std::fs::write(
        &cfg_path,
        academic_core::Config::starter_toml().replace(
            "api_key = \"${ACADEMIC_LLM_API_KEY}\"",
            "api_key = \"secret-literal\"",
        ),
    )
    .unwrap();

    let mut child = Command::new(env!("CARGO_BIN_EXE_academic-agent"))
        .current_dir(dir.path())
        .args([
            "--config",
            cfg_path.to_str().unwrap(),
            "--db",
            db_path.to_str().unwrap(),
            "mcp",
        ])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();

    let mut stdin = child.stdin.take().unwrap();
    let stdout = child.stdout.take().unwrap();
    let mut reader = BufReader::new(stdout);

    let send_recv = |stdin: &mut std::process::ChildStdin,
                     reader: &mut BufReader<std::process::ChildStdout>,
                     msg: &str|
     -> Value {
        writeln!(stdin, "{msg}").unwrap();
        stdin.flush().unwrap();
        let mut line = String::new();
        reader.read_line(&mut line).unwrap();
        serde_json::from_str(line.trim()).expect("valid JSON-RPC response")
    };

    let init = send_recv(
        &mut stdin,
        &mut reader,
        r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{}}"#,
    );
    assert_eq!(init["result"]["serverInfo"]["name"], "academic-agent");

    let tools = send_recv(
        &mut stdin,
        &mut reader,
        r#"{"jsonrpc":"2.0","id":2,"method":"tools/list"}"#,
    );
    assert_eq!(tools["result"]["tools"].as_array().unwrap().len(), 8);

    let added = send_recv(
        &mut stdin,
        &mut reader,
        r#"{"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"add_task","arguments":{"title":"OS Kernel Lab","course":"Operating Systems","deadline":"2026-12-20 23:59","priority":"HIGH"}}}"#,
    );
    let added_json: Value =
        serde_json::from_str(added["result"]["content"][0]["text"].as_str().unwrap()).unwrap();
    assert_eq!(added_json["task"]["id"], 1);

    let listed = send_recv(
        &mut stdin,
        &mut reader,
        r#"{"jsonrpc":"2.0","id":4,"method":"tools/call","params":{"name":"list_tasks","arguments":{}}}"#,
    );
    let listed_json: Value =
        serde_json::from_str(listed["result"]["content"][0]["text"].as_str().unwrap()).unwrap();
    assert_eq!(listed_json["count"], 1);

    let cfg_read = send_recv(
        &mut stdin,
        &mut reader,
        r#"{"jsonrpc":"2.0","id":5,"method":"resources/read","params":{"uri":"academic://config"}}"#,
    );
    let cfg_txt = cfg_read["result"]["contents"][0]["text"].as_str().unwrap();
    assert!(!cfg_txt.contains("secret-literal"));
    assert!(cfg_txt.contains("<redacted>"));

    let cal_read = send_recv(
        &mut stdin,
        &mut reader,
        r#"{"jsonrpc":"2.0","id":6,"method":"resources/read","params":{"uri":"academic://calendar/1"}}"#,
    );
    assert!(cal_read["result"]["contents"][0]["text"]
        .as_str()
        .unwrap()
        .contains("BEGIN:VCALENDAR"));

    let feed_read = send_recv(
        &mut stdin,
        &mut reader,
        r#"{"jsonrpc":"2.0","id":7,"method":"resources/read","params":{"uri":"academic://calendar/feed"}}"#,
    );
    let feed_txt = feed_read["result"]["contents"][0]["text"].as_str().unwrap();
    assert!(feed_txt.contains("BEGIN:VCALENDAR"));
    assert!(feed_txt.contains("X-WR-CALNAME:academic-agent Deadlines"));
    assert!(feed_txt.contains("OS Kernel Lab"));

    drop(stdin);
    let status = child.wait().unwrap();
    assert!(status.success());
}
