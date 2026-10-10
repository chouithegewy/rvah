use std::sync::mpsc;
use std::sync::Arc;
use std::time::Duration;

use rvah_addon_sdk::{
    AccountStatus, AddonError, AddonErrorCode, AddonProvider, AuthPoll, Credentials, DeviceAuth,
    Filter, HostApi, ItemDetails, Page, Release, ResolveResult, ScrobbleAction, ScrobbleItem,
    SourceRequest,
};

use crate::manifest::Capability;

/// A single provider operation, serialized across the runtime boundary.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum AddonOp {
    List {
        path: Option<String>,
        cursor: Option<String>,
    },
    Search {
        query: String,
        filters: Vec<Filter>,
        cursor: Option<String>,
    },
    Resolve {
        item_id: String,
        source_id: Option<String>,
    },
    AccountStatus,
    /// Start a device-code sign-in (UC-04).
    AuthBegin,
    AuthPoll {
        device_code: String,
    },
    /// Trade stored refresh material for fresh credentials.
    AuthRefresh,
    Details {
        item_id: String,
    },
    Sources {
        request: SourceRequest,
    },
    ResolveRelease {
        release: Release,
        season: Option<u32>,
        episode: Option<u32>,
    },
    Scrobble {
        action: ScrobbleAction,
        item: ScrobbleItem,
        progress: f32,
    },
}

impl AddonOp {
    /// The manifest capability this operation requires (FR-021).
    pub fn required_capability(&self) -> Capability {
        match self {
            AddonOp::List { .. } => Capability::Catalog,
            AddonOp::Search { .. } => Capability::Search,
            AddonOp::Resolve { .. } => Capability::Resolve,
            AddonOp::AccountStatus => Capability::AccountStatus,
            AddonOp::AuthBegin | AddonOp::AuthPoll { .. } | AddonOp::AuthRefresh => {
                Capability::Auth
            }
            AddonOp::Details { .. } => Capability::Details,
            AddonOp::Sources { .. } => Capability::Sources,
            AddonOp::ResolveRelease { .. } => Capability::Debrid,
            AddonOp::Scrobble { .. } => Capability::Scrobble,
        }
    }
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(tag = "kind", content = "value", rename_all = "snake_case")]
pub enum AddonResponse {
    Page(Page),
    Resolve(ResolveResult),
    Account(AccountStatus),
    DeviceAuth(DeviceAuth),
    AuthPoll(AuthPoll),
    Credentials(Credentials),
    Details(Box<ItemDetails>),
    Releases(Vec<Release>),
    /// An operation that answers nothing, such as `scrobble`.
    Done,
}

/// Per-invocation environment: deadline plus the capability-checked host API.
pub struct InvocationCtx {
    pub timeout: Duration,
    pub host: Arc<dyn HostApi>,
}

/// Uniform boundary between the host and any add-on execution strategy:
/// in-process fixtures (tests/simulator, FR-027) or the wasm sandbox (FR-020).
pub trait AddonRuntime: Send {
    fn invoke(&self, op: AddonOp, ctx: &InvocationCtx) -> Result<AddonResponse, AddonError>;
}

/// Wraps any runtime with manifest-capability gating (FR-021). The
/// in-process runtime gates internally; sandboxed runtimes (wasm) are
/// registered behind this wrapper.
pub struct CapabilityGated<R> {
    inner: R,
    capabilities: Vec<Capability>,
}

impl<R: AddonRuntime> CapabilityGated<R> {
    pub fn new(inner: R, capabilities: Vec<Capability>) -> Self {
        Self {
            inner,
            capabilities,
        }
    }
}

impl<R: AddonRuntime> AddonRuntime for CapabilityGated<R> {
    fn invoke(&self, op: AddonOp, ctx: &InvocationCtx) -> Result<AddonResponse, AddonError> {
        let required = op.required_capability();
        if !self.capabilities.contains(&required) {
            return Err(AddonError::new(
                AddonErrorCode::PermissionDenied,
                format!("capability {required:?} not declared in manifest"),
            ));
        }
        self.inner.invoke(op, ctx)
    }
}

/// Runs a native provider on a worker thread with a wall-clock timeout
/// (FR-022): a hung provider strands its thread, but never the host.
pub struct InProcessRuntime {
    provider: Arc<dyn AddonProvider + Send + Sync>,
    capabilities: Vec<Capability>,
}

