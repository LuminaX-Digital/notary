//! Noticing calls: when to offer a recording, when a call is over, when to ask whether to stop.
//!
//! The decisions are a pure state machine fed with what the helper sees (which apps use the microphone)
//! and what the engine hears (how long it has been quiet), so the rules can be tested with a fake clock.

use std::collections::{HashMap, HashSet};

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum CallApp {
    Zoom,
    Teams,
    Webex,
    FaceTime,
    Slack,
    Discord,
    WhatsApp,
    /// A browser, which only counts as a call when a Meet, Teams or Zoom tab is open.
    Browser,
}

impl CallApp {
    pub fn name(self) -> &'static str {
        match self {
            CallApp::Zoom => "Zoom",
            CallApp::Teams => "Teams",
            CallApp::Webex => "Webex",
            CallApp::FaceTime => "FaceTime",
            CallApp::Slack => "Slack",
            CallApp::Discord => "Discord",
            CallApp::WhatsApp => "WhatsApp",
            CallApp::Browser => "Browser call",
        }
    }

    /// The bundle ids the call-watch helper should read for speaker names.
    pub fn watch_ids(self) -> &'static [&'static str] {
        match self {
            CallApp::Zoom => &["us.zoom.xos"],
            CallApp::Teams => &["com.microsoft.teams", "com.microsoft.teams2"],
            CallApp::Webex => &["Cisco-Systems.Spark", "com.cisco.webexmeetingsapp"],
            CallApp::Browser => &["com.google.Chrome", "company.thebrowser.Browser", "com.apple.Safari", "com.microsoft.edgemac", "org.mozilla.firefox", "com.brave.Browser"],
            _ => &[],
        }
    }
}

/// Which call app a process that uses the microphone belongs to. Helper processes map to their owner.
pub fn owner(bundle_id: &str) -> Option<CallApp> {
    const TABLE: [(&str, CallApp); 17] = [
        ("us.zoom.", CallApp::Zoom),
        ("com.microsoft.teams", CallApp::Teams),
        ("Cisco-Systems.Spark", CallApp::Webex),
        ("com.cisco.webex", CallApp::Webex),
        ("com.webex.", CallApp::Webex),
        ("com.apple.FaceTime", CallApp::FaceTime),
        ("com.apple.avconferenced", CallApp::FaceTime),
        ("com.tinyspeck.slackmacgap", CallApp::Slack),
        ("com.hnc.Discord", CallApp::Discord),
        ("net.whatsapp.", CallApp::WhatsApp),
        ("com.google.Chrome", CallApp::Browser),
        ("com.apple.Safari", CallApp::Browser),
        ("com.apple.WebKit.", CallApp::Browser),
        ("com.microsoft.edgemac", CallApp::Browser),
        ("org.mozilla.firefox", CallApp::Browser),
        ("com.brave.Browser", CallApp::Browser),
        ("company.thebrowser.Browser", CallApp::Browser),
    ];
    TABLE.iter().find(|(prefix, _)| bundle_id.starts_with(prefix)).map(|(_, app)| *app)
}

/// Does a browser window title look like a call?
pub fn looks_like_call(title: &str) -> bool {
    let words: Vec<&str> = title.split(|c: char| !c.is_alphanumeric()).collect();
    words.iter().any(|w| ["Meet", "Teams", "Zoom"].contains(w))
}

#[derive(Clone, Debug, PartialEq)]
pub enum Action {
    /// Offer to record this call.
    Offer(CallApp),
    /// A recording started by hand turned out to be a call in this app.
    Bind(CallApp),
    /// The call is over: stop the recording.
    AutoStop,
    /// Nothing is happening any more in a recording started by hand: ask whether to stop.
    AskStop,
}

#[derive(Clone, Debug)]
pub struct Recording {
    pub started_ms: i64,
    /// The call app this recording is tied to, when it was started from a call.
    pub bound: Option<CallApp>,
}

pub struct Poll<'a> {
    pub now_ms: i64,
    /// Call apps that are using the microphone right now.
    pub on_mic: &'a [CallApp],
    pub recording: Option<&'a Recording>,
    /// How long both the microphone and the call audio have been silent, in seconds.
    pub quiet_secs: u64,
}

pub struct Rules {
    pub offer_to_record: bool,
    pub stop_when_call_ends: bool,
}

const NEW_CALL_AFTER_MS: i64 = 2 * 60_000;
const CALL_GONE_MS: i64 = 4_000;
const CALL_SILENCE_SECS: u64 = 60;
const BIND_WINDOW_MS: i64 = 6_000;
const BY_HAND_IDLE_MS: i64 = 30_000;
const SNOOZE_MS: i64 = 5 * 60_000;

