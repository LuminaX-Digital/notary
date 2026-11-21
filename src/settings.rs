//! The user's choices, saved as a small JSON file next to the meetings database.

use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Debug)]
pub enum Spoken {
    /// Whisper listens for English only: faster and a little more accurate for English meetings.
    EnglishOnly,
    /// Whisper detects the language of every phrase, so a meeting can switch between languages.
    Multi,
}

#[derive(Clone, Serialize, Deserialize, Debug, PartialEq)]
#[serde(default)]
pub struct Settings {
    pub your_name: String,
    pub spoken: Spoken,
    /// "Same as the meeting" or the name of a language.
    pub summary_language: String,
    pub custom_words: String,
    pub keep_audio: bool,
    pub stop_when_call_ends: bool,
    pub offer_to_record: bool,
    pub ai_apps: bool,
    pub hide_events_without_link: bool,
    pub hidden_calendars: Vec<String>,
}

pub const SAME_LANGUAGE: &str = "Same as the meeting";
pub const SUMMARY_LANGUAGES: [&str; 9] =
    [SAME_LANGUAGE, "English", "German", "French", "Spanish", "Italian", "Dutch", "Portuguese", "Turkish"];

impl Default for Settings {
    fn default() -> Self {
        Self {
            your_name: default_name(),
            spoken: Spoken::Multi,
            summary_language: SAME_LANGUAGE.to_string(),
            custom_words: String::new(),
            keep_audio: true,
            stop_when_call_ends: true,
            offer_to_record: true,
            ai_apps: false,
            hide_events_without_link: false,
            hidden_calendars: Vec::new(),
        }
    }
}

/// The macOS full name of the user, or "You".
fn default_name() -> String {
    std::process::Command::new("id")
        .arg("-F")
        .output()
        .ok()
        .and_then(|o| String::from_utf8(o.stdout).ok())
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "You".to_string())
}

/// The folder that holds the database, settings and meeting audio.
pub fn data_dir() -> PathBuf {
    if let Ok(db) = std::env::var("NOTARY_DB") {
        if let Some(dir) = PathBuf::from(db).parent() {
            return dir.to_path_buf();
        }
    }
    PathBuf::from(std::env::var("HOME").unwrap_or_default()).join("Library/Application Support/Notary")
}

fn path() -> PathBuf {
    data_dir().join("settings.json")
}

impl Settings {
    pub fn load() -> Self {
        std::fs::read_to_string(path())
            .ok()
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_default()
    }

    pub fn save(&self) -> anyhow::Result<()> {
        let dir = data_dir();
        std::fs::create_dir_all(&dir)?;
        std::fs::write(path(), serde_json::to_string_pretty(self)?)?;
        Ok(())
    }

    /// The whisper language code: "en" for English only, "auto" otherwise.
    pub fn whisper_language(&self) -> &'static str {
        match self.spoken {
            Spoken::EnglishOnly => "en",
            Spoken::Multi => "auto",
        }
    }

    /// Words that help whisper spell names and jargon, joined for its prompt.
    pub fn prompt_words(&self) -> String {
        self.custom_words
            .split([',', '\n', ';'])
            .map(str::trim)
            .filter(|w| !w.is_empty())
            .collect::<Vec<_>>()
            .join(", ")
    }

    /// The language the summary is written in, or None to follow the meeting.
    pub fn forced_summary_language(&self) -> Option<&str> {
        (self.summary_language != SAME_LANGUAGE && !self.summary_language.is_empty()).then_some(self.summary_language.as_str())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn missing_fields_fall_back_to_defaults() {
        let s: Settings = serde_json::from_str(r#"{"your_name":"Fabian"}"#).unwrap();
        assert_eq!(s.your_name, "Fabian");
        assert_eq!(s.spoken, Spoken::Multi);
        assert!(s.keep_audio && !s.ai_apps);
    }

    #[test]
    fn settings_round_trip_through_json() {
        let mut s = Settings::default();
        s.spoken = Spoken::EnglishOnly;
        s.custom_words = "Notary, Kantwerk".into();
        let back: Settings = serde_json::from_str(&serde_json::to_string(&s).unwrap()).unwrap();
        assert_eq!(back, s);
    }

    #[test]
    fn custom_words_become_one_prompt() {
        let mut s = Settings::default();
        s.custom_words = "Notary, Kantwerk\nLuminaX ; ".into();
        assert_eq!(s.prompt_words(), "Notary, Kantwerk, LuminaX");
        assert_eq!(s.whisper_language(), "auto");
        s.spoken = Spoken::EnglishOnly;
        assert_eq!(s.whisper_language(), "en");
    }

    #[test]
    fn summary_language_can_follow_the_meeting() {
        let mut s = Settings::default();
        assert_eq!(s.forced_summary_language(), None);
        s.summary_language = "German".into();
        assert_eq!(s.forced_summary_language(), Some("German"));
    }
}
