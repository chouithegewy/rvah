//! The real Real-Debrid wasm guest under Wasmi and the bridged host, as on
//! Android, against a scripted Real-Debrid API: device-code sign-in, then an
//! expired access token refreshed and the call retried. Needs the guest
//! built first, so it is ignored by default:
//!
//! ```sh
//! examples/addons/real-debrid/build.sh
//! cargo test -p rvah-android --test real_debrid_guest -- --ignored
//! ```
use std::sync::{Arc, Mutex};

use rvah_addon_host::{
    AddonManager, BridgedHostApi, CapabilityGated, InstallDecision, Manifest, WasmiLimits,
    WasmiRuntime,
};
use rvah_addon_sdk::{HttpRequest, HttpResponse};
use rvah_auth::{DeviceCodeState, MemoryVault, SecretVault};
use rvah_core::AppCore;
use rvah_data::Database;
use rvah_net::{NetError, Transport};

struct FakeRd {
    credential_polls: Mutex<u32>,
    log: Mutex<Vec<String>>,
}

fn json(status: u16, value: serde_json::Value) -> Result<HttpResponse, NetError> {
    Ok(HttpResponse {
        status,
        truncated: false,
        headers: Default::default(),
        body: serde_json::to_vec(&value).unwrap(),
    })
}

impl Transport for FakeRd {
    fn execute(&self, req: &HttpRequest) -> Result<HttpResponse, NetError> {
        let body = String::from_utf8(req.body.clone().unwrap_or_default()).unwrap();
        self.log
            .lock()
            .unwrap()
            .push(format!("{} {} {}", req.method, req.url, body));
        let url = req.url.as_str();
        if url.starts_with("https://api.real-debrid.com/oauth/v2/device/code?") {
            return json(
                200,
                serde_json::json!({
                    "device_code": "dc1", "user_code": "RVAHTEST", "interval": 0,
                    "expires_in": 600, "verification_url": "https://real-debrid.com/device"
                }),
            );
        }
        if url.starts_with("https://api.real-debrid.com/oauth/v2/device/credentials?") {
            assert!(url.contains("code=dc1"));
            let mut polls = self.credential_polls.lock().unwrap();
            *polls += 1;
            return if *polls == 1 {
                json(403, serde_json::json!({ "error": "pending" }))
            } else {
                json(
                    200,
                    serde_json::json!({ "client_id": "cid", "client_secret": "csec" }),
                )
            };
        }
        if url == "https://api.real-debrid.com/oauth/v2/token" {
            assert_eq!(
                req.headers.get("content-type").map(String::as_str),
                Some("application/x-www-form-urlencoded")
            );
            let (access, refresh) = if body.contains("code=dc1") {
                ("at1", "rt1")
            } else if body.contains("code=rt1") {
                ("at2", "rt2")
            } else {
                return json(400, serde_json::json!({ "error": "bad code" }));
            };
            return json(
                200,
                serde_json::json!({
                    "access_token": access, "refresh_token": refresh,
                    "expires_in": 3600, "token_type": "Bearer"
                }),
            );
        }
        if url.starts_with("https://api.real-debrid.com/rest/1.0/downloads") {
            return match req.headers.get("authorization").map(String::as_str) {
                Some("Bearer at2") => json(
                    200,
                    serde_json::json!([{
                        "id": "d1", "filename": "Linked.Movie.mkv", "filesize": 1073741824u64,
                        "download": "https://x.download.real-debrid.com/d1"
                    }]),
                ),
                _ => json(
                    401,
                    serde_json::json!({ "error": "bad_token", "error_code": 8 }),
                ),
            };
        }
        panic!("unrouted {url}");
    }
}

struct Shared(Arc<FakeRd>);
impl Transport for Shared {
    fn execute(&self, req: &HttpRequest) -> Result<HttpResponse, NetError> {
        self.0.execute(req)
    }
}

