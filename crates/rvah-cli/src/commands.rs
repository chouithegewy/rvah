use std::path::{Path, PathBuf};
use std::sync::Arc;

use ed25519_dalek::{SigningKey, VerifyingKey};
use rvah_addon_host::{
    keygen, package_dir, read_package_file, sign_package, AddonManager, Capability,
    InProcessRuntime, InstallDecision, Manifest,
};
use rvah_addon_sdk::{AddonProvider, PageItem, ResolveResult};
use rvah_auth::{MemoryVault, Redactor};
use rvah_core::AppCore;
use rvah_data::Database;
use rvah_player::{validate_source, PlayerEvent};
use rvah_testkit::{CatalogEchoProvider, MockPlayer, NoopHost};

const USAGE: &str = "\
rvah — RVAH developer CLI and simulator (mock UI)

USAGE:
  rvah validate <manifest.toml>
  rvah keygen -o <keydir>
  rvah package <addon-dir> -o <pkg.rvah> --key <signing.key>
  rvah install <pkg.rvah> --pubkey <verify.pub> --data-dir <DIR> [--approve]
  rvah addons --data-dir <DIR>
  rvah enable|disable <id> --data-dir <DIR>
  rvah uninstall <id> [--purge] --data-dir <DIR>
  rvah browse <id> [path] --data-dir <DIR>
  rvah resolve <id> <item-id> --data-dir <DIR>
  rvah play <id> <item-id> --data-dir <DIR>
  rvah diagnostics --data-dir <DIR>
";

pub fn run(args: &[String]) -> i32 {
    match dispatch(args) {
        Ok(()) => 0,
        Err(CliError::Usage(msg)) => {
            eprintln!("{msg}\n\n{USAGE}");
            2
        }
        Err(CliError::Failed(msg)) => {
            eprintln!("error: {msg}");
            1
        }
    }
}

enum CliError {
    Usage(String),
    Failed(String),
}

fn fail(msg: impl std::fmt::Display) -> CliError {
    CliError::Failed(msg.to_string())
}

fn dispatch(args: &[String]) -> Result<(), CliError> {
    let cmd = args
        .first()
        .ok_or_else(|| CliError::Usage("missing command".into()))?;
    let rest = &args[1..];
    match cmd.as_str() {
        "validate" => cmd_validate(rest),
        "keygen" => cmd_keygen(rest),
        "package" => cmd_package(rest),
        "install" => cmd_install(rest),
        "addons" => cmd_addons(rest),
        "enable" | "disable" => cmd_set_enabled(cmd == "enable", rest),
        "uninstall" => cmd_uninstall(rest),
        "browse" => cmd_browse(rest),
        "resolve" => cmd_resolve(rest),
        "play" => cmd_play(rest),
        "diagnostics" => cmd_diagnostics(rest),
        other => Err(CliError::Usage(format!("unknown command: {other}"))),
    }
}

// ---- argument helpers ----

fn flag_value(args: &[String], name: &str) -> Option<String> {
    args.iter()
        .position(|a| a == name)
        .and_then(|i| args.get(i + 1))
        .cloned()
}

fn has_flag(args: &[String], name: &str) -> bool {
    args.iter().any(|a| a == name)
}

fn positional(args: &[String]) -> Vec<&String> {
    let mut out = Vec::new();
    let mut skip = false;
    for a in args {
        if skip {
            skip = false;
            continue;
        }
        if a.starts_with("--") || a == "-o" {
            skip = a != "--approve" && a != "--purge";
            continue;
        }
        out.push(a);
    }
    out
}

fn require_data_dir(args: &[String]) -> Result<PathBuf, CliError> {
    flag_value(args, "--data-dir")
        .map(PathBuf::from)
        .ok_or_else(|| CliError::Usage("missing --data-dir".into()))
}

// ---- state ----

fn open_db(data_dir: &Path) -> Result<Arc<Database>, CliError> {
    std::fs::create_dir_all(data_dir).map_err(fail)?;
    Ok(Arc::new(
        Database::open(&data_dir.join("rvah.sqlite")).map_err(fail)?,
    ))
}

fn open_manager(data_dir: &Path) -> Result<(Arc<Database>, AddonManager), CliError> {
    let db = open_db(data_dir)?;
    let manager = AddonManager::new(Arc::clone(&db), data_dir.join("addons"));
    Ok((db, manager))
}

