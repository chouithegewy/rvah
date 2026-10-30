use std::collections::VecDeque;
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStderr, Command, Stdio};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use rvah_addon_sdk::PlayableSource;
use rvah_auth::Redactor;
use rvah_player::{PlayerBackend, PlayerError, PlayerErrorCode, PlayerEvent};
use serde_json::{json, Value};

use crate::ipc::MpvIpc;

const OBS_PLAYBACK_TIME: u64 = 1;
const OBS_DURATION: u64 = 2;
const MAX_LOG_LINES: usize = 200;
type LogBuffer = Arc<Mutex<VecDeque<String>>>;

fn launch_args(socket_path: &Path, window_id: Option<u64>) -> Vec<String> {
    let mut args = vec![
        "--idle=yes".into(),
        "--force-window=yes".into(),
        "--no-terminal".into(),
        "--msg-level=all=warn".into(),
        "--input-default-bindings=no".into(),
        "--input-cursor=no".into(),
        format!("--input-ipc-server={}", socket_path.display()),
    ];
    if let Some(window_id) = window_id {
        args.push(format!("--wid={window_id}"));
    }
    args
}

/// [`PlayerBackend`] driving an external mpv process over JSON IPC.
pub struct MpvBackend {
    ipc: MpvIpc,
    child: Option<Child>,
    /// `Loaded` is emitted once per `load`, on the first duration report.
    loaded_emitted: bool,
    logs: LogBuffer,
}

fn capture_stderr(stderr: ChildStderr, logs: LogBuffer) {
    std::thread::spawn(move || {
        let redactor = Redactor::new();
        for line in BufReader::new(stderr).lines().map_while(Result::ok) {
            let mut logs = logs.lock().unwrap();
            if logs.len() == MAX_LOG_LINES {
                logs.pop_front();
            }
            logs.push_back(redactor.redact(&line));
        }
    });
}

fn actionable_mpv_error(file_error: Option<&str>, last_log: Option<&str>) -> String {
    let raw = file_error.unwrap_or("mpv failed to play the source");
    let lower = format!("{raw} {}", last_log.unwrap_or_default()).to_ascii_lowercase();
    if lower.contains("http error 401") || lower.contains("http error 403") {
        "video server rejected the request (HTTP authorization failed)".into()
    } else if lower.contains("http error 404") {
        "video source is unavailable or expired (HTTP 404)".into()
    } else if lower.contains("protocol not found") || lower.contains("unsupported protocol") {
        "unsupported video protocol".into()
    } else if lower.contains("decoder") || lower.contains("codec") {
        "video decoder failed or the codec is unsupported".into()
    } else {
        Redactor::new().redact(raw)
    }
}

fn io_error(e: impl std::fmt::Display, fatal: bool) -> PlayerError {
    PlayerError {
        code: PlayerErrorCode::Io,
        message: e.to_string(),
        fatal,
    }
}

impl MpvBackend {
    /// Launch the system `mpv` binary and connect to its IPC socket.
    pub fn spawn(socket_path: &Path) -> Result<Self, PlayerError> {
        Self::spawn_binary_with_window("mpv", socket_path, None)
    }

    /// Launch mpv with its video output attached to an application-owned
    /// native child window instead of creating a separate top-level window.
    pub fn spawn_embedded(socket_path: &Path, window_id: u64) -> Result<Self, PlayerError> {
        Self::spawn_binary_with_window("mpv", socket_path, Some(window_id))
    }

    pub fn spawn_binary(mpv: &str, socket_path: &Path) -> Result<Self, PlayerError> {
        Self::spawn_binary_with_window(mpv, socket_path, None)
    }

