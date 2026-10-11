//! Trakt's API against the guest [`Host`], so it unit-tests natively with
//! a scripted host. Every call sends `trakt-api-key` (the app's client id)
//! and `trakt-api-version: 2`; calls about you add the access token.

use std::collections::BTreeMap;

use rvah_addon_sdk::guest::{self, Host};
use rvah_addon_sdk::{
    AccountStatus, AddonError, AddonErrorCode, Artwork, AuthPoll, ContentType, Credentials,
    DeviceAuth, DirectoryItem, MediaItem, Page, PageItem, ScrobbleAction, ScrobbleItem, SourceKind,
};
use serde::de::DeserializeOwned;
use serde::Deserialize;
use serde_json::{json, Value};

const API: &str = "https://api.trakt.tv";
const LIMIT: usize = 20;
/// Nobody browses further than this in a row.
const MAX_PAGES: u32 = 10;

/// What a row's answer holds: `{"movie": …}` wrappers, or the films or
/// series themselves.
#[derive(Clone, Copy)]
enum Shape {
    Wrapped,
    Movies,
}

/// (title, path, needs a sign-in, shape, extra query)
const ROWS: &[(&str, &str, bool, Shape, &str)] = &[
    // Series you are watching, by their next episode; see `up_next`.
    ("Up next", "up_next", true, Shape::Movies, ""),
    // Films and series together; see `recommendations`.
    ("You might like", "recommendations", true, Shape::Movies, ""),
    ("Your watchlist", "sync/watchlist", true, Shape::Wrapped, ""),
    (
        "Trending films",
        "movies/trending",
        false,
        Shape::Wrapped,
        "",
    ),
    (
        "Trending series",
        "shows/trending",
        false,
        Shape::Wrapped,
        "",
    ),
    (
        "Most anticipated films",
        "movies/anticipated",
        false,
        Shape::Wrapped,
        "",
    ),
    (
        "Most anticipated series",
        "shows/anticipated",
        false,
        Shape::Wrapped,
        "",
    ),
];

fn auth_error(message: &str) -> AddonError {
    let mut err = AddonError::new(AddonErrorCode::Auth, message);
    err.user_visible = true;
    err
}

fn client_id(host: &dyn Host) -> Result<String, AddonError> {
    guest::secret_get(host, "client_id")?.ok_or_else(|| {
        auth_error("no Trakt client id: set the client_id secret (app.trakt.tv/settings/apps)")
    })
}

fn access_token(host: &dyn Host) -> Result<Option<String>, AddonError> {
    Ok(guest::secret_get(host, "access_token")?.filter(|t| !t.is_empty()))
}

fn signed_in(host: &dyn Host) -> Result<String, AddonError> {
    access_token(host)?.ok_or_else(|| auth_error("not signed in to Trakt: use Link account"))
}

/// One call: `body` as JSON when given; `token` adds the Authorization.
fn call(
    host: &dyn Host,
    method: &str,
    path: &str,
    token: Option<&str>,
    body: Option<Value>,
) -> Result<rvah_addon_sdk::HttpResponse, AddonError> {
    let mut headers = BTreeMap::from([
        ("trakt-api-key".to_string(), client_id(host)?),
        ("trakt-api-version".to_string(), "2".to_string()),
        ("content-type".to_string(), "application/json".to_string()),
    ]);
    if let Some(token) = token {
        headers.insert("authorization".to_string(), format!("Bearer {token}"));
    }
    let body = body.map(|b| serde_json::to_vec(&b).expect("JSON body serializes"));
    guest::http_send(host, method, &format!("{API}{path}"), headers, body)
}

fn decode<T: DeserializeOwned>(what: &str, body: &[u8]) -> Result<T, AddonError> {
    serde_json::from_slice(body)
        .map_err(|e| AddonError::new(AddonErrorCode::Malformed, format!("Trakt {what}: {e}")))
}

fn failed(what: &str, status: u16) -> AddonError {
    match status {
        // Also how an expired access token shows; the host refreshes it.
        401 => auth_error("Trakt rejected the sign-in (401)"),
        status => AddonError::new(
            AddonErrorCode::Network,
            format!("Trakt answered {status} for {what}"),
        ),
    }
}

