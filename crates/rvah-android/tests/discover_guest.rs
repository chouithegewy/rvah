//! The real RVAH Discover wasm guest under Wasmi and the bridged host, as on
//! Android, with the default fuel and time limits: the Up next column read
//! from Trakt answers as big as Trakt's own (whole series records with
//! images, a long overview and every translation), for a full list of
//! series you are behind on and the busiest week still read with posters,
//! or a busier one read again without them. Needs the guest built first, so
//! it is ignored by default:
//!
//! ```sh
//! examples/addons/discover/build.sh
//! cargo test -p rvah-android --test discover_guest -- --ignored
//! ```
use std::sync::{Arc, Mutex};

use rvah_addon_host::{
    AddonManager, BridgedHostApi, CapabilityGated, InstallDecision, Manifest, WasmiLimits,
    WasmiRuntime,
};
use rvah_addon_sdk::{HttpRequest, HttpResponse, PageItem};
use rvah_auth::{MemoryVault, SecretVault};
use rvah_core::AppCore;
use rvah_data::Database;
use rvah_net::{NetError, Transport};
use serde_json::{json, Value};

/// Series you are behind on: as many as the add-on asks for.
const BEHIND: u64 = 15;

/// A series as Trakt sends it with `extended=images`, about 2.5 KB.
fn show(n: u64) -> Value {
    let dir = format!(
        "{:03}/{:03}/{:03}",
        n / 1_000_000,
        n / 1_000 % 1_000,
        n % 1_000
    );
    json!({
        "ids": { "imdb": format!("tt{n:08}"), "plex": { "guid": "6718206155bd1fcb553d686f", "slug": format!("series-{n}") },
                 "slug": format!("series-{n}"), "tmdb": 10_000 + n, "tvdb": 450_000 + n, "trakt": 260_000 + n },
        "airs": { "day": "Monday", "time": "20:00", "timezone": "Europe/Istanbul" },
        "year": 2024, "title": format!("Series {n}"), "votes": 94,
        "colors": { "poster": ["#C3CBC4", "#725D53"] },
        "genres": ["drama", "family", "action", "adventure"],
        "images": {
            "logo": [], "thumb": [], "banner": [], "clearart": [],
            "fanart": [format!("media.trakt.tv/images/shows/{dir}/fanarts/medium/3c2eb76ebb.jpg.webp")],
            "poster": [format!("media.trakt.tv/images/shows/{dir}/posters/medium/0ab6e87cb7.jpg.webp")]
        },
        "rating": 8.28723430633545, "status": "returning series", "country": "tr", "network": "Kanal D",
        "runtime": 140, "tagline": "", "trailer": "https://youtube.com/watch?v=9BAs4nDhJH8",
        "homepage": "https://www.example.com/series", "language": "tr",
        "overview": "A widow arrives in a distant city with her late husband's funeral and their five-year-old son \
    to fulfil his last will. This arrival marks a point of no return. The head of the family is not indifferent \
    to her struggles, but he refuses to let her take her child and leave. Confronting the darkness of the past, \
    hidden secrets and the harsh realities of the region, she finds herself caught in a fierce struggle with her \
    late husband's family.",
        "languages": ["es", "tr", "hu"], "subgenres": [], "last_aired": "2026-10-05T17:00:00.000Z",
        "social_ids": { "twitter": "series", "facebook": "series", "instagram": "series", "wikipedia": "Series" },
        "updated_at": "2026-10-10T16:24:27.000Z", "first_aired": "2024-11-11T17:00:00.000Z",
        "certification": null, "comment_count": 0, "total_runtime": 9425, "aired_episodes": 67,
        "original_title": format!("Series {n}"),
        "available_translations": ["ar", "bg", "bs", "ca", "en", "es", "fa", "hr", "hu", "it", "ka", "lt", "pt", "ro", "ru", "tr", "zh"]
    })
}

/// An episode as Trakt sends it with `extended=images`, about 1.3 KB.
fn episode(season: u64, number: u64, at: &str) -> Value {
    json!({
        "ids": { "imdb": null, "plex": { "guid": null }, "tmdb": 7_716_746, "tvdb": null, "trakt": 14_399_999 },
        "title": format!("Episode {number}"), "votes": 0,
        "images": { "screenshot": ["media.trakt.tv/images/episodes/014/399/999/screenshots/medium/7d308d5f98.jpg.webp"] },
        "number": number, "rating": 0, "season": season, "runtime": 142,
        "overview": "Freedom does not end her war with the family but pushes their conflict to a breaking point. \
    Facing the possibility of being separated from her son, she refuses to back down, while he sets his own rules \
    to avoid losing her. Meanwhile, the danger surrounding the family continues to grow. As he fights those \
    threatening his family, he realizes that his enemies are joining forces, and an unexpected decision turns \
    everything upside down.",
        "released": &at[..10], "number_abs": 68, "updated_at": "2026-10-10T15:38:44.000Z",
        "first_aired": at, "episode_type": "standard", "after_credits": false, "comment_count": 0,
        "during_credits": false, "original_title": format!("Episode {number}"),
        "available_translations": ["en", "it", "tr"], "effective_release_date": at
    })
}

/// `days` from now (negative: ago) at 17:00 UTC, as Trakt writes times.
fn day(days: i64) -> String {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs() as i64;
    // Howard Hinnant's civil-from-days.
    let z = now.div_euclid(86_400) + days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + i64::from(m <= 2);
    format!("{y:04}-{m:02}-{d:02}T17:00:00.000Z")
}