    fn spawn_binary_with_window(
        mpv: &str,
        socket_path: &Path,
        window_id: Option<u64>,
    ) -> Result<Self, PlayerError> {
        let mut command = Command::new(mpv);
        command.args(launch_args(socket_path, window_id));
        let mut child = command
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|e| PlayerError {
                code: PlayerErrorCode::BackendMissing,
                message: format!("failed to launch mpv: {e}"),
                fatal: true,
            })?;
        let logs = Arc::new(Mutex::new(VecDeque::new()));
        if let Some(stderr) = child.stderr.take() {
            capture_stderr(stderr, Arc::clone(&logs));
        }
        // The socket appears once mpv is up; poll briefly.
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            match MpvIpc::connect(socket_path) {
                Ok(ipc) => return Self::from_parts_with_logs(ipc, Some(child), logs),
                Err(_) if Instant::now() < deadline => {
                    std::thread::sleep(Duration::from_millis(100))
                }
                Err(e) => {
                    let detail = logs.lock().unwrap().back().cloned();
                    let _ = child.kill();
                    let _ = child.wait();
                    return Err(io_error(
                        format!(
                            "mpv IPC socket: {e}{}",
                            detail.map(|line| format!("; {line}")).unwrap_or_default()
                        ),
                        true,
                    ));
                }
            }
        }
    }

    /// Attach to an already-running IPC endpoint.
    pub fn connect(socket_path: &Path) -> Result<Self, PlayerError> {
        let ipc = MpvIpc::connect(socket_path).map_err(|e| io_error(e, true))?;
        Self::from_parts(ipc, None)
    }

    /// Wrap an already-connected IPC session (tests, custom transports).
    pub fn from_ipc(ipc: MpvIpc) -> Result<Self, PlayerError> {
        Self::from_parts(ipc, None)
    }

    fn from_parts(ipc: MpvIpc, child: Option<Child>) -> Result<Self, PlayerError> {
        Self::from_parts_with_logs(ipc, child, Arc::new(Mutex::new(VecDeque::new())))
    }

    fn from_parts_with_logs(
        ipc: MpvIpc,
        child: Option<Child>,
        logs: LogBuffer,
    ) -> Result<Self, PlayerError> {
        ipc.observe_property(OBS_PLAYBACK_TIME, "playback-time")
            .and_then(|()| ipc.observe_property(OBS_DURATION, "duration"))
            .map_err(|e| io_error(e, true))?;
        Ok(Self {
            ipc,
            child,
            loaded_emitted: false,
            logs,
        })
    }

    /// Redacted, bounded stderr tail for diagnostics.
    pub fn recent_logs(&self) -> Vec<String> {
        self.logs.lock().unwrap().iter().cloned().collect()
    }

    /// Locate mpv on PATH, if present.
    pub fn find_mpv() -> Option<PathBuf> {
        let binary = if cfg!(windows) { "mpv.exe" } else { "mpv" };
        let path = std::env::var_os("PATH")?;
        std::env::split_paths(&path)
            .map(|dir| dir.join(binary))
            .find(|candidate| candidate.is_file())
    }

    fn translate(&mut self, value: &Value) -> Option<PlayerEvent> {
        match value["event"].as_str()? {
            "property-change" => match value["name"].as_str()? {
                "playback-time" => value["data"]
                    .as_f64()
                    .map(|position_secs| PlayerEvent::Progress { position_secs }),
                "duration" => {
                    let duration_secs = value["data"].as_f64().filter(|d| *d > 0.0)?;
                    if self.loaded_emitted {
                        None
                    } else {
                        self.loaded_emitted = true;
                        Some(PlayerEvent::Loaded { duration_secs })
                    }
                }
                _ => None,
            },
            "end-file" => match value["reason"].as_str() {
                Some("eof") => Some(PlayerEvent::Ended),
                Some("error") => Some(PlayerEvent::Error(PlayerError {
                    code: PlayerErrorCode::UnsupportedMedia,
                    message: actionable_mpv_error(
                        value["file_error"].as_str(),
                        self.logs.lock().unwrap().back().map(String::as_str),
                    ),
                    fatal: true,
                })),
                // stop/quit/redirect: the controller initiated it or mpv moved on.
                _ => None,
            },
            "seeking" | "playback-restart" => Some(PlayerEvent::Buffering),
            _ => None,
        }
    }
}

