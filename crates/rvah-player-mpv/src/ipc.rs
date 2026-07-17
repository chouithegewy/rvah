use std::io::{BufRead, BufReader, Read, Write};
use std::path::{Path, PathBuf};
use std::sync::mpsc::{channel, Receiver};
use std::sync::Mutex;

use serde_json::{json, Value};

/// Minimal mpv JSON IPC client: fire-and-forget commands on the write half,
/// a reader thread that queues every incoming JSON line (events and command
/// replies alike) for the owner to drain.
pub struct MpvIpc {
    writer: Mutex<Box<dyn Write + Send>>,
    incoming: Receiver<Value>,
    next_request_id: Mutex<u64>,
}

impl MpvIpc {
    /// Connect to mpv's IPC endpoint: a Unix socket path on unix, a named
    /// pipe (`\\.\pipe\…`, openable like a file) on Windows.
    pub fn connect(endpoint: &Path) -> std::io::Result<Self> {
        #[cfg(unix)]
        {
            let stream = std::os::unix::net::UnixStream::connect(endpoint)?;
            let reader = stream.try_clone()?;
            Ok(Self::from_stream(stream, reader))
        }
        #[cfg(windows)]
        {
            let stream = std::fs::OpenOptions::new()
                .read(true)
                .write(true)
                .open(endpoint)?;
            let reader = stream.try_clone()?;
            Ok(Self::from_stream(stream, reader))
        }
        #[cfg(not(any(unix, windows)))]
        {
            let _ = endpoint;
            Err(std::io::Error::new(
                std::io::ErrorKind::Unsupported,
                "mpv IPC needs unix sockets or windows named pipes",
            ))
        }
    }

    /// Build over any duplex byte stream, as writer + its cloned read half
    /// (tests use a TCP loopback pair).
    pub fn from_stream(
        writer: impl Write + Send + 'static,
        reader: impl Read + Send + 'static,
    ) -> Self {
        let (tx, rx) = channel();
        std::thread::spawn(move || {
            let reader = BufReader::new(reader);
            for line in reader.lines() {
                let Ok(line) = line else { break };
                if let Ok(value) = serde_json::from_str::<Value>(&line) {
                    if tx.send(value).is_err() {
                        break;
                    }
                }
            }
        });
        Self {
            writer: Mutex::new(Box::new(writer)),
            incoming: rx,
            next_request_id: Mutex::new(1),
        }
    }

    /// The endpoint `MpvBackend::spawn` should serve on for this data dir:
    /// a socket file beside the data on unix, a pid-scoped named pipe on
    /// Windows (pipes live in the pipe namespace, not the filesystem).
    pub fn default_endpoint(data_dir: &Path) -> PathBuf {
        if cfg!(windows) {
            PathBuf::from(format!(r"\\.\pipe\rvah-mpv-{}", std::process::id()))
        } else {
            data_dir.join("mpv.sock")
        }
    }

    /// Send one command, e.g. `command(&["set_property", "pause", "yes"])`.
    /// Replies are not awaited; they surface via [`MpvIpc::drain`].
    pub fn command(&self, args: &[Value]) -> std::io::Result<()> {
        let id = {
            let mut guard = self.next_request_id.lock().unwrap();
            *guard += 1;
            *guard
        };
        let msg = json!({ "command": args, "request_id": id });
        let mut writer = self.writer.lock().unwrap();
        writer.write_all(msg.to_string().as_bytes())?;
        writer.write_all(b"\n")
    }

    pub fn set_property(&self, name: &str, value: Value) -> std::io::Result<()> {
        self.command(&["set_property".into(), name.into(), value])
    }

    pub fn observe_property(&self, id: u64, name: &str) -> std::io::Result<()> {
        self.command(&["observe_property".into(), id.into(), name.into()])
    }

    /// Non-blocking drain of everything mpv has sent since the last call.
    pub fn drain(&self) -> Vec<Value> {
        let mut out = Vec::new();
        while let Ok(v) = self.incoming.try_recv() {
            out.push(v);
        }
        out
    }
}
