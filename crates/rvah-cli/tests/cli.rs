//! End-to-end CLI tests: spawn the real `rvah` binary (STP CLI suite).

use std::io::Write;
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
fn sign_preserves_existing_package_bytes_and_makes_them_installable() {
    let tmp = tempfile::tempdir().unwrap();
    let (pkg, pubkey) = package_fixture(tmp.path(), "com.example.resigned");
    let signature = format!("{pkg}.sig");
    let package_bytes = std::fs::read(&pkg).unwrap();

    std::fs::remove_file(&signature).unwrap();
    let out = rvah(&[
        "sign",
        &pkg,
        "--key",
        tmp.path().join("keys/signing.key").to_str().unwrap(),
    ]);
    assert!(out.status.success(), "sign failed: {}", stderr(&out));
    assert!(Path::new(&signature).is_file());
    assert_eq!(std::fs::read(&pkg).unwrap(), package_bytes);

    let out = rvah(&[
        "install",
        &pkg,
        "--pubkey",
        &pubkey,
        "--data-dir",
        tmp.path().join("data").to_str().unwrap(),
        "--approve",
    ]);
    assert!(out.status.success(), "install failed: {}", stderr(&out));
    assert!(stdout(&out).contains("installed com.example.resigned v1.0.0"));
}

#[test]
fn repo_index_check_install_flow() {
    let tmp = tempfile::tempdir().unwrap();
    let packages = tmp.path().join("packages");
    std::fs::create_dir_all(&packages).unwrap();
    let data = tmp.path().join("data");
    let data = data.to_str().unwrap();
    let keys = tmp.path().join("keys");
    let out = rvah(&["keygen", "-o", keys.to_str().unwrap()]);
    assert!(out.status.success());
    let key = keys.join("signing.key");
    let pubkey = keys.join("verify.pub").to_str().unwrap().to_string();

    // Package v1.0.0 into the repo dir and index it.
    let src = tmp.path().join("addon-src");
    std::fs::create_dir_all(&src).unwrap();
    std::fs::write(
        src.join("manifest.toml"),
        fixture_manifest("com.example.repoflow", &["catalog"], &[], &[]),
    )
    .unwrap();
    std::fs::write(src.join("main"), "entry").unwrap();
    let pkg_v1 = packages.join("repoflow-1.0.0.rvah");
    let out = rvah(&[
        "package",
        src.to_str().unwrap(),
        "-o",
        pkg_v1.to_str().unwrap(),
        "--key",
        key.to_str().unwrap(),
    ]);
    assert!(out.status.success(), "package: {}", stderr(&out));
    let index = tmp.path().join("index.json");
    let index_str = index.to_str().unwrap().to_string();
    let out = rvah(&[
        "repo-index",
        packages.to_str().unwrap(),
        "-o",
        &index_str,
        "--key",
        key.to_str().unwrap(),
    ]);
    assert!(out.status.success(), "repo-index: {}", stderr(&out));

    // Install v1.0.0 from the repo, then everything is up to date.
    let out = rvah(&[
        "repo-install",
        "com.example.repoflow",
        "--index",
        &index_str,
        "--pubkey",
        &pubkey,
        "--data-dir",
        data,
        "--approve",
    ]);
    assert!(out.status.success(), "repo-install: {}", stderr(&out));
    assert!(stdout(&out).contains("installed com.example.repoflow v1.0.0"));
    let out = rvah(&[
        "repo-check",
        "--index",
        &index_str,
        "--pubkey",
        &pubkey,
        "--data-dir",
        data,
    ]);
    assert!(stdout(&out).contains("up-to-date"));

    // Publish v1.1.0; check reports it; install upgrades.
    std::fs::write(
        src.join("manifest.toml"),
        fixture_manifest("com.example.repoflow", &["catalog"], &[], &[])
            .replace("version = \"1.0.0\"", "version = \"1.1.0\""),
    )
    .unwrap();
    let pkg_v2 = packages.join("repoflow-1.1.0.rvah");
    std::fs::remove_file(&pkg_v1).unwrap();
    std::fs::remove_file(format!("{}.sig", pkg_v1.display())).unwrap();
    let out = rvah(&[
        "package",
        src.to_str().unwrap(),
        "-o",
        pkg_v2.to_str().unwrap(),
        "--key",
        key.to_str().unwrap(),
    ]);
    assert!(out.status.success());
    let out = rvah(&[
        "repo-index",
        packages.to_str().unwrap(),
        "-o",
        &index_str,
        "--key",
        key.to_str().unwrap(),
    ]);
    assert!(out.status.success(), "re-index: {}", stderr(&out));

    let out = rvah(&[
        "repo-check",
        "--index",
        &index_str,
        "--pubkey",
        &pubkey,
        "--data-dir",
        data,
    ]);
    assert!(
        stdout(&out).contains("update\tcom.example.repoflow\t1.0.0 -> 1.1.0"),
        "repo-check: {}",
        stdout(&out)
    );
    let out = rvah(&[
        "repo-install",
        "com.example.repoflow",
        "--index",
        &index_str,
        "--pubkey",
        &pubkey,
        "--data-dir",
        data,
        "--approve",
    ]);
    assert!(out.status.success(), "upgrade: {}", stderr(&out));
    assert!(stdout(&out).contains("v1.1.0"));
}