struct FakeTrakt {
    log: Mutex<Vec<String>>,
    /// Episodes airing in the week ahead, of 30 series.
    airing: u64,
}

fn answer(value: Value) -> Result<HttpResponse, NetError> {
    Ok(HttpResponse {
        status: 200,
        headers: Default::default(),
        body: serde_json::to_vec(&value).unwrap(),
    })
}

impl Transport for FakeTrakt {
    fn execute(&self, req: &HttpRequest) -> Result<HttpResponse, NetError> {
        self.log.lock().unwrap().push(req.url.clone());
        let url = req.url.as_str();
        if url.starts_with("https://api.trakt.tv/sync/progress/up_next?") {
            let behind: Vec<Value> = (0..BEHIND)
                .map(|n| {
                    let at = day(-1 - (n % 10) as i64);
                    json!({
                        "show": show(n),
                        "progress": {
                            "aired": 67, "completed": 60, "last_watched_at": "2026-10-01T20:00:00.000Z",
                            "reset_at": null, "next_episode": episode(3, 5, &at), "last_episode": episode(3, 4, &at)
                        }
                    })
                })
                .collect();
            return answer(Value::Array(behind));
        }
        if url.starts_with("https://api.trakt.tv/calendars/my/shows/") {
            let images = url.ends_with("?extended=images");
            let airing: Vec<Value> = (0..self.airing)
                .map(|n| {
                    let at = day(1 + (n % 6) as i64);
                    let series = 1_000 + n % 30;
                    if images {
                        json!({ "first_aired": at, "episode": episode(2, n, &at), "show": show(series) })
                    } else {
                        json!({
                            "first_aired": at,
                            "episode": { "season": 2, "number": n, "title": format!("Episode {n}"),
                                         "ids": { "trakt": 14_399_999, "tvdb": null, "imdb": null, "tmdb": 7_716_746 } },
                            "show": { "title": format!("Series {series}"), "year": 2024,
                                      "ids": { "trakt": 260_000 + series, "slug": format!("series-{series}"),
                                               "tvdb": 450_000 + series, "imdb": format!("tt{series:08}"), "tmdb": 10_000 + series } }
                        })
                    }
                })
                .collect();
            return answer(Value::Array(airing));
        }
        Ok(HttpResponse {
            status: 404,
            headers: Default::default(),
            body: b"not routed".to_vec(),
        })
    }
}

struct Shared(Arc<FakeTrakt>);
impl Transport for Shared {
    fn execute(&self, req: &HttpRequest) -> Result<HttpResponse, NetError> {
        self.0.execute(req)
    }
}

/// The Up next column, as Discover lists it with `airing` episodes in the
/// week ahead; and the URLs it fetched.
fn up_next_column(airing: u64) -> (Vec<rvah_addon_sdk::MediaItem>, Vec<String>) {
    let tmp = tempfile::tempdir().unwrap();
    let dist = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../examples/addons/discover/dist");
    let pkg = tmp.path().join("discover.rvah");
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
    vault
        .set(&id, "tmdb_key", "0123456789abcdef0123456789abcdef")
        .unwrap();
    vault.set(&id, "client_id", "trakt-client").unwrap();
    vault.set(&id, "access_token", "at1").unwrap();
    let trakt = Arc::new(FakeTrakt {
        log: Mutex::new(Vec::new()),
        airing,
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
            Box::new(Shared(Arc::clone(&trakt))),
            100,
        )),
    );

    let columns = core.browse(&id, None, None).unwrap();
    let PageItem::Directory(first) = &columns.items[0] else {
        panic!("expected columns");
    };
    assert_eq!(first.path, "trakt/up_next");

    let started = std::time::Instant::now();
    let column = core
        .browse(&id, Some("trakt/up_next".into()), None)
        .unwrap();
    eprintln!("up next, {airing} airing: {:?}", started.elapsed());
    let items = column
        .items
        .into_iter()
        .map(|item| match item {
            PageItem::Media(m) => m,
            other => panic!("expected media, got {other:?}"),
        })
        .collect();
    let urls = trakt.log.lock().unwrap().clone();
    (items, urls)
}

fn trakt_poster(item: &rvah_addon_sdk::MediaItem) -> bool {
    // media.trakt.tv is declared, so the posters survive the core's rule.
    item.artwork
        .poster
        .as_deref()
        .is_some_and(|p| p.starts_with("https://media.trakt.tv/"))
}

#[test]
#[ignore = "needs examples/addons/discover/build.sh"]
fn discover_guest_reads_the_busiest_week_with_posters() {
    let (items, urls) = up_next_column(38);
    // Every series behind on, then the 30 airing ones (each once).
    assert_eq!(items.len() as u64, BEHIND + 30);
    assert!(items[0].id.ends_with(":s3e5"), "{}", items[0].id);
    assert_eq!(items[0].note.as_deref(), Some("S3E5 · 7 to watch"));
    assert!(items.iter().all(trakt_poster));
    assert_eq!(urls.len(), 2);
}

#[test]
#[ignore = "needs examples/addons/discover/build.sh"]
fn discover_guest_reads_a_busier_week_without_posters() {
    let (items, urls) = up_next_column(300);
    assert_eq!(items.len() as u64, BEHIND + 30);
    let (behind, ahead) = items.split_at(BEHIND as usize);
    assert!(behind.iter().all(trakt_poster));
    assert!(ahead.iter().all(|item| item.artwork.poster.is_none()));
    assert!(ahead[0]
        .note
        .as_deref()
        .is_some_and(|n| n.starts_with("S2E")));
    assert!(urls[1].ends_with("?extended=images") && !urls[2].contains('?'));
}
