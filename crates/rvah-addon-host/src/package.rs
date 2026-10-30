use std::io::{Read, Write};
use std::path::Path;

use ed25519_dalek::{Signature, Signer, SigningKey, Verifier, VerifyingKey};
use sha2::{Digest, Sha256};

/// Errors from the `.rvah` packaging and signature pipeline (FR-004).
#[derive(Debug, thiserror::Error)]
pub enum PackageError {
    #[error("package io error: {0}")]
    Io(String),
    #[error("signature verification failed")]
    BadSignature,
    #[error("malformed package: {0}")]
    Malformed(String),
}

impl From<std::io::Error> for PackageError {
    fn from(e: std::io::Error) -> Self {
        PackageError::Io(e.to_string())
    }
}

/// Generate a fresh ed25519 keypair for signing packages.
pub fn keygen() -> (SigningKey, VerifyingKey) {
    let signing = SigningKey::generate(&mut rand::rngs::OsRng);
    let verifying = signing.verifying_key();
    (signing, verifying)
}

/// Create a `.rvah` package (tar.gz of the add-on directory contents) at
/// `out`; returns the sha256 of the archive bytes.
pub fn package_dir(dir: &Path, out: &Path) -> Result<[u8; 32], PackageError> {
    let file = std::fs::File::create(out)?;
    let enc = flate2::write::GzEncoder::new(file, flate2::Compression::default());
    let mut tar = tar::Builder::new(enc);
    tar.append_dir_all(".", dir)?;
    tar.into_inner()?.finish()?.flush()?;
    sha256_file(out)
}

pub fn sha256_file(path: &Path) -> Result<[u8; 32], PackageError> {
    let bytes = std::fs::read(path)?;
    Ok(Sha256::digest(&bytes).into())
}

/// Detached signature: ed25519 over sha256(archive bytes).
pub fn sign_package(pkg_path: &Path, key: &SigningKey) -> Result<[u8; 64], PackageError> {
    let digest = sha256_file(pkg_path)?;
    Ok(key.sign(&digest).to_bytes())
}

/// Verify a package against its detached signature (TC-SEC-001).
pub fn verify_package(
    pkg_path: &Path,
    sig: &[u8; 64],
    pubkey: &VerifyingKey,
) -> Result<(), PackageError> {
    let digest = sha256_file(pkg_path)?;
    let signature = Signature::from_bytes(sig);
    pubkey
        .verify(&digest, &signature)
        .map_err(|_| PackageError::BadSignature)
}

/// Walk every archive entry, refusing anything ambiguous before the visitor
/// sees it: only regular files and directories, only relative paths without
/// `..`, and no path twice (compared case-insensitively, since extraction on
/// macOS/Windows would let a later entry overwrite an earlier one). This is
/// what keeps the manifest approved at preview identical to the one the
/// runtime later reads from the extracted directory.
fn for_each_entry(
    pkg_path: &Path,
    mut visit: impl FnMut(
        &Path,
        &mut tar::Entry<'_, flate2::read::GzDecoder<std::fs::File>>,
    ) -> Result<(), PackageError>,
) -> Result<(), PackageError> {
    let file = std::fs::File::open(pkg_path)?;
    walk_entries(flate2::read::GzDecoder::new(file), |path, entry| {
        visit(path, entry)
    })
}

