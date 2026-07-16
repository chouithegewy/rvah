use rvah_addon_sdk::{AddonError, AddonErrorCode};
use rvah_player::{PlayerError, SourceRejection};

/// How the UI should treat a failure (SDD §11).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ErrorClass {
    /// The user can fix it (sign in, re-enable, grant permission).
    UserActionable,
    /// Transient — retrying may succeed.
    Recoverable,
    /// An add-on defect; surface diagnostics, never crash the host.
    Developer,
    /// Host-level persistence/integrity failure.
    FatalHost,
}

#[derive(Debug, thiserror::Error)]
pub enum CoreError {
    #[error("add-on not installed: {0}")]
    AddonNotFound(String),
    #[error("add-on is disabled or quarantined: {0}")]
    AddonDisabled(String),
    #[error("add-on error: {0}")]
    Provider(AddonError),
    #[error("source rejected: {0}")]
    SourceRejected(SourceRejection),
    #[error("player error: {0}")]
    Player(PlayerError),
    #[error("data error: {0}")]
    Data(String),
}

impl CoreError {
    pub fn class(&self) -> ErrorClass {
        match self {
            CoreError::AddonNotFound(_) | CoreError::AddonDisabled(_) => ErrorClass::UserActionable,
            CoreError::Provider(e) => match e.code {
                AddonErrorCode::Auth
                | AddonErrorCode::PermissionDenied
                | AddonErrorCode::NotFound => ErrorClass::UserActionable,
                AddonErrorCode::Network | AddonErrorCode::Timeout | AddonErrorCode::Quota => {
                    ErrorClass::Recoverable
                }
                AddonErrorCode::Malformed | AddonErrorCode::Internal => ErrorClass::Developer,
            },
            CoreError::SourceRejected(SourceRejection::Expired) => ErrorClass::Recoverable,
            CoreError::SourceRejected(_) => ErrorClass::Developer,
            CoreError::Player(_) => ErrorClass::Recoverable,
            CoreError::Data(_) => ErrorClass::FatalHost,
        }
    }

    /// Whether retrying the same action may succeed.
    pub fn retryable(&self) -> bool {
        match self {
            CoreError::Provider(e) => e.retryable,
            CoreError::SourceRejected(SourceRejection::Expired) => true,
            _ => self.class() == ErrorClass::Recoverable,
        }
    }
}

impl From<rvah_data::DataError> for CoreError {
    fn from(e: rvah_data::DataError) -> Self {
        CoreError::Data(e.to_string())
    }
}