#[derive(Deserialize, Default)]
struct Ids {
    #[serde(default)]
    trakt: Option<u64>,
    #[serde(default)]
    imdb: Option<String>,
    #[serde(default)]
    tmdb: Option<u64>,
}

#[derive(Deserialize, Default)]
struct Images {
    #[serde(default)]
    poster: Vec<String>,
    #[serde(default)]
    fanart: Vec<String>,
}

/// A film or series (fields we use).
#[derive(Deserialize)]
struct Media {
    #[serde(default)]
    title: Option<String>,
    #[serde(default)]
    year: Option<u16>,
    #[serde(default)]
    ids: Ids,
    #[serde(default)]
    overview: Option<String>,
    #[serde(default)]
    runtime: Option<u32>,
    #[serde(default)]
    rating: Option<f32>,
    #[serde(default)]
    images: Images,
}

#[derive(Deserialize)]
struct Wrapper {
    #[serde(default)]
    movie: Option<Media>,
    #[serde(default)]
    show: Option<Media>,
}

/// Trakt's image paths come without a scheme.
fn image(path: Option<&String>) -> Option<String> {
    let path = path?;
    Some(if path.starts_with("https://") {
        path.clone()
    } else {
        format!("https://{}", path.trim_start_matches("//"))
    })
}

/// An item the TMDB add-on can describe: `movie:<tmdb>` or `tv:<tmdb>`.
fn item(media: Media, kind: ContentType) -> Option<MediaItem> {
    let tmdb = media.ids.tmdb?;
    let prefix = if kind == ContentType::Movie {
        "movie"
    } else {
        "tv"
    };
    let mut provider_ids = BTreeMap::from([("tmdb".to_string(), tmdb.to_string())]);
    if let Some(imdb) = media.ids.imdb.filter(|i| !i.is_empty()) {
        provider_ids.insert("imdb".into(), imdb);
    }
    if let Some(trakt) = media.ids.trakt {
        provider_ids.insert("trakt".into(), trakt.to_string());
    }
    Some(MediaItem {
        id: format!("{prefix}:{tmdb}"),
        content_type: kind,
        title: media.title.unwrap_or_default(),
        year: media.year,
        season: None,
        episode: None,
        runtime_secs: media.runtime.filter(|m| *m > 0).map(|m| m * 60),
        synopsis: media.overview.filter(|o| !o.trim().is_empty()),
        rating: media.rating.filter(|r| *r > 0.0),
        artwork: Artwork {
            poster: image(media.images.poster.first()),
            fanart: image(media.images.fanart.first()),
            thumb: None,
        },
        provider_ids,
        note: None,
    })
}

fn rows(signed_in: bool) -> Page {
    Page {
        items: ROWS
            .iter()
            .filter(|(_, _, personal, _, _)| signed_in || !personal)
            .map(|(title, path, _, _, _)| {
                PageItem::Directory(DirectoryItem {
                    id: path.to_string(),
                    title: title.to_string(),
                    subtitle: None,
                    artwork: Artwork::default(),
                    path: path.to_string(),
                    of_directories: false,
                    badges: vec![],
                })
            })
            .collect(),
        cursor_next: None,
        total_hint: None,
    }
}

