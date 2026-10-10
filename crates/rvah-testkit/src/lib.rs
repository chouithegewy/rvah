//! Shared fixtures for RVAH tests and the developer simulator (STP §5):
//! deterministic providers, manifest/package builders, and re-exported mocks.

mod fixtures;

pub use fixtures::{
    fixture_manifest, mp4_source, write_fixture_package, write_fixture_package_with_permissions,
    write_fixture_package_with_secrets, write_named_fixture_package, BadPermissionsProvider,
    CatalogEchoProvider, DeviceAuthProvider, FailingProvider, LargeCatalogProvider, NoopHost,
    ResolverStaticProvider, SlowProvider, StaticDetailsProvider, StaticPageProvider,
    ECHO_FIXTURE_ID,
};
pub use rvah_player::MockPlayer;
