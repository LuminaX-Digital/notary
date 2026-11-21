//! A one-line text field: cursor, selection, word jumps, copy and paste.

use gpui::KeyDownEvent;

#[derive(Clone, Debug, PartialEq)]
pub enum Action {
    Insert(String),
    Left { select: bool, word: bool, line: bool },
    Right { select: bool, word: bool, line: bool },
    Home { select: bool },
    End { select: bool },
    Backspace { word: bool, line: bool },
    Delete { word: bool },
    SelectAll,
    Copy,
    Cut,
    Paste(String),
    Submit,
    Escape,
}

#[derive(Debug, PartialEq)]
pub enum Outcome {
    Nothing,
    Changed,
    Submit,
    Escape,
    /// The text to put on the clipboard (the field may have changed too).
    Copied(String),
}

/// Translates a key press into an action. `clipboard` is read only for Command-V.
pub fn action_for(ev: &KeyDownEvent, clipboard: impl FnOnce() -> Option<String>) -> Option<Action> {
    let k = &ev.keystroke;
    let m = k.modifiers;
    match k.key.as_str() {
        "enter" => return Some(Action::Submit),
        "escape" => return Some(Action::Escape),
        "left" => return Some(Action::Left { select: m.shift, word: m.alt, line: m.platform }),
        "right" => return Some(Action::Right { select: m.shift, word: m.alt, line: m.platform }),
        "home" | "up" => return Some(Action::Home { select: m.shift }),
        "end" | "down" => return Some(Action::End { select: m.shift }),
        "backspace" => return Some(Action::Backspace { word: m.alt, line: m.platform }),
        "delete" => return Some(Action::Delete { word: m.alt }),
        "a" if m.platform => return Some(Action::SelectAll),
        "c" if m.platform => return Some(Action::Copy),
        "x" if m.platform => return Some(Action::Cut),
        "v" if m.platform => return clipboard().map(Action::Paste),
        _ => {}
    }
    if m.control || m.platform {
        return None;
    }
    match &k.key_char {
        Some(c) if !c.chars().any(char::is_control) => Some(Action::Insert(c.clone())),
        _ => None,
    }
}

#[derive(Default)]
pub struct TextInput {
    text: String,
    cursor: usize,
    anchor: usize,
}

impl TextInput {
    pub fn text(&self) -> &str {
        &self.text
    }

    pub fn is_empty(&self) -> bool {
        self.text.is_empty()
    }

    pub fn clear(&mut self) {
        *self = Self::default();
    }

    /// Replaces the text and puts the cursor at the end.
    pub fn set(&mut self, text: &str) {
        self.text = text.to_string();
        self.cursor = self.text.len();
        self.anchor = self.cursor;
    }

    fn has_selection(&self) -> bool {
        self.cursor != self.anchor
    }

    fn start(&self) -> usize {
        self.cursor.min(self.anchor)
    }

    fn end(&self) -> usize {
        self.cursor.max(self.anchor)
    }

    fn prev(&self, i: usize) -> usize {
        self.text[..i].char_indices().next_back().map_or(0, |(p, _)| p)
    }

    fn next(&self, i: usize) -> usize {
        self.text[i..].chars().next().map_or(i, |c| i + c.len_utf8())
    }

    fn word_left(&self, mut i: usize) -> usize {
        while i > 0 && self.text[..i].ends_with(char::is_whitespace) {
            i = self.prev(i);
        }
        while i > 0 && !self.text[..i].ends_with(char::is_whitespace) {
            i = self.prev(i);
        }
        i
    }

    fn word_right(&self, mut i: usize) -> usize {
        while i < self.text.len() && self.text[i..].starts_with(char::is_whitespace) {
            i = self.next(i);
        }
        while i < self.text.len() && !self.text[i..].starts_with(char::is_whitespace) {
            i = self.next(i);
        }
        i
    }

    fn move_to(&mut self, target: usize, select: bool) {
        self.cursor = target;
        if !select {
            self.anchor = target;
        }
    }

    fn delete_range(&mut self, from: usize, to: usize) {
        self.text.replace_range(from..to, "");
        self.cursor = from;
        self.anchor = from;
    }

