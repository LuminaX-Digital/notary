use chrono::{DateTime, Local, TimeZone};
use gpui::SharedString;

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Speaker {
    Me,
    Other,
    Note,
}

impl Speaker {
    pub fn to_db(self) -> i64 {
        match self {
            Speaker::Me => 0,
            Speaker::Other => 1,
            Speaker::Note => 2,
        }
    }

    pub fn from_db(v: i64) -> Self {
        match v {
            0 => Speaker::Me,
            2 => Speaker::Note,
            _ => Speaker::Other,
        }
    }
}

pub struct Line {
    pub ts: SharedString,
    pub name: SharedString,
    pub who: Speaker,
    pub text: SharedString,
}

pub struct Action {
    pub id: i64,
    pub label: SharedString,
    pub owner: SharedString,
    pub done: bool,
}

pub struct Meeting {
    pub id: i64,
    pub title: SharedString,
    pub started: i64,
    pub live: bool,
    pub summarizing: bool,
    pub lines: Vec<Line>,
    pub summary: Vec<(SharedString, SharedString)>,
    pub actions: Vec<Action>,
}

pub fn line(ts: &str, name: &str, who: Speaker, text: &str) -> Line {
    Line {
        ts: ts.to_string().into(),
        name: name.to_string().into(),
        who,
        text: text.to_string().into(),
    }
}

fn local(secs: i64) -> DateTime<Local> {
    Local.timestamp_opt(secs, 0).single().unwrap_or_else(Local::now)
}

impl Meeting {
    pub fn new(id: i64, title: &str, started: i64, live: bool, lines: Vec<Line>) -> Self {
        Self {
            id,
            title: title.to_string().into(),
            started,
            live,
            summarizing: false,
            lines,
            summary: Vec::new(),
            actions: Vec::new(),
        }
    }

    /// The spoken lines as "Name: text", the form the summary model reads. Typed notes are left out.
    pub fn transcript_text(&self) -> String {
        self.lines
            .iter()
            .filter(|l| l.who != Speaker::Note)
            .map(|l| format!("{}: {}\n", l.name, l.text))
            .collect()
    }

    pub fn word_count(&self) -> usize {
        self.lines.iter().map(|l| l.text.split_whitespace().count()).sum()
    }

    pub fn time(&self) -> SharedString {
        local(self.started).format("%H:%M").to_string().into()
    }

    pub fn meta(&self) -> SharedString {
        local(self.started)
            .format("%a %b %-d \u{b7} %H:%M \u{b7} audio stays on this Mac")
            .to_string()
            .into()
    }

    pub fn sub(&self) -> SharedString {
        let n = self.lines.len();
        match (self.live, n) {
            (true, 0) => "Recording".into(),
            (true, n) => format!("Recording \u{b7} {n} lines").into(),
            (false, 0) => "No speech".into(),
            (false, 1) => "1 line".into(),
            (false, n) => format!("{n} lines").into(),
        }
    }

    pub fn day_label(&self) -> String {
        let date = local(self.started).date_naive();
        let today = Local::now().date_naive();
        if date == today {
            "Today".into()
        } else if today.pred_opt() == Some(date) {
            "Yesterday".into()
        } else {
            date.format("%b %-d").to_string()
        }
    }
}