#[derive(Default)]
pub struct Detector {
    offered: HashSet<CallApp>,
    last_on_mic: HashMap<CallApp, i64>,
    seen_on_mic: bool,
    gone_since: Option<i64>,
    mic_idle_since: Option<i64>,
    snoozed_until: i64,
    asked_stop: bool,
    stop_sent: bool,
}

impl Detector {
    /// Forget everything about the last recording; call when a recording starts.
    pub fn recording_started(&mut self) {
        self.seen_on_mic = false;
        self.gone_since = None;
        self.mic_idle_since = None;
        self.asked_stop = false;
        self.stop_sent = false;
    }

    /// The user closed the card or keeps going: do not ask again for a while.
    pub fn snooze(&mut self, now_ms: i64) {
        self.snoozed_until = now_ms + SNOOZE_MS;
        self.asked_stop = false;
    }

    pub fn step(&mut self, poll: &Poll, rules: &Rules) -> Vec<Action> {
        let mut actions = Vec::new();
        for app in poll.on_mic {
            self.last_on_mic.insert(*app, poll.now_ms);
        }
        // An app that has been off the microphone for a while counts as a new call next time.
        let stale: Vec<CallApp> = self.offered.iter().copied().filter(|a| {
            self.last_on_mic.get(a).is_none_or(|t| poll.now_ms - t >= NEW_CALL_AFTER_MS)
        }).collect();
        for app in stale {
            self.offered.remove(&app);
        }

        match poll.recording {
            None => {
                if rules.offer_to_record {
                    if let Some(app) = poll.on_mic.iter().find(|a| !self.offered.contains(a)) {
                        self.offered.insert(*app);
                        actions.push(Action::Offer(*app));
                    }
                }
            }
            Some(rec) => self.while_recording(poll, rec, rules, &mut actions),
        }
        actions
    }

