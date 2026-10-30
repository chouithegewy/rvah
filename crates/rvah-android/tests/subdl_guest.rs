//! The real SubDL wasm guest under Wasmi and the bridged host, as on
//! Android, with the default fuel and time limits, against a scripted SubDL
//! answering as much as it does for a popular episode (season packs and
//! all, some 100 KB): the episode's files come back screened by the core,
//! and without the API key SubDL writes into its links. Needs the guest
//! built first, so it is ignored by default:
//!
//! ```sh
//! examples/addons/subdl/build.sh
//! cargo test -p rvah-android --test subdl_guest -- --ignored
//! ```
use std::sync::{Arc, Mutex};

use rvah_addon_host::{
    AddonManager, BridgedHostApi, CapabilityGated, InstallDecision, Manifest, WasmiLimits,
    WasmiRuntime,
};
use rvah_addon_sdk::{HttpRequest, HttpResponse, SourceKind, SubtitleRequest};
use rvah_auth::{MemoryVault, SecretVault};
use rvah_core::AppCore;
use rvah_data::Database;
use rvah_net::{NetError, Transport};
use serde_json::{json, Value};

const KEY: &str = "k-0123456789abcdefghijklmnopqrst";

/// One upload as SubDL lists it with `unpack=1`: a season pack of
/// `files` episodes, or one episode's file.
fn upload(n: u64, files: u64) -> Value {
    let unpacked: Vec<Value> = (1..=files)
        .map(|e| {
            let episode = if files == 1 { 2 } else { e };
            json!({
                "file_n_id": format!("f{n}e{e}"), "name": format!("Acme Corp (2008) - S01E{episode:02} - An Episode.eng.srt"),
                "release_name": format!("Acme Corp (2008) - S01E{episode:02} - An Episode.eng"),
                "season": if files == 1 { 1 } else { 0 }, "episode": episode, "language": "EN", "hi": n % 4 == 0,
                "format": "srt", "size": 43_070, "md5": "ac8f644f36a96f9547a969bf68b7571b",
                "url": format!("/subtitle/page{n}/f{n}e{e}?api_key={KEY}")
            })
        })
        .collect();
    json!({
        "release_name": format!("Acme Corp, Season 1 (1080p BluRay x265 HEVC 10bit AAC 5.1 Group{n}) [23.976 FPS]"),
        "name": format!("{n} Acme Corp.zip"), "lang": "English", "author": "Someone",
        "url": format!("/subtitle/{n}-{n}.zip?api_key={KEY}"), "subtitlePage": format!("/s/info/page{n}"),
        "season": 1, "episode": if files == 1 { json!(2) } else { Value::Null }, "language": "EN",
        "framerate": 2, "fps": "23.976", "hi": n % 4 == 0, "episode_from": null, "episode_end": 0,
        "full_season": files > 1, "unpack_files": unpacked
    })
}

struct FakeSubdl {
    log: Mutex<Vec<String>>,
}

impl Transport for FakeSubdl {
    fn execute(&self, req: &HttpRequest) -> Result<HttpResponse, NetError> {
        self.log.lock().unwrap().push(req.url.clone());
        if req
            .url
            .starts_with("https://api.subdl.com/api/v1/subtitles?")
        {
            // 30 uploads: 20 season packs of 13 files, 10 of the episode alone.
            let subtitles: Vec<Value> = (0..30)
                .map(|n| upload(n, if n < 20 { 13 } else { 1 }))
                .collect();
            let body = json!({
                "status": true, "totalPages": 1, "currentPage": 1, "subtitles": subtitles,
                "results": [{ "sd_id": 1, "name": "Acme Corp", "type": "tv", "imdb_id": "tt0903747", "tmdb_id": 1396 }]
            });
            let body = serde_json::to_vec(&body).unwrap();
            eprintln!("SubDL answer: {} KB", body.len() / 1024);
            return Ok(HttpResponse {
                status: 200,
                truncated: false,
                headers: Default::default(),
                body,
            });
        }
        Ok(HttpResponse {
            status: 404,
            truncated: false,
            headers: Default::default(),
            body: b"not routed".to_vec(),
        })
    }
}

struct Shared(Arc<FakeSubdl>);
impl Transport for Shared {
    fn execute(&self, req: &HttpRequest) -> Result<HttpResponse, NetError> {
        self.0.execute(req)
    }
}

#[test]
#[ignore = "needs examples/addons/subdl/build.sh"]
fn subdl_guest_finds_an_episodes_files_without_its_key() {
    let tmp = tempfile::tempdir().unwrap();
    let dist =
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../examples/addons/subdl/dist");
    let pkg = tmp.path().join("subdl.rvah");
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
    vault.set(&id, "api_key", KEY).unwrap();
    let subdl = Arc::new(FakeSubdl {
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
            Box::new(Shared(Arc::clone(&subdl))),
            100,
        )),
    );

    let request = SubtitleRequest {
        kind: SourceKind::Episode,
        imdb_id: Some("tt0903747".into()),
        title: "Acme Corp".into(),
        year: Some(2008),
        season: Some(1),
        episode: Some(2),
        languages: vec!["en".into()],
        release_name: Some("Acme.Corp.S01E02.1080p.BluRay.x265-Group21".into()),
    };
    let started = std::time::Instant::now();
    let results = core.subtitles_all(&request);
    eprintln!("subtitles: {:?}", started.elapsed());
    assert_eq!(results.len(), 1);
    let tracks = results[0].1.as_ref().unwrap();
    // Every upload has one file for episode 2; 20 are offered at most.
    assert_eq!(tracks.len(), 20);
    assert!(
        tracks[0].label.as_deref().unwrap().contains("Group21"),
        "{:?}",
        tracks[0].label
    );
    assert!(tracks
        .iter()
        .all(|t| t.url.starts_with("https://dl.subdl.com/subtitle/") && !t.url.contains(KEY)));
    assert!(subdl.log.lock().unwrap()[0].contains("episode_number=2"));
}
