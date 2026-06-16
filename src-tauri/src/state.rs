use crate::audio::passthrough::{self, Passthrough};
use std::sync::mpsc::{channel, Sender};
use std::sync::Mutex;

/// Commands sent to the dedicated audio thread.
pub enum AudioCommand {
    Start {
        output_name: String,
        respond: Sender<Result<(), String>>,
    },
    Stop,
}

/// App state: a handle to the dedicated audio thread.
///
/// cpal streams are `!Send` on Windows (WASAPI), so the streams must be created,
/// owned, and dropped on a single thread. We never store the Stream in shared
/// state; instead AppState holds a channel Sender (which is Send + Sync).
pub struct AppState {
    pub sender: Mutex<Sender<AudioCommand>>,
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
                    AudioCommand::Stop => {
                        // Dropping the Passthrough stops the streams, on this thread.
                        current = None;
                    }
                }
            }
        });
        AppState {
            sender: Mutex::new(tx),
        }
    }
}
