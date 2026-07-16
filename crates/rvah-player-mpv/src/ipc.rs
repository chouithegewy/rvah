use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixStream;
use std::path::Path;
use std::sync::mpsc::{channel, Receiver, TryRecvError};
use std::sync::Mutex;

use serde_json::{json, Value};

/// Minimal mpv JSON IPC client: fire-and-forget commands on the write half,
/// a reader thread that queues every incoming JSON line (events and command
/// replies alike) for the owner to drain.
pub struct MpvIpc {
    writer: Mutex<UnixStream>,
    incoming: Receiver<Value>,
    next_request_id: Mutex<u64>,
}

impl MpvIpc {
    pub fn connect(socket_path: &Path) -> std::io::Result<Self> {
        let stream = UnixStream::connect(socket_path)?;
        let reader_half = stream.try_clone()?;
        let (tx, rx) = channel();
        std::thread::spawn(move || {
            let reader = BufReader::new(reader_half);
            for line in reader.lines() {
                let Ok(line) = line else { break };
                if let Ok(value) = serde_json::from_str::<Value>(&line) {
                    if tx.send(value).is_err() {
                        break;
                    }
                }
            }
        });
        Ok(Self {
            writer: Mutex::new(stream),
            incoming: rx,
            next_request_id: Mutex::new(1),
        })
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
        loop {
            match self.incoming.try_recv() {
                Ok(v) => out.push(v),
                Err(TryRecvError::Empty) | Err(TryRecvError::Disconnected) => break,
            }
        }
        out
    }
}
