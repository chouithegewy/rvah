//! Hostile archive layouts. The manifest a user approves at install time
//! (read via `read_package_file`) must be exactly the one extraction leaves
//! on disk for the runtime, so ambiguous archives are refused outright.

use std::io::Write;
use std::path::Path;

use rvah_addon_host::{extract_package, read_package_file, PackageError};

type TarBuilder = tar::Builder<flate2::write::GzEncoder<std::fs::File>>;

fn build(pkg: &Path, fill: impl FnOnce(&mut TarBuilder)) {
    let file = std::fs::File::create(pkg).unwrap();
    let encoder = flate2::write::GzEncoder::new(file, flate2::Compression::default());
    let mut tar = tar::Builder::new(encoder);
    fill(&mut tar);
    tar.into_inner().unwrap().finish().unwrap().flush().unwrap();
}

fn append(tar: &mut TarBuilder, name: &str, body: &[u8]) {
    let mut header = tar::Header::new_gnu();
    // Store the name verbatim, as a hostile packer would: `set_path` normalizes
    // it, and on Windows refuses names like `C:manifest.toml` outright.
    let raw = name.as_bytes();
    assert!(raw.len() < 100, "test names must fit the ustar name field");
    header.as_old_mut().name[..raw.len()].copy_from_slice(raw);
    header.set_size(body.len() as u64);
    header.set_mode(0o644);
    header.set_cksum();
    tar.append(&header, body).unwrap();
}

fn assert_refused(pkg: &Path, out: &Path) {
    assert!(matches!(
        read_package_file(pkg, "manifest.toml"),
        Err(PackageError::Malformed(_))
    ));
    assert!(matches!(
        extract_package(pkg, out),
        Err(PackageError::Malformed(_))
    ));
    // Validation happens before anything is written.
    assert!(!out.join("manifest.toml").exists());
}

#[test]
fn duplicate_manifest_is_refused_by_preview_and_extraction() {
    let tmp = tempfile::tempdir().unwrap();
    let pkg = tmp.path().join("duplicate.rvah");
    build(&pkg, |tar| {
        append(tar, "manifest.toml", b"preview permissions");
        append(tar, "manifest.toml", b"runtime permissions");
    });
    assert_refused(&pkg, &tmp.path().join("out"));
}

#[test]
fn dot_prefixed_duplicate_is_refused() {
    let tmp = tempfile::tempdir().unwrap();
    let pkg = tmp.path().join("dot.rvah");
    build(&pkg, |tar| {
        append(tar, "manifest.toml", b"preview permissions");
        append(tar, "./manifest.toml", b"runtime permissions");
    });
    assert_refused(&pkg, &tmp.path().join("out"));
}

#[test]
fn case_colliding_entries_are_refused() {
    // On case-insensitive filesystems (macOS, Windows) the second entry
    // would overwrite the first during extraction.
    let tmp = tempfile::tempdir().unwrap();
    let pkg = tmp.path().join("case.rvah");
    build(&pkg, |tar| {
        append(tar, "manifest.toml", b"preview permissions");
        append(tar, "MANIFEST.TOML", b"runtime permissions");
    });
    assert_refused(&pkg, &tmp.path().join("out"));
}

#[test]
fn platform_ambiguous_paths_are_refused() {
    for name in ["assets\\manifest.toml", "C:manifest.toml"] {
        let tmp = tempfile::tempdir().unwrap();
        let pkg = tmp.path().join("nonportable.rvah");
        build(&pkg, |tar| {
            append(tar, "manifest.toml", b"preview permissions");
            append(tar, name, b"ambiguous on another platform");
        });
        assert_refused(&pkg, &tmp.path().join("out"));
    }
}

#[test]
fn symlink_entries_are_refused() {
    let tmp = tempfile::tempdir().unwrap();
    let pkg = tmp.path().join("link.rvah");
    build(&pkg, |tar| {
        append(tar, "manifest.toml", b"id = \"x\"");
        let mut header = tar::Header::new_gnu();
        header.set_entry_type(tar::EntryType::Symlink);
        header.set_size(0);
        header.set_mode(0o777);
        tar.append_link(&mut header, "module.wasm", "/etc/passwd")
            .unwrap();
    });
    let out = tmp.path().join("out");
    assert!(matches!(
        read_package_file(&pkg, "manifest.toml"),
        Err(PackageError::Malformed(_))
    ));
    assert!(matches!(
        extract_package(&pkg, &out),
        Err(PackageError::Malformed(_))
    ));
    assert!(!out.join("module.wasm").exists());
}

#[test]
fn ordinary_package_with_directories_still_extracts() {
    let tmp = tempfile::tempdir().unwrap();
    let src = tmp.path().join("addon");
    std::fs::create_dir_all(src.join("assets")).unwrap();
    std::fs::write(src.join("manifest.toml"), "id = \"x\"").unwrap();
    std::fs::write(src.join("assets/icon.txt"), "icon").unwrap();
    let pkg = tmp.path().join("ok.rvah");
    rvah_addon_host::package_dir(&src, &pkg).unwrap();

    assert_eq!(
        read_package_file(&pkg, "manifest.toml").unwrap(),
        b"id = \"x\""
    );
    let out = tmp.path().join("out");
    extract_package(&pkg, &out).unwrap();
    assert_eq!(
        std::fs::read_to_string(out.join("assets/icon.txt")).unwrap(),
        "icon"
    );
}
