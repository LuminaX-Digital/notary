//! Turning a meeting into text people can copy or save: notes, transcript, or both as Markdown.

use crate::data::{Meeting, Speaker};

fn bullets(body: &str) -> String {
    body.lines()
        .map(|l| match l.strip_prefix('\u{2022}') {
            Some(rest) => format!("- {}", rest.trim()),
            None => l.to_string(),
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// Title, date, summary, decisions and next steps.
pub fn notes_markdown(m: &Meeting) -> String {
    let mut out = format!("# {}\n\n*{}*\n", m.title, m.meta().split(" \u{b7} audio").next().unwrap_or(""));
    for (title, body) in &m.summary {
        out.push_str(&format!("\n## {}\n\n{}\n", title, bullets(body)));
    }
    if !m.actions.is_empty() {
        out.push_str("\n## Next steps\n\n");
        for a in &m.actions {
            out.push_str(&format!("- [{}] {} ({})\n", if a.done { "x" } else { " " }, a.label, a.owner));
        }
    }
    out
}

/// The transcript as Markdown lines, with the user's own notes marked as quotes.
pub fn transcript_markdown(m: &Meeting) -> String {
    let mut out = String::new();
    for l in &m.lines {
        match l.who {
            Speaker::Note => out.push_str(&format!("> **{} \u{b7} your note:** {}\n\n", l.ts, l.text)),
            _ => out.push_str(&format!("**{} {}:** {}\n\n", l.ts, l.name, l.text)),
        }
    }
    out
}

/// Plain text for pasting into a chat or an email.
pub fn transcript_text(m: &Meeting) -> String {
    m.lines
        .iter()
        .map(|l| match l.who {
            Speaker::Note => format!("[{}] Note: {}", l.ts, l.text),
            _ => format!("[{}] {}: {}", l.ts, l.name, l.text),
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// Notes followed by the transcript: what "Export" saves.
pub fn full_markdown(m: &Meeting) -> String {
    let mut out = notes_markdown(m);
    if !m.lines.is_empty() {
        out.push_str("\n## Transcript\n\n");
        out.push_str(&transcript_markdown(m));
    }
    out
}

/// A safe file name such as "2026-11-21 Launch planning.md".
pub fn file_name(m: &Meeting) -> String {
    let date = m.meta().split(" \u{b7} ").next().map(|d| d.to_string()).unwrap_or_default();
    let title: String = m.title.chars().map(|c| if c.is_alphanumeric() || " -._".contains(c) { c } else { ' ' }).collect();
    let title = title.split_whitespace().collect::<Vec<_>>().join(" ");
    let day = chrono::DateTime::from_timestamp(m.started, 0)
        .map(|d| d.with_timezone(&chrono::Local).format("%Y-%m-%d").to_string())
        .unwrap_or(date);
    format!("{day} {}.md", if title.is_empty() { "Meeting" } else { &title })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::data::{Action, line};

    fn meeting() -> Meeting {
        let mut m = Meeting::new(
            1,
            "Launch / planning: v0.3",
            1_795_000_000,
            false,
            vec![
                line("00:12", "Lena", Speaker::Other, "Keep it small."),
                line("00:20", "Fabian", Speaker::Me, "Agreed."),
                line("00:30", "You wrote", Speaker::Note, "Send the DMG."),
            ],
        );
        m.summary = vec![
            ("Summary".into(), "Ship a small first release.".into()),
            ("Decisions".into(), "\u{2022} Mac only\n\u{2022} Cut MCP if late".into()),
        ];
        m.actions = vec![
            Action { id: 1, label: "Send the DMG".into(), owner: "Fabian".into(), done: true },
            Action { id: 2, label: "Write the README".into(), owner: "Lena".into(), done: false },
        ];
        m
    }

    #[test]
    fn notes_have_title_summary_decisions_and_a_checklist() {
        let n = notes_markdown(&meeting());
        assert!(n.starts_with("# Launch / planning: v0.3\n"));
        assert!(n.contains("## Summary\n\nShip a small first release."));
        assert!(n.contains("- Mac only\n- Cut MCP if late"));
        assert!(n.contains("- [x] Send the DMG (Fabian)"));
        assert!(n.contains("- [ ] Write the README (Lena)"));
    }

    #[test]
    fn transcript_marks_notes_and_names_speakers() {
        let t = transcript_markdown(&meeting());
        assert!(t.contains("**00:12 Lena:** Keep it small."));
        assert!(t.contains("> **00:30 \u{b7} your note:** Send the DMG."));
        assert!(transcript_text(&meeting()).contains("[00:20] Fabian: Agreed."));
    }

    #[test]
    fn full_export_has_both_parts() {
        let f = full_markdown(&meeting());
        assert!(f.contains("## Next steps") && f.contains("## Transcript"));
    }

    #[test]
    fn file_names_are_safe() {
        let n = file_name(&meeting());
        assert!(n.ends_with("Launch planning v0.3.md"), "{n}");
        assert!(!n.contains('/') && !n.contains(':'));
    }
}
