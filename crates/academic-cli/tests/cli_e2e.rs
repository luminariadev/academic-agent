use std::fs::File;
use std::io::Write;
use std::process::Command;

use serde_json::Value;
use tempfile::tempdir;

fn academic_bin() -> Command {
    Command::new(env!("CARGO_BIN_EXE_academic-agent"))
}

#[test]
fn test_cli_full_offline_lifecycle() {
    let dir = tempdir().unwrap();
    let cfg_path = dir.path().join("academic-agent.toml");
    let db_path = dir.path().join("state.redb");

    // 1. init & validate
    let init_out = academic_bin()
        .current_dir(dir.path())
        .args(["--config", cfg_path.to_str().unwrap(), "init"])
        .output()
        .unwrap();
    assert!(
        init_out.status.success(),
        "init stderr: {}",
        String::from_utf8_lossy(&init_out.stderr)
    );

    let val_out = academic_bin()
        .current_dir(dir.path())
        .args([
            "--config",
            cfg_path.to_str().unwrap(),
            "--db",
            db_path.to_str().unwrap(),
            "validate",
        ])
        .output()
        .unwrap();
    assert!(
        val_out.status.success(),
        "validate stderr: {}",
        String::from_utf8_lossy(&val_out.stderr)
    );

    // 2. scan a .txt assignment with heuristic extractor
    let txt_path = dir.path().join("assignment.txt");
    std::fs::write(
        &txt_path,
        "Course: Distributed Systems\nTitle: Raft Lab\nDeadline: 2026-12-10 23:59\nDeliverable: PDF report",
    )
    .unwrap();

    let scan1 = academic_bin()
        .current_dir(dir.path())
        .args([
            "--config",
            cfg_path.to_str().unwrap(),
            "--db",
            db_path.to_str().unwrap(),
            "--json",
            "scan",
            txt_path.to_str().unwrap(),
            "--heuristic",
        ])
        .output()
        .unwrap();
    assert!(
        scan1.status.success(),
        "scan1 failed: {}",
        String::from_utf8_lossy(&scan1.stderr)
    );
    let res1: Value = serde_json::from_slice(&scan1.stdout).unwrap();
    assert_eq!(res1["duplicate"], false);
    assert_eq!(res1["task"]["course"], "Distributed Systems");

    // 3. rescanning identical content must deduplicate cleanly
    let scan2 = academic_bin()
        .current_dir(dir.path())
        .args([
            "--config",
            cfg_path.to_str().unwrap(),
            "--db",
            db_path.to_str().unwrap(),
            "--json",
            "scan",
            txt_path.to_str().unwrap(),
            "--heuristic",
        ])
        .output()
        .unwrap();
    assert!(scan2.status.success());
    let res2: Value = serde_json::from_slice(&scan2.stdout).unwrap();
    assert_eq!(res2["duplicate"], true);
    assert_eq!(res2["task"]["id"], res1["task"]["id"]);

    // 4. scan a generated .docx document
    let docx_path = dir.path().join("quiz.docx");
    {
        let file = File::create(&docx_path).unwrap();
        let mut zw = zip::ZipWriter::new(file);
        zw.start_file(
            "word/document.xml",
            zip::write::SimpleFileOptions::default(),
        )
        .unwrap();
        let xml = r#"<?xml version="1.0" encoding="UTF-8"?>
<w:document xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main">
  <w:body>
    <w:p><w:r><w:t>Mata Kuliah: Jaringan Komputer</w:t></w:r></w:p>
    <w:p><w:r><w:t>Judul: Kuis 2 BGP dan SDN</w:t></w:r></w:p>
    <w:p><w:r><w:t>Deadline: 2026-12-12 10:00</w:t></w:r></w:p>
  </w:body>
</w:document>"#;
        zw.write_all(xml.as_bytes()).unwrap();
        zw.finish().unwrap();
    }
    let scan_docx = academic_bin()
        .current_dir(dir.path())
        .args([
            "--config",
            cfg_path.to_str().unwrap(),
            "--db",
            db_path.to_str().unwrap(),
            "--json",
            "scan",
            docx_path.to_str().unwrap(),
            "--heuristic",
        ])
        .output()
        .unwrap();
    assert!(scan_docx.status.success());
    let res_docx: Value = serde_json::from_slice(&scan_docx.stdout).unwrap();
    assert_eq!(res_docx["duplicate"], false);
    assert_eq!(res_docx["task"]["course"], "Jaringan Komputer");

    // 5. list tasks & CSV export, snooze, done, cron-tick, digest dry-run
    let tasks_out = academic_bin()
        .current_dir(dir.path())
        .args([
            "--config",
            cfg_path.to_str().unwrap(),
            "--db",
            db_path.to_str().unwrap(),
            "tasks",
            "--export",
            "csv",
        ])
        .output()
        .unwrap();
    assert!(tasks_out.status.success());
    let csv = String::from_utf8_lossy(&tasks_out.stdout);
    assert!(csv.contains("Raft Lab"));
    assert!(csv.contains("Kuis 2 BGP dan SDN"));

    let snooze = academic_bin()
        .current_dir(dir.path())
        .args([
            "--config",
            cfg_path.to_str().unwrap(),
            "--db",
            db_path.to_str().unwrap(),
            "snooze",
            "1",
            "--hours",
            "12",
        ])
        .output()
        .unwrap();
    assert!(snooze.status.success());

    let done = academic_bin()
        .current_dir(dir.path())
        .args([
            "--config",
            cfg_path.to_str().unwrap(),
            "--db",
            db_path.to_str().unwrap(),
            "done",
            "1",
        ])
        .output()
        .unwrap();
    assert!(done.status.success());

    let cron = academic_bin()
        .current_dir(dir.path())
        .args([
            "--config",
            cfg_path.to_str().unwrap(),
            "--db",
            db_path.to_str().unwrap(),
            "--json",
            "cron-tick",
            "--dry-run",
        ])
        .output()
        .unwrap();
    assert!(cron.status.success());
    let cron_json: Value = serde_json::from_slice(&cron.stdout).unwrap();
    assert_eq!(cron_json["dry_run"], true);

    let digest = academic_bin()
        .current_dir(dir.path())
        .args([
            "--config",
            cfg_path.to_str().unwrap(),
            "--db",
            db_path.to_str().unwrap(),
            "--json",
            "digest",
        ])
        .output()
        .unwrap();
    assert!(digest.status.success());
    let digest_json: Value = serde_json::from_slice(&digest.stdout).unwrap();
    assert_eq!(digest_json["dry_run"], true);
    assert_eq!(digest_json["open_task_count"], 1);
}