#[test]
fn encrypted_profile_moves_a_linked_account_without_exposing_its_value() {
    use rvah_auth::{EncryptedFileVault, SecretVault};
    use std::process::Stdio;

    let tmp = tempfile::tempdir().unwrap();
    let (pkg, pubkey) = package_fixture(tmp.path(), "com.example.portable");
    let source = tmp.path().join("source");
    let target = tmp.path().join("target");
    let key = tmp.path().join("account.profile.key");
    let bundle = tmp.path().join("account.rvah-profile");

    let out = rvah(&[
        "install",
        &pkg,
        "--pubkey",
        &pubkey,
        "--data-dir",
        source.to_str().unwrap(),
        "--approve",
    ]);
    assert!(out.status.success(), "install failed: {}", stderr(&out));

    let mut child = Command::new(env!("CARGO_BIN_EXE_rvah"))
        .args([
            "secret-set",
            "com.example.portable",
            "oauth_token",
            "--data-dir",
            source.to_str().unwrap(),
        ])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(b"portable-secret\n")
        .unwrap();
    let out = child.wait_with_output().unwrap();
    assert!(out.status.success(), "secret-set failed: {}", stderr(&out));
    assert!(!stdout(&out).contains("portable-secret"));

    let out = rvah(&["profile-keygen", "-o", key.to_str().unwrap()]);
    assert!(out.status.success(), "keygen failed: {}", stderr(&out));
    let out = rvah(&[
        "profile-export",
        "-o",
        bundle.to_str().unwrap(),
        "--key",
        key.to_str().unwrap(),
        "--data-dir",
        source.to_str().unwrap(),
    ]);
    assert!(out.status.success(), "export failed: {}", stderr(&out));
    assert!(!stdout(&out).contains("portable-secret"));

    let out = rvah(&[
        "profile-import",
        bundle.to_str().unwrap(),
        "--key",
        key.to_str().unwrap(),
        "--data-dir",
        target.to_str().unwrap(),
    ]);
    assert!(out.status.success(), "import failed: {}", stderr(&out));
    assert!(!stdout(&out).contains("portable-secret"));

    let vault =
        EncryptedFileVault::open(target.join("secrets.vault"), target.join("vault.key")).unwrap();
    assert_eq!(
        vault
            .get("com.example.portable", "oauth_token")
            .unwrap()
            .as_deref(),
        Some("portable-secret")
    );

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(
            std::fs::metadata(&key).unwrap().permissions().mode() & 0o777,
            0o600
        );
        assert_eq!(
            std::fs::metadata(&bundle).unwrap().permissions().mode() & 0o777,
            0o600
        );
    }
}