    fn while_recording(&mut self, poll: &Poll, rec: &Recording, rules: &Rules, actions: &mut Vec<Action>) {
        match rec.bound {
            Some(app) => {
                if poll.on_mic.contains(&app) {
                    self.seen_on_mic = true;
                    self.gone_since = None;
                } else if self.seen_on_mic {
                    let since = *self.gone_since.get_or_insert(poll.now_ms);
                    if rules.stop_when_call_ends && !self.stop_sent && poll.now_ms - since >= CALL_GONE_MS && poll.quiet_secs >= CALL_SILENCE_SECS {
                        self.stop_sent = true;
                        actions.push(Action::AutoStop);
                    }
                }
            }
            None => {
                if let Some(app) = poll.on_mic.first() {
                    self.mic_idle_since = None;
                    if poll.now_ms - rec.started_ms <= BIND_WINDOW_MS && !self.seen_on_mic {
                        self.seen_on_mic = true;
                        actions.push(Action::Bind(*app));
                    }
                } else {
                    let since = *self.mic_idle_since.get_or_insert(poll.now_ms);
                    if !self.asked_stop
                        && poll.now_ms >= self.snoozed_until
                        && poll.now_ms - since >= BY_HAND_IDLE_MS
                        && poll.quiet_secs >= CALL_SILENCE_SECS
                    {
                        self.asked_stop = true;
                        actions.push(Action::AskStop);
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const RULES: Rules = Rules { offer_to_record: true, stop_when_call_ends: true };

    fn poll<'a>(now_s: i64, on_mic: &'a [CallApp], recording: Option<&'a Recording>, quiet: u64) -> Poll<'a> {
        Poll { now_ms: now_s * 1000, on_mic, recording, quiet_secs: quiet }
    }

    #[test]
    fn maps_helper_processes_to_their_app() {
        assert_eq!(owner("us.zoom.CptHost"), Some(CallApp::Zoom));
        assert_eq!(owner("com.microsoft.teams2.helper"), Some(CallApp::Teams));
        assert_eq!(owner("com.apple.avconferenced"), Some(CallApp::FaceTime));
        assert_eq!(owner("com.google.Chrome.helper"), Some(CallApp::Browser));
        assert_eq!(owner("com.rogueamoeba.audiohijack"), None);
    }

    #[test]
    fn browser_titles_need_a_call_word() {
        assert!(looks_like_call("Meet - abc-defg-hij - Google Chrome"));
        assert!(looks_like_call("Weekly sync | Microsoft Teams"));
        assert!(!looks_like_call("Sunshine Meeting Notes"));
        assert!(!looks_like_call("Inbox - Mail"));
    }

    #[test]
    fn offers_once_per_call_and_again_after_a_two_minute_pause() {
        let mut d = Detector::default();
        let zoom = [CallApp::Zoom];
        assert_eq!(d.step(&poll(0, &zoom, None, 0), &RULES), vec![Action::Offer(CallApp::Zoom)]);
        assert!(d.step(&poll(3, &zoom, None, 0), &RULES).is_empty());
        assert!(d.step(&poll(60, &[], None, 0), &RULES).is_empty());
        assert!(d.step(&poll(100, &zoom, None, 0), &RULES).is_empty(), "off the mic for under 2 minutes is the same call");
        assert!(d.step(&poll(150, &[], None, 0), &RULES).is_empty());
        assert!(d.step(&poll(280, &[], None, 0), &RULES).is_empty());
        assert_eq!(d.step(&poll(300, &zoom, None, 0), &RULES), vec![Action::Offer(CallApp::Zoom)]);
    }

    #[test]
    fn offers_nothing_when_switched_off_or_already_recording() {
        let mut d = Detector::default();
        let off = Rules { offer_to_record: false, stop_when_call_ends: true };
        assert!(d.step(&poll(0, &[CallApp::Zoom], None, 0), &off).is_empty());
        let rec = Recording { started_ms: 0, bound: Some(CallApp::Zoom) };
        assert!(d.step(&poll(1, &[CallApp::Teams], Some(&rec), 0), &RULES).is_empty());
    }

    #[test]
    fn stops_when_the_call_is_gone_and_it_has_been_quiet() {
        let mut d = Detector::default();
        let rec = Recording { started_ms: 0, bound: Some(CallApp::Zoom) };
        d.recording_started();
        let zoom = [CallApp::Zoom];
        assert!(d.step(&poll(10, &zoom, Some(&rec), 0), &RULES).is_empty());
        assert!(d.step(&poll(100, &[], Some(&rec), 10), &RULES).is_empty(), "gone, but not for 4 s yet");
        assert!(d.step(&poll(103, &[], Some(&rec), 20), &RULES).is_empty());
        assert!(d.step(&poll(105, &[], Some(&rec), 30), &RULES).is_empty(), "gone long enough but not quiet long enough");
        assert_eq!(d.step(&poll(110, &[], Some(&rec), 65), &RULES), vec![Action::AutoStop]);
        assert!(d.step(&poll(112, &[], Some(&rec), 67), &RULES).is_empty(), "only once");
    }

    #[test]
    fn never_stops_a_call_that_was_not_seen_or_when_the_setting_is_off() {
        let mut d = Detector::default();
        let rec = Recording { started_ms: 0, bound: Some(CallApp::Zoom) };
        d.recording_started();
        assert!(d.step(&poll(100, &[], Some(&rec), 120), &RULES).is_empty());
        let off = Rules { offer_to_record: true, stop_when_call_ends: false };
        d.step(&poll(101, &[CallApp::Zoom], Some(&rec), 0), &off);
        assert!(d.step(&poll(200, &[], Some(&rec), 120), &off).is_empty());
        assert!(d.step(&poll(210, &[], Some(&rec), 130), &off).is_empty());
    }

    #[test]
    fn a_recording_started_by_hand_binds_to_a_call_that_shows_up_at_once() {
        let mut d = Detector::default();
        let rec = Recording { started_ms: 0, bound: None };
        d.recording_started();
        assert_eq!(d.step(&poll(3, &[CallApp::Teams], Some(&rec), 0), &RULES), vec![Action::Bind(CallApp::Teams)]);
    }

    #[test]
    fn a_recording_started_by_hand_asks_before_stopping_and_can_be_snoozed() {
        let mut d = Detector::default();
        let rec = Recording { started_ms: 0, bound: None };
        d.recording_started();
        assert!(d.step(&poll(10, &[], Some(&rec), 10), &RULES).is_empty());
        assert!(d.step(&poll(50, &[], Some(&rec), 50), &RULES).is_empty(), "idle for 40 s but quiet only 50 s");
        assert_eq!(d.step(&poll(75, &[], Some(&rec), 70), &RULES), vec![Action::AskStop]);
        assert!(d.step(&poll(80, &[], Some(&rec), 75), &RULES).is_empty(), "asks once");
        d.snooze(80_000);
        assert!(d.step(&poll(200, &[], Some(&rec), 200), &RULES).is_empty(), "snoozed for 5 minutes");
        assert_eq!(d.step(&poll(400, &[], Some(&rec), 400), &RULES), vec![Action::AskStop]);
    }
}
