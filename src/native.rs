//! Talking to the notary-mac helper: permissions, microphone use, calendar events.

use anyhow::{Context, Result, bail};
use futures::channel::oneshot;
use serde::Deserialize;
use serde::de::DeserializeOwned;
use std::path::PathBuf;
use std::process::Command;

#[derive(Deserialize, Clone, Debug, PartialEq)]
pub struct Status {
    pub microphone: String,
    pub screen: bool,
    pub accessibility: bool,
    pub calendar: String,
}

#[derive(Deserialize, Clone, Debug)]
pub struct MicApp {
    #[serde(rename = "bundleId")]
    pub bundle_id: String,
    pub name: String,
    pub pid: i32,
}

#[derive(Deserialize, Clone, Debug, PartialEq)]
pub struct Attendee {
    pub name: String,
    pub email: String,
}

#[derive(Deserialize, Clone, Debug)]
pub struct CalendarInfo {
    pub id: String,
    pub title: String,
    pub source: String,
}

#[derive(Deserialize, Clone, Debug)]
pub struct CalendarEvent {
    pub id: String,
    pub title: String,
    pub start: String,
    pub end: String,
    pub calendar: String,
    #[serde(rename = "calendarId")]
    pub calendar_id: String,
    #[serde(default)]
    pub location: Option<String>,
    #[serde(default)]
    pub notes: Option<String>,
    #[serde(default)]
    pub url: Option<String>,
    #[serde(default)]
    pub me: Option<String>,
    #[serde(default)]
    pub declined: bool,
    #[serde(default)]
    pub attendees: Vec<Attendee>,
}

pub fn helper_path() -> Option<PathBuf> {
    let helper = std::env::current_exe().ok()?.with_file_name("notary-mac");
    helper.exists().then_some(helper)
}

/// Runs the helper and parses its JSON. An `{"error": code}` answer becomes an error.
pub fn run<T: DeserializeOwned>(args: &[&str]) -> Result<T> {
    let helper = helper_path().context("the native helper (notary-mac) is missing next to the app")?;
    let output = Command::new(helper).args(args).output().context("could not start the native helper")?;
    parse(&String::from_utf8_lossy(&output.stdout))
}

fn parse<T: DeserializeOwned>(text: &str) -> Result<T> {
    let value: serde_json::Value = serde_json::from_str(text).context("the native helper answered with something unexpected")?;
    if let Some(code) = value.get("error").and_then(|e| e.as_str()) {
        bail!("{code}");
    }
    Ok(serde_json::from_value(value)?)
}

pub fn status() -> Result<Status> {
    run(&["status"])
}

pub fn request(kind: &str) -> Result<Status> {
    run(&["request", kind])
}

pub fn mic_apps() -> Result<Vec<MicApp>> {
    run(&["mic-apps"])
}

pub fn calendars() -> Result<Vec<CalendarInfo>> {
    run(&["calendars"])
}

pub fn events(from: &str, to: &str) -> Result<Vec<CalendarEvent>> {
    run(&["events", from, to])
}

/// Runs `f` on a background thread; the result arrives on the returned channel.
pub fn in_background<T: Send + 'static>(f: impl FnOnce() -> Result<T> + Send + 'static) -> oneshot::Receiver<Result<T>> {
    let (tx, rx) = oneshot::channel();
    std::thread::spawn(move || {
        let _ = tx.send(f());
    });
    rx
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_the_status_object() {
        let s: Status = parse(r#"{"accessibility":true,"calendar":"writeOnly","microphone":"authorized","screen":false}"#).unwrap();
        assert!(s.accessibility && !s.screen);
        assert_eq!(s.calendar, "writeOnly");
    }

    #[test]
    fn helper_errors_become_errors() {
        let e = parse::<Status>(r#"{"error":"calendar-not-authorized"}"#).unwrap_err();
        assert_eq!(e.to_string(), "calendar-not-authorized");
        assert!(parse::<Status>("not json").is_err());
    }

    #[test]
    fn reads_events_with_missing_optional_fields() {
        let events: Vec<CalendarEvent> = parse(
            r#"[{"id":"a:1","title":"Standup","start":"2026-11-21T10:00:00+01:00","end":"2026-11-21T10:15:00+01:00",
                "calendar":"Work","calendarId":"c1","attendees":[{"name":"Lena","email":"lena@example.com"}]}]"#,
        )
        .unwrap();
        assert_eq!(events[0].attendees[0].name, "Lena");
        assert!(events[0].url.is_none() && !events[0].declined);
    }

    #[test]
    fn reads_mic_apps() {
        let apps: Vec<MicApp> = parse(r#"[{"bundleId":"us.zoom.CptHost","name":"zoom.us","pid":42}]"#).unwrap();
        assert_eq!(apps[0].bundle_id, "us.zoom.CptHost");
    }
}
