//! Background watchers: which call apps use the microphone, and what a call window shows while recording.

use crate::callnames::Reading;
use crate::calls::{self, CallApp};
use crate::native;
use futures::channel::mpsc::{UnboundedReceiver, unbounded};
use std::io::{BufRead, BufReader};
use std::process::{Child, Command, Stdio};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

const MIC_POLL: Duration = Duration::from_secs(3);
const WATCH_MS: &str = "400";

/// Call apps that are using the microphone, once per poll. The thread ends when the receiver is dropped.
pub fn mic_poll() -> UnboundedReceiver<Vec<CallApp>> {
    let (tx, rx) = unbounded();
    std::thread::spawn(move || {
        loop {
            let mut apps: Vec<CallApp> = Vec::new();
            if let Ok(list) = native::mic_apps() {
                for app in list.iter().filter_map(|m| calls::owner(&m.bundle_id)) {
                    if !apps.contains(&app) {
                        apps.push(app);
                    }
                }
            }
            // A browser only counts as a call when one of its tabs looks like a call.
            if apps.contains(&CallApp::Browser) {
                let ids = CallApp::Browser.watch_ids();
                let has_call_tab = native::call_titles(ids).map(|t| !t.is_empty()).unwrap_or(false);
                if !has_call_tab {
                    apps.retain(|a| *a != CallApp::Browser);
                }
            }
            if tx.unbounded_send(apps).is_err() {
                break;
            }
            std::thread::sleep(MIC_POLL);
        }
    });
    rx
}

/// A running call-watch helper that streams what the call window shows.
pub struct CallWatch {
    child: Arc<std::sync::Mutex<Option<Child>>>,
    stopped: Arc<AtomicBool>,
}

impl CallWatch {
    pub fn stop(&self) {
        self.stopped.store(true, Ordering::Relaxed);
        if let Some(mut child) = self.child.lock().ok().and_then(|mut c| c.take()) {
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}

impl Drop for CallWatch {
    fn drop(&mut self) {
        self.stop();
    }
}

/// Starts reading the windows of `app`. None when the app has nothing to read or the helper is missing.
pub fn call_watch(app: CallApp) -> Option<(CallWatch, UnboundedReceiver<Reading>)> {
    let ids = app.watch_ids();
    if ids.is_empty() {
        return None;
    }
    let helper = native::helper_path()?;
    let mut child = Command::new(helper)
        .arg("call-watch")
        .arg(WATCH_MS)
        .args(ids)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .ok()?;
    let stdout = child.stdout.take()?;
    // Keep stdin open: the helper quits when it closes.
    let stdin = child.stdin.take();
    let child = Arc::new(std::sync::Mutex::new(Some(child)));
    let stopped = Arc::new(AtomicBool::new(false));
    let (tx, rx) = unbounded();
    let flag = stopped.clone();
    std::thread::spawn(move || {
        let _keep_open = stdin;
        for line in BufReader::new(stdout).lines() {
            let Ok(line) = line else { break };
            if flag.load(Ordering::Relaxed) {
                break;
            }
            if let Ok(reading) = serde_json::from_str::<Reading>(&line) {
                if tx.unbounded_send(reading).is_err() {
                    break;
                }
            }
        }
    });
    Some((CallWatch { child, stopped }, rx))
}

/// Audio playback through the helper. Dropping it stops the sound.
pub struct Player {
    child: Arc<std::sync::Mutex<Option<Child>>>,
}

impl Player {
    pub fn stop(&self) {
        if let Some(mut child) = self.child.lock().ok().and_then(|mut c| c.take()) {
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}

impl Drop for Player {
    fn drop(&mut self) {
        self.stop();
    }
}

/// Plays `files` together from `start` seconds. The receiver gets a message when the sound ends.
pub fn play(start: f32, files: &[std::path::PathBuf]) -> Option<(Player, futures::channel::oneshot::Receiver<()>)> {
    let helper = native::helper_path()?;
    let mut child = Command::new(helper)
        .arg("play")
        .arg(format!("{start}"))
        .args(files)
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .ok()?;
    let stdin = child.stdin.take();
    let child = Arc::new(std::sync::Mutex::new(Some(child)));
    let (tx, rx) = futures::channel::oneshot::channel();
    let watched = child.clone();
    std::thread::spawn(move || {
        let _keep_open = stdin;
        loop {
            std::thread::sleep(Duration::from_millis(300));
            let mut guard = match watched.lock() {
                Ok(g) => g,
                Err(_) => break,
            };
            match guard.as_mut() {
                None => break,
                Some(c) => {
                    if matches!(c.try_wait(), Ok(Some(_))) {
                        *guard = None;
                        break;
                    }
                }
            }
        }
        let _ = tx.send(());
    });
    Some((Player { child }, rx))
}
