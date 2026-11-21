//! Runs the notary-llm helper on a transcript and turns its output into a title, summary and next steps.

use anyhow::{Context, Result, bail};
use futures::channel::oneshot;
use std::io::Write;
use std::path::PathBuf;
use std::process::{Command, Stdio};

pub struct Notes {
    pub title: Option<String>,
    pub sections: Vec<(String, String)>,
    pub actions: Vec<(String, String)>,
}

pub fn model_path() -> Option<PathBuf> {
    crate::models::find(crate::models::Kind::Summary)
}

fn helper_path() -> Result<PathBuf> {
    let exe = std::env::current_exe()?;
    let helper = exe.with_file_name("notary-llm");
    if helper.exists() {
        Ok(helper)
    } else {
        bail!("the summary helper (notary-llm) is missing next to the app")
    }
}

/// Writes notes for `transcript` on a background thread; the result arrives on the returned channel.
pub fn summarize(model: PathBuf, transcript: String, language: Option<String>) -> oneshot::Receiver<Result<Notes>> {
    let (tx, rx) = oneshot::channel();
    std::thread::spawn(move || {
        let _ = tx.send(run(&model, &transcript, language.as_deref()));
    });
    rx
}

/// Picks the language the notes are written in by counting common German and English words.
pub fn detect_language(text: &str) -> &'static str {
    const GERMAN: &[&str] = &[
        "der", "die", "das", "und", "ist", "nicht", "wir", "ich", "ein", "eine", "zu", "den", "mit", "auf", "für", "dass", "auch", "dann",
        "wird", "bis", "noch", "sie", "es", "von", "im", "ja", "aber", "oder", "dem", "kann",
    ];
    const ENGLISH: &[&str] = &[
        "the", "and", "is", "we", "to", "of", "that", "in", "it", "you", "for", "on", "with", "this", "be", "are", "will", "can", "have",
        "not", "so", "but", "i", "if", "by",
    ];
    let (mut de, mut en) = (0, 0);
    for w in text.split(|c: char| !c.is_alphabetic()).filter(|w| !w.is_empty()) {
        let w = w.to_lowercase();
        de += GERMAN.contains(&w.as_str()) as usize;
        en += ENGLISH.contains(&w.as_str()) as usize;
    }
    if de > en { "German" } else { "English" }
}

/// Runs the helper program in `mode` ("notes" or "ask") and returns what it wrote.
pub fn run_helper(model: &PathBuf, language: &str, mode: &str, input: &str) -> Result<String> {
    let mut child = Command::new(helper_path()?)
        .arg(model)
        .arg(language)
        .arg(mode)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .context("could not start the language model helper")?;
    child
        .stdin
        .take()
        .context("no stdin")?
        .write_all(input.as_bytes())
        .context("could not hand the text to the language model helper")?;
    let output = child.wait_with_output()?;
    if !output.status.success() {
        let err = String::from_utf8_lossy(&output.stderr);
        let last = err.lines().rev().find(|l| !l.trim().is_empty()).unwrap_or("unknown error");
        bail!("the language model failed: {last}");
    }
    Ok(String::from_utf8_lossy(&output.stdout).to_string())
}

fn run(model: &PathBuf, transcript: &str, language: Option<&str>) -> Result<Notes> {
    let notes = parse(&run_helper(model, language.unwrap_or_else(|| detect_language(transcript)), "notes", transcript)?);
    if notes.sections.is_empty() && notes.actions.is_empty() {
        bail!("the model returned nothing usable");
    }
    Ok(notes)
}

fn heading(name: &str) -> Option<&'static str> {
    match name.trim().to_uppercase().as_str() {
        "SUMMARY" => Some("Summary"),
        "DECISIONS" => Some("Decisions"),
        "NEXT STEPS" => Some("Next steps"),
        _ => None,
    }
}

fn is_placeholder(line: &str) -> bool {
    let t = line.trim_start_matches(['-', '*', '\u{2022}', ' ']).trim_end_matches(['.', ' ']).to_lowercase();
    matches!(t.as_str(), "none" | "keine" | "keiner" | "nichts" | "n/a" | "no decisions" | "keine entscheidungen" | "")
}

