//! Searching saved meetings: titles, transcripts, summaries and next steps.

use crate::data::Meeting;

pub struct Hit {
    /// The line that matched, shortened around the match, when the match is in the transcript.
    pub snippet: Option<String>,
}

pub fn terms(query: &str) -> Vec<String> {
    query.split_whitespace().map(str::to_lowercase).collect()
}

/// A meeting matches when every search word appears somewhere in it, in any order.
pub fn search(meeting: &Meeting, terms: &[String]) -> Option<Hit> {
    if terms.is_empty() {
        return Some(Hit { snippet: None });
    }
    let title = meeting.title.to_lowercase();
    let summary: String = meeting
        .summary
        .iter()
        .map(|(t, b)| format!("{t} {b}"))
        .chain(meeting.actions.iter().map(|a| format!("{} {}", a.label, a.owner)))
        .collect::<Vec<_>>()
        .join("\n")
        .to_lowercase();
    let lines: Vec<String> = meeting.lines.iter().map(|l| l.text.to_lowercase()).collect();

    let found = |term: &String| title.contains(term) || summary.contains(term) || lines.iter().any(|l| l.contains(term));
    if !terms.iter().all(found) {
        return None;
    }

    let snippet = terms.iter().find_map(|term| {
        lines
            .iter()
            .position(|l| l.contains(term))
            .map(|i| around(&meeting.lines[i].text, term, 90))
    });
    Some(Hit { snippet })
}

/// Cuts `text` to about `width` characters around the first occurrence of `term`.
fn around(text: &str, term: &str, width: usize) -> String {
    let lower = text.to_lowercase();
    let chars: Vec<char> = text.chars().collect();
    let Some(byte_at) = lower.find(term) else {
        return text.chars().take(width).collect();
    };
    let at = lower[..byte_at].chars().count();
    if lower.chars().count() != chars.len() {
        return text.chars().take(width).collect();
    }
    let start = at.saturating_sub(width / 3);
    let end = (start + width).min(chars.len());
    let mut out: String = chars[start..end].iter().collect();
    if start > 0 {
        out.insert(0, '\u{2026}');
    }
    if end < chars.len() {
        out.push('\u{2026}');
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::data::{Speaker, line};

    fn meeting(title: &str, lines: &[&str]) -> Meeting {
        let lines = lines.iter().map(|t| line("00:00", "Lena", Speaker::Other, t)).collect();
        Meeting::new(1, title, 0, false, lines)
    }

    #[test]
    fn finds_words_in_the_transcript_ignoring_case() {
        let m = meeting("New note", &["The beta ships on December 4."]);
        let hit = search(&m, &terms("DECEMBER")).unwrap();
        assert!(hit.snippet.unwrap().contains("December"));
    }

    #[test]
    fn every_word_must_match() {
        let m = meeting("New note", &["The beta ships on Monday."]);
        assert!(search(&m, &terms("beta monday")).is_some());
        assert!(search(&m, &terms("beta tuesday")).is_none());
    }

    #[test]
    fn matches_the_title_and_handles_umlauts() {
        let m = meeting("AVV Besprechung", &["Das Kontrollkästchen ist wichtig."]);
        assert!(search(&m, &terms("besprechung")).is_some());
        assert!(search(&m, &terms("kontrollkästchen")).is_some());
        assert!(search(&m, &terms("KONTROLLKÄSTCHEN")).is_some());
    }

    #[test]
    fn an_empty_search_matches_everything() {
        assert!(search(&meeting("x", &[]), &[]).is_some());
    }

    #[test]
    fn long_lines_are_cut_around_the_match() {
        let text = format!("{} needle {}", "a".repeat(200), "b".repeat(200));
        let m = meeting("x", &[&text]);
        let s = search(&m, &terms("needle")).unwrap().snippet.unwrap();
        assert!(s.contains("needle") && s.chars().count() < 120);
    }
}
