use crate::audio::passthrough::Passthrough;
use std::sync::Mutex;

/// App-wide state holding the active passthrough (if running).
#[derive(Default)]
pub struct AppState {
    pub passthrough: Mutex<Option<Passthrough>>,
}