#[test]
#[ignore = "needs examples/addons/real-debrid/build.sh"]
fn real_debrid_guest_links_refreshes_and_retries() {
    let tmp = tempfile::tempdir().unwrap();
    let dist = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../examples/addons/real-debrid/dist");
    let pkg = tmp.path().join("rd.rvah");
    rvah_addon_host::package_dir(&dist, &pkg).unwrap();
    let (signing, verifying) = rvah_addon_host::keygen();
    let sig = rvah_addon_host::sign_package(&pkg, &signing).unwrap();

    let db = Arc::new(Database::open_in_memory().unwrap());
    let manager = AddonManager::new(Arc::clone(&db), tmp.path().join("addons"));
    manager
        .install(&pkg, &sig, &verifying, InstallDecision::Approve)
        .unwrap();
    let manifest =
        Manifest::parse(&std::fs::read_to_string(dist.join("manifest.toml")).unwrap()).unwrap();
    let id = manifest.addon.id.clone();
    let vault: Arc<dyn SecretVault> = Arc::new(MemoryVault::new());
    let rd = Arc::new(FakeRd {
        credential_polls: Mutex::new(0),
        log: Mutex::new(Vec::new()),
    });
    let runtime = WasmiRuntime::load(
        &std::fs::read(dist.join("module.wasm")).unwrap(),
        WasmiLimits::default(),
    )
    .unwrap();
    let mut core = AppCore::new(
        manager,
        Arc::clone(&db),
        Arc::clone(&vault),
        rvah_player::MockPlayer::new(),
    );
    core.register_runtime(
        id.clone(),
        Box::new(CapabilityGated::new(
            runtime,
            manifest.addon.capabilities.clone(),
        )),
        Arc::new(BridgedHostApi::new(
            &manifest,
            Arc::clone(&db),
            Arc::clone(&vault),
            Box::new(Shared(Arc::clone(&rd))),
            100,
        )),
    );

    let prompt = core.auth_begin(&id).unwrap();
    assert_eq!(prompt.user_code, "RVAHTEST");
    assert_eq!(prompt.verification_url, "https://real-debrid.com/device");
    assert!(matches!(
        core.auth_poll(&id).unwrap(),
        DeviceCodeState::Pending { .. }
    ));
    assert_eq!(core.auth_poll(&id).unwrap(), DeviceCodeState::Authorized);
    assert_eq!(vault.get(&id, "api_token").unwrap().as_deref(), Some("at1"));
    assert!(vault
        .get(&id, "oauth_client")
        .unwrap()
        .unwrap()
        .contains("rt1"));

    // at1 is "expired": the downloads call 401s, the host refreshes, retries.
    let page = core.browse(&id, None, None).unwrap();
    assert_eq!(page.items.len(), 1);
    assert_eq!(vault.get(&id, "api_token").unwrap().as_deref(), Some("at2"));
    let calls: Vec<String> = rd
        .log
        .lock()
        .unwrap()
        .iter()
        .map(|line| line.split('?').next().unwrap().to_string())
        .collect();
    let rest = "https://api.real-debrid.com/rest/1.0/downloads";
    let oauth = "https://api.real-debrid.com/oauth/v2";
    assert_eq!(
        calls
            .iter()
            .map(|c| c.split(' ').take(2).collect::<Vec<_>>().join(" "))
            .collect::<Vec<_>>(),
        [
            format!("GET {oauth}/device/code"),
            format!("GET {oauth}/device/credentials"),
            format!("GET {oauth}/device/credentials"),
            format!("POST {oauth}/token"),
            format!("GET {rest}"),
            format!("POST {oauth}/token"),
            format!("GET {rest}"),
        ]
    );
    for event in db.recent_events(20).unwrap() {
        for secret in ["at1", "at2", "rt1", "rt2", "csec"] {
            assert!(
                !event.message.contains(secret),
                "{secret} leaked: {event:?}"
            );
        }
    }
}
