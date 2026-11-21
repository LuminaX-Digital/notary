//! Asking questions across meetings: pick the passages that matter, let the local model answer from them.

use crate::data::{Meeting, Speaker};
use crate::{semantic, summarizer};
use anyhow::Result;
use futures::channel::oneshot;
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::path::PathBuf;

const WINDOW: usize = 4;
const STEP: usize = 2;
const MAX_PER_MEETING: usize = 3;
const MAX_CHARS: usize = 1400;
const RANK_DEPTH: usize = 20;
const RRF_K: f32 = 60.0;

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
    key: String,
    text: String,
    lower: String,
}

fn chunks(meetings: &[Meeting]) -> Vec<Chunk> {
    let mut out = Vec::new();
    for (mi, m) in meetings.iter().enumerate() {
        let mut n = 0;
        let mut push = |text: String, out: &mut Vec<Chunk>| {
            out.push(Chunk { meeting: mi, key: format!("{}:{}", m.id, n), lower: fold(&text), text });
            n += 1;
        };
        let notes: Vec<String> = m.summary.iter().map(|(t, b)| format!("{t}: {b}")).collect();
        let mut notes_text = notes.join("\n");
        for a in &m.actions {
            notes_text.push_str(&format!("\nTask: {} ({})", a.label, a.owner));
        }
        if !notes_text.is_empty() {
            push(format!("Notes of \"{}\":\n{}", m.title, notes_text), &mut out);
        }
        let spoken: Vec<String> = m
            .lines
            .iter()
            .map(|l| match l.who {
                Speaker::Note => format!("Note written by the user: {}", l.text),
                _ => format!("{}: {}", l.name, l.text),
            })
            .collect();
        let mut start = 0;
        while start < spoken.len() {
            let end = (start + WINDOW).min(spoken.len());
            push(spoken[start..end].join("\n"), &mut out);
            if end == spoken.len() {
                break;
            }
            start += STEP;
        }
    }
    out
}

/// A passage that gets a meaning vector.
#[derive(Clone)]
pub struct IndexChunk {
    pub key: String,
    pub meeting_id: i64,
    pub hash: String,
    pub text: String,
}

fn hash(text: &str) -> String {
    Sha256::digest(text.as_bytes()).iter().take(8).map(|b| format!("{b:02x}")).collect()
}

/// All passages of all meetings, with a stable key and a hash that changes when the text does.
pub fn index_chunks(meetings: &[Meeting]) -> Vec<IndexChunk> {
    chunks(meetings)
        .into_iter()
        .map(|c| IndexChunk { key: c.key, meeting_id: meetings[c.meeting].id, hash: hash(&c.text), text: c.text })
        .collect()
}

/// The question's meaning vector and the vectors of the passages, by passage key.
pub struct Semantic {
    pub query: Vec<f32>,
    pub vectors: HashMap<String, Vec<f32>>,
}