pub fn parse(text: &str) -> Notes {
    let mut title = None;
    let mut sections: Vec<(String, String)> = Vec::new();
    let mut actions = Vec::new();
    let mut current: Option<&'static str> = None;
    let mut body: Vec<String> = Vec::new();

    let flush = |current: &mut Option<&'static str>, body: &mut Vec<String>, sections: &mut Vec<(String, String)>| {
        if let Some(name) = current.take() {
            let joined = body.join("\n").trim().to_string();
            if !joined.is_empty() && name != "Next steps" {
                sections.push((name.to_string(), joined));
            }
        }
        body.clear();
    };

    for raw in text.lines() {
        let line = raw.trim();
        if let Some(rest) = line.strip_prefix("##") {
            flush(&mut current, &mut body, &mut sections);
            current = heading(rest.trim_start_matches('#'));
        } else if let Some(rest) = line.strip_prefix('#') {
            let t = rest.trim();
            let t = t.strip_prefix("TITLE:").unwrap_or(t).trim();
            if !t.is_empty() {
                title = Some(t.trim_matches('*').trim().to_string());
            }
        } else if current == Some("Next steps") {
            let item = line.trim_start_matches(['-', '*', '\u{2022}']).trim();
            if item.is_empty() {
                continue;
            }
            let (label, owner) = match item.rsplit_once('|') {
                Some((l, o)) => (l.trim(), o.trim()),
                None => (item, "Unassigned"),
            };
            if !label.is_empty() {
                actions.push((label.to_string(), if owner.is_empty() { "Unassigned".into() } else { owner.to_string() }));
            }
        } else if !line.is_empty() && !is_placeholder(line) {
            let item = line.strip_prefix("- ").or_else(|| line.strip_prefix("* "));
            body.push(match item {
                Some(i) => format!("\u{2022} {}", i.trim()),
                None => line.to_string(),
            });
        }
    }
    flush(&mut current, &mut body, &mut sections);

    Notes { title, sections, actions }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_the_expected_format() {
        let n = parse("# TITLE: Launch planning\n## SUMMARY\nShip on Mac first.\n## DECISIONS\n- Cut MCP if late\n- Mac only\n## NEXT STEPS\n- Send the DMG | Fabian\n- Write the README note | Lena\n");
        assert_eq!(n.title.as_deref(), Some("Launch planning"));
        assert_eq!(n.sections.len(), 2);
        assert_eq!(n.sections[0], ("Summary".into(), "Ship on Mac first.".into()));
        assert_eq!(n.sections[1].1, "\u{2022} Cut MCP if late\n\u{2022} Mac only");
        assert_eq!(n.actions, vec![("Send the DMG".into(), "Fabian".into()), ("Write the README note".into(), "Lena".into())]);
    }

    #[test]
    fn detects_german_and_english() {
        assert_eq!(detect_language("Wir schicken die Version am Montag raus, und das ist nicht schwer."), "German");
        assert_eq!(detect_language("We will send the build on Monday and it is not hard."), "English");
        assert_eq!(detect_language("Ok so the beta ships on Monday, aber wir haben noch Zeit."), "English");
    }

    #[test]
    fn drops_placeholder_decisions() {
        let n = parse("# TITLE: X\n## SUMMARY\nText.\n## DECISIONS\n- None\n");
        assert_eq!(n.sections.len(), 1);
        let n = parse("# TITLE: X\n## SUMMARY\nText.\n## DECISIONS\n- Keine.\n");
        assert_eq!(n.sections.len(), 1);
    }

    #[test]
    fn tolerates_missing_owner_and_sections() {
        let n = parse("# Kurz\n## SUMMARY\nNur eine Zusammenfassung.\n## NEXT STEPS\n- Link schicken\n");
        assert_eq!(n.title.as_deref(), Some("Kurz"));
        assert_eq!(n.sections.len(), 1);
        assert_eq!(n.actions, vec![("Link schicken".into(), "Unassigned".into())]);
    }
}