impl PlayerBackend for MpvBackend {
    fn load(&mut self, source: &PlayableSource, start_at: f64) -> Result<(), PlayerError> {
        self.loaded_emitted = false;
        self.logs.lock().unwrap().clear();
        // Header policy: only user-agent/referer ever reach mpv (FR-063);
        // the source was already validated upstream.
        let mut header_fields = Vec::new();
        for (k, v) in &source.headers {
            match k.to_ascii_lowercase().as_str() {
                "user-agent" => {
                    self.ipc
                        .set_property("user-agent", json!(v))
                        .map_err(|e| io_error(e, false))?;
                }
                "referer" => header_fields.push(format!("Referer: {v}")),
                _ => {}
            }
        }
        self.ipc
            .set_property("http-header-fields", json!(header_fields))
            .map_err(|e| io_error(e, false))?;
        // Start paused at the resume position; the controller decides when to play.
        self.ipc
            .set_property("pause", json!(true))
            .map_err(|e| io_error(e, false))?;
        self.ipc
            .command(&[
                "loadfile".into(),
                json!(source.url),
                "replace".into(),
                json!(format!("start={start_at}")),
            ])
            .map_err(|e| io_error(e, true))?;
        // Attach resolver-provided subtitle tracks (FR-064 direction); the
        // UI selects among them via `select_subtitle`.
        for track in &source.subtitles {
            let label = track.label.clone().unwrap_or_else(|| track.lang.clone());
            self.ipc
                .command(&[
                    "sub-add".into(),
                    json!(track.url),
                    "auto".into(),
                    json!(label),
                    json!(track.lang),
                ])
                .map_err(|e| io_error(e, false))?;
        }
        Ok(())
    }

    fn play(&mut self) -> Result<(), PlayerError> {
        self.ipc
            .set_property("pause", json!(false))
            .map_err(|e| io_error(e, false))
    }

    fn pause(&mut self) -> Result<(), PlayerError> {
        self.ipc
            .set_property("pause", json!(true))
            .map_err(|e| io_error(e, false))
    }

    fn stop(&mut self) -> Result<(), PlayerError> {
        self.ipc
            .command(&["stop".into()])
            .map_err(|e| io_error(e, false))
    }

    fn seek(&mut self, secs: f64) -> Result<(), PlayerError> {
        self.ipc
            .command(&["seek".into(), json!(secs), "absolute".into()])
            .map_err(|e| io_error(e, false))
    }

    fn set_volume(&mut self, volume: f32) -> Result<(), PlayerError> {
        self.ipc
            .set_property("volume", json!((volume.clamp(0.0, 1.0) * 100.0) as f64))
            .map_err(|e| io_error(e, false))
    }

    fn poll_events(&mut self) -> Vec<PlayerEvent> {
        self.ipc
            .drain()
            .iter()
            .filter_map(|v| self.translate(v))
            .collect()
    }

    /// mpv demuxes DASH natively.
    fn supports_dash(&self) -> bool {
        true
    }

    fn select_subtitle(&mut self, index: Option<usize>) -> Result<(), PlayerError> {
        // mpv sub ids are 1-based; "no" disables subtitles.
        let sid = match index {
            Some(i) => json!(i as u64 + 1),
            None => json!("no"),
        };
        self.ipc
            .set_property("sid", sid)
            .map_err(|e| io_error(e, false))
    }
}