    fn replace_selection(&mut self, with: &str) {
        let (s, e) = (self.start(), self.end());
        self.text.replace_range(s..e, with);
        self.cursor = s + with.len();
        self.anchor = self.cursor;
    }

    pub fn apply(&mut self, action: Action) -> Outcome {
        match action {
            Action::Insert(s) => self.replace_selection(&s),
            Action::Paste(s) => self.replace_selection(&s.replace(['\n', '\r'], " ")),
            Action::Left { select, word, line } => {
                let target = if line {
                    0
                } else if word {
                    self.word_left(self.cursor)
                } else if self.has_selection() && !select {
                    self.start()
                } else {
                    self.prev(self.cursor)
                };
                self.move_to(target, select);
            }
            Action::Right { select, word, line } => {
                let target = if line {
                    self.text.len()
                } else if word {
                    self.word_right(self.cursor)
                } else if self.has_selection() && !select {
                    self.end()
                } else {
                    self.next(self.cursor)
                };
                self.move_to(target, select);
            }
            Action::Home { select } => self.move_to(0, select),
            Action::End { select } => self.move_to(self.text.len(), select),
            Action::Backspace { word, line } => {
                if self.has_selection() {
                    self.delete_range(self.start(), self.end());
                } else if line {
                    self.delete_range(0, self.cursor);
                } else if word {
                    self.delete_range(self.word_left(self.cursor), self.cursor);
                } else if self.cursor > 0 {
                    self.delete_range(self.prev(self.cursor), self.cursor);
                } else {
                    return Outcome::Nothing;
                }
            }
            Action::Delete { word } => {
                if self.has_selection() {
                    self.delete_range(self.start(), self.end());
                } else if word {
                    self.delete_range(self.cursor, self.word_right(self.cursor));
                } else if self.cursor < self.text.len() {
                    self.delete_range(self.cursor, self.next(self.cursor));
                } else {
                    return Outcome::Nothing;
                }
            }
            Action::SelectAll => {
                self.anchor = 0;
                self.cursor = self.text.len();
            }
            Action::Copy => {
                return if self.has_selection() {
                    Outcome::Copied(self.text[self.start()..self.end()].to_string())
                } else {
                    Outcome::Nothing
                };
            }
            Action::Cut => {
                if !self.has_selection() {
                    return Outcome::Nothing;
                }
                let copied = self.text[self.start()..self.end()].to_string();
                self.delete_range(self.start(), self.end());
                return Outcome::Copied(copied);
            }
            Action::Submit => return Outcome::Submit,
            Action::Escape => return Outcome::Escape,
        }
        Outcome::Changed
    }

    /// The text split for drawing: before the selection, the selection, after it, and whether the
    /// cursor sits at the start of the selection. Long texts are cut to about `max_chars` around the cursor.
    pub fn parts(&self, max_chars: usize) -> Parts {
        let chars: Vec<(usize, char)> = self.text.char_indices().collect();
        let total = chars.len();
        let cursor_char = chars.iter().position(|(i, _)| *i >= self.cursor).unwrap_or(total);
        let (window_start, window_end) = if total <= max_chars {
            (0, total)
        } else {
            let end = (cursor_char + 4).min(total).max(max_chars.min(total));
            (end.saturating_sub(max_chars), end)
        };
        let byte = |c: usize| chars.get(c).map_or(self.text.len(), |(i, _)| *i);
        let (ws, we) = (byte(window_start), byte(window_end));
        let (s, e) = (self.start().clamp(ws, we), self.end().clamp(ws, we));
        Parts {
            before: format!("{}{}", if window_start > 0 { "\u{2026}" } else { "" }, &self.text[ws..s]),
            selected: self.text[s..e].to_string(),
            after: format!("{}{}", &self.text[e..we], if window_end < total { "\u{2026}" } else { "" }),
            caret_first: self.cursor <= self.anchor,
        }
    }
}

pub struct Parts {
    pub before: String,
    pub selected: String,
    pub after: String,
    pub caret_first: bool,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn typed(text: &str) -> TextInput {
        let mut t = TextInput::default();
        t.apply(Action::Insert(text.to_string()));
        t
    }

    fn left() -> Action {
        Action::Left { select: false, word: false, line: false }
    }