/// The most relevant passages for a question, best first. Word matches and, when available,
/// matches by meaning are merged with reciprocal rank fusion.
pub fn retrieve(meetings: &[Meeting], question: &str, max: usize, semantic: Option<&Semantic>) -> Vec<Source> {
    let keys = keys(question);
    if keys.is_empty() && semantic.is_none() {
        return Vec::new();
    }
    let all = chunks(meetings);
    let newer = |a: usize, b: usize| meetings[all[b].meeting].started.cmp(&meetings[all[a].meeting].started);

    let mut keyword: Vec<(f32, usize)> = Vec::new();
    if !keys.is_empty() {
        let total = all.len() as f32;
        let weights: Vec<f32> = keys
            .iter()
            .map(|k| {
                let df = all.iter().filter(|c| c.lower.contains(k.as_str())).count() as f32;
                (1.0 + total / (1.0 + df)).ln()
            })
            .collect();
        keyword = all
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
        keyword.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap_or(std::cmp::Ordering::Equal).then(newer(a.1, b.1)));
    }

    let mut meaning: Vec<(f32, usize)> = semantic
        .map(|s| {
            all.iter()
                .enumerate()
                .filter_map(|(i, c)| s.vectors.get(&c.key).map(|v| (semantic::dot(&s.query, v), i)))
                .collect()
        })
        .unwrap_or_default();
    meaning.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap_or(std::cmp::Ordering::Equal).then(newer(a.1, b.1)));
    meaning.truncate(RANK_DEPTH);

    let order: Vec<usize> = if meaning.is_empty() {
        keyword.into_iter().map(|(_, i)| i).collect()
    } else {
        let mut fused: HashMap<usize, f32> = HashMap::new();
        for (rank, (_, i)) in keyword.iter().enumerate() {
            *fused.entry(*i).or_default() += 1.0 / (RRF_K + rank as f32);
        }
        for (rank, (_, i)) in meaning.iter().enumerate() {
            *fused.entry(*i).or_default() += 1.0 / (RRF_K + rank as f32);
        }
        let mut v: Vec<(f32, usize)> = fused.into_iter().map(|(i, s)| (s, i)).collect();
        v.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap_or(std::cmp::Ordering::Equal).then(newer(a.1, b.1)));
        v.into_iter().map(|(_, i)| i).collect()
    };

    let mut per_meeting = HashMap::new();
    let mut picked = Vec::new();
    for i in order {
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
        let s = retrieve(&library(), "When does the MCP server move to v0.4?", 4, None);
        assert_eq!(s[0].meeting_id, 1);
        assert!(s[0].text.contains("December 4"));
    }

    #[test]
    fn finds_the_german_passage() {
        let s = retrieve(&library(), "Wer baut die Checkbox bis wann ein?", 4, None);
        assert_eq!(s[0].meeting_id, 2);
        assert!(s[0].text.contains("Freitag"));
    }

    #[test]
    fn matches_word_forms() {
        let s = retrieve(&library(), "Welche Verträge gibt es?", 4, None);
        assert!(s.iter().any(|x| x.meeting_id == 2));
    }

    #[test]
    fn returns_nothing_when_no_word_matches() {
        assert!(retrieve(&library(), "What is the weather?", 4, None).is_empty());
        assert!(retrieve(&library(), "Wie ist das?", 4, None).is_empty());
    }

    #[test]
    fn meaning_finds_passages_without_a_shared_word() {
        let lib = library();
        let chunks = index_chunks(&lib);
        // Pretend the model says the question is closest to the German checkout passage.
        let target = chunks.iter().find(|c| c.text.contains("Checkbox")).unwrap().key.clone();
        let mut vectors = HashMap::new();
        for c in &chunks {
            vectors.insert(c.key.clone(), if c.key == target { vec![1.0, 0.0] } else { vec![0.0, 1.0] });
        }
        let sem = Semantic { query: vec![1.0, 0.0], vectors };
        assert!(retrieve(&lib, "Was ist der Liefertermin?", 4, None).is_empty());
        let s = retrieve(&lib, "Was ist der Liefertermin?", 4, Some(&sem));
        assert_eq!(s[0].meeting_id, 2);
        assert!(s[0].text.contains("Checkbox"));
    }

    #[test]
    fn word_matches_and_meaning_are_merged() {
        let lib = library();
        let chunks = index_chunks(&lib);
        let mut vectors = HashMap::new();
        for c in &chunks {
            vectors.insert(c.key.clone(), vec![0.0, 1.0]);
        }
        let sem = Semantic { query: vec![1.0, 0.0], vectors };
        let s = retrieve(&lib, "When does the MCP server move to v0.4?", 4, Some(&sem));
        assert_eq!(s[0].meeting_id, 1);
    }

    #[test]
    fn passage_hashes_change_with_the_text() {
        let a = index_chunks(&library());
        let mut changed = library();
        changed[0].lines.push(line("00:30", "Lena", Speaker::Other, "A new remark."));
        let b = index_chunks(&changed);
        assert_ne!(
            a.iter().map(|c| c.hash.clone()).collect::<Vec<_>>(),
            b.iter().map(|c| c.hash.clone()).collect::<Vec<_>>()
        );
        assert_eq!(a[0].key, b[0].key);
    }

    #[test]
    fn reads_citations_in_order() {
        assert_eq!(cited("The date is December 4 [2]. Also [1, 2] and [9].", 3), vec![2, 1]);
        assert!(cited("No sources here.", 3).is_empty());
    }

    #[test]
    fn numbers_the_sources_in_the_prompt() {
        let s = retrieve(&library(), "beta December", 4, None);
        let p = prompt_input("When is the beta?", &s);
        assert!(p.starts_with("Sources:\n[1] Meeting"));
        assert!(p.ends_with("Question: When is the beta?\n"));
    }
}
