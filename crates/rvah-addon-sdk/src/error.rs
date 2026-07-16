use serde::{Deserialize, Serialize};

/// Machine-readable error class returned by add-ons and host bridge calls.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AddonErrorCode {
    Network,
    Auth,
    NotFound,
    Quota,
    Timeout,
    Internal,
    Malformed,
    PermissionDenied,
}

/// Structured error surfaced to the host without crashing it (FR-023).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AddonError {
    pub code: AddonErrorCode,
    pub message: String,
    /// Whether retrying the same call may succeed.
    pub retryable: bool,
    /// Whether `message` is safe and useful to show to the user.
    pub user_visible: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub diagnostics_id: Option<String>,
}

impl AddonError {
    pub fn new(code: AddonErrorCode, message: impl Into<String>) -> Self {
        let retryable = matches!(
            code,
            AddonErrorCode::Network | AddonErrorCode::Timeout | AddonErrorCode::Quota
        );
        AddonError {
            code,
            message: message.into(),
            retryable,
            user_visible: false,
            diagnostics_id: None,
        }
    }

    pub fn user_visible(mut self) -> Self {
        self.user_visible = true;
        self
    }
}

impl std::fmt::Display for AddonError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{:?}: {}", self.code, self.message)
    }
}

impl std::error::Error for AddonError {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn error_roundtrips_and_defaults_retryable_by_code() {
        let e = AddonError::new(AddonErrorCode::Timeout, "took too long");
        assert!(e.retryable);
        assert!(!e.user_visible);
        let e2 = AddonError::new(AddonErrorCode::Malformed, "bad json");
        assert!(!e2.retryable);
        let json = serde_json::to_string(&e).unwrap();
        assert_eq!(e, serde_json::from_str::<AddonError>(&json).unwrap());
    }
}
