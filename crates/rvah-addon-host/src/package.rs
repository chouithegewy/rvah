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

/// Extract a verified package into `dest`, refusing entries that would
/// escape it (absolute paths or `..` traversal).
pub fn extract_package(pkg_path: &Path, dest: &Path) -> Result<(), PackageError> {
    let file = std::fs::File::open(pkg_path)?;
    let mut archive = tar::Archive::new(flate2::read::GzDecoder::new(file));
    std::fs::create_dir_all(dest)?;
    for entry in archive.entries()? {
        let mut entry = entry?;
        let path = entry
            .path()
            .map_err(|e| PackageError::Malformed(e.to_string()))?;
        let ok = path.components().all(|c| {
            matches!(
                c,
                std::path::Component::Normal(_) | std::path::Component::CurDir
            )
        });
        if !ok {
            return Err(PackageError::Malformed(format!(
                "unsafe entry path: {}",
                path.display()
            )));
        }
        entry.unpack_in(dest)?;
    }
    Ok(())
}

/// Read a file out of a package without extracting the rest (e.g. the
/// manifest during install preview).
pub fn read_package_file(pkg_path: &Path, name: &str) -> Result<Vec<u8>, PackageError> {
    let file = std::fs::File::open(pkg_path)?;
    let mut archive = tar::Archive::new(flate2::read::GzDecoder::new(file));
    for entry in archive.entries()? {
        let mut entry = entry?;
        let path = entry
            .path()
            .map_err(|e| PackageError::Malformed(e.to_string()))?;
        let normalized = path
            .components()
            .filter(|c| matches!(c, std::path::Component::Normal(_)))
            .collect::<std::path::PathBuf>();
        if normalized == Path::new(name) {
            let mut buf = Vec::new();
            entry.read_to_end(&mut buf)?;
            return Ok(buf);
        }
    }
    Err(PackageError::Malformed(format!(
        "{name} not found in package"
    )))
}

#[cfg(test)]
mod tests {
    use super::*;

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