/// The root is the rows (yours only once linked); a row's path lists it.
pub fn list(host: &dyn Host, path: Option<&str>, cursor: Option<&str>) -> Result<Page, AddonError> {
    let Some(path) = path.filter(|p| *p != "/") else {
        return Ok(rows(access_token(host)?.is_some()));
    };
    let &(_, path, personal, shape, extra) = ROWS
        .iter()
        .find(|(_, row, ..)| *row == path)
        .ok_or_else(|| AddonError::new(AddonErrorCode::NotFound, format!("no such row: {path}")))?;
    let page: u32 = match cursor {
        None => 1,
        Some(c) => c
            .parse()
            .ok()
            .filter(|n| *n >= 1)
            .ok_or_else(|| AddonError::new(AddonErrorCode::Malformed, "bad cursor"))?,
    };
    let token = if personal {
        Some(signed_in(host)?)
    } else {
        None
    };
    match path {
        "recommendations" => {
            return recommendations(host, token.as_deref().unwrap_or_default(), page)
        }
        "up_next" => return up_next(host, token.as_deref().unwrap_or_default()),
        _ => {}
    }
    let resp = call(
        host,
        "GET",
        &format!("/{path}?extended=full&page={page}&limit={LIMIT}{extra}"),
        token.as_deref(),
        None,
    )?;
    if !(200..300).contains(&resp.status) {
        return Err(failed(path, resp.status));
    }
    let media: Vec<(Media, ContentType)> = match shape {
        Shape::Wrapped => decode::<Vec<Wrapper>>(path, &resp.body)?
            .into_iter()
            .filter_map(|w| match (w.movie, w.show) {
                (Some(movie), _) => Some((movie, ContentType::Movie)),
                (None, Some(show)) => Some((show, ContentType::Series)),
                _ => None,
            })
            .collect(),
        Shape::Movies => decode::<Vec<Media>>(path, &resp.body)?
            .into_iter()
            .map(|m| (m, ContentType::Movie))
            .collect(),
    };
    let full = media.len() >= LIMIT;
    // Trakt says how many pages there are; without that, a full page may
    // have another after it.
    let pages = resp
        .headers
        .iter()
        .find(|(name, _)| name.eq_ignore_ascii_case("x-pagination-page-count"))
        .and_then(|(_, value)| value.trim().parse::<u32>().ok());
    let more = match pages {
        Some(count) => page < count,
        None => full,
    };
    Ok(Page {
        items: media
            .into_iter()
            .filter_map(|(m, kind)| item(m, kind))
            .map(PageItem::Media)
            .collect(),
        cursor_next: (more && page < MAX_PAGES).then(|| (page + 1).to_string()),
        total_hint: None,
    })
}

#[derive(Deserialize)]
struct DeviceCode {
    device_code: String,
    user_code: String,
    verification_url: String,
    expires_in: u32,
    interval: u32,
}

#[derive(Deserialize)]
struct Tokens {
    access_token: String,
    refresh_token: String,
}

fn credentials(tokens: Tokens) -> Credentials {
    Credentials(BTreeMap::from([
        ("access_token".to_string(), tokens.access_token),
        ("refresh_token".to_string(), tokens.refresh_token),
    ]))
}

/// Start linking: a code to enter at trakt.tv/activate. Trakt apps have no
/// client secret any more; the client id is enough.
pub fn auth_begin(host: &dyn Host) -> Result<DeviceAuth, AddonError> {
    let body = json!({ "client_id": client_id(host)? });
    let resp = call(host, "POST", "/oauth/device/code", None, Some(body))?;
    if !(200..300).contains(&resp.status) {
        return Err(failed("device code", resp.status));
    }
    let code: DeviceCode = decode("device code", &resp.body)?;
    Ok(DeviceAuth {
        user_code: code.user_code,
        verification_url: code.verification_url,
        device_code: code.device_code,
        interval_secs: code.interval,
        expires_in_secs: code.expires_in,
    })
}

/// Whether the user approved the code yet.
pub fn auth_poll(host: &dyn Host, device_code: &str) -> Result<AuthPoll, AddonError> {
    let body = json!({ "code": device_code, "client_id": client_id(host)? });
    let resp = call(host, "POST", "/oauth/device/token", None, Some(body))?;
    match resp.status {
        200 => Ok(AuthPoll::Authorized(credentials(decode(
            "token", &resp.body,
        )?))),
        // Waiting for the user; or polling a little too fast.
        400 | 429 => Ok(AuthPoll::Pending),
        418 => Ok(AuthPoll::Denied),
        404 | 409 | 410 => Err(auth_error(
            "the Trakt code expired or was used: start again",
        )),
        status => Err(failed("device token", status)),
    }
}

/// New tokens for the stored refresh token (each is single-use).
pub fn auth_refresh(host: &dyn Host) -> Result<Credentials, AddonError> {
    let refresh = guest::secret_get(host, "refresh_token")?
        .ok_or_else(|| auth_error("not signed in to Trakt: use Link account"))?;
    let redirect = guest::setting_get(host, "redirect_uri")?
        .unwrap_or_else(|| "urn:ietf:wg:oauth:2.0:oob".into());
    let body = json!({
        "refresh_token": refresh,
        "client_id": client_id(host)?,
        "redirect_uri": redirect,
        "grant_type": "refresh_token",
    });
    let resp = call(host, "POST", "/oauth/token", None, Some(body))?;
    match resp.status {
        200 => Ok(credentials(decode("token", &resp.body)?)),
        400 | 401 => Err(auth_error(
            "Trakt refused to renew the sign-in: use Link account",
        )),
        status => Err(failed("token", status)),
    }
}

