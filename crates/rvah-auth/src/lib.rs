//! Secret storage, log redaction, and device-code auth flows (FR-083, FR-084).
//!
//! Secrets live in a [`SecretVault`] keyed by `(addon_id, name)`; add-ons never
//! see other add-ons' secrets. The [`Redactor`] scrubs secret material from any
//! text destined for logs or diagnostics (TC-SEC-003).

#[cfg(feature = "keychain")]
mod keychain;
mod redact;
mod vault;

#[cfg(feature = "keychain")]
pub use keychain::KeychainVault;
pub use redact::Redactor;
pub use vault::{
    DeviceCodeFlow, DeviceCodeState, EncryptedFileVault, MemoryVault, PollOutcome, SecretVault,
    VaultError,
};
