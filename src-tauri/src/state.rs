use crate::audio::passthrough::{self, Passthrough};
use std::path::PathBuf;
use std::process::Child;
use std::sync::atomic::AtomicBool;
use std::sync::mpsc::{channel, Sender};
use std::sync::{Arc, Mutex};

/// Commands sent to the dedicated audio thread.
pub enum AudioCommand {
    Start {
        output_name: String,
        respond: Sender<Result<(), String>>,
    },
    /// Stop carries a response channel so callers can wait until the streams
    /// are actually dropped (avoids a start-after-stop device race on Windows).
    Stop { respond: Sender<()> },
}

/// App state: a handle to the dedicated audio thread.
///
/// cpal streams are `!Send` on Windows (WASAPI), so the streams must be created,
/// owned, and dropped on a single thread. We never store the Stream in shared
/// state; instead AppState holds a channel Sender (which is Send + Sync).
pub struct AppState {
    sender: Mutex<Sender<AudioCommand>>,
    pub server: Mutex<Option<Child>>,
    pub live: Mutex<Option<crate::translation::live::LiveSession>>,
    /// Temp dir created by the last offline translate_file call. Cleaned up before the next one.
    pub last_translation_out: Mutex<Option<PathBuf>>,
    /// Shared flag toggled by the global PTT shortcut; the PTT producer reads this.
    pub ptt_recording: Arc<AtomicBool>,
    /// The shortcut string currently registered for PTT (e.g. "ctrl+shift+space").
    pub ptt_shortcut: Mutex<Option<String>>,
}

impl Default for AppState {
    fn default() -> Self {
        Self::new()
    }
}

impl AppState {
    pub fn new() -> Self {
        let (tx, rx) = channel::<AudioCommand>();
        std::thread::spawn(move || {
            // The active passthrough lives ONLY on this thread.
            let mut current: Option<Passthrough> = None;
            while let Ok(cmd) = rx.recv() {
                match cmd {
                    AudioCommand::Start {
                        output_name,
                        respond,
                    } => match passthrough::start(&output_name) {
                        Ok(pt) => {
                            current = Some(pt);
                            let _ = respond.send(Ok(()));
                        }
                        Err(e) => {
                            let _ = respond.send(Err(e));
                        }
                    },
                    AudioCommand::Stop { respond } => {
                        // Dropping the Passthrough stops the streams, on this thread.
                        current = None;
                        let _ = respond.send(());
                    }
                }
            }
        });
        AppState {
            sender: Mutex::new(tx),
            server: Mutex::new(None),
            live: Mutex::new(None),
            last_translation_out: Mutex::new(None),
            ptt_recording: Arc::new(AtomicBool::new(false)),
            ptt_shortcut: Mutex::new(None),
        }
    }

    /// Sends a command to the audio thread. Returns Err if the channel is closed.
    pub fn send_command(&self, cmd: AudioCommand) -> Result<(), String> {
        self.sender
            .lock()
            .map_err(|e| e.to_string())?
            .send(cmd)
            .map_err(|e| e.to_string())
    }
}

impl Drop for AppState {
    fn drop(&mut self) {
        if let Ok(mut g) = self.live.lock() {
            if let Some(sess) = g.take() {
                sess.stop.store(true, std::sync::atomic::Ordering::Relaxed);
            }
        }
        if let Ok(mut g) = self.server.lock() {
            if let Some(mut child) = g.take() {
                let _ = child.kill();
            }
        }
    }
}
