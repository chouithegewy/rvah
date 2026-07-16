//! End-to-end CLI tests: spawn the real `rvah` binary (STP CLI suite).

use std::path::Path;
use std::process::{Command, Output};

use rvah_testkit::fixture_manifest;

fn rvah(args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_rvah"))
        .args(args)
        .output()
        .expect("spawn rvah")
}

fn stdout(out: &Output) -> String {
    String::from_utf8_lossy(&out.stdout).into_owned()
}

fn stderr(out: &Output) -> String {
    String::from_utf8_lossy(&out.stderr).into_owned()
}

/// Write an add-on source dir + keys, package and sign it via the CLI.
/// Returns (package path, pubkey path).
fn package_fixture(root: &Path, id: &str) -> (String, String) {
    let src = root.join("addon-src");
    std::fs::create_dir_all(&src).unwrap();
    std::fs::write(
        src.join("manifest.toml"),
        fixture_manifest(id, &["catalog", "search", "resolve"], &[], &[]),
    )
    .unwrap();
    std::fs::write(src.join("main"), "entry").unwrap();

    let keys = root.join("keys");
    let out = rvah(&["keygen", "-o", keys.to_str().unwrap()]);
    assert!(out.status.success(), "keygen failed: {}", stderr(&out));

    let pkg = root.join("addon.rvah");
    let out = rvah(&[
        "package",
        src.to_str().unwrap(),
        "-o",
        pkg.to_str().unwrap(),
        "--key",
        keys.join("signing.key").to_str().unwrap(),
    ]);
    assert!(out.status.success(), "package failed: {}", stderr(&out));

    (
        pkg.to_str().unwrap().to_string(),
        keys.join("verify.pub").to_str().unwrap().to_string(),
    )
}

#[test]
fn validate_accepts_good_rejects_bad_manifest() {
    let tmp = tempfile::tempdir().unwrap();

    let good = tmp.path().join("good.toml");
    std::fs::write(
        &good,
        fixture_manifest("com.example.good", &["catalog"], &[], &[]),
    )
    .unwrap();
    let out = rvah(&["validate", good.to_str().unwrap()]);
    assert!(out.status.success(), "stderr: {}", stderr(&out));
    assert!(stdout(&out).contains("OK: com.example.good"));

    let bad = tmp.path().join("bad.toml");
    std::fs::write(&bad, "[addon]\nname = \"No Id\"\n").unwrap();
    let out = rvah(&["validate", bad.to_str().unwrap()]);
    assert_eq!(out.status.code(), Some(1));
    assert!(stderr(&out).contains("invalid manifest"));
}

#[test]
fn package_install_browse_flow() {
    let tmp = tempfile::tempdir().unwrap();
    let (pkg, pubkey) = package_fixture(tmp.path(), "com.example.flow");
    let data = tmp.path().join("data");
    let data = data.to_str().unwrap();

    // Install without approval shows permissions but refuses (FR-005).
    let out = rvah(&["install", &pkg, "--pubkey", &pubkey, "--data-dir", data]);
    assert_eq!(out.status.code(), Some(1));
    assert!(stdout(&out).contains("network"));

    let out = rvah(&[
        "install",
        &pkg,
        "--pubkey",
        &pubkey,
        "--data-dir",
        data,
        "--approve",
    ]);
    assert!(out.status.success(), "install failed: {}", stderr(&out));
    assert!(stdout(&out).contains("installed com.example.flow v1.0.0"));

    let out = rvah(&["addons", "--data-dir", data]);
    assert!(stdout(&out).contains("com.example.flow"));

    // Browse through the simulator runtime (FR-027).
    let out = rvah(&["browse", "com.example.flow", "--data-dir", data]);
    assert!(out.status.success(), "browse failed: {}", stderr(&out));
    assert!(stdout(&out).contains("Movies"));

    let out = rvah(&["browse", "com.example.flow", "/movies", "--data-dir", data]);
    assert!(stdout(&out).contains("First Movie"));

    // Resolve prints a validated source; play persists watch state.
    let out = rvah(&["resolve", "com.example.flow", "m1", "--data-dir", data]);
    assert!(stdout(&out).contains("https://cdn.example.com/m1.mp4"));

    let out = rvah(&["play", "com.example.flow", "m1", "--data-dir", data]);
    assert!(out.status.success(), "play failed: {}", stderr(&out));
    let text = stdout(&out);
    assert!(text.contains("progress"));
    assert!(text.contains("watched=true"));
}

#[test]
fn install_rejects_tampered_package() {
    let tmp = tempfile::tempdir().unwrap();
    let (pkg, pubkey) = package_fixture(tmp.path(), "com.example.tampered");
    let data = tmp.path().join("data");

    let mut bytes = std::fs::read(&pkg).unwrap();
    let last = bytes.len() - 1;
    bytes[last] ^= 0xFF;
    std::fs::write(&pkg, bytes).unwrap();

    let out = rvah(&[
        "install",
        &pkg,
        "--pubkey",
        &pubkey,
        "--data-dir",
        data.to_str().unwrap(),
        "--approve",
    ]);
    assert_eq!(out.status.code(), Some(1));
    assert!(
        stderr(&out).contains("signature"),
        "stderr: {}",
        stderr(&out)
    );
}

#[test]
fn diagnostics_output_contains_no_secret() {
    let tmp = tempfile::tempdir().unwrap();
    let data = tmp.path().join("data");
    std::fs::create_dir_all(&data).unwrap();

    // Simulate a leaky event written by something outside the host bridge.
    let db = rvah_data::Database::open(&data.join("rvah.sqlite")).unwrap();
    db.log_event(&rvah_data::DiagnosticEvent {
        level: "error".into(),
        addon_id: Some("com.example.leaky".into()),
        code: "http".into(),
        message: "request failed: Authorization: Bearer supersecret123".into(),
    })
    .unwrap();
    drop(db);

    let out = rvah(&["diagnostics", "--data-dir", data.to_str().unwrap()]);
    assert!(out.status.success(), "stderr: {}", stderr(&out));
    let text = stdout(&out);
    assert!(text.contains("com.example.leaky"));
    assert!(!text.contains("supersecret123"), "secret leaked: {text}");
    assert!(text.contains("[REDACTED]"));
}