impl Drop for MpvBackend {
    fn drop(&mut self) {
        let _ = self.ipc.command(&["quit".into()]);
        if let Some(child) = &mut self.child {
            std::thread::sleep(Duration::from_millis(50));
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;
    use std::io::{BufRead, BufReader, Write};
    use std::net::{TcpListener, TcpStream};
    use std::sync::mpsc::{channel, Receiver};

    use rvah_addon_sdk::Protocol;

    /// Fake mpv: accepts one IPC connection, records every command line, and
    /// lets the test inject event lines toward the client. Runs over TCP
    /// loopback so the suite exercises every platform, including the CI
    /// windows job (the real transport differs only in `MpvIpc::connect`).
    struct FakeMpv {
        received: Receiver<Value>,
        event_tx: std::sync::mpsc::Sender<String>,
    }

    fn fake_mpv(listener: TcpListener) -> FakeMpv {
        let (cmd_tx, cmd_rx) = channel();
        let (event_tx, event_rx) = channel::<String>();
        std::thread::spawn(move || {
            let (stream, _) = listener.accept().unwrap();
            let mut write_half = stream.try_clone().unwrap();
            std::thread::spawn(move || {
                for line in event_rx {
                    write_half.write_all(line.as_bytes()).unwrap();
                    write_half.write_all(b"\n").unwrap();
                }
            });
            let reader = BufReader::new(stream);
            for line in reader.lines() {
                let Ok(line) = line else { break };
                let v: Value = serde_json::from_str(&line).unwrap();
                if cmd_tx.send(v).is_err() {
                    break;
                }
            }
        });
        FakeMpv {
            received: cmd_rx,
            event_tx,
        }
    }

    impl FakeMpv {
        fn commands(&self) -> Vec<Vec<Value>> {
            std::thread::sleep(Duration::from_millis(100));
            self.received
                .try_iter()
                .map(|v| v["command"].as_array().unwrap().clone())
                .collect()
        }

        fn emit(&self, line: &str) {
            self.event_tx.send(line.to_string()).unwrap();
        }
    }

    fn source_with_headers() -> PlayableSource {
        let mut headers = BTreeMap::new();
        headers.insert("User-Agent".into(), "rvah/0.1".into());
        headers.insert("Referer".into(), "https://ex.com".into());
        PlayableSource {
            url: "https://cdn.example.com/movie.mp4".into(),
            protocol: Protocol::Https,
            headers,
            subtitles: vec![],
            quality: None,
            audio_lang: None,
            expires_at: None,
            drm: None,
            never_block: Vec::new(),
        }
    }

    fn setup() -> (FakeMpv, MpvBackend) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let fake = fake_mpv(listener);
        let stream = TcpStream::connect(addr).unwrap();
        let reader = stream.try_clone().unwrap();
        let backend = MpvBackend::from_ipc(MpvIpc::from_stream(stream, reader)).unwrap();
        (fake, backend)
    }

    #[test]
    fn embedded_launch_targets_app_window_without_own_input_bindings() {
        let args = launch_args(Path::new("/tmp/rvah-mpv.sock"), Some(4242));
        assert!(args.iter().any(|arg| arg == "--wid=4242"));
        assert!(args.iter().any(|arg| arg == "--input-default-bindings=no"));
        assert!(args.iter().any(|arg| arg == "--input-cursor=no"));
    }

    #[test]
    fn player_errors_are_actionable_and_redacted() {
        assert!(actionable_mpv_error(Some("HTTP error 403"), None).contains("rejected"));
        assert!(actionable_mpv_error(Some("decoder failed"), None).contains("decoder"));
        let message =
            actionable_mpv_error(Some("GET /video?access_token=secret-token failed"), None);
        assert!(!message.contains("secret-token"));
    }

    /// The real transport still works end-to-end on unix: same suite entry
    /// point, actual `MpvIpc::connect` over a Unix socket.
    #[cfg(unix)]
    #[test]
    fn unix_socket_transport_connects_and_commands() {
        let tmp = tempfile::tempdir().unwrap();
        let socket = tmp.path().join("mpv.sock");
        let listener = std::os::unix::net::UnixListener::bind(&socket).unwrap();
        let (cmd_tx, cmd_rx) = channel();
        std::thread::spawn(move || {
            let (stream, _) = listener.accept().unwrap();
            let reader = BufReader::new(stream);
            for line in reader.lines() {
                let Ok(line) = line else { break };
                let v: Value = serde_json::from_str(&line).unwrap();
                if cmd_tx.send(v).is_err() {
                    break;
                }
            }
        });
        let mut backend = MpvBackend::connect(&socket).unwrap();
        backend.play().unwrap();
        std::thread::sleep(Duration::from_millis(100));
        let cmds: Vec<Value> = cmd_rx.try_iter().collect();
        assert!(cmds
            .iter()
            .any(|v| v["command"][0] == "set_property" && v["command"][1] == "pause"));
    }

    #[test]
    fn load_sends_headers_pause_and_loadfile_with_resume() {
        let (fake, mut backend) = setup();
        backend.load(&source_with_headers(), 42.5).unwrap();

        let cmds = fake.commands();
        // observe_property x2 first, then user-agent, header-fields, pause, loadfile.
        assert_eq!(cmds[0][0], "observe_property");
        assert_eq!(cmds[1][0], "observe_property");
        assert!(cmds
            .iter()
            .any(|c| c[0] == "set_property" && c[1] == "user-agent" && c[2] == "rvah/0.1"));
        assert!(cmds.iter().any(|c| c[0] == "set_property"
            && c[1] == "http-header-fields"
            && c[2] == json!(["Referer: https://ex.com"])));
        assert!(cmds
            .iter()
            .any(|c| c[0] == "set_property" && c[1] == "pause" && c[2] == json!(true)));
        let load = cmds.iter().find(|c| c[0] == "loadfile").unwrap();
        assert_eq!(load[1], "https://cdn.example.com/movie.mp4");
        assert_eq!(load[3], "start=42.5");
    }

    #[test]
    fn transport_commands_map_to_ipc() {
        let (fake, mut backend) = setup();
        backend.play().unwrap();
        backend.pause().unwrap();
        backend.seek(300.0).unwrap();
        backend.set_volume(0.5).unwrap();
        backend.stop().unwrap();

        let cmds = fake.commands();
        assert!(cmds
            .iter()
            .any(|c| c[0] == "set_property" && c[1] == "pause" && c[2] == json!(false)));
        assert!(cmds
            .iter()
            .any(|c| c[0] == "seek" && c[1] == json!(300.0) && c[2] == "absolute"));
        assert!(cmds
            .iter()
            .any(|c| c[0] == "set_property" && c[1] == "volume" && c[2] == json!(50.0)));
        assert!(cmds.iter().any(|c| c[0] == "stop"));
    }

    #[test]
    fn subtitles_added_and_selected() {
        let (fake, mut backend) = setup();
        let mut src = source_with_headers();
        src.subtitles.push(rvah_addon_sdk::SubtitleTrack {
            url: "https://cdn.example.com/en.srt".into(),
            lang: "en".into(),
            label: Some("English".into()),
            format: None,
        });
        backend.load(&src, 0.0).unwrap();
        backend.select_subtitle(Some(0)).unwrap();
        backend.select_subtitle(None).unwrap();

        let cmds = fake.commands();
        let sub_add = cmds
            .iter()
            .find(|c| c[0] == "sub-add")
            .expect("sub-add sent");
        assert_eq!(sub_add[1], "https://cdn.example.com/en.srt");
        assert_eq!(sub_add[3], "English");
        assert_eq!(sub_add[4], "en");
        assert!(cmds
            .iter()
            .any(|c| c[0] == "set_property" && c[1] == "sid" && c[2] == json!(1)));
        assert!(cmds
            .iter()
            .any(|c| c[0] == "set_property" && c[1] == "sid" && c[2] == json!("no")));
    }

    #[test]
    fn events_translate_to_player_events() {
        let (fake, mut backend) = setup();
        backend.load(&source_with_headers(), 0.0).unwrap();

        fake.emit(r#"{"event":"property-change","id":2,"name":"duration","data":100.5}"#);
        fake.emit(r#"{"event":"property-change","id":1,"name":"playback-time","data":7.25}"#);
        fake.emit(r#"{"event":"property-change","id":2,"name":"duration","data":100.5}"#);
        fake.emit(r#"{"event":"end-file","reason":"eof"}"#);
        std::thread::sleep(Duration::from_millis(100));

        let events = backend.poll_events();
        assert_eq!(
            events,
            vec![
                PlayerEvent::Loaded {
                    duration_secs: 100.5
                },
                PlayerEvent::Progress {
                    position_secs: 7.25
                },
                // duplicate duration suppressed
                PlayerEvent::Ended,
            ]
        );
    }

    #[test]
    fn end_file_error_is_fatal() {
        let (fake, mut backend) = setup();
        fake.emit(r#"{"event":"end-file","reason":"error","file_error":"unsupported codec"}"#);
        std::thread::sleep(Duration::from_millis(100));
        let events = backend.poll_events();
        assert_eq!(events.len(), 1);
        let PlayerEvent::Error(err) = &events[0] else {
            panic!("expected error event");
        };
        assert!(err.fatal);
        assert_eq!(err.code, PlayerErrorCode::UnsupportedMedia);
        assert!(err.message.contains("decoder") && err.message.contains("unsupported"));
    }
}
