use std::net::TcpListener;
use std::process::{Child, Command, Stdio};
use std::time::Duration;

use hmac::{Hmac, Mac};
use reqwest::multipart::{Form, Part};
use serde_json::{json, Value};
use sha2::Sha256;
use tempfile::tempdir;

type HmacSha256 = Hmac<Sha256>;

struct ChildGuard(Child);

impl Drop for ChildGuard {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

fn free_port() -> u16 {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    listener.local_addr().unwrap().port()
}

#[tokio::test]
async fn test_serve_security_upload_webdelta_hmac_and_mcp_http() {
    let dir = tempdir().unwrap();
    let cfg_path = dir.path().join("academic-agent.toml");
    let db_path = dir.path().join("state.redb");

    let secret = "webdelta-hmac-secret";
    let token = "api-bearer-secret";
    let toml_cfg = academic_core::Config::starter_toml()
        .replace(
            "api_token = \"${ACADEMIC_API_TOKEN}\"",
            &format!("api_token = \"{token}\""),
        )
        .replace(
            "webdelta_webhook_secret = \"${WEBDELTA_WEBHOOK_SECRET}\"",
            &format!("webdelta_webhook_secret = \"{secret}\""),
        );
    std::fs::write(&cfg_path, toml_cfg).unwrap();

    let port = free_port();
    let bind = format!("127.0.0.1:{port}");
    let child = Command::new(env!("CARGO_BIN_EXE_academic-agent"))
        .current_dir(dir.path())
        .args([
            "--config",
            cfg_path.to_str().unwrap(),
            "--db",
            db_path.to_str().unwrap(),
            "serve",
            "--bind",
            &bind,
        ])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let _guard = ChildGuard(child);

    let client = reqwest::Client::new();
    let base = format!("http://{bind}");

    // Wait until /health returns 200 OK
    let mut ready = false;
    for _ in 0..40 {
        if let Ok(resp) = client.get(format!("{base}/health")).send().await {
            if resp.status().is_success() {
                ready = true;
                break;
            }
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    assert!(ready, "serve endpoint did not become healthy in time");

    // 1. Unauthorized without Bearer token
    let unauth = client
        .get(format!("{base}/api/tasks"))
        .send()
        .await
        .unwrap();
    assert_eq!(unauth.status().as_u16(), 401);

    // 2. Reject path traversal filename on multipart upload
    let bad_part = Part::bytes(b"Course: AI\nTitle: Hack".to_vec()).file_name("../../evil.txt");
    let bad_form = Form::new().part("file", bad_part);
    let bad_upload = client
        .post(format!("{base}/api/upload?heuristic=true&notify=false"))
        .bearer_auth(token)
        .multipart(bad_form)
        .send()
        .await
        .unwrap();
    assert_eq!(bad_upload.status().as_u16(), 400);

    // 3. Valid upload stores task and generates .ics download
    let ok_part = Part::bytes(
        b"Course: Software Engineering\nTitle: Architecture RFC\nDeadline: 2026-12-18 23:59"
            .to_vec(),
    )
    .file_name("rfc.txt");
    let ok_form = Form::new().part("file", ok_part);
    let ok_upload = client
        .post(format!("{base}/api/upload?heuristic=true&notify=false"))
        .bearer_auth(token)
        .multipart(ok_form)
        .send()
        .await
        .unwrap();
    assert_eq!(ok_upload.status().as_u16(), 200);
    let uploaded: Value = ok_upload.json().await.unwrap();
    let ics_name = uploaded["task"]["ics_file"].as_str().unwrap();

    let cal_ok = client
        .get(format!("{base}/api/calendar/{ics_name}"))
        .bearer_auth(token)
        .send()
        .await
        .unwrap();
    assert_eq!(cal_ok.status().as_u16(), 200);
    assert!(cal_ok.text().await.unwrap().contains("BEGIN:VCALENDAR"));

    // 4. Reject calendar path traversal
    let cal_bad = client
        .get(format!("{base}/api/calendar/..%2Fsecret.ics"))
        .bearer_auth(token)
        .send()
        .await
        .unwrap();
    assert_eq!(cal_bad.status().as_u16(), 400);

    // 5. webdelta webhook HMAC validation (invalid -> 401, valid -> 200)
    let webhook_body = serde_json::to_vec(&json!({
        "event": "watch.changed",
        "watch_id": "campus-portal",
        "watch_name": "Campus Notice Board",
        "url": "https://lms.example.edu/announcements",
        "current_value": "Tugas: Laporan Akhir\nDeadline: 2026-12-22 23:59"
    }))
    .unwrap();

    let bad_hook = client
        .post(format!("{base}/api/webhooks/webdelta"))
        .header("Content-Type", "application/json")
        .header("X-WebDelta-Signature", "sha256=deadbeef")
        .body(webhook_body.clone())
        .send()
        .await
        .unwrap();
    assert_eq!(bad_hook.status().as_u16(), 401);

    let mut mac = HmacSha256::new_from_slice(secret.as_bytes()).unwrap();
    mac.update(&webhook_body);
    let valid_sig = format!("sha256={}", hex::encode(mac.finalize().into_bytes()));

    let ok_hook = client
        .post(format!("{base}/api/webhooks/webdelta"))
        .header("Content-Type", "application/json")
        .header("X-WebDelta-Signature", valid_sig)
        .body(webhook_body)
        .send()
        .await
        .unwrap();
    assert_eq!(ok_hook.status().as_u16(), 200);

    // 6. Streamable HTTP POST /mcp sees both tasks through the shared TaskStore
    let mcp_unauth = client
        .post(format!("{base}/mcp"))
        .body(r#"{"jsonrpc":"2.0","id":1,"method":"tools/list"}"#)
        .send()
        .await
        .unwrap();
    assert_eq!(mcp_unauth.status().as_u16(), 401);

    let mcp_tasks = client
        .post(format!("{base}/mcp"))
        .bearer_auth(token)
        .body(r#"{"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"list_tasks","arguments":{}}}"#)
        .send()
        .await
        .unwrap();
    assert_eq!(mcp_tasks.status().as_u16(), 200);
    let mcp_resp: Value = mcp_tasks.json().await.unwrap();
    let inner: Value =
        serde_json::from_str(mcp_resp["result"]["content"][0]["text"].as_str().unwrap()).unwrap();
    assert_eq!(inner["count"], 2);
}