#[test]
fn profile_restore_reinstalls_exact_signed_addon_before_credentials() {
    use rvah_auth::{EncryptedFileVault, SecretVault};
    use std::process::Stdio;

    let tmp = tempfile::tempdir().unwrap();
    let repo_dir = tmp.path().join("repo");
    std::fs::create_dir_all(&repo_dir).unwrap();
    let (pkg, pubkey) = package_fixture(&repo_dir, "com.example.rebuildable");
    let source = tmp.path().join("source");
    let target = tmp.path().join("target");
    let key = tmp.path().join("restore.profile.key");
    let bundle = tmp.path().join("restore.rvah-profile");
    let index = tmp.path().join("index.json");

    let out = rvah(&[
        "repo-index",
        repo_dir.to_str().unwrap(),
        "-o",
        index.to_str().unwrap(),
        "--key",
        repo_dir.join("keys/signing.key").to_str().unwrap(),
    ]);
    assert!(out.status.success(), "repo-index failed: {}", stderr(&out));

    let out = rvah(&[
        "install",
        &pkg,
        "--pubkey",
        &pubkey,
        "--data-dir",
        source.to_str().unwrap(),
        "--approve",
    ]);
    assert!(out.status.success(), "install failed: {}", stderr(&out));

    let mut child = Command::new(env!("CARGO_BIN_EXE_rvah"))
        .args([
            "secret-set",
            "com.example.rebuildable",
            "api_token",
            "--data-dir",
            source.to_str().unwrap(),
        ])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(b"restored-secret\n")
        .unwrap();
    assert!(child.wait_with_output().unwrap().status.success());

    assert!(rvah(&["profile-keygen", "-o", key.to_str().unwrap()])
        .status
        .success());
    assert!(rvah(&[
        "profile-export",
        "-o",
        bundle.to_str().unwrap(),
        "--key",
        key.to_str().unwrap(),
        "--data-dir",
        source.to_str().unwrap(),
    ])
    .status
    .success());

    let restore_args = [
        "profile-restore",
        bundle.to_str().unwrap(),
        "--key",
        key.to_str().unwrap(),
        "--index",
        index.to_str().unwrap(),
        "--pubkey",
        &pubkey,
        "--data-dir",
        target.to_str().unwrap(),
    ];
    let out = rvah(&restore_args);
    assert_eq!(out.status.code(), Some(1));
    assert!(stdout(&out).contains("restore\tcom.example.rebuildable\t1.0.0"));
    assert!(stderr(&out).contains("not approved"));
    assert!(
        !target.exists(),
        "dry-run restore mutated the target device"
    );

    let mut approved = restore_args.to_vec();
    approved.push("--approve");
    let out = rvah(&approved);
    assert!(out.status.success(), "restore failed: {}", stderr(&out));
    assert!(stdout(&out).contains("installed\tcom.example.rebuildable\t1.0.0"));
    assert!(stdout(&out).contains("restored 1 add-on(s)"));

    let vault =
        EncryptedFileVault::open(target.join("secrets.vault"), target.join("vault.key")).unwrap();
    assert_eq!(
        vault
            .get("com.example.rebuildable", "api_token")
            .unwrap()
            .as_deref(),
        Some("restored-secret")
    );
}

#[test]
fn repo_install_rejects_wrong_pubkey() {
    let tmp = tempfile::tempdir().unwrap();
    let packages = tmp.path().join("packages");
    std::fs::create_dir_all(&packages).unwrap();
    let keys = tmp.path().join("keys");
    let other_keys = tmp.path().join("other-keys");
    assert!(rvah(&["keygen", "-o", keys.to_str().unwrap()])
        .status
        .success());
    assert!(rvah(&["keygen", "-o", other_keys.to_str().unwrap()])
        .status
        .success());

    let src = tmp.path().join("addon-src");
    std::fs::create_dir_all(&src).unwrap();
    std::fs::write(
        src.join("manifest.toml"),
        fixture_manifest("com.example.wrongkey", &["catalog"], &[], &[]),
    )
    .unwrap();
    std::fs::write(src.join("main"), "entry").unwrap();
    let pkg = packages.join("wrongkey.rvah");
    assert!(rvah(&[
        "package",
        src.to_str().unwrap(),
        "-o",
        pkg.to_str().unwrap(),
        "--key",
        keys.join("signing.key").to_str().unwrap(),
    ])
    .status
    .success());
    let index = tmp.path().join("index.json");
    assert!(rvah(&[
        "repo-index",
        packages.to_str().unwrap(),
        "-o",
        index.to_str().unwrap(),
        "--key",
        keys.join("signing.key").to_str().unwrap(),
    ])
    .status
    .success());

    // Trusting the wrong key must fail before anything is parsed/installed.
    let out = rvah(&[
        "repo-install",
        "com.example.wrongkey",
        "--index",
        index.to_str().unwrap(),
        "--pubkey",
        other_keys.join("verify.pub").to_str().unwrap(),
        "--data-dir",
        tmp.path().join("data").to_str().unwrap(),
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