    #[test]
    fn types_and_deletes() {
        let mut t = typed("hallo");
        t.apply(Action::Backspace { word: false, line: false });
        assert_eq!(t.text(), "hall");
        assert_eq!(t.apply(Action::Backspace { word: false, line: false }), Outcome::Changed);
    }

    #[test]
    fn inserts_at_the_cursor() {
        let mut t = typed("hllo");
        t.apply(left());
        t.apply(left());
        t.apply(left());
        t.apply(Action::Insert("a".into()));
        assert_eq!(t.text(), "hallo");
    }

    #[test]
    fn handles_umlauts_as_single_characters() {
        let mut t = typed("Käse");
        t.apply(left());
        t.apply(left());
        t.apply(Action::Backspace { word: false, line: false });
        assert_eq!(t.text(), "Kse");
        t.apply(Action::Insert("ö".into()));
        assert_eq!(t.text(), "Kös" .to_string() + "e");
    }

    #[test]
    fn shift_extends_the_selection_and_typing_replaces_it() {
        let mut t = typed("hello world");
        for _ in 0..5 {
            t.apply(Action::Left { select: true, word: false, line: false });
        }
        t.apply(Action::Insert("there".into()));
        assert_eq!(t.text(), "hello there");
    }

    #[test]
    fn select_all_copy_and_cut() {
        let mut t = typed("one two");
        t.apply(Action::SelectAll);
        assert_eq!(t.apply(Action::Copy), Outcome::Copied("one two".into()));
        assert_eq!(t.apply(Action::Cut), Outcome::Copied("one two".into()));
        assert!(t.is_empty());
        assert_eq!(t.apply(Action::Copy), Outcome::Nothing);
    }

    #[test]
    fn paste_replaces_the_selection_and_flattens_lines() {
        let mut t = typed("abc");
        t.apply(Action::SelectAll);
        t.apply(Action::Paste("x\ny".into()));
        assert_eq!(t.text(), "x y");
    }

    #[test]
    fn word_jumps_and_word_delete() {
        let mut t = typed("das ist ein test");
        t.apply(Action::Left { select: false, word: true, line: false });
        t.apply(Action::Left { select: false, word: true, line: false });
        t.apply(Action::Insert("X".into()));
        assert_eq!(t.text(), "das ist Xein test");
        let mut t = typed("das ist ein test");
        t.apply(Action::Backspace { word: true, line: false });
        assert_eq!(t.text(), "das ist ein ");
        t.apply(Action::Backspace { word: false, line: true });
        assert!(t.is_empty());
    }

    #[test]
    fn forward_delete_and_home_end() {
        let mut t = typed("abc");
        t.apply(Action::Home { select: false });
        t.apply(Action::Delete { word: false });
        assert_eq!(t.text(), "bc");
        t.apply(Action::End { select: false });
        t.apply(Action::Insert("d".into()));
        assert_eq!(t.text(), "bcd");
    }

    #[test]
    fn arrows_collapse_a_selection_to_its_edge() {
        let mut t = typed("abcd");
        t.apply(Action::SelectAll);
        t.apply(left());
        t.apply(Action::Insert("X".into()));
        assert_eq!(t.text(), "Xabcd");
    }

    #[test]
    fn submit_and_escape_are_reported() {
        let mut t = typed("x");
        assert_eq!(t.apply(Action::Submit), Outcome::Submit);
        assert_eq!(t.apply(Action::Escape), Outcome::Escape);
    }

    #[test]
    fn long_text_is_cut_around_the_cursor() {
        let t = typed(&"a".repeat(100));
        let p = t.parts(20);
        assert!(p.before.starts_with('\u{2026}'));
        assert!(p.before.chars().count() <= 22);
        let mut t = typed(&"b".repeat(100));
        t.apply(Action::Home { select: false });
        let p = t.parts(20);
        assert!(p.after.ends_with('\u{2026}') && !p.before.starts_with('\u{2026}'));
    }

    #[test]
    fn parts_mark_the_selection() {
        let mut t = typed("hello world");
        for _ in 0..5 {
            t.apply(Action::Left { select: true, word: false, line: false });
        }
        let p = t.parts(50);
        assert_eq!((p.before.as_str(), p.selected.as_str(), p.after.as_str()), ("hello ", "world", ""));
        assert!(p.caret_first, "marking leftwards leaves the cursor at the left edge");
    }
}