impl InProcessRuntime {
    pub fn new(
        provider: Arc<dyn AddonProvider + Send + Sync>,
        capabilities: Vec<Capability>,
    ) -> Self {
        Self {
            provider,
            capabilities,
        }
    }
}

impl AddonRuntime for InProcessRuntime {
    fn invoke(&self, op: AddonOp, ctx: &InvocationCtx) -> Result<AddonResponse, AddonError> {
        let required = op.required_capability();
        if !self.capabilities.contains(&required) {
            return Err(AddonError::new(
                AddonErrorCode::PermissionDenied,
                format!("capability {required:?} not declared in manifest"),
            ));
        }

        let provider = Arc::clone(&self.provider);
        let host = Arc::clone(&ctx.host);
        let (tx, rx) = mpsc::channel();
        std::thread::spawn(move || {
            let result = match op {
                AddonOp::List { path, cursor } => provider
                    .list(host.as_ref(), path.as_deref(), cursor.as_deref())
                    .map(AddonResponse::Page),
                AddonOp::Search {
                    query,
                    filters,
                    cursor,
                } => provider
                    .search(host.as_ref(), &query, &filters, cursor.as_deref())
                    .map(AddonResponse::Page),
                AddonOp::Resolve { item_id, source_id } => provider
                    .resolve(host.as_ref(), &item_id, source_id.as_deref())
                    .map(AddonResponse::Resolve),
                AddonOp::AccountStatus => provider
                    .account_status(host.as_ref())
                    .map(AddonResponse::Account),
                AddonOp::AuthBegin => provider
                    .auth_begin(host.as_ref())
                    .map(AddonResponse::DeviceAuth),
                AddonOp::AuthPoll { device_code } => provider
                    .auth_poll(host.as_ref(), &device_code)
                    .map(AddonResponse::AuthPoll),
                AddonOp::AuthRefresh => provider
                    .auth_refresh(host.as_ref())
                    .map(AddonResponse::Credentials),
                AddonOp::Details { item_id } => provider
                    .details(host.as_ref(), &item_id)
                    .map(|details| AddonResponse::Details(Box::new(details))),
                AddonOp::Sources { request } => provider
                    .sources(host.as_ref(), &request)
                    .map(AddonResponse::Releases),
                AddonOp::ResolveRelease {
                    release,
                    season,
                    episode,
                } => provider
                    .resolve_release(host.as_ref(), &release, season, episode)
                    .map(AddonResponse::Resolve),
                AddonOp::Scrobble {
                    action,
                    item,
                    progress,
                } => provider
                    .scrobble(host.as_ref(), action, &item, progress)
                    .map(|()| AddonResponse::Done),
            };
            let _ = tx.send(result);
        });

        match rx.recv_timeout(ctx.timeout) {
            Ok(result) => result,
            Err(_) => Err(AddonError::new(
                AddonErrorCode::Timeout,
                format!("add-on call exceeded {:?}", ctx.timeout),
            )),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rvah_addon_sdk::{HttpRequest, HttpResponse, LogLevel, PageItem};

    struct NullHost;
    impl HostApi for NullHost {
        fn http_fetch(&self, _req: HttpRequest) -> Result<HttpResponse, AddonError> {
            Err(AddonError::new(AddonErrorCode::PermissionDenied, "no net"))
        }
        fn setting_get(&self, _key: &str) -> Option<String> {
            None
        }
        fn secret_get(&self, _name: &str) -> Result<Option<String>, AddonError> {
            Ok(None)
        }
        fn storage_get(&self, _key: &str) -> Option<Vec<u8>> {
            None
        }
        fn storage_set(&self, _key: &str, _value: &[u8]) -> Result<(), AddonError> {
            Ok(())
        }
        fn storage_delete(&self, _key: &str) {}
        fn log(&self, _level: LogLevel, _message: &str) {}
    }

    fn ctx(timeout_ms: u64) -> InvocationCtx {
        InvocationCtx {
            timeout: Duration::from_millis(timeout_ms),
            host: Arc::new(NullHost),
        }
    }

    struct EchoProvider;
    impl AddonProvider for EchoProvider {
        fn list(
            &self,
            _ctx: &dyn HostApi,
            path: Option<&str>,
            _cursor: Option<&str>,
        ) -> Result<Page, AddonError> {
            let mut page = Page::default();
            page.items
                .push(PageItem::Directory(rvah_addon_sdk::DirectoryItem {
                    id: "d1".into(),
                    title: format!("echo:{}", path.unwrap_or("/")),
                    subtitle: None,
                    artwork: Default::default(),
                    path: "/d1".into(),
                    badges: vec![],
                }));
            Ok(page)
        }
    }

    struct SlowProvider;
    impl AddonProvider for SlowProvider {
        fn list(
            &self,
            _ctx: &dyn HostApi,
            _path: Option<&str>,
            _cursor: Option<&str>,
        ) -> Result<Page, AddonError> {
            std::thread::sleep(Duration::from_secs(5));
            Ok(Page::default())
        }
    }

    #[test]
    fn in_process_runtime_lists_pages() {
        let rt = InProcessRuntime::new(Arc::new(EchoProvider), vec![Capability::Catalog]);
        let resp = rt
            .invoke(
                AddonOp::List {
                    path: Some("/movies".into()),
                    cursor: None,
                },
                &ctx(1_000),
            )
            .unwrap();
        match resp {
            AddonResponse::Page(page) => {
                assert_eq!(page.items.len(), 1);
                match &page.items[0] {
                    PageItem::Directory(d) => assert_eq!(d.title, "echo:/movies"),
                    other => panic!("unexpected item {other:?}"),
                }
            }
            other => panic!("unexpected response {other:?}"),
        }
    }

    #[test]
    fn runtime_times_out_slow_provider() {
        // TC-RUN-004: the host regains control; the error is retryable.
        let rt = InProcessRuntime::new(Arc::new(SlowProvider), vec![Capability::Catalog]);
        let err = rt
            .invoke(
                AddonOp::List {
                    path: None,
                    cursor: None,
                },
                &ctx(50),
            )
            .unwrap_err();
        assert_eq!(err.code, AddonErrorCode::Timeout);
        assert!(err.retryable);
    }

    struct LinkProvider;
    impl AddonProvider for LinkProvider {
        fn auth_poll(&self, _ctx: &dyn HostApi, device_code: &str) -> Result<AuthPoll, AddonError> {
            Ok(match device_code {
                "approved" => AuthPoll::Authorized(Credentials(
                    [("api_token".to_string(), "tok".to_string())].into(),
                )),
                _ => AuthPoll::Pending,
            })
        }
    }

    #[test]
    fn auth_ops_need_the_auth_capability() {
        let poll = |code: &str| AddonOp::AuthPoll {
            device_code: code.into(),
        };
        let rt = InProcessRuntime::new(Arc::new(LinkProvider), vec![Capability::Auth]);
        assert_eq!(
            rt.invoke(poll("waiting"), &ctx(1_000)).unwrap(),
            AddonResponse::AuthPoll(AuthPoll::Pending)
        );
        assert!(matches!(
            rt.invoke(poll("approved"), &ctx(1_000)).unwrap(),
            AddonResponse::AuthPoll(AuthPoll::Authorized(_))
        ));
        // Not implemented by the provider: a structured error, not a panic.
        let err = rt.invoke(AddonOp::AuthRefresh, &ctx(1_000)).unwrap_err();
        assert_eq!(err.code, AddonErrorCode::Internal);

        let undeclared = InProcessRuntime::new(Arc::new(LinkProvider), vec![Capability::Catalog]);
        for op in [AddonOp::AuthBegin, poll("approved"), AddonOp::AuthRefresh] {
            let err = undeclared.invoke(op, &ctx(1_000)).unwrap_err();
            assert_eq!(err.code, AddonErrorCode::PermissionDenied);
        }
    }

    #[test]
    fn auth_ops_wire_format() {
        let wire = serde_json::to_value(AddonOp::AuthPoll {
            device_code: "dc".into(),
        })
        .unwrap();
        assert_eq!(
            wire,
            serde_json::json!({ "op": "auth_poll", "device_code": "dc" })
        );
        let begin = serde_json::to_value(AddonOp::AuthBegin).unwrap();
        assert_eq!(begin, serde_json::json!({ "op": "auth_begin" }));
    }

    struct DetailsProvider;
    impl AddonProvider for DetailsProvider {
        fn details(
            &self,
            _ctx: &dyn HostApi,
            item_id: &str,
        ) -> Result<rvah_addon_sdk::ItemDetails, AddonError> {
            Ok(rvah_addon_sdk::ItemDetails {
                item: rvah_addon_sdk::MediaItem {
                    id: item_id.into(),
                    content_type: rvah_addon_sdk::ContentType::Movie,
                    title: "Detailed".into(),
                    year: Some(2026),
                    season: None,
                    episode: None,
                    runtime_secs: None,
                    synopsis: None,
                    rating: None,
                    artwork: Default::default(),
                    provider_ids: Default::default(),
                },
                series_title: None,
                tagline: None,
                genres: vec!["Comedy".into()],
                released: Some("2026-03-14".into()),
                trailers: vec![rvah_addon_sdk::Trailer {
                    name: "Official Trailer".into(),
                    site: rvah_addon_sdk::VideoSite::YouTube,
                    key: "abc123XYZ_-".into(),
                }],
                ratings: vec![rvah_addon_sdk::Rating {
                    source: "rotten_tomatoes".into(),
                    value: "93%".into(),
                }],
            })
        }
    }

    #[test]
    fn details_needs_the_details_capability() {
        let op = || AddonOp::Details {
            item_id: "movie:1".into(),
        };
        let rt = InProcessRuntime::new(Arc::new(DetailsProvider), vec![Capability::Details]);
        let AddonResponse::Details(details) = rt.invoke(op(), &ctx(1_000)).unwrap() else {
            panic!("expected details");
        };
        assert_eq!(details.item.id, "movie:1");
        let undeclared =
            InProcessRuntime::new(Arc::new(DetailsProvider), vec![Capability::Catalog]);
        let err = undeclared.invoke(op(), &ctx(1_000)).unwrap_err();
        assert_eq!(err.code, AddonErrorCode::PermissionDenied);
    }

    #[test]
    fn sources_and_debrid_need_their_capabilities() {
        let sources = AddonOp::Sources {
            request: SourceRequest {
                kind: rvah_addon_sdk::SourceKind::Movie,
                imdb_id: Some("tt1".into()),
                title: "Film".into(),
                year: Some(2026),
                season: None,
                episode: None,
            },
        };
        let release = AddonOp::ResolveRelease {
            release: Release {
                name: "Film.2026.2160p".into(),
                info_hash: "b".repeat(40),
                size_bytes: Some(1),
                seeders: Some(2),
                origin: None,
            },
            season: None,
            episode: None,
        };
        let rt = InProcessRuntime::new(Arc::new(DetailsProvider), vec![Capability::Details]);
        for op in [sources.clone(), release.clone()] {
            let err = rt.invoke(op, &ctx(1_000)).unwrap_err();
            assert_eq!(err.code, AddonErrorCode::PermissionDenied);
        }
        let rt = InProcessRuntime::new(
            Arc::new(DetailsProvider),
            vec![Capability::Sources, Capability::Debrid],
        );
        // Declared but not implemented: a structured error, not a panic.
        for op in [sources, release] {
            let err = rt.invoke(op, &ctx(1_000)).unwrap_err();
            assert_eq!(err.code, AddonErrorCode::Internal);
        }
        let wire = serde_json::to_value(AddonOp::Sources {
            request: SourceRequest {
                kind: rvah_addon_sdk::SourceKind::Episode,
                imdb_id: None,
                title: "Show".into(),
                year: None,
                season: Some(1),
                episode: Some(2),
            },
        })
        .unwrap();
        assert_eq!(
            wire,
            serde_json::json!({
                "op": "sources",
                "request": { "kind": "episode", "title": "Show", "season": 1, "episode": 2 }
            })
        );
        let caps: Vec<Capability> = serde_json::from_str(r#"["sources","debrid"]"#).unwrap();
        assert_eq!(caps, [Capability::Sources, Capability::Debrid]);
    }

    #[test]
    fn details_wire_format() {
        let wire = serde_json::to_value(AddonOp::Details {
            item_id: "movie:1".into(),
        })
        .unwrap();
        assert_eq!(
            wire,
            serde_json::json!({ "op": "details", "item_id": "movie:1" })
        );
        let rt = InProcessRuntime::new(Arc::new(DetailsProvider), vec![Capability::Details]);
        let response = rt
            .invoke(
                AddonOp::Details {
                    item_id: "movie:1".into(),
                },
                &ctx(1_000),
            )
            .unwrap();
        let wire = serde_json::to_value(&response).unwrap();
        assert_eq!(wire["kind"], "details");
        assert_eq!(
            wire["value"]["trailers"][0],
            serde_json::json!({ "name": "Official Trailer", "site": "youtube", "key": "abc123XYZ_-" })
        );
        assert_eq!(
            wire["value"]["ratings"][0],
            serde_json::json!({ "source": "rotten_tomatoes", "value": "93%" })
        );
        let manifest_caps: Vec<Capability> = serde_json::from_str(r#"["details"]"#).unwrap();
        assert_eq!(manifest_caps, [Capability::Details]);
    }

    /// The SDK's guest mirrors must read every op the host sends, and the
    /// host must read every answer a guest sends.
    #[test]
    fn guest_wire_types_match_the_host() {
        use rvah_addon_sdk::guest::{Op, Response};
        let ops = [
            AddonOp::List {
                path: Some("/a".into()),
                cursor: Some("2".into()),
            },
            AddonOp::Search {
                query: "q".into(),
                filters: vec![],
                cursor: None,
            },
            AddonOp::Resolve {
                item_id: "i".into(),
                source_id: None,
            },
            AddonOp::AccountStatus,
            AddonOp::AuthBegin,
            AddonOp::AuthPoll {
                device_code: "d".into(),
            },
            AddonOp::AuthRefresh,
            AddonOp::Details {
                item_id: "i".into(),
            },
            AddonOp::Sources {
                request: SourceRequest {
                    kind: rvah_addon_sdk::SourceKind::Episode,
                    imdb_id: Some("tt1".into()),
                    title: "Show".into(),
                    year: None,
                    season: Some(1),
                    episode: Some(2),
                },
            },
            AddonOp::ResolveRelease {
                release: Release {
                    name: "Show.S01E02".into(),
                    info_hash: "a".repeat(40),
                    size_bytes: None,
                    seeders: None,
                    origin: None,
                },
                season: Some(1),
                episode: Some(2),
            },
            AddonOp::Scrobble {
                action: ScrobbleAction::Stop,
                item: ScrobbleItem {
                    kind: rvah_addon_sdk::SourceKind::Episode,
                    tmdb_id: "1399".into(),
                    imdb_id: None,
                    season: Some(1),
                    episode: Some(2),
                },
                progress: 91.5,
            },
        ];
        for op in ops {
            let wire = serde_json::to_vec(&op).unwrap();
            let guest: Op = serde_json::from_slice(&wire).unwrap();
            assert_eq!(
                format!("{op:?}").split([' ', '{']).next(),
                format!("{guest:?}").split([' ', '{']).next()
            );
        }
        let answers = [
            Response::Page(Page::default()),
            Response::Account(AccountStatus::SignedOut),
            Response::AuthPoll(AuthPoll::Pending),
            Response::Credentials(Credentials::default()),
            Response::Releases(vec![]),
            Response::Done,
        ];
        for answer in answers {
            let wire = serde_json::to_vec(&answer).unwrap();
            let host: AddonResponse = serde_json::from_slice(&wire).unwrap();
            assert_eq!(serde_json::to_vec(&host).unwrap(), wire);
        }
    }

    #[test]
    fn capability_gate_denies_undeclared_op() {
        // Provider implements list, but the manifest never declared catalog.
        let rt = InProcessRuntime::new(Arc::new(EchoProvider), vec![Capability::Resolve]);
        let err = rt
            .invoke(
                AddonOp::List {
                    path: None,
                    cursor: None,
                },
                &ctx(1_000),
            )
            .unwrap_err();
        assert_eq!(err.code, AddonErrorCode::PermissionDenied);
    }

    #[test]
    fn capability_gate_wraps_any_runtime() {
        struct AlwaysPage;
        impl AddonRuntime for AlwaysPage {
            fn invoke(
                &self,
                _op: AddonOp,
                _ctx: &InvocationCtx,
            ) -> Result<AddonResponse, AddonError> {
                Ok(AddonResponse::Page(Page::default()))
            }
        }
        let gated = CapabilityGated::new(AlwaysPage, vec![Capability::Catalog]);
        let ctx = ctx(1000);
        assert!(gated
            .invoke(
                AddonOp::List {
                    path: None,
                    cursor: None
                },
                &ctx
            )
            .is_ok());
        let err = gated
            .invoke(
                AddonOp::Resolve {
                    item_id: "x".into(),
                    source_id: None,
                },
                &ctx,
            )
            .unwrap_err();
        assert_eq!(err.code, AddonErrorCode::PermissionDenied);
        assert!(err.message.contains("Resolve"));
    }
}
