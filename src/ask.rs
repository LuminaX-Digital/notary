//! Asking questions across meetings: pick the passages that matter, let the local model answer from them.

use crate::data::{Meeting, Speaker};
use crate::summarizer;
use anyhow::Result;
use futures::channel::oneshot;
use std::path::PathBuf;

const WINDOW: usize = 4;
const STEP: usize = 2;
const MAX_PER_MEETING: usize = 3;
const MAX_CHARS: usize = 1400;

pub struct Source {
    pub meeting_id: i64,
    pub title: String,
    pub when: String,
    pub text: String,
}

const STOPWORDS: &[&str] = &[
    // English
    "the", "and", "was", "were", "what", "when", "who", "how", "why", "did", "does", "are", "for", "with", "about", "that", "this", "have", "has",
    "had", "from", "our", "your", "you", "they", "them", "there", "which", "would", "could", "should", "any", "all", "can", "will", "not", "but",
    "into", "been", "being", "last", "next", "which", "welche", "welcher", "welches",
    // German
    "der", "die", "das", "und", "ist", "war", "wer", "wie", "was", "wann", "wo", "warum", "wir", "ihr", "ich", "sie", "ein", "eine", "einen", "einem",
    "einer", "dem", "den", "des", "mit", "von", "vom", "zum", "zur", "auf", "für", "bei", "aus", "nach", "auch", "noch", "nicht", "wurde", "wurden",
    "haben", "hat", "hatte", "sich", "wird", "werden", "kann", "kannst", "über", "diese", "dieser", "dieses", "gibt", "gab", "mal", "mir",
];

/// Lowercases and folds umlauts, so "Verträge" matches "Vertrag".
fn fold(s: &str) -> String {
    s.to_lowercase().replace('\u{e4}', "a").replace('\u{f6}', "o").replace('\u{fc}', "u").replace('\u{df}', "ss")
}

/// The words of a question that carry meaning, cut to a stem so "Entscheidungen" also finds "Entscheidung".
fn keys(question: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for w in question.split(|c: char| !c.is_alphanumeric()).filter(|w| !w.is_empty()) {
        let w = w.to_lowercase();
        if w.chars().count() < 3 || STOPWORDS.contains(&w.as_str()) {
            continue;
        }
        let key: String = fold(&w).chars().take(6).collect();
        if !out.contains(&key) {
            out.push(key);
        }
    }
    out
}

struct Chunk {
    meeting: usize,
    text: String,
    lower: String,
}

fn chunks(meetings: &[Meeting]) -> Vec<Chunk> {
    let mut out = Vec::new();
    for (mi, m) in meetings.iter().enumerate() {
        let notes: Vec<String> = m.summary.iter().map(|(t, b)| format!("{t}: {b}")).collect();
        let mut notes_text = notes.join("\n");
        for a in &m.actions {
            notes_text.push_str(&format!("\nTask: {} ({})", a.label, a.owner));
        }
        if !notes_text.is_empty() {
            let text = format!("Notes of \"{}\":\n{}", m.title, notes_text);
            out.push(Chunk { meeting: mi, lower: fold(&text), text });
        }
        let spoken: Vec<String> = m
            .lines
            .iter()
            .filter(|l| l.who != Speaker::Note)
            .map(|l| format!("{}: {}", l.name, l.text))
            .collect();
        let mut start = 0;
        while start < spoken.len() {
            let end = (start + WINDOW).min(spoken.len());
            let text = spoken[start..end].join("\n");
            out.push(Chunk { meeting: mi, lower: fold(&text), text });
            if end == spoken.len() {
                break;
            }
            start += STEP;
        }
    }
    out
}

/// The most relevant passages for a question, best first.
pub fn retrieve(meetings: &[Meeting], question: &str, max: usize) -> Vec<Source> {
    let keys = keys(question);
    if keys.is_empty() {
        return Vec::new();
    }
    let all = chunks(meetings);
    let total = all.len() as f32;
    let weights: Vec<f32> = keys
        .iter()
        .map(|k| {
            let df = all.iter().filter(|c| c.lower.contains(k.as_str())).count() as f32;
            (1.0 + total / (1.0 + df)).ln()
        })
        .collect();

    let mut scored: Vec<(f32, usize)> = all
        .iter()
        .enumerate()
        .filter_map(|(i, c)| {
            let score: f32 = keys
                .iter()
                .zip(&weights)
                .map(|(k, w)| (c.lower.matches(k.as_str()).count().min(3) as f32) * w)
                .sum();
            (score > 0.0).then_some((score, i))
        })
        .collect();
    scored.sort_by(|a, b| {
        b.0.partial_cmp(&a.0)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then(meetings[all[b.1].meeting].started.cmp(&meetings[all[a.1].meeting].started))
    });

    let mut per_meeting = std::collections::HashMap::new();
    let mut picked = Vec::new();
    for (_, i) in scored {
        let c = &all[i];
        let n = per_meeting.entry(c.meeting).or_insert(0);
        if *n >= MAX_PER_MEETING {
            continue;
        }
        *n += 1;
        let m = &meetings[c.meeting];
        picked.push(Source {
            meeting_id: m.id,
            title: m.title.to_string(),
            when: m.meta().split(" \u{b7} ").take(2).collect::<Vec<_>>().join(" \u{b7} "),
            text: c.text.chars().take(MAX_CHARS).collect(),
        });
        if picked.len() == max {
            break;
        }
    }
    picked
}