fn capability_from_str(s: &str) -> Option<Capability> {
    match s {
        "catalog" => Some(Capability::Catalog),
        "search" => Some(Capability::Search),
        "resolve" => Some(Capability::Resolve),
        "auth" => Some(Capability::Auth),
        "account_status" | "accountstatus" => Some(Capability::AccountStatus),
        _ => None,
    }
}

/// The simulator (FR-027): every installed add-on is driven by the in-process
/// echo fixture provider, gated by the capabilities it declared at install.
fn open_core(data_dir: &Path) -> Result<AppCore<MockPlayer>, CliError> {
    let (db, manager) = open_manager(data_dir)?;
    let mut core = AppCore::new(manager, db, Arc::new(MemoryVault::new()), MockPlayer::new());
    for rec in core.db().list_addons().map_err(fail)? {
        let caps = rec
            .capabilities
            .iter()
            .filter_map(|c| capability_from_str(c))
            .collect();
        let provider: Arc<dyn AddonProvider + Send + Sync> = Arc::new(CatalogEchoProvider);
        core.register_runtime(
            rec.id,
            Box::new(InProcessRuntime::new(provider, caps)),
            Arc::new(NoopHost::default()),
        );
    }
    Ok(core)
}

// ---- key/signature file formats (hex) ----

fn read_signing_key(path: &Path) -> Result<SigningKey, CliError> {
    let bytes = read_hex_file(path, 32)?;
    Ok(SigningKey::from_bytes(&bytes.try_into().unwrap()))
}

fn read_verifying_key(path: &Path) -> Result<VerifyingKey, CliError> {
    let bytes = read_hex_file(path, 32)?;
    VerifyingKey::from_bytes(&bytes.try_into().unwrap())
        .map_err(|e| fail(format!("invalid public key: {e}")))
}

fn read_signature(path: &Path) -> Result<[u8; 64], CliError> {
    let bytes = read_hex_file(path, 64)?;
    Ok(bytes.try_into().unwrap())
}

fn read_hex_file(path: &Path, expect_len: usize) -> Result<Vec<u8>, CliError> {
    let text = std::fs::read_to_string(path)
        .map_err(|e| fail(format!("read {}: {e}", path.display())))?;
    let bytes = hex::decode(text.trim()).map_err(|e| fail(format!("{}: {e}", path.display())))?;
    if bytes.len() != expect_len {
        return Err(fail(format!(
            "{}: expected {expect_len} bytes, got {}",
            path.display(),
            bytes.len()
        )));
    }
    Ok(bytes)
}

// ---- commands ----

fn cmd_validate(args: &[String]) -> Result<(), CliError> {
    let pos = positional(args);
    let [path] = pos.as_slice() else {
        return Err(CliError::Usage("validate takes one manifest path".into()));
    };
    let text = std::fs::read_to_string(path).map_err(fail)?;
    let manifest = Manifest::parse(&text).map_err(|e| fail(format!("invalid manifest: {e}")))?;
    if !manifest.is_sdk_compatible() {
        return Err(fail(format!(
            "manifest sdk range {} does not include host SDK {}",
            manifest.addon.sdk,
            rvah_addon_host::HOST_SDK_VERSION
        )));
    }
    println!("OK: {} v{}", manifest.addon.id, manifest.addon.version);
    Ok(())
}

fn cmd_keygen(args: &[String]) -> Result<(), CliError> {
    let out = flag_value(args, "-o").ok_or_else(|| CliError::Usage("keygen needs -o".into()))?;
    let dir = PathBuf::from(out);
    std::fs::create_dir_all(&dir).map_err(fail)?;
    let (signing, verifying) = keygen();
    std::fs::write(dir.join("signing.key"), hex::encode(signing.to_bytes())).map_err(fail)?;
    std::fs::write(dir.join("verify.pub"), hex::encode(verifying.to_bytes())).map_err(fail)?;
    println!("wrote {}/signing.key and verify.pub", dir.display());
    Ok(())
}

fn cmd_package(args: &[String]) -> Result<(), CliError> {
    let pos = positional(args);
    let [addon_dir] = pos.as_slice() else {
        return Err(CliError::Usage("package takes one add-on directory".into()));
    };
    let out = flag_value(args, "-o").ok_or_else(|| CliError::Usage("package needs -o".into()))?;
    let key_path =
        flag_value(args, "--key").ok_or_else(|| CliError::Usage("package needs --key".into()))?;
    let key = read_signing_key(Path::new(&key_path))?;
    let out = PathBuf::from(out);
    let hash = package_dir(Path::new(addon_dir), &out).map_err(fail)?;
    let sig = sign_package(&out, &key).map_err(fail)?;
    let sig_path = format!("{}.sig", out.display());
    std::fs::write(&sig_path, hex::encode(sig)).map_err(fail)?;
    println!("packaged {} (sha256 {})", out.display(), hex::encode(hash));
    println!("signature {sig_path}");
    Ok(())
}