/// Who is linked, from `/users/settings`.
pub fn account(host: &dyn Host) -> Result<AccountStatus, AddonError> {
    #[derive(Deserialize)]
    struct User {
        username: String,
        #[serde(default)]
        name: Option<String>,
    }
    #[derive(Deserialize)]
    struct Settings {
        user: User,
    }
    let Some(token) = access_token(host)? else {
        return Ok(AccountStatus::SignedOut);
    };
    let resp = call(host, "GET", "/users/settings", Some(&token), None)?;
    match resp.status {
        200..=299 => {
            let user = decode::<Settings>("settings", &resp.body)?.user;
            let name = user
                .name
                .filter(|n| !n.trim().is_empty())
                .unwrap_or(user.username);
            Ok(AccountStatus::SignedIn {
                display_name: Some(name),
            })
        }
        // Expired: the host refreshes and asks again.
        status => Err(failed("settings", status)),
    }
}

/// Tell Trakt playback started, paused or stopped. Only an expired sign-in
/// is an error (so the host refreshes it); anything else Trakt says, such
/// as "already scrobbled", is logged, so history never gets in the way.
pub fn scrobble(
    host: &dyn Host,
    action: ScrobbleAction,
    item: &ScrobbleItem,
    progress: f32,
) -> Result<(), AddonError> {
    let Some(token) = access_token(host)? else {
        return Ok(());
    };
    // Trakt ignores a pause or stop under 1% (answering 422): nothing to say.
    if action != ScrobbleAction::Start && progress < 1.0 {
        return Ok(());
    }
    let Ok(tmdb) = item.tmdb_id.parse::<u64>() else {
        guest::log(host, &format!("not a TMDB id: {}", item.tmdb_id));
        return Ok(());
    };
    let body = match (item.kind, item.season, item.episode) {
        (SourceKind::Episode, Some(season), Some(number)) => json!({
            "show": { "ids": { "tmdb": tmdb } },
            "episode": { "season": season, "number": number },
            "progress": progress,
        }),
        _ => json!({ "movie": { "ids": { "tmdb": tmdb } }, "progress": progress }),
    };
    let verb = match action {
        ScrobbleAction::Start => "start",
        ScrobbleAction::Pause => "pause",
        ScrobbleAction::Stop => "stop",
    };
    let resp = call(
        host,
        "POST",
        &format!("/scrobble/{verb}"),
        Some(&token),
        Some(body),
    )?;
    match resp.status {
        200..=299 => Ok(()),
        401 => Err(failed("scrobble", 401)),
        status => {
            guest::log(
                host,
                &format!("Trakt scrobble {verb} at {progress:.1}%: {status}"),
            );
            Ok(())
        }
    }
}

/// One of your rows, for an add-on nesting this one: only once linked.
fn your_row(host: &dyn Host, path: &str) -> Result<Option<PageItem>, AddonError> {
    if access_token(host)?.is_none() {
        return Ok(None);
    }
    Ok(rows(true)
        .items
        .into_iter()
        .find(|row| matches!(row, PageItem::Directory(d) if d.path == path)))
}

/// The "You might like" column, for an add-on nesting this one.
pub fn you_might_like(host: &dyn Host) -> Result<Option<PageItem>, AddonError> {
    your_row(host, "recommendations")
}

/// The "Up next" column, for an add-on nesting this one.
pub fn up_next_row(host: &dyn Host) -> Result<Option<PageItem>, AddonError> {
    your_row(host, "up_next")
}

