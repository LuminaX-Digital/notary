//! Who is speaking in a call. The call apps show it in their windows, so the native helper reads the
//! accessibility tree of the call window, and this module finds the speaker's name in it and matches the
//! names to the phrases of the transcript.
//!
//! The readers follow what the apps show today (Zoom's "Active speaker" tiles, Meet's talking tile, Teams'
//! roster). Apps change their windows without notice, so everything here fails soft: no name found means
//! the line stays "Call audio".

use crate::calls::CallApp;
use serde::Deserialize;
use std::collections::BTreeSet;

#[derive(Deserialize, Clone, Default, Debug)]
#[serde(default)]
pub struct Node {
    pub role: String,
    pub subrole: String,
    pub description: String,
    pub title: String,
    pub value: String,
    pub identifier: String,
    #[serde(rename = "domClasses")]
    pub dom_classes: Vec<String>,
    pub children: Vec<Node>,
}

#[derive(Deserialize, Clone, Default, Debug)]
#[serde(default)]
pub struct WindowDump {
    pub title: String,
    pub id: String,
    pub tree: Node,
}

/// One line of the call-watch stream.
#[derive(Deserialize, Clone, Default, Debug)]
#[serde(default)]
pub struct Reading {
    pub t: i64,
    pub app: String,
    pub windows: Vec<WindowDump>,
    pub error: Option<String>,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Sample {
    /// Wall clock, milliseconds since the epoch.
    pub ms: i64,
    pub speaking: Vec<String>,
    pub participants: Vec<String>,
    pub me: Option<String>,
}

fn walk<'a>(n: &'a Node, f: &mut impl FnMut(&'a Node)) {
    f(n);
    for c in &n.children {
        walk(c, f);
    }
}

const ME_MARKS: [&str; 7] = ["(me)", "(you)", "(ich)", "(du)", "(moi)", "(yo)", "(tú)"];
const ACTIVE_SPEAKER: [&str; 6] = ["active speaker", "aktiver sprecher", "orador activo", "intervenant actif", "oratore attivo", "spreker actief"];
const TALKING_PREFIXES: [&str; 4] = ["talking:", "spricht:", "parle :", "habla:"];

/// Splits "Name (me)" into the name and whether it marks the user.
fn clean(name: &str) -> (String, bool) {
    let trimmed = name.trim();
    let lower = trimmed.to_lowercase();
    for mark in ME_MARKS {
        if lower.ends_with(mark) {
            return (trimmed[..trimmed.len() - mark.len()].trim().to_string(), true);
        }
    }
    (trimmed.to_string(), false)
}

fn plausible_name(s: &str) -> bool {
    let s = s.trim();
    !s.is_empty() && s.chars().count() <= 60 && s.chars().any(char::is_alphabetic) && !s.contains('\n') && !s.contains("http")
}

fn zoom(windows: &[WindowDump]) -> Sample {
    let mut sample = Sample::default();
    for w in windows {
        walk(&w.tree, &mut |n| {
            // A video tile: "Name, Computer audio unmuted, Video off, Active speaker"
            if n.role == "AXTabGroup" && n.description.contains(',') {
                let (name, me) = clean(n.description.split(',').next().unwrap_or(""));
                if plausible_name(&name) {
                    if me {
                        sample.me = Some(name.clone());
                    }
                    let d = n.description.to_lowercase();
                    if ACTIVE_SPEAKER.iter().any(|m| d.contains(m)) {
                        sample.speaking.push(name.clone());
                    }
                    sample.participants.push(name);
                }
            }
            // The participants list.
            let label = format!("{} {}", n.title, n.description).to_lowercase();
            if n.role == "AXOutline" && (label.contains("participants") || label.contains("teilnehmer")) {
                walk(n, &mut |row| {
                    if row.role == "AXStaticText" || row.role == "AXRow" {
                        let text = if row.value.is_empty() { &row.description } else { &row.value };
                        let (name, me) = clean(text.split(',').next().unwrap_or(""));
                        if plausible_name(&name) && row.role == "AXStaticText" {
                            if me {
                                sample.me = Some(name.clone());
                            }
                            sample.participants.push(name);
                        }
                    }
                });
            }
            // The floating window: "Talking: Name"
            for text in [&n.value, &n.title] {
                let lower = text.to_lowercase();
                if let Some(p) = TALKING_PREFIXES.iter().find(|p| lower.starts_with(*p)) {
                    let (name, _) = clean(&text[p.len()..]);
                    if plausible_name(&name) {
                        sample.speaking.push(name);
                    }
                }
            }
        });
    }
    sample
}

/// Google Meet marks the tile of the person talking with a class that Google renames between builds.
const MEET_TALKING_CLASSES: [&str; 1] = ["v5h6Xc"];

fn meet(windows: &[WindowDump]) -> Sample {
    let mut sample = Sample::default();
    for w in windows {
        walk(&w.tree, &mut |n| {
            if n.dom_classes.iter().any(|c| MEET_TALKING_CLASSES.contains(&c.as_str())) {
                let mut found = None;
                walk(n, &mut |d| {
                    if found.is_none() && d.role == "AXStaticText" && plausible_name(&d.value) {
                        found = Some(d.value.trim().to_string());
                    }
                });
                if let Some(name) = found {
                    if ["you", "du"].contains(&name.to_lowercase().as_str()) {
                        sample.me = Some(name);
                    } else {
                        sample.speaking.push(name.clone());
                        sample.participants.push(name);
                    }
                }
            }
        });
    }
    sample
}

fn teams(windows: &[WindowDump]) -> Sample {
    let mut sample = Sample::default();
    for w in windows {
        walk(&w.tree, &mut |n| {
            let text = if n.description.is_empty() { &n.title } else { &n.description };
            if !matches!(n.role.as_str(), "AXRow" | "AXCell" | "AXGroup" | "AXStaticText" | "AXListItem") || text.is_empty() {
                return;
            }
            let lower = text.to_lowercase();
            // Roster rows: "Name, Organizer, Muted" / "Name, Attendee, Unmuted"
            let is_roster = lower.contains(", muted") || lower.contains(", unmuted") || lower.contains(", stummgeschaltet");
            // Speaking marks: "Name, speaking" / "Name is speaking"
            let speaking_mark = lower.ends_with(", speaking") || lower.ends_with(" is speaking") || lower.ends_with(", spricht");
            if !is_roster && !speaking_mark {
                return;
            }
            let (name, me) = clean(text.split(',').next().unwrap_or("").trim_end_matches(" is speaking"));
            if plausible_name(&name) {
                if me {
                    sample.me = Some(name.clone());
                }
                if speaking_mark {
                    sample.speaking.push(name.clone());
                }
                sample.participants.push(name);
            }
        });
    }
    sample
}

/// Reads one tick of the call-watch stream into a sample, for the given app's windows.
pub fn read(app: CallApp, reading: &Reading) -> Sample {
    let mut sample = match app {
        CallApp::Zoom => zoom(&reading.windows),
        CallApp::Teams => teams(&reading.windows),
        CallApp::Browser => {
            let mut s = meet(&reading.windows);
            let t = teams(&reading.windows);
            s.speaking.extend(t.speaking);
            s.participants.extend(t.participants);
            s.me = s.me.or(t.me);
            s
        }
        _ => Sample::default(),
    };
    for list in [&mut sample.speaking, &mut sample.participants] {
        list.sort();
        list.dedup();
    }
    sample.ms = reading.t;
    sample
}

const APP_LAG_MS: i64 = 750;
const HOLD_MS: i64 = 2_000;

/// Everything the call window showed during a recording.
#[derive(Default)]
pub struct Timeline {
    samples: Vec<Sample>,
    /// Wall clock of the start of the recording.
    zero_ms: i64,
    pub seen: BTreeSet<String>,
}

impl Timeline {
    pub fn new(zero_ms: i64) -> Self {
        Self { zero_ms, ..Self::default() }
    }