fn cmd_install(args: &[String]) -> Result<(), CliError> {
    let pos = positional(args);
    let [pkg] = pos.as_slice() else {
        return Err(CliError::Usage("install takes one package path".into()));
    };
    let pkg = PathBuf::from(pkg.as_str());
    let pubkey_path = flag_value(args, "--pubkey")
        .ok_or_else(|| CliError::Usage("install needs --pubkey".into()))?;
    let data_dir = require_data_dir(args)?;
    let pubkey = read_verifying_key(Path::new(&pubkey_path))?;
    let sig = read_signature(Path::new(&format!("{}.sig", pkg.display())))?;
    // Never parse an unverified package (TC-SEC-001).
    rvah_addon_host::verify_package(&pkg, &sig, &pubkey).map_err(fail)?;

    // Show the permission grant before deciding (FR-005).
    let manifest_bytes = read_package_file(&pkg, "manifest.toml").map_err(fail)?;
    let manifest = Manifest::parse(&String::from_utf8(manifest_bytes).map_err(fail)?)
        .map_err(|e| fail(format!("invalid manifest: {e}")))?;
    println!("{} v{} requests:", manifest.addon.id, manifest.addon.version);
    println!("  network: {:?}", manifest.permissions.network);
    println!("  storage: {} bytes", manifest.permissions.storage_bytes);
    println!("  secrets: {:?}", manifest.permissions.secrets);

    if !has_flag(args, "--approve") {
        return Err(fail("permissions not approved; re-run with --approve"));
    }
    let (_db, manager) = open_manager(&data_dir)?;
    let installed = manager
        .install(&pkg, &sig, &pubkey, InstallDecision::Approve)
        .map_err(fail)?;
    println!("installed {} v{}", installed.id, installed.version);
    Ok(())
}

fn cmd_addons(args: &[String]) -> Result<(), CliError> {
    let data_dir = require_data_dir(args)?;
    let db = open_db(&data_dir)?;
    let addons = db.list_addons().map_err(fail)?;
    if addons.is_empty() {
        println!("no add-ons installed");
    }
    for a in addons {
        println!("{}\t{}\t{:?}\t[{}]", a.id, a.version, a.status, a.capabilities.join(","));
    }
    Ok(())
}

fn cmd_set_enabled(enable: bool, args: &[String]) -> Result<(), CliError> {
    let pos = positional(args);
    let [id] = pos.as_slice() else {
        return Err(CliError::Usage("expected one add-on id".into()));
    };
    let data_dir = require_data_dir(args)?;
    let (_db, manager) = open_manager(&data_dir)?;
    if enable {
        manager.enable(id).map_err(fail)?;
    } else {
        manager.disable(id).map_err(fail)?;
    }
    println!("{} {}", if enable { "enabled" } else { "disabled" }, id);
    Ok(())
}

fn cmd_uninstall(args: &[String]) -> Result<(), CliError> {
    let pos = positional(args);
    let [id] = pos.as_slice() else {
        return Err(CliError::Usage("uninstall takes one add-on id".into()));
    };
    let data_dir = require_data_dir(args)?;
    let purge = has_flag(args, "--purge");
    let (_db, manager) = open_manager(&data_dir)?;
    manager.uninstall(id, purge).map_err(fail)?;
    println!("uninstalled {id}{}", if purge { " (data purged)" } else { "" });
    Ok(())
}

fn cmd_browse(args: &[String]) -> Result<(), CliError> {
    let pos = positional(args);
    let (id, path) = match pos.as_slice() {
        [id] => (id, None),
        [id, path] => (id, Some(path.to_string())),
        _ => return Err(CliError::Usage("browse takes an id and optional path".into())),
    };
    let data_dir = require_data_dir(args)?;
    let mut core = open_core(&data_dir)?;
    let page = core.browse(id, path, None).map_err(fail)?;
    for item in &page.items {
        match item {
            PageItem::Directory(d) => println!("dir\t{}\t{}", d.title, d.path),
            PageItem::Media(m) => println!("media\t{}\t{}", m.title, m.id),
        }
    }
    if let Some(cursor) = &page.cursor_next {
        println!("next-cursor\t{cursor}");
    }
    Ok(())
}