/// Recommended films and series together, alternating.
fn recommendations(host: &dyn Host, token: &str, page: u32) -> Result<Page, AddonError> {
    const EACH: usize = LIMIT / 2;
    let fetch = |kind: &str| -> Result<Vec<Media>, AddonError> {
        let path = format!(
            "/recommendations/{kind}?extended=full&page={page}&limit={EACH}&ignore_collected=true"
        );
        let resp = call(host, "GET", &path, Some(token), None)?;
        if !(200..300).contains(&resp.status) {
            return Err(failed("recommendations", resp.status));
        }
        decode("recommendations", &resp.body)
    };
    let films = fetch("movies")?;
    let shows = fetch("shows")?;
    let more = films.len() >= EACH || shows.len() >= EACH;
    let mut films = films.into_iter().map(|m| (m, ContentType::Movie));
    let mut shows = shows.into_iter().map(|m| (m, ContentType::Series));
    let mut media = Vec::new();
    loop {
        match (films.next(), shows.next()) {
            (None, None) => break,
            (a, b) => media.extend(a.into_iter().chain(b)),
        }
    }
    Ok(Page {
        items: media
            .into_iter()
            .filter_map(|(m, kind)| item(m, kind))
            .map(PageItem::Media)
            .collect(),
        cursor_next: (more && page < MAX_PAGES).then(|| (page + 1).to_string()),
        total_hint: None,
    })
}

/// How far ahead "Up next" looks for new episodes.
const AHEAD_DAYS: u32 = 7;
/// Series you are behind on, at most. Trakt answers with whole series and
/// episode records (about 5 KB each with images), and a call's fuel is
/// spent reading them.
const BEHIND: usize = 15;
/// The most of the week's calendar read with images, as it crosses from
/// the host: about 150 KB of Trakt's answer, some 40 episodes. A busier week
/// is read again without images (a tenth the size), so without posters.
const CALENDAR_MAX: usize = 520_000;

#[derive(Deserialize)]
struct Episode {
    season: u32,
    number: u32,
    #[serde(default)]
    first_aired: Option<String>,
}

#[derive(Deserialize)]
struct Progress {
    #[serde(default)]
    aired: u32,
    #[serde(default)]
    completed: u32,
    #[serde(default)]
    next_episode: Option<Episode>,
}

#[derive(Deserialize)]
struct Watching {
    show: Media,
    progress: Progress,
}

#[derive(Deserialize)]
struct Airing {
    first_aired: String,
    episode: Episode,
    show: Media,
}

/// Series you are watching: first those with an episode out for you to
/// watch, the newest release first, each opening on that episode; then
/// those with one coming in the week ahead, the soonest first, each opening
/// on the series. Each says which episode, and when.
fn up_next(host: &dyn Host, token: &str) -> Result<Page, AddonError> {
    let now = guest::now_unix(host).unwrap_or_default();
    let get = |what: &str, path: &str| -> Result<Vec<u8>, AddonError> {
        let resp = call(host, "GET", path, Some(token), None)?;
        if !(200..300).contains(&resp.status) {
            return Err(failed(what, resp.status));
        }
        Ok(resp.body)
    };

    let watching: Vec<Watching> = decode(
        "up next",
        &get(
            "up next",
            &format!("/sync/progress/up_next?extended=images&page=1&limit={BEHIND}"),
        )?,
    )?;
    let mut behind: Vec<(u64, MediaItem)> = watching
        .into_iter()
        .filter_map(|w| {
            let next = w.progress.next_episode?;
            let aired = next.first_aired.as_deref().and_then(unix_time).unwrap_or(0);
            let left = w.progress.aired.saturating_sub(w.progress.completed);
            let when = if left > 1 {
                format!("{left} to watch")
            } else {
                "out now".to_string()
            };
            let mut item = item(w.show, ContentType::Episode)?;
            item.id = format!("{}:s{}e{}", item.id, next.season, next.number);
            item.season = Some(next.season);
            item.episode = Some(next.number);
            item.note = Some(format!("S{}E{} · {when}", next.season, next.number));
            Some((aired, item))
        })
        .collect();
    behind.sort_by_key(|(aired, _)| std::cmp::Reverse(*aired));

    let calendar = format!(
        "/calendars/my/shows/{}/{AHEAD_DAYS}",
        guest::civil_date(now)
    );
    let airing: Vec<Airing> =
        match calendar_within(host, token, &format!("{calendar}?extended=images"))? {
            Some(body) => decode("calendar", &body)?,
            None => match calendar_within(host, token, &calendar)? {
                Some(body) => decode("calendar", &body)?,
                None => {
                    guest::log(
                        host,
                        "the week's calendar is too long to read: Up next leaves it out",
                    );
                    Vec::new()
                }
            },
        };
    let mut ahead: Vec<(u64, MediaItem)> = Vec::new();
    for entry in airing {
        let Some(at) = unix_time(&entry.first_aired).filter(|at| *at > now) else {
            continue;
        };
        let Some(mut item) = item(entry.show, ContentType::Series) else {
            continue;
        };
        // A series with an episode out already is listed for that.
        let series = format!("{}:", item.id);
        if behind.iter().any(|(_, b)| b.id.starts_with(&series)) {
            continue;
        }
        // The soonest episode of each series.
        if let Some(pos) = ahead.iter().position(|(_, a)| a.id == item.id) {
            if ahead[pos].0 <= at {
                continue;
            }
            ahead.remove(pos);
        }
        let (season, number) = (entry.episode.season, entry.episode.number);
        item.note = Some(format!("S{season}E{number} · {}", until(at - now)));
        ahead.push((at, item));
    }
    ahead.sort_by_key(|(at, _)| *at);

    Ok(Page {
        items: behind
            .into_iter()
            .chain(ahead)
            .map(|(_, item)| PageItem::Media(item))
            .collect(),
        cursor_next: None,
        total_hint: None,
    })
}