/// The message handed to the model: numbered passages, then the question.
pub fn prompt_input(question: &str, sources: &[Source]) -> String {
    let mut out = String::from("Sources:\n");
    for (i, s) in sources.iter().enumerate() {
        out.push_str(&format!("[{}] Meeting \"{}\", {}\n{}\n\n", i + 1, s.title, s.when, s.text));
    }
    out.push_str(&format!("Question: {question}\n"));
    out
}

/// Numbers cited in an answer as [1], [2], in order of first appearance, limited to existing sources.
pub fn cited(answer: &str, source_count: usize) -> Vec<usize> {
    let mut out = Vec::new();
    let bytes = answer.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'[' {
            if let Some(close) = answer[i + 1..].find(']') {
                for part in answer[i + 1..i + 1 + close].split(',') {
                    if let Ok(n) = part.trim().parse::<usize>() {
                        if n >= 1 && n <= source_count && !out.contains(&n) {
                            out.push(n);
                        }
                    }
                }
                i += close + 1;
            }
        }
        i += 1;
    }
    out
}

/// Asks the local model on a background thread; the answer arrives on the returned channel.
pub fn answer(model: PathBuf, question: String, sources: &[Source]) -> oneshot::Receiver<Result<String>> {
    let language = summarizer::detect_language(&question);
    let input = prompt_input(&question, sources);
    let (tx, rx) = oneshot::channel();
    std::thread::spawn(move || {
        let _ = tx.send(summarizer::run_helper(&model, language, "ask", &input).map(|s| s.trim().to_string()));
    });
    rx
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::data::line;

    fn meeting(id: i64, title: &str, lines: &[(&str, &str)]) -> Meeting {
        let l = lines.iter().map(|(n, t)| line("00:00", n, Speaker::Other, t)).collect();
        Meeting::new(id, title, id * 1000, false, l)
    }

    fn library() -> Vec<Meeting> {
        vec![
            meeting(
                1,
                "Launch planning",
                &[
                    ("Lena", "Let's keep v0.3 to the Mac audio tap."),
                    ("Fabian", "If the beta isn't out by December 4, the MCP server moves to v0.4."),
                    ("Lena", "Agreed, a signed DMG by Monday."),
                ],
            ),
            meeting(
                2,
                "AVV Besprechung",
                &[
                    ("Beratung", "Der Auftragsverarbeitungsvertrag wird Teil jedes Vertrags."),
                    ("Fabian", "Ich baue die Checkbox bis Freitag in den Checkout ein."),
                ],
            ),
        ]
    }

    #[test]
    fn finds_the_english_passage() {
        let s = retrieve(&library(), "When does the MCP server move to v0.4?", 4);
        assert_eq!(s[0].meeting_id, 1);
        assert!(s[0].text.contains("December 4"));
    }

    #[test]
    fn finds_the_german_passage() {
        let s = retrieve(&library(), "Wer baut die Checkbox bis wann ein?", 4);
        assert_eq!(s[0].meeting_id, 2);
        assert!(s[0].text.contains("Freitag"));
    }

    #[test]
    fn matches_word_forms() {
        let s = retrieve(&library(), "Welche Verträge gibt es?", 4);
        assert!(s.iter().any(|x| x.meeting_id == 2));
    }

    #[test]
    fn returns_nothing_when_no_word_matches() {
        assert!(retrieve(&library(), "What is the weather?", 4).is_empty());
        assert!(retrieve(&library(), "Wie ist das?", 4).is_empty());
    }

    #[test]
    fn reads_citations_in_order() {
        assert_eq!(cited("The date is December 4 [2]. Also [1, 2] and [9].", 3), vec![2, 1]);
        assert!(cited("No sources here.", 3).is_empty());
    }

    #[test]
    fn numbers_the_sources_in_the_prompt() {
        let s = retrieve(&library(), "beta December", 4);
        let p = prompt_input("When is the beta?", &s);
        assert!(p.starts_with("Sources:\n[1] Meeting"));
        assert!(p.ends_with("Question: When is the beta?\n"));
    }
}