fn cmd_resolve(args: &[String]) -> Result<(), CliError> {
    let pos = positional(args);
    let [id, item_id] = pos.as_slice() else {
        return Err(CliError::Usage("resolve takes an id and item-id".into()));
    };
    let data_dir = require_data_dir(args)?;
    let core = open_core(&data_dir)?;
    // Use the runtime directly so all candidate sources can be shown.
    let manifest_caps = core
        .db()
        .get_addon(id)
        .map_err(fail)?
        .ok_or_else(|| fail(format!("add-on not installed: {id}")))?;
    let caps = manifest_caps
        .capabilities
        .iter()
        .filter_map(|c| capability_from_str(c))
        .collect();
    let runtime = InProcessRuntime::new(Arc::new(CatalogEchoProvider), caps);
    let ctx = rvah_addon_host::InvocationCtx {
        timeout: std::time::Duration::from_secs(2),
        host: Arc::new(NoopHost::default()),
    };
    use rvah_addon_host::{AddonOp, AddonResponse, AddonRuntime};
    let resp = runtime
        .invoke(
            AddonOp::Resolve { item_id: item_id.to_string(), source_id: None },
            &ctx,
        )
        .map_err(|e| fail(format!("resolve failed: {e}")))?;
    let AddonResponse::Resolve(result) = resp else {
        return Err(fail("provider returned unexpected response"));
    };
    match result {
        ResolveResult::Playables(sources) => {
            let now = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_secs())
                .unwrap_or(0);
            for s in sources {
                match validate_source(&s, now) {
                    Ok(()) => println!(
                        "source\t{}\t{:?}\t{}",
                        s.url,
                        s.protocol,
                        s.quality.as_deref().unwrap_or("-")
                    ),
                    Err(r) => println!("rejected\t{}\t{r}", s.url),
                }
            }
        }
        ResolveResult::AuthRequired { provider_hint } => {
            println!("auth-required\t{provider_hint}")
        }
        ResolveResult::Unavailable { reason } => println!("unavailable\t{reason}"),
    }
    Ok(())
}

fn cmd_play(args: &[String]) -> Result<(), CliError> {
    let pos = positional(args);
    let [id, item_id] = pos.as_slice() else {
        return Err(CliError::Usage("play takes an id and item-id".into()));
    };
    let data_dir = require_data_dir(args)?;
    let mut core = open_core(&data_dir)?;
    let session = core.resolve_and_play(id, item_id).map_err(fail)?;
    println!("playing\t{}", session.source.url);

    // Mock playback: script a deterministic session through the mock backend.
    core.player.backend_mut().script_events([
        PlayerEvent::Loaded { duration_secs: 100.0 },
    ]);
    core.tick_playback().map_err(fail)?;
    core.player.play().map_err(|e| fail(e.message))?;
    core.player.backend_mut().script_events([
        PlayerEvent::Progress { position_secs: 25.0 },
        PlayerEvent::Progress { position_secs: 50.0 },
        PlayerEvent::Progress { position_secs: 95.0 },
        PlayerEvent::Ended,
    ]);
    for event in core.tick_playback().map_err(fail)? {
        match event {
            PlayerEvent::Progress { position_secs } => println!("progress\t{position_secs}"),
            PlayerEvent::Ended => println!("ended"),
            other => println!("event\t{other:?}"),
        }
    }
    let ws = core
        .db()
        .get_watch_state(id, item_id)
        .map_err(fail)?
        .ok_or_else(|| fail("no watch state persisted"))?;
    println!(
        "watch-state\tposition={} watched={}",
        ws.position_secs, ws.watched
    );
    Ok(())
}

fn cmd_diagnostics(args: &[String]) -> Result<(), CliError> {
    let data_dir = require_data_dir(args)?;
    let db = open_db(&data_dir)?;
    // Defense in depth: events are redacted at write time by the host bridge,
    // and pattern-scrubbed again before display (FR-103, TC-SEC-003).
    let redactor = Redactor::new();
    for e in db.recent_events(50).map_err(fail)? {
        println!(
            "{}\t{}\t{}\t{}",
            e.level,
            e.addon_id.as_deref().unwrap_or("-"),
            e.code,
            redactor.redact(&e.message)
        );
    }
    Ok(())
}