fn walk_entries<R: Read>(
    reader: R,
    mut visit: impl FnMut(&Path, &mut tar::Entry<'_, R>) -> Result<(), PackageError>,
) -> Result<(), PackageError> {
    let mut archive = tar::Archive::new(reader);
    let mut seen = std::collections::HashSet::new();
    for entry in archive.entries()? {
        let mut entry = entry?;
        let path = entry
            .path()
            .map_err(|e| PackageError::Malformed(e.to_string()))?
            .into_owned();
        let safe = path.components().all(|c| {
            matches!(
                c,
                std::path::Component::Normal(_) | std::path::Component::CurDir
            )
        });
        if !safe {
            return Err(PackageError::Malformed(format!(
                "unsafe entry path: {}",
                path.display()
            )));
        }
        let kind = entry.header().entry_type();
        if !(kind.is_file() || kind.is_dir()) {
            return Err(PackageError::Malformed(format!(
                "unsupported entry type {kind:?}: {}",
                path.display()
            )));
        }
        let normalized = path
            .components()
            .filter(|c| matches!(c, std::path::Component::Normal(_)))
            .collect::<std::path::PathBuf>();
        let portable = normalized
            .to_str()
            .ok_or_else(|| PackageError::Malformed("non-UTF8 archive path".into()))?;
        if portable.contains(['\\', ':']) {
            return Err(PackageError::Malformed(format!(
                "nonportable archive path: {portable}"
            )));
        }
        let key = portable.to_lowercase();
        if !key.is_empty() && !seen.insert(key) {
            return Err(PackageError::Malformed(format!(
                "duplicate entry: {}",
                normalized.display()
            )));
        }
        visit(&normalized, &mut entry)?;
    }
    Ok(())
}

/// Verify before decompressing, then read a bounded archive entirely in memory.
/// Browser and native package readers share the same path/type/duplicate checks.
pub fn verified_package_bytes(
    bytes: &[u8],
    signature: &[u8],
    public_key: &[u8],
) -> Result<std::collections::BTreeMap<String, Vec<u8>>, PackageError> {
    const MAX: usize = 16 * 1024 * 1024;
    if bytes.len() > MAX {
        return Err(PackageError::Malformed("archive exceeds 16 MiB".into()));
    }
    let key: &[u8; 32] = public_key
        .try_into()
        .map_err(|_| PackageError::BadSignature)?;
    let key = VerifyingKey::from_bytes(key).map_err(|_| PackageError::BadSignature)?;
    let signature = Signature::from_slice(signature).map_err(|_| PackageError::BadSignature)?;
    key.verify_strict(&Sha256::digest(bytes), &signature)
        .map_err(|_| PackageError::BadSignature)?;
    // Bound the decompressed tar stream including headers and ignored padding.
    let mut decoded = Vec::new();
    flate2::read::GzDecoder::new(bytes)
        .take((MAX + 1) as u64)
        .read_to_end(&mut decoded)?;
    if decoded.len() > MAX {
        return Err(PackageError::Malformed(
            "expanded archive exceeds 16 MiB".into(),
        ));
    }
    let mut files = std::collections::BTreeMap::new();
    let mut entries = 0;
    walk_entries(decoded.as_slice(), |path, entry| {
        entries += 1;
        if entries > 256 {
            return Err(PackageError::Malformed("too many archive entries".into()));
        }
        if entry.header().entry_type().is_file() {
            let name = path.to_str().expect("walk_entries validates UTF-8 paths");
            let mut contents = Vec::new();
            entry.read_to_end(&mut contents)?;
            files.insert(name.to_owned(), contents);
        }
        Ok(())
    })?;
    Ok(files)
}

/// Extract a verified package into `dest`. The whole archive is validated
/// (see [`for_each_entry`]) before anything is written.
pub fn extract_package(pkg_path: &Path, dest: &Path) -> Result<(), PackageError> {
    for_each_entry(pkg_path, |_, _| Ok(()))?;
    std::fs::create_dir_all(dest)?;
    for_each_entry(pkg_path, |_, entry| {
        entry.unpack_in(dest)?;
        Ok(())
    })
}

/// Read a file out of a package without extracting the rest (e.g. the
/// manifest during install preview). The whole archive is validated, so an
/// ambiguous package fails here exactly as it would at extraction.
pub fn read_package_file(pkg_path: &Path, name: &str) -> Result<Vec<u8>, PackageError> {
    let mut found = None;
    for_each_entry(pkg_path, |path, entry| {
        if path == Path::new(name) {
            let mut buf = Vec::new();
            entry.read_to_end(&mut buf)?;
            found = Some(buf);
        }
        Ok(())
    })?;
    found.ok_or_else(|| PackageError::Malformed(format!("{name} not found in package")))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn memory_reader_rejects_duplicates_links_and_expansion() {
        let (signing, verifying) = keygen();
        let make = |names: &[(&str, tar::EntryType)], content: &[u8]| {
            let encoder = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
            let mut archive = tar::Builder::new(encoder);
            for (name, kind) in names {
                let mut header = tar::Header::new_gnu();
                header.set_entry_type(*kind);
                header.set_size(content.len() as u64);
                header.set_mode(0o644);
                header.set_cksum();
                archive.append_data(&mut header, name, content).unwrap();
            }
            archive.into_inner().unwrap().finish().unwrap()
        };
        for bytes in [
            make(
                &[
                    ("manifest.toml", tar::EntryType::Regular),
                    ("MANIFEST.toml", tar::EntryType::Regular),
                ],
                b"a",
            ),
            make(&[("main.wasm", tar::EntryType::Symlink)], b""),
            make(
                &[("main.wasm", tar::EntryType::Regular)],
                &vec![0; 16 * 1024 * 1024],
            ),
        ] {
            let signature = signing.sign(&Sha256::digest(&bytes)).to_bytes();
            assert!(verified_package_bytes(&bytes, &signature, &verifying.to_bytes()).is_err());
        }
    }

    fn addon_dir(dir: &Path) {
        std::fs::write(dir.join("manifest.toml"), "id = \"x\"").unwrap();
        std::fs::write(dir.join("main.txt"), "entry point").unwrap();
    }

    #[test]
    fn package_sign_verify_roundtrip() {
        let tmp = tempfile::tempdir().unwrap();
        let src = tmp.path().join("addon");
        std::fs::create_dir(&src).unwrap();
        addon_dir(&src);
        let pkg = tmp.path().join("addon.rvah");

        let hash = package_dir(&src, &pkg).unwrap();
        assert_eq!(hash, sha256_file(&pkg).unwrap());

        let (signing, verifying) = keygen();
        let sig = sign_package(&pkg, &signing).unwrap();
        verify_package(&pkg, &sig, &verifying).unwrap();

        // Extraction restores the files.
        let out = tmp.path().join("out");
        extract_package(&pkg, &out).unwrap();
        assert_eq!(
            std::fs::read_to_string(out.join("main.txt")).unwrap(),
            "entry point"
        );
        // Manifest readable without extraction.
        assert_eq!(
            read_package_file(&pkg, "manifest.toml").unwrap(),
            b"id = \"x\""
        );
    }

    #[test]
    fn tampered_package_rejected() {
        // TC-SEC-001 / TC-ADD-002: any byte flip breaks the signature.
        let tmp = tempfile::tempdir().unwrap();
        let src = tmp.path().join("addon");
        std::fs::create_dir(&src).unwrap();
        addon_dir(&src);
        let pkg = tmp.path().join("addon.rvah");
        package_dir(&src, &pkg).unwrap();

        let (signing, verifying) = keygen();
        let sig = sign_package(&pkg, &signing).unwrap();

        let mut bytes = std::fs::read(&pkg).unwrap();
        let last = bytes.len() - 1;
        bytes[last] ^= 0xFF;
        std::fs::write(&pkg, bytes).unwrap();

        assert!(matches!(
            verify_package(&pkg, &sig, &verifying),
            Err(PackageError::BadSignature)
        ));

        // Wrong key also fails even on an untampered package.
        let pkg2 = tmp.path().join("addon2.rvah");
        package_dir(&src, &pkg2).unwrap();
        let sig2 = sign_package(&pkg2, &signing).unwrap();
        let (_, other_key) = keygen();
        assert!(matches!(
            verify_package(&pkg2, &sig2, &other_key),
            Err(PackageError::BadSignature)
        ));
    }
}
