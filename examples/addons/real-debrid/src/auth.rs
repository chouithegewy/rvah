//! Real-Debrid device sign-in (UC-04) for the host's `auth` flow.
//!
//! `begin` gets a code for the user to enter at real-debrid.com/device;
//! `poll` waits for approval, then trades the device code for tokens. The
//! host stores what `poll` and `refresh` return: the access token as
//! `api_token` (the bearer every REST call already sends) and the refresh
//! material as `oauth_client`. When the access token expires the REST API
//! answers 401, and the host calls `refresh` and retries.

use rvah_addon_sdk::{AddonError, AddonErrorCode, AuthPoll, Credentials, DeviceAuth};
use serde::{Deserialize, Serialize};

use crate::host::{self, form_encode, Host};
use crate::rd::auth_error;

const OAUTH: &str = "https://api.real-debrid.com/oauth/v2";
/// Real-Debrid's published client id for open-source apps.
const OPEN_SOURCE_CLIENT_ID: &str = "X245A4XAIBGVM";
const DEVICE_GRANT: &str = "http://oauth.net/grant_type/device/1.0";

/// The per-user OAuth client issued on approval, plus its refresh token:
/// stored by the host as the `oauth_client` secret.
#[derive(Serialize, Deserialize)]
struct OAuthClient {
    client_id: String,
    client_secret: String,
    refresh_token: String,
}

fn parse<T: for<'de> Deserialize<'de>>(body: &[u8], what: &str) -> Result<T, AddonError> {
    serde_json::from_slice(body).map_err(|e| {
        AddonError::new(
            AddonErrorCode::Malformed,
            format!("Real-Debrid {what} JSON: {e}"),
        )
    })
}

pub fn begin(host: &dyn Host) -> Result<DeviceAuth, AddonError> {
    #[derive(Deserialize)]
    struct DeviceCode {
        device_code: String,
        user_code: String,
        interval: u32,
        expires_in: u32,
        verification_url: String,
    }
    let query = form_encode(&[
        ("client_id", OPEN_SOURCE_CLIENT_ID),
        ("new_credentials", "yes"),
    ]);
    let resp = host::http_get(
        host,
        &format!("{OAUTH}/device/code?{query}"),
        Default::default(),
    )?;
    if !(200..300).contains(&resp.status) {
        return Err(AddonError::new(
            AddonErrorCode::Network,
            format!("Real-Debrid device code request returned {}", resp.status),
        ));
    }
    let code: DeviceCode = parse(&resp.body, "device code")?;
    Ok(DeviceAuth {
        user_code: code.user_code,
        verification_url: code.verification_url,
        device_code: code.device_code,
        interval_secs: code.interval,
        expires_in_secs: code.expires_in,
    })
}

pub fn poll(host: &dyn Host, device_code: &str) -> Result<AuthPoll, AddonError> {
    #[derive(Deserialize)]
    struct Issued {
        client_id: String,
        client_secret: String,
    }
    let query = form_encode(&[("client_id", OPEN_SOURCE_CLIENT_ID), ("code", device_code)]);
    let resp = host::http_get(
        host,
        &format!("{OAUTH}/device/credentials?{query}"),
        Default::default(),
    )?;
    // Real-Debrid answers with an error until the user enters the code.
    if !(200..300).contains(&resp.status) {
        return Ok(AuthPoll::Pending);
    }
    let issued: Issued = parse(&resp.body, "credentials")?;
    let credentials = token(host, issued.client_id, issued.client_secret, device_code)?;
    Ok(AuthPoll::Authorized(credentials))
}

pub fn refresh(host: &dyn Host) -> Result<Credentials, AddonError> {
    let stored = host::secret_get(host, "oauth_client")?
        .ok_or_else(|| auth_error("Real-Debrid is not linked: use Link account"))?;
    let client: OAuthClient = serde_json::from_str(&stored).map_err(|e| {
        AddonError::new(
            AddonErrorCode::Malformed,
            format!("stored oauth_client: {e}"),
        )
    })?;
    token(
        host,
        client.client_id,
        client.client_secret,
        &client.refresh_token,
    )
}

/// The token endpoint serves both the first exchange (`code` = device
/// code) and refreshes (`code` = refresh token).
fn token(
    host: &dyn Host,
    client_id: String,
    client_secret: String,
    code: &str,
) -> Result<Credentials, AddonError> {
    #[derive(Deserialize)]
    struct Tokens {
        access_token: String,
        refresh_token: String,
    }
    let resp = host::http_post_form(
        host,
        &format!("{OAUTH}/token"),
        &[
            ("client_id", &client_id),
            ("client_secret", &client_secret),
            ("code", code),
            ("grant_type", DEVICE_GRANT),
        ],
    )?;
    match resp.status {
        200..=299 => {}
        400 | 401 | 403 => {
            return Err(auth_error(
                "Real-Debrid refused the sign-in; link the account again",
            ))
        }
        status => {
            return Err(AddonError::new(
                AddonErrorCode::Network,
                format!("Real-Debrid token request returned {status}"),
            ))
        }
    }
    let tokens: Tokens = parse(&resp.body, "token")?;
    let client = OAuthClient {
        client_id,
        client_secret,
        refresh_token: tokens.refresh_token,
    };
    let client = serde_json::to_string(&client).expect("oauth client serializes");
    Ok(Credentials(
        [
            ("api_token".to_string(), tokens.access_token),
            ("oauth_client".to_string(), client),
        ]
        .into(),
    ))
}
