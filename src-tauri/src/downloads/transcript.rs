use std::{
    collections::VecDeque,
    sync::{
        Mutex,
        atomic::{AtomicBool, Ordering},
    },
};

use serde::Serialize;
use ts_rs::TS;

use super::timestamp;

static VERBOSE: AtomicBool = AtomicBool::new(false);

pub(crate) fn set_verbose(enabled: bool) {
    VERBOSE.store(enabled, Ordering::Release);
}

static SESSIONS: Mutex<VecDeque<TerminalSession>> = Mutex::new(VecDeque::new());

#[derive(Clone, Serialize, TS)]
pub struct TerminalLine {
    pub time: String,
    pub text: String,
}

#[derive(Clone, Serialize, TS)]
pub struct TerminalSession {
    pub id: String,
    pub title: String,
    pub started: String,
    pub finished: Option<String>,
    #[ts(as = "Vec<TerminalLine>")]
    pub lines: VecDeque<TerminalLine>,
}

pub(super) struct Transcript(String);

impl Transcript {
    pub(super) fn start(title: String) -> Self {
        let id = uuid::Uuid::new_v4().to_string();
        if let Ok(mut sessions) = SESSIONS.lock() {
            if sessions.len() >= 12 {
                sessions.pop_front();
            }
            sessions.push_back(TerminalSession {
                id: id.clone(),
                title,
                started: timestamp(),
                finished: None,
                lines: VecDeque::new(),
            });
        }
        Self(id)
    }

    pub(super) fn detail(&self, message: &str) {
        if VERBOSE.load(Ordering::Acquire) {
            self.line(format!("[app] {message}"));
        }
    }

    pub(super) fn line(&self, text: String) {
        if let Ok(mut sessions) = SESSIONS.lock()
            && let Some(session) = sessions.iter_mut().find(|session| session.id == self.0)
        {
            if session.lines.len() >= 250 {
                session.lines.pop_front();
            }
            session.lines.push_back(TerminalLine {
                time: timestamp(),
                text,
            });
        }
    }
}

impl Drop for Transcript {
    fn drop(&mut self) {
        if let Ok(mut sessions) = SESSIONS.lock()
            && let Some(session) = sessions.iter_mut().find(|session| session.id == self.0)
        {
            session.finished = Some(timestamp());
        }
    }
}

pub(crate) fn sessions() -> Vec<TerminalSession> {
    SESSIONS
        .lock()
        .map(|sessions| sessions.iter().rev().cloned().collect())
        .unwrap_or_default()
}