/// The calendar at `path`, unless its answer is over [`CALENDAR_MAX`].
fn calendar_within(
    host: &dyn Host,
    token: &str,
    path: &str,
) -> Result<Option<Vec<u8>>, AddonError> {
    let headers = BTreeMap::from([
        ("trakt-api-key".to_string(), client_id(host)?),
        ("trakt-api-version".to_string(), "2".to_string()),
        ("authorization".to_string(), format!("Bearer {token}")),
    ]);
    let Some(resp) = guest::http_send_within(
        host,
        "GET",
        &format!("{API}{path}"),
        headers,
        None,
        CALENDAR_MAX,
    )?
    else {
        return Ok(None);
    };
    if !(200..300).contains(&resp.status) {
        return Err(failed("calendar", resp.status));
    }
    Ok(Some(resp.body))
}

/// "in 5 h", "in 3 days": how long until a new episode.
fn until(secs: u64) -> String {
    match secs {
        0..3_600 => "within the hour".to_string(),
        3_600..86_400 => format!("in {} h", secs / 3_600),
        86_400..172_800 => "in 1 day".to_string(),
        _ => format!("in {} days", secs / 86_400),
    }
}

/// A Trakt time (`2026-10-11T01:00:00.000Z`, UTC) as Unix seconds.
fn unix_time(at: &str) -> Option<u64> {
    let num = |range: std::ops::Range<usize>| at.get(range)?.parse::<i64>().ok();
    let (year, month, day) = (num(0..4)?, num(5..7)?, num(8..10)?);
    let (hour, minute, second) = (num(11..13)?, num(14..16)?, num(17..19)?);
    // Howard Hinnant's days-from-civil.
    let y = if month <= 2 { year - 1 } else { year };
    let era = y.div_euclid(400);
    let year_of_era = y - era * 400;
    let day_of_year = (153 * ((month + 9) % 12) + 2) / 5 + day - 1;
    let day_of_era = year_of_era * 365 + year_of_era / 4 - year_of_era / 100 + day_of_year;
    let days = era * 146_097 + day_of_era - 719_468;
    u64::try_from(days * 86_400 + hour * 3_600 + minute * 60 + second).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn trakt_times_read_as_unix_seconds() {
        assert_eq!(unix_time("1970-01-01T00:00:00.000Z"), Some(0));
        assert_eq!(unix_time("2000-02-29T23:59:59.000Z"), Some(951_868_799));
        assert_eq!(unix_time("2026-10-11T12:00:00Z"), Some(1_791_720_000));
        assert_eq!(unix_time("soon"), None);
    }

    #[test]
    fn until_says_how_long() {
        assert_eq!(until(59), "within the hour");
        assert_eq!(until(3 * 3_600 + 5), "in 3 h");
        assert_eq!(until(86_400 + 1), "in 1 day");
        assert_eq!(until(5 * 86_400), "in 5 days");
    }
}