    pub fn push(&mut self, sample: Sample) {
        for p in &sample.participants {
            self.seen.insert(p.clone());
        }
        self.samples.push(sample);
    }

    pub fn is_empty(&self) -> bool {
        self.samples.is_empty()
    }

    fn meeting_ms(&self, wall: i64) -> i64 {
        wall - self.zero_ms - APP_LAG_MS
    }

    /// Who spoke during `start_ms..end_ms` of the meeting, if one person covers more than half of it.
    /// `mine` are the names that belong to the user and are never the other side.
    pub fn who(&self, start_ms: i64, end_ms: i64, mine: &[String]) -> Option<String> {
        if end_ms <= start_ms {
            return None;
        }
        // In a call between two, the other person is the only one the call audio can be.
        let latest = self.samples.last()?;
        let others: Vec<&String> = latest.participants.iter().filter(|p| !mine.contains(p) && latest.me.as_ref() != Some(*p)).collect();
        if latest.participants.len() == 2 && others.len() == 1 && (latest.me.is_some() || latest.participants.len() > others.len()) {
            return Some(others[0].clone());
        }
        let mut totals: std::collections::BTreeMap<&str, i64> = std::collections::BTreeMap::new();
        for (i, s) in self.samples.iter().enumerate() {
            let from = self.meeting_ms(s.ms);
            let until = self
                .samples
                .get(i + 1)
                .map(|n| self.meeting_ms(n.ms))
                .unwrap_or(from + HOLD_MS)
                .min(from + HOLD_MS);
            let overlap = until.min(end_ms) - from.max(start_ms);
            if overlap <= 0 {
                continue;
            }
            for name in &s.speaking {
                if !mine.contains(name) {
                    *totals.entry(name.as_str()).or_default() += overlap;
                }
            }
        }
        let (name, covered) = totals.into_iter().max_by_key(|(_, d)| *d)?;
        (covered * 2 > end_ms - start_ms).then(|| name.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn node(role: &str, description: &str, value: &str, children: Vec<Node>) -> Node {
        Node { role: role.into(), description: description.into(), value: value.into(), children, ..Node::default() }
    }

    fn window(tree: Node) -> Reading {
        Reading { t: 1000, app: "x".into(), windows: vec![WindowDump { title: "w".into(), id: String::new(), tree }], error: None }
    }

    #[test]
    fn zoom_tiles_name_the_active_speaker() {
        let tree = node(
            "AXWindow",
            "",
            "",
            vec![
                node("AXTabGroup", "Lena Meyer, Computer audio unmuted, Video on, Active speaker", "", vec![]),
                node("AXTabGroup", "Fabian Farid (me), Computer audio unmuted, Video off", "", vec![]),
                node("AXTabGroup", "Max, Computer audio muted, Video off", "", vec![]),
            ],
        );
        let s = read(CallApp::Zoom, &window(tree));
        assert_eq!(s.speaking, vec!["Lena Meyer"]);
        assert_eq!(s.me.as_deref(), Some("Fabian Farid"));
        assert_eq!(s.participants.len(), 3);
    }

    #[test]
    fn zoom_reads_german_labels_and_the_floating_window() {
        let tree = node("AXWindow", "", "", vec![node("AXTabGroup", "Anna, Computeraudio aktiviert, Aktiver Sprecher", "", vec![])]);
        assert_eq!(read(CallApp::Zoom, &window(tree)).speaking, vec!["Anna"]);
        let floating = node("AXWindow", "", "", vec![node("AXStaticText", "", "Talking: Tom Berg", vec![])]);
        assert_eq!(read(CallApp::Zoom, &window(floating)).speaking, vec!["Tom Berg"]);
    }

    #[test]
    fn zoom_participant_list_adds_names() {
        let list = node(
            "AXOutline",
            "Participants list",
            "",
            vec![node("AXRow", "", "", vec![node("AXStaticText", "", "Lena Meyer", vec![])]), node("AXRow", "", "", vec![node("AXStaticText", "", "Fabian Farid (me)", vec![])])],
        );
        let s = read(CallApp::Zoom, &window(node("AXWindow", "", "", vec![list])));
        assert_eq!(s.participants, vec!["Fabian Farid", "Lena Meyer"]);
        assert_eq!(s.me.as_deref(), Some("Fabian Farid"));
    }

    #[test]
    fn meet_reads_the_talking_tile() {
        let mut tile = node("AXGroup", "", "", vec![node("AXGroup", "", "", vec![node("AXStaticText", "", "Sam Okoro", vec![])])]);
        tile.dom_classes = vec!["xyz".into(), "v5h6Xc".into()];
        let quiet = node("AXGroup", "", "", vec![node("AXStaticText", "", "Pia", vec![])]);
        let s = read(CallApp::Browser, &window(node("AXWindow", "", "", vec![tile, quiet])));
        assert_eq!(s.speaking, vec!["Sam Okoro"]);
    }

    #[test]
    fn teams_reads_roster_and_speaking_marks() {
        let tree = node(
            "AXWindow",
            "",
            "",
            vec![
                node("AXRow", "Lena Meyer, Organizer, Unmuted", "", vec![]),
                node("AXRow", "Fabian Farid (You), Attendee, Muted", "", vec![]),
                node("AXGroup", "Lena Meyer, speaking", "", vec![]),
            ],
        );
        let s = read(CallApp::Teams, &window(tree));
        assert_eq!(s.speaking, vec!["Lena Meyer"]);
        assert_eq!(s.me.as_deref(), Some("Fabian Farid"));
        assert_eq!(s.participants, vec!["Fabian Farid", "Lena Meyer"]);
    }

    #[test]
    fn unknown_windows_give_no_names() {
        assert!(read(CallApp::Zoom, &window(node("AXWindow", "", "", vec![]))).speaking.is_empty());
        assert_eq!(read(CallApp::Slack, &window(node("AXWindow", "x, y", "", vec![]))), Sample { ms: 1000, ..Sample::default() });
    }

    fn sample(ms: i64, speaking: &[&str], participants: &[&str], me: Option<&str>) -> Sample {
        Sample {
            ms,
            speaking: speaking.iter().map(|s| s.to_string()).collect(),
            participants: participants.iter().map(|s| s.to_string()).collect(),
            me: me.map(str::to_string),
        }
    }

    #[test]
    fn a_name_needs_more_than_half_of_the_phrase() {
        let mut t = Timeline::new(0);
        // Lena speaks 0 to 4 s of the meeting (plus the app's lag), Max from 4 s on, in a call of three.
        let all = ["Fabian", "Lena", "Max"];
        for (ms, who) in [(750, "Lena"), (1750, "Lena"), (2750, "Lena"), (3750, "Lena"), (4750, "Max"), (5750, "Max"), (6750, "Max")] {
            t.push(sample(ms, &[who], &all, Some("Fabian")));
        }
        let mine = vec!["Fabian".to_string()];
        assert_eq!(t.who(500, 3500, &mine).as_deref(), Some("Lena"));
        assert_eq!(t.who(4500, 6500, &mine).as_deref(), Some("Max"));
        assert_eq!(t.who(2000, 6000, &mine), None, "split evenly between two people");
    }

    #[test]
    fn in_a_call_between_two_the_other_person_is_always_named() {
        let mut t = Timeline::new(0);
        t.push(sample(1000, &[], &["Fabian", "Lena"], Some("Fabian")));
        assert_eq!(t.who(0, 3000, &["Fabian".to_string()]).as_deref(), Some("Lena"));
    }

    #[test]
    fn your_own_name_is_never_the_other_side() {
        let mut t = Timeline::new(0);
        let all = ["Fabian", "Lena", "Max"];
        for ms in [750, 1750, 2750] {
            t.push(sample(ms, &["Fabian"], &all, Some("Fabian")));
        }
        assert_eq!(t.who(500, 3000, &["Fabian".to_string()]), None);
    }

    #[test]
    fn collects_everyone_seen_in_the_call() {
        let mut t = Timeline::new(0);
        t.push(sample(1000, &[], &["Lena", "Max"], None));
        t.push(sample(2000, &[], &["Lena", "Tom"], None));
        assert_eq!(t.seen.iter().cloned().collect::<Vec<_>>(), vec!["Lena", "Max", "Tom"]);
    }
}
