use std::io::Write;

use rvah_addon_host::{extract_package, read_package_file};

fn append(tar: &mut tar::Builder<flate2::write::GzEncoder<std::fs::File>>, name: &str, body: &[u8]) {
    let mut header = tar::Header::new_gnu();
    header.set_size(body.len() as u64);
    header.set_mode(0o644);
    header.set_cksum();
    tar.append_data(&mut header, name, body).unwrap();
}

#[test]
fn preview_and_extraction_disagree_on_duplicate_manifest() {
    let tmp = tempfile::tempdir().unwrap();
    let package = tmp.path().join("duplicate.rvah");
    let file = std::fs::File::create(&package).unwrap();
    let encoder = flate2::write::GzEncoder::new(file, flate2::Compression::default());
    let mut tar = tar::Builder::new(encoder);
    append(&mut tar, "manifest.toml", b"preview permissions");
    append(&mut tar, "manifest.toml", b"runtime permissions");
    tar.into_inner().unwrap().finish().unwrap().flush().unwrap();

    assert_eq!(read_package_file(&package, "manifest.toml").unwrap(), b"preview permissions");
    let dest = tmp.path().join("out");
    extract_package(&package, &dest).unwrap();
    assert_eq!(std::fs::read(dest.join("manifest.toml")).unwrap(), b"runtime permissions");
}
