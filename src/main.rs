mod data;
mod ask;
mod engine;
mod models;
mod search;
mod store;
mod summarizer;
mod theme;

use data::{Action, Meeting, Speaker, line};
use engine::{Event, Recording, Source};
use models::{Download, Kind, Progress};
use futures::StreamExt;
use gpui::{
    AnyElement, App, Application, Bounds, FocusHandle, KeyDownEvent, Context, FontWeight, IntoElement, Render, ScrollHandle,
    SharedString, TitlebarOptions, Window, WindowBounds, WindowOptions, div, point, prelude::*, px, relative,
    rgb, size,
};
use store::Store;
use theme::Theme;

const SANS: &str = ".SystemUIFont";
const SERIF: &str = "Iowan Old Style";
const MONO: &str = "Menlo";
const DEFAULT_TITLE: &str = "New note";
const MIN_WORDS_TO_SUMMARIZE: usize = 25;
const STALL_HINT_SECS: u64 = 8;

enum Dl {
    Idle,
    Active { done: u64, total: u64, verifying: bool, started: std::time::Instant, handle: Download },
    Failed(String),
}

struct Qa {
    question: SharedString,
    answer: QaState,
}

enum QaState {
    Pending,
    Done { text: SharedString, sources: Vec<(usize, i64, SharedString)> },
    Failed(SharedString),
}

enum Edit {
    Nothing,
    Changed,
    Submit,
    Escape,
}

/// Applies one key press to a one-line text: typing, backspace (Option: a word, Command: all), paste.
fn edit_text(buf: &mut String, ev: &KeyDownEvent, cx: &App) -> Edit {
    let k = &ev.keystroke;
    let m = k.modifiers;
    match k.key.as_str() {
        "enter" => return Edit::Submit,
        "escape" => return Edit::Escape,
        "backspace" => {
            if m.platform {
                buf.clear();
            } else if m.alt {
                while buf.ends_with(char::is_whitespace) {
                    buf.pop();
                }
                while !buf.is_empty() && !buf.ends_with(char::is_whitespace) {
                    buf.pop();
                }
            } else {
                buf.pop();
            }
            return Edit::Changed;
        }
        "v" if m.platform => {
            return match cx.read_from_clipboard().and_then(|c| c.text()) {
                Some(text) => {
                    buf.push_str(&text.replace(['\n', '\r'], " "));
                    Edit::Changed
                }
                None => Edit::Nothing,
            };
        }
        _ => {}
    }
    if m.control || m.platform {
        return Edit::Nothing;
    }
    match &k.key_char {
        Some(c) if !c.chars().any(char::is_control) => {
            buf.push_str(c);
            Edit::Changed
        }
        _ => Edit::Nothing,
    }
}

struct Notary {
    ask_open: bool,
    search: String,
    focus_search: FocusHandle,
    ask_input: String,
    focus_ask: FocusHandle,
    qa: Vec<Qa>,
    asking: bool,
    ask_scroll: ScrollHandle,
    setup_open: bool,
    downloads: [Dl; 2],
    store: Option<Store>,
    meetings: Vec<Meeting>,
    selected: usize,
    recording: Option<Recording>,
    stopping: bool,
    confirm_delete: Option<i64>,
    status: SharedString,
    scroll: ScrollHandle,
}

fn clock(secs: f32) -> String {
    let s = secs as u32;
    format!("{:02}:{:02}", s / 60, s % 60)
}

impl Notary {
    fn new(cx: &mut Context<Self>) -> Self {
        let mut status: SharedString = "Ready".into();
        let (store, meetings) = match Store::open().and_then(|s| s.load_meetings().map(|m| (s, m))) {
            Ok((s, m)) => (Some(s), m),
            Err(e) => {
                status = format!("Meetings will not be saved: {e:#}").into();
                (None, Vec::new())
            }
        };
        Self {
            ask_open: false,
            search: String::new(),
            focus_search: cx.focus_handle(),
            ask_input: String::new(),
            focus_ask: cx.focus_handle(),
            qa: Vec::new(),
            asking: false,
            ask_scroll: ScrollHandle::new(),
            setup_open: models::find(Kind::Speech).is_none(),
            downloads: [Dl::Idle, Dl::Idle],
            store,
            meetings,
            selected: 0,
            recording: None,
            stopping: false,
            confirm_delete: None,
            status,
            scroll: ScrollHandle::new(),
        }
    }

    fn save<T>(&mut self, result: anyhow::Result<T>) {
        if let Err(e) = result {
            self.status = format!("Could not save: {e:#}").into();
        }
    }

    fn toggle_recording(&mut self, cx: &mut Context<Self>) {
        if let Some(rec) = &self.recording {
            if !self.stopping {
                rec.stop();
                self.stopping = true;
                self.status = "Finishing the last words".into();
                cx.notify();
            }
            return;
        }
        let Some(model) = engine::model_path() else {
            self.setup_open = true;
            self.status = "Download the speech model to start recording".into();
            cx.notify();
            return;
        };

        let started = chrono::Local::now().timestamp();
        let id = match &self.store {
            Some(store) => match store.create_meeting("New note", started) {
                Ok(id) => id,
                Err(e) => {
                    self.status = format!("Could not save: {e:#}").into();
                    cx.notify();
                    return;
                }
            },
            None => self.meetings.iter().map(|m| m.id).max().unwrap_or(0) + 1,
        };
        self.meetings.insert(0, Meeting::new(id, "New note", started, true, Vec::new()));
        self.selected = 0;
        self.confirm_delete = None;

        let (rec, mut events) = engine::start(model);
        self.recording = Some(rec);
        self.status = "Loading the model".into();

        cx.spawn(async move |this, cx| {
            while let Some(event) = events.next().await {
                let alive = this.update(cx, |this, cx| this.apply(id, event, cx));
                if alive.is_err() {
                    break;
                }
            }
        })
        .detach();
        cx.notify();
    }

    fn apply(&mut self, id: i64, event: Event, cx: &mut Context<Self>) {
        match event {
            Event::Loading => self.status = "Loading the model".into(),
            Event::Listening { mic } => {
                self.status = if mic {
                    "Listening to your Mac's audio and your microphone".into()
                } else {
                    "Listening to your Mac's audio (microphone needs macOS 15)".into()
                };
            }
            Event::Segment { source, start, text } => {
                let (name, who) = match source {
                    Source::System => ("Call audio", Speaker::Other),
                    Source::Mic => ("You", Speaker::Me),
                };
                let l = line(&clock(start), name, who, &text);
                let saved = self.store.as_ref().map(|s| s.add_line(id, &l));
                if let Some(m) = self.meetings.iter_mut().find(|m| m.id == id) {
                    m.lines.push(l);
                }
                if let Some(result) = saved {
                    self.save(result);
                }
                self.scroll.scroll_to_bottom();
            }
            Event::Failed(message) => {
                self.recording = None;
                self.stopping = false;
                self.status = message.into();
            }
            Event::Stopped => {
                self.recording = None;
                self.stopping = false;
                if let Some(m) = self.meetings.iter_mut().find(|m| m.id == id) {
                    m.live = false;
                }
                if let Some(store) = &self.store {
                    let result = store.finish(id, chrono::Local::now().timestamp());
                    self.save(result);
                }
                let routine = ["Finishing", "Listening", "Loading"];
                if routine.iter().any(|p| self.status.starts_with(p)) {
                    self.status = "Saved on this Mac".into();
                }
                let words = self.meetings.iter().find(|m| m.id == id).map_or(0, |m| m.word_count());
                if words >= MIN_WORDS_TO_SUMMARIZE && summarizer::model_path().is_some() {
                    self.start_summary(id, cx);
                }
            }
        }
        cx.notify();
    }

    fn start_download(&mut self, kind: Kind, cx: &mut Context<Self>) {
        if matches!(self.downloads[kind as usize], Dl::Active { .. }) {
            return;
        }
        let (handle, mut progress) = models::start(kind);
        self.downloads[kind as usize] = Dl::Active {
            done: 0,
            total: models::spec(kind).size,
            verifying: false,
            started: std::time::Instant::now(),
            handle,
        };
        cx.spawn(async move |this, cx| {
            cx.background_executor().timer(std::time::Duration::from_secs(STALL_HINT_SECS)).await;
            let _ = this.update(cx, |_, cx| cx.notify());
        })
        .detach();
        cx.spawn(async move |this, cx| {
            while let Some(p) = progress.next().await {
                let finished = matches!(p, Progress::Done | Progress::Failed(_) | Progress::Cancelled);
                if this.update(cx, |this, cx| this.apply_progress(kind, p, cx)).is_err() || finished {
                    break;
                }
            }
        })
        .detach();
        cx.notify();
    }

    fn cancel_download(&mut self, kind: Kind, cx: &mut Context<Self>) {
        if let Dl::Active { handle, .. } = &self.downloads[kind as usize] {
            handle.cancel();
            cx.notify();
        }
    }

    fn apply_progress(&mut self, kind: Kind, progress: Progress, cx: &mut Context<Self>) {
        let slot = &mut self.downloads[kind as usize];
        match progress {
            Progress::Bytes { done: d, total: t } => {
                if let Dl::Active { done, total, .. } = slot {
                    *done = d;
                    *total = t;
                }
            }
            Progress::Verifying => {
                if let Dl::Active { verifying, .. } = slot {
                    *verifying = true;
                }
            }
            Progress::Done => {
                *slot = Dl::Idle;
                self.status = format!("{} model installed", models::spec(kind).name).into();
            }
            Progress::Failed(message) => *slot = Dl::Failed(message),
            Progress::Cancelled => *slot = Dl::Idle,
        }
        cx.notify();
    }

    fn start_summary(&mut self, id: i64, cx: &mut Context<Self>) {
        let Some(m) = self.meetings.iter_mut().find(|m| m.id == id) else { return };
        if m.summarizing || m.live {
            return;
        }
        if m.word_count() < MIN_WORDS_TO_SUMMARIZE {
            self.status = "Too little was said to summarize".into();
            cx.notify();
            return;
        }
        let Some(model) = summarizer::model_path() else {
            self.setup_open = true;
            self.status = "Download the summary model to write summaries".into();
            cx.notify();
            return;
        };
        m.summarizing = true;
        let transcript = m.transcript_text();
        self.status = "Writing the summary on this Mac".into();
        let rx = summarizer::summarize(model, transcript);
        cx.spawn(async move |this, cx| {
            let result = rx.await;
            let _ = this.update(cx, |this, cx| this.apply_notes(id, result, cx));
        })
        .detach();
        cx.notify();
    }

    fn apply_notes(
        &mut self,
        id: i64,
        result: Result<anyhow::Result<summarizer::Notes>, futures::channel::oneshot::Canceled>,
        cx: &mut Context<Self>,
    ) {
        let Some(index) = self.meetings.iter().position(|m| m.id == id) else { return };
        self.meetings[index].summarizing = false;
        match result {
            Ok(Ok(notes)) => {
                let rename = self.meetings[index].title.as_ref() == DEFAULT_TITLE;
                let new_title = if rename { notes.title.as_deref() } else { None };
                let ids = match &self.store {
                    Some(store) => store.save_notes(id, new_title, &notes.sections, &notes.actions),
                    None => Ok((0..notes.actions.len() as i64).map(|i| -(i + 1)).collect()),
                };
                let ids = match ids {
                    Ok(ids) => ids,
                    Err(e) => {
                        self.status = format!("Could not save: {e:#}").into();
                        cx.notify();
                        return;
                    }
                };
                let m = &mut self.meetings[index];
                if let Some(t) = new_title {
                    m.title = t.to_string().into();
                }
                m.summary = notes.sections.into_iter().map(|(t, b)| (t.into(), b.into())).collect();
                m.actions = notes
                    .actions
                    .into_iter()
                    .zip(ids)
                    .map(|((label, owner), id)| Action { id, label: label.into(), owner: owner.into(), done: false })
                    .collect();
                self.status = "Summary ready".into();
            }
            Ok(Err(e)) => self.status = format!("{e:#}").into(),
            Err(_) => self.status = "The summary was interrupted".into(),
        }
        cx.notify();
    }

    fn toggle_action(&mut self, meeting_id: i64, action_id: i64, cx: &mut Context<Self>) {
        let Some(a) = self
            .meetings
            .iter_mut()
            .find(|m| m.id == meeting_id)
            .and_then(|m| m.actions.iter_mut().find(|a| a.id == action_id))
        else {
            return;
        };
        a.done = !a.done;
        let done = a.done;
        if let Some(store) = &self.store {
            let result = store.set_action_done(action_id, done);
            self.save(result);
        }
        cx.notify();
    }

    fn delete_selected(&mut self, cx: &mut Context<Self>) {
        let Some(m) = self.meetings.get(self.selected) else { return };
        if m.live {
            return;
        }
        let id = m.id;
        if self.confirm_delete != Some(id) {
            self.confirm_delete = Some(id);
            cx.notify();
            return;
        }
        if let Some(store) = &self.store {
            let result = store.delete_meeting(id);
            self.save(result);
        }
        self.meetings.remove(self.selected);
        self.selected = self.selected.min(self.meetings.len().saturating_sub(1));
        self.confirm_delete = None;
        self.status = "Meeting deleted".into();
        cx.notify();
    }

    fn titlebar(&self, t: &Theme, live: bool) -> impl IntoElement {
        div()
            .h(px(44.))
            .flex_none()
            .flex()
            .items_center()
            .pl(px(86.))
            .pr(px(16.))
            .gap_3()
            .bg(rgb(t.raised))
            .border_b_1()
            .border_color(rgb(t.line))
            .child(div().text_size(px(13.)).text_color(rgb(t.soft)).child("Notary"))
            .child(div().flex_1())
            .when(live, |bar| {
                bar.child(
                    div()
                        .flex()
                        .items_center()
                        .gap_2()
                        .px_2()
                        .py(px(3.))
                        .rounded_full()
                        .bg(rgb(t.rail))
                        .child(div().size(px(7.)).rounded_full().bg(rgb(t.record)))
                        .child(div().text_size(px(12.)).text_color(rgb(t.soft)).child("Recording")),
                )
            })
            .child(
                div()
                    .text_size(px(12.))
                    .text_color(rgb(t.faint))
                    .child("Local \u{b7} whisper.cpp"),
            )
    }

    fn rail(&self, t: &Theme, cx: &mut Context<Self>) -> impl IntoElement {
        let glyph = |s: &'static str, active: bool| {
            div()
                .size(px(36.))
                .flex()
                .items_center()
                .justify_center()
                .rounded(px(10.))
                .text_size(px(15.))
                .when(active, |d| d.bg(rgb(t.raised)).text_color(rgb(t.accent)))
                .when(!active, |d| d.text_color(rgb(t.faint)))
                .child(s)
        };
        div()
            .w(px(56.))
            .flex_none()
            .h_full()
            .flex()
            .flex_col()
            .items_center()
            .gap_1()
            .pt(px(14.))
            .pb(px(12.))
            .bg(rgb(t.rail))
            .border_r_1()
            .border_color(rgb(t.line))
            .child(
                glyph("\u{25a4}", !self.setup_open && !self.ask_open)
                    .id("nav-meetings")
                    .cursor_pointer()
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.setup_open = false;
                        this.ask_open = false;
                        cx.notify();
                    })),
            )
            .child(
                glyph("\u{2315}", self.ask_open && !self.setup_open)
                    .id("nav-ask")
                    .cursor_pointer()
                    .on_click(cx.listener(|this, _, window, cx| {
                        this.setup_open = false;
                        this.ask_open = true;
                        window.focus(&this.focus_ask);
                        cx.notify();
                    })),
            )
            .child(glyph("\u{25a2}", false))
            .child(div().flex_1())
            .child(
                glyph("\u{2699}", self.setup_open)
                    .id("nav-setup")
                    .cursor_pointer()
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.setup_open = true;
                        cx.notify();
                    })),
            )
    }

    fn search_field(&self, t: &Theme, window: &Window, cx: &mut Context<Self>) -> impl IntoElement {
        let focused = self.focus_search.is_focused(window);
        let empty = self.search.is_empty();
        let shown: SharedString = if empty { "Search meetings".into() } else { self.search.clone().into() };
        div()
            .id("search")
            .track_focus(&self.focus_search)
            .on_key_down(cx.listener(|this, ev: &KeyDownEvent, _, cx| this.on_search_key(ev, cx)))
            .on_click(cx.listener(|this, _, window, cx| {
                window.focus(&this.focus_search);
                cx.notify();
            }))
            .flex()
            .items_center()
            .gap_2()
            .px(px(10.))
            .py(px(7.))
            .rounded(px(8.))
            .bg(rgb(t.rail))
            .border_1()
            .border_color(rgb(if focused { t.accent } else { t.rail }))
            .cursor_pointer()
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .flex()
                    .items_center()
                    .child(
                        div()
                            .truncate()
                            .text_size(px(13.))
                            .text_color(rgb(if empty { t.faint } else { t.ink }))
                            .when(!(empty && focused), |d| d.child(shown)),
                    )
                    .when(focused, |d| d.child(div().w(px(1.5)).h(px(15.)).bg(rgb(t.accent)))),
            )
            .when(!empty, |d| {
                d.child(
                    div()
                        .id("search-clear")
                        .text_size(px(14.))
                        .text_color(rgb(t.faint))
                        .cursor_pointer()
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.search.clear();
                            cx.notify();
                        }))
                        .child("\u{d7}"),
                )
            })
    }

    fn on_search_key(&mut self, ev: &KeyDownEvent, cx: &mut Context<Self>) {
        match edit_text(&mut self.search, ev, cx) {
            Edit::Escape => self.search.clear(),
            Edit::Nothing => return,
            _ => {}
        }
        cx.notify();
    }

    fn on_ask_key(&mut self, ev: &KeyDownEvent, cx: &mut Context<Self>) {
        match edit_text(&mut self.ask_input, ev, cx) {
            Edit::Submit => return self.submit_question(cx),
            Edit::Escape => self.ask_input.clear(),
            Edit::Nothing => return,
            Edit::Changed => {}
        }
        cx.notify();
    }

    fn open_meeting(&mut self, id: i64, cx: &mut Context<Self>) {
        if let Some(i) = self.meetings.iter().position(|m| m.id == id) {
            self.selected = i;
            self.ask_open = false;
            self.setup_open = false;
            self.confirm_delete = None;
            cx.notify();
        }
    }

    fn submit_question(&mut self, cx: &mut Context<Self>) {
        let question = self.ask_input.trim().to_string();
        if question.is_empty() || self.asking {
            return;
        }
        let Some(model) = summarizer::model_path() else {
            self.ask_open = false;
            self.setup_open = true;
            self.status = "Download the summary model to ask questions".into();
            cx.notify();
            return;
        };
        self.ask_input.clear();
        let sources = ask::retrieve(&self.meetings, &question, 6);
        let slot = self.qa.len();
        if sources.is_empty() {
            let text = if summarizer::detect_language(&question) == "German" {
                "Dazu habe ich in deinen Meetings nichts gefunden. Versuche andere Worte oder nimm das Meeting zuerst auf."
            } else {
                "I couldn't find anything about that in your meetings. Try other words, or record the meeting first."
            };
            self.qa.push(Qa { question: question.into(), answer: QaState::Done { text: text.into(), sources: Vec::new() } });
            self.ask_scroll.scroll_to_bottom();
            cx.notify();
            return;
        }
        self.qa.push(Qa { question: question.clone().into(), answer: QaState::Pending });
        self.asking = true;
        let refs: Vec<(i64, String)> = sources.iter().map(|s| (s.meeting_id, format!("{} \u{b7} {}", s.title, s.when))).collect();
        let rx = ask::answer(model, question, &sources);
        cx.spawn(async move |this, cx| {
            let result = rx.await;
            let _ = this.update(cx, |this, cx| this.apply_answer(slot, refs, result, cx));
        })
        .detach();
        self.ask_scroll.scroll_to_bottom();
        cx.notify();
    }

    fn apply_answer(
        &mut self,
        slot: usize,
        refs: Vec<(i64, String)>,
        result: Result<anyhow::Result<String>, futures::channel::oneshot::Canceled>,
        cx: &mut Context<Self>,
    ) {
        self.asking = false;
        let answer = match result {
            Ok(Ok(text)) => {
                let mut sources: Vec<(usize, i64, SharedString)> = Vec::new();
                for n in ask::cited(&text, refs.len()) {
                    let (id, label) = &refs[n - 1];
                    sources.push((n, *id, label.clone().into()));
                }
                QaState::Done { text: text.into(), sources }
            }
            Ok(Err(e)) => QaState::Failed(format!("{e:#}").into()),
            Err(_) => QaState::Failed("The answer was interrupted".into()),
        };
        if let Some(qa) = self.qa.get_mut(slot) {
            qa.answer = answer;
        }
        self.ask_scroll.scroll_to_bottom();
        cx.notify();
    }

    fn list(&self, t: &Theme, window: &Window, cx: &mut Context<Self>) -> impl IntoElement {
        let terms = search::terms(&self.search);
        let mut items = div().id("meetings").flex_1().min_h_0().overflow_y_scroll().flex().flex_col().px(px(6.)).pb(px(10.));
        let mut last_day = String::new();
        let mut shown = 0;
        for (i, m) in self.meetings.iter().enumerate() {
            let Some(hit) = search::search(m, &terms) else { continue };
            shown += 1;
            let day = m.day_label();
            if day != last_day {
                items = items.child(
                    div()
                        .px(px(8.))
                        .pt(px(14.))
                        .pb(px(4.))
                        .text_size(px(11.))
                        .text_color(rgb(t.faint))
                        .child(day.to_uppercase()),
                );
                last_day = day;
            }
            let active = i == self.selected && !self.ask_open && !self.setup_open;
            let sub: SharedString = match hit.snippet {
                Some(s) => s.into(),
                None => m.sub(),
            };
            items = items.child(
                div()
                    .id(("meeting", i))
                    .flex()
                    .flex_col()
                    .gap(px(2.))
                    .px(px(10.))
                    .py(px(9.))
                    .rounded(px(8.))
                    .cursor_pointer()
                    .when(active, |d| d.bg(rgb(t.accent_soft)))
                    .hover(|d| d.bg(rgb(if active { t.accent_soft } else { t.rail })))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.selected = i;
                        this.ask_open = false;
                        this.setup_open = false;
                        this.confirm_delete = None;
                        cx.notify();
                    }))
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap_2()
                            .child(
                                div()
                                    .flex_1()
                                    .truncate()
                                    .text_size(px(13.5))
                                    .font_weight(FontWeight::MEDIUM)
                                    .text_color(rgb(t.ink))
                                    .child(m.title.clone()),
                            )
                            .child(
                                div()
                                    .font_family(MONO)
                                    .text_size(px(11.))
                                    .text_color(rgb(t.faint))
                                    .child(m.time()),
                            ),
                    )
                    .child(div().truncate().text_size(px(12.)).text_color(rgb(t.soft)).child(sub)),
            );
        }
        if shown == 0 && !terms.is_empty() {
            items = items.child(
                div()
                    .px(px(10.))
                    .py(px(16.))
                    .text_size(px(13.))
                    .line_height(px(20.))
                    .text_color(rgb(t.faint))
                    .child("No meeting matches. Try fewer or other words."),
            );
        }

        div()
            .w(px(280.))
            .flex_none()
            .h_full()
            .flex()
            .flex_col()
            .bg(rgb(t.raised))
            .border_r_1()
            .border_color(rgb(t.line))
            .child(
                div()
                    .px(px(12.))
                    .py(px(12.))
                    .border_b_1()
                    .border_color(rgb(t.line))
                    .child(self.search_field(t, window, cx)),
            )
            .child(items)
    }

    fn ask_body(&self, t: &Theme, window: &Window, cx: &mut Context<Self>) -> AnyElement {
        let focused = self.focus_ask.is_focused(window);
        let mut column = div().w_full().max_w(px(660.)).flex().flex_col().gap(px(26.));
        if self.qa.is_empty() {
            column = column
                .child(
                    div()
                        .font_family(SERIF)
                        .text_size(px(26.))
                        .text_color(rgb(t.ink))
                        .child("Ask your meetings"),
                )
                .child(
                    div()
                        .text_size(px(14.))
                        .line_height(px(22.))
                        .text_color(rgb(t.soft))
                        .child("Ask in English or German. Notary looks through your saved meetings and answers from what was said. Everything is worked out on this Mac."),
                );
            for (i, example) in ["What did we decide about the MCP server?", "Wer macht bis Freitag was?"].into_iter().enumerate() {
                column = column.child(
                    div()
                        .id(("example", i))
                        .px(px(14.))
                        .py(px(10.))
                        .rounded(px(10.))
                        .border_1()
                        .border_color(rgb(t.line))
                        .bg(rgb(t.raised))
                        .cursor_pointer()
                        .text_size(px(13.5))
                        .text_color(rgb(t.soft))
                        .hover(|h| h.text_color(rgb(t.accent)))
                        .on_click(cx.listener(move |this, _, window, cx| {
                            this.ask_input = example.to_string();
                            window.focus(&this.focus_ask);
                            cx.notify();
                        }))
                        .child(example),
                );
            }
        }
        for (slot, qa) in self.qa.iter().enumerate() {
            let mut block = div().flex().flex_col().gap(px(10.)).child(
                div().font_family(SERIF).text_size(px(17.)).text_color(rgb(t.ink)).child(qa.question.clone()),
            );
            match &qa.answer {
                QaState::Pending => {
                    block = block.child(div().text_size(px(13.5)).text_color(rgb(t.faint)).child("Reading your meetings on this Mac"));
                }
                QaState::Failed(message) => {
                    block = block.child(
                        div().text_size(px(13.5)).line_height(px(21.)).text_color(rgb(t.record)).child(message.clone()),
                    );
                }
                QaState::Done { text, sources } => {
                    block = block.child(
                        div().text_size(px(14.)).line_height(px(22.)).text_color(rgb(t.ink)).child(text.clone()),
                    );
                    if !sources.is_empty() {
                        let mut chips = div().flex().flex_wrap().gap(px(8.));
                        for (n, id, label) in sources {
                            let id = *id;
                            chips = chips.child(
                                div()
                                    .id(("source", slot * 100 + *n))
                                    .px(px(10.))
                                    .py(px(4.))
                                    .rounded(px(7.))
                                    .bg(rgb(t.accent_soft))
                                    .cursor_pointer()
                                    .text_size(px(12.))
                                    .text_color(rgb(t.accent))
                                    .on_click(cx.listener(move |this, _, _, cx| this.open_meeting(id, cx)))
                                    .child(format!("[{n}] {label}")),
                            );
                        }
                        block = block.child(chips);
                    }
                }
            }
            column = column.child(block);
        }

        let empty = self.ask_input.is_empty();
        let shown: SharedString = if empty { "Ask something about your meetings".into() } else { self.ask_input.clone().into() };
        let bar = div()
            .w_full()
            .max_w(px(660.))
            .flex()
            .items_center()
            .gap(px(10.))
            .px(px(14.))
            .py(px(10.))
            .rounded(px(12.))
            .bg(rgb(t.raised))
            .border_1()
            .border_color(rgb(if focused { t.accent } else { t.line }))
            .child(
                div()
                    .id("ask-input")
                    .track_focus(&self.focus_ask)
                    .on_key_down(cx.listener(|this, ev: &KeyDownEvent, _, cx| this.on_ask_key(ev, cx)))
                    .on_click(cx.listener(|this, _, window, cx| {
                        window.focus(&this.focus_ask);
                        cx.notify();
                    }))
                    .flex_1()
                    .min_w_0()
                    .flex()
                    .items_center()
                    .cursor_pointer()
                    .child(
                        div()
                            .truncate()
                            .text_size(px(14.))
                            .text_color(rgb(if empty { t.faint } else { t.ink }))
                            .when(!(empty && focused), |d| d.child(shown)),
                    )
                    .when(focused, |d| d.child(div().w(px(1.5)).h(px(16.)).bg(rgb(t.accent)))),
            )
            .child(
                div()
                    .id("ask-go")
                    .flex_none()
                    .px(px(14.))
                    .py(px(6.))
                    .rounded(px(8.))
                    .text_size(px(12.5))
                    .font_weight(FontWeight::SEMIBOLD)
                    .when(self.asking || empty, |d| d.bg(rgb(t.rail)).text_color(rgb(t.faint)))
                    .when(!self.asking && !empty, |d| {
                        d.cursor_pointer()
                            .bg(rgb(t.accent))
                            .text_color(rgb(t.raised))
                            .on_click(cx.listener(|this, _, _, cx| this.submit_question(cx)))
                    })
                    .child(if self.asking { "Thinking" } else { "Ask" }),
            );

        div()
            .flex_1()
            .min_h_0()
            .flex()
            .flex_col()
            .child(
                div()
                    .id("qa")
                    .track_scroll(&self.ask_scroll)
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .flex()
                    .flex_col()
                    .items_center()
                    .px(px(40.))
                    .py(px(36.))
                    .child(column),
            )
            .child(div().flex_none().flex().justify_center().px(px(40.)).pb(px(18.)).child(bar))
            .into_any_element()
    }

    fn model_row(&self, kind: Kind, t: &Theme, cx: &mut Context<Self>) -> impl IntoElement {
        let spec = models::spec(kind);
        let installed = models::find(kind).is_some();
        let state = &self.downloads[kind as usize];
        let active = matches!(state, Dl::Active { .. });

        let button = |id: &'static str, label: &'static str, primary: bool| {
            div()
                .id(id)
                .flex_none()
                .px(px(14.))
                .py(px(6.))
                .rounded(px(8.))
                .cursor_pointer()
                .text_size(px(12.5))
                .font_weight(FontWeight::SEMIBOLD)
                .when(primary, |d| d.bg(rgb(t.accent)).text_color(rgb(t.raised)))
                .when(!primary, |d| d.bg(rgb(t.rail)).text_color(rgb(t.soft)))
                .child(label)
        };

        let action: AnyElement = if installed {
            div().text_size(px(12.5)).text_color(rgb(t.accent)).child("Installed").into_any_element()
        } else if active {
            button(if kind == Kind::Speech { "cancel-speech" } else { "cancel-summary" }, "Cancel", false)
                .on_click(cx.listener(move |this, _, _, cx| this.cancel_download(kind, cx)))
                .into_any_element()
        } else {
            let label = if matches!(state, Dl::Failed(_)) { "Retry" } else { "Download" };
            button(if kind == Kind::Speech { "get-speech" } else { "get-summary" }, label, true)
                .on_click(cx.listener(move |this, _, _, cx| this.start_download(kind, cx)))
                .into_any_element()
        };

        let mut card = div()
            .flex()
            .flex_col()
            .gap(px(12.))
            .p(px(16.))
            .rounded(px(12.))
            .bg(rgb(t.raised))
            .border_1()
            .border_color(rgb(t.line))
            .child(
                div()
                    .flex()
                    .items_start()
                    .gap(px(16.))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .flex()
                            .flex_col()
                            .gap(px(4.))
                            .child(
                                div()
                                    .flex()
                                    .items_baseline()
                                    .gap(px(8.))
                                    .child(
                                        div()
                                            .font_family(SERIF)
                                            .text_size(px(16.))
                                            .text_color(rgb(t.ink))
                                            .child(spec.name),
                                    )
                                    .child(
                                        div()
                                            .font_family(MONO)
                                            .text_size(px(11.))
                                            .text_color(rgb(t.faint))
                                            .child(models::format_bytes(spec.size)),
                                    ),
                            )
                            .child(
                                div()
                                    .text_size(px(12.5))
                                    .line_height(px(19.))
                                    .text_color(rgb(t.soft))
                                    .child(spec.blurb),
                            ),
                    )
                    .child(action),
            );

        if let Dl::Active { done, total, verifying, started, .. } = state {
            let fraction = if *verifying { 1.0 } else { (*done as f32 / *total as f32).clamp(0.0, 1.0) };
            let text = if *verifying {
                "Checking the download".to_string()
            } else {
                format!("{} of {}", models::format_bytes(*done), models::format_bytes(*total))
            };
            card = card
                .child(
                    div()
                        .h(px(6.))
                        .w_full()
                        .rounded_full()
                        .bg(rgb(t.rail))
                        .child(div().h_full().w(relative(fraction)).rounded_full().bg(rgb(t.accent))),
                )
                .child(div().text_size(px(12.)).text_color(rgb(t.faint)).child(text));
            if *done == 0 && !*verifying && started.elapsed().as_secs() >= STALL_HINT_SECS {
                card = card.child(
                    div()
                        .text_size(px(12.5))
                        .line_height(px(19.))
                        .text_color(rgb(t.soft))
                        .child("Waiting for a connection. If a firewall such as LuLu or Little Snitch asks, allow Notary to connect to huggingface.co."),
                );
            }
        }
        if let Dl::Failed(message) = state {
            card = card.child(div().text_size(px(12.5)).line_height(px(19.)).text_color(rgb(t.record)).child(message.clone()));
        }
        card
    }

    fn setup_body(&self, t: &Theme, cx: &mut Context<Self>) -> AnyElement {
        let ready = models::find(Kind::Speech).is_some();
        let busy = self.downloads.iter().any(|d| matches!(d, Dl::Active { .. }));
        let mut column = div()
            .w_full()
            .max_w(px(600.))
            .flex()
            .flex_col()
            .gap(px(16.))
            .child(
                div()
                    .font_family(SERIF)
                    .text_size(px(26.))
                    .text_color(rgb(t.ink))
                    .child("Set up Notary"),
            )
            .child(
                div()
                    .text_size(px(14.))
                    .line_height(px(22.))
                    .text_color(rgb(t.soft))
                    .child("Notary works without an internet connection. These models are downloaded once from huggingface.co and stay on this Mac. Nothing is downloaded until you press Download."),
            );
        for kind in models::ALL {
            column = column.child(self.model_row(kind, t, cx));
        }
        column = column.child(
            div()
                .flex()
                .items_center()
                .gap(px(12.))
                .child(
                    div()
                        .id("setup-done")
                        .px(px(16.))
                        .py(px(8.))
                        .rounded(px(9.))
                        .when(ready && !busy, |d| {
                            d.cursor_pointer()
                                .bg(rgb(t.accent))
                                .text_color(rgb(t.raised))
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.setup_open = false;
                                    cx.notify();
                                }))
                        })
                        .when(!(ready && !busy), |d| d.bg(rgb(t.rail)).text_color(rgb(t.faint)))
                        .text_size(px(13.))
                        .font_weight(FontWeight::SEMIBOLD)
                        .child("Start using Notary"),
                )
                .child(div().text_size(px(12.5)).text_color(rgb(t.faint)).child(if ready {
                    "The speech model is installed."
                } else {
                    "Download the speech model to begin."
                })),
        );
        div()
            .id("setup")
            .flex_1()
            .min_h_0()
            .overflow_y_scroll()
            .flex()
            .flex_col()
            .items_center()
            .px(px(40.))
            .py(px(48.))
            .child(column)
            .into_any_element()
    }

    fn empty_body(&self, t: &Theme) -> AnyElement {
        div()
            .flex_1()
            .min_h_0()
            .flex()
            .flex_col()
            .items_center()
            .justify_center()
            .gap(px(8.))
            .px(px(40.))
            .child(
                div()
                    .font_family(SERIF)
                    .text_size(px(24.))
                    .text_color(rgb(t.ink))
                    .child("No meetings yet"),
            )
            .child(
                div()
                    .max_w(px(420.))
                    .text_size(px(14.))
                    .line_height(px(22.))
                    .text_color(rgb(t.soft))
                    .child("Press Record, then play a video or join a call. The words appear here and stay on this Mac."),
            )
            .into_any_element()
    }

    fn meeting_body(&self, m: &Meeting, t: &Theme, cx: &mut Context<Self>) -> AnyElement {
        let mut transcript = div()
            .id("transcript")
            .track_scroll(&self.scroll)
            .flex_1()
            .min_w_0()
            .h_full()
            .overflow_y_scroll()
            .flex()
            .flex_col()
            .px(px(24.))
            .py(px(8.));
        for l in &m.lines {
            let (who_color, note) = match l.who {
                Speaker::Me => (t.accent, false),
                Speaker::Other => (t.ink, false),
                Speaker::Note => (t.soft, true),
            };
            let text = div()
                .flex_1()
                .min_w_0()
                .text_size(px(14.))
                .line_height(px(22.))
                .text_color(rgb(t.ink))
                .when(note, |d| {
                    d.font_family(SERIF)
                        .italic()
                        .bg(rgb(t.note))
                        .border_l_2()
                        .border_color(rgb(t.focus))
                        .px(px(12.))
                        .py(px(6.))
                })
                .child(l.text.clone());
            transcript = transcript.child(
                div()
                    .flex()
                    .gap(px(14.))
                    .py(px(9.))
                    .border_b_1()
                    .border_color(rgb(t.line))
                    .child(
                        div()
                            .w(px(50.))
                            .flex_none()
                            .pt(px(3.))
                            .font_family(MONO)
                            .text_size(px(11.))
                            .text_color(rgb(t.faint))
                            .child(l.ts.clone()),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .flex()
                            .flex_col()
                            .gap(px(2.))
                            .child(
                                div()
                                    .text_size(px(12.))
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .text_color(rgb(who_color))
                                    .child(l.name.clone()),
                            )
                            .child(text),
                    ),
            );
        }
        if m.lines.is_empty() {
            let hint = if m.live {
                "Nothing yet. Play a video or join a call and the words appear here."
            } else {
                "No speech was heard in this recording."
            };
            transcript = transcript.child(
                div().py(px(24.)).text_size(px(13.5)).text_color(rgb(t.faint)).child(hint),
            );
        }

        let heading = |s: &'static str| {
            div().font_family(SERIF).text_size(px(14.)).text_color(rgb(t.ink)).child(s)
        };
        let meeting_id = m.id;
        let can_summarize = !m.live && !m.summarizing && m.word_count() >= MIN_WORDS_TO_SUMMARIZE;
        let summarize_button = div()
            .id("summarize")
            .px(px(9.))
            .py(px(3.))
            .rounded(px(6.))
            .cursor_pointer()
            .bg(rgb(t.rail))
            .text_size(px(11.5))
            .text_color(rgb(t.soft))
            .hover(|h| h.text_color(rgb(t.accent)))
            .on_click(cx.listener(move |this, _, _, cx| this.start_summary(meeting_id, cx)))
            .child(if m.summary.is_empty() { "Summarize" } else { "Regenerate" });
        let mut summary = div().flex().flex_col().gap(px(10.)).child(
            div()
                .flex()
                .items_center()
                .child(heading("Summary"))
                .child(div().flex_1())
                .when(can_summarize, |d| d.child(summarize_button)),
        );
        if m.summarizing || m.summary.is_empty() {
            let placeholder = if m.summarizing {
                "Writing the summary on this Mac. This takes a few seconds."
            } else if m.live {
                "The summary is written when you stop recording."
            } else if m.word_count() < MIN_WORDS_TO_SUMMARIZE {
                "Too little was said to summarize."
            } else {
                "Press Summarize to write notes from this transcript."
            };
            summary = summary.child(
                div().text_size(px(13.)).line_height(px(20.)).text_color(rgb(t.faint)).child(placeholder),
            );
        }
        for (lead, body) in m.summary.iter().filter(|_| !m.summarizing) {
            summary = summary.child(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(2.))
                    .when(lead.as_ref() != "Summary", |d| {
                        d.child(
                            div()
                                .text_size(px(12.))
                                .font_weight(FontWeight::SEMIBOLD)
                                .text_color(rgb(t.ink))
                                .child(lead.clone()),
                        )
                    })
                    .child(
                        div()
                            .text_size(px(13.5))
                            .line_height(px(21.))
                            .text_color(rgb(t.soft))
                            .child(body.clone()),
                    ),
            );
        }

        let mut actions = div().flex().flex_col().gap(px(8.)).child(heading("Next steps"));
        for a in m.actions.iter().filter(|_| !m.summarizing) {
            let action_id = a.id;
            actions = actions.child(
                div()
                    .flex()
                    .items_start()
                    .gap(px(10.))
                    .px(px(10.))
                    .py(px(9.))
                    .rounded(px(8.))
                    .bg(rgb(t.paper))
                    .border_1()
                    .border_color(rgb(t.line))
                    .child(
                        div()
                            .id(("action", a.id as usize))
                            .mt(px(2.))
                            .size(px(15.))
                            .flex_none()
                            .cursor_pointer()
                            .on_click(cx.listener(move |this, _, _, cx| this.toggle_action(meeting_id, action_id, cx)))
                            .rounded(px(4.))
                            .border_1()
                            .border_color(rgb(if a.done { t.accent } else { t.faint }))
                            .when(a.done, |d| d.bg(rgb(t.accent))),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .text_size(px(13.))
                            .text_color(rgb(if a.done { t.faint } else { t.ink }))
                            .when(a.done, |d| d.line_through())
                            .child(a.label.clone()),
                    )
                    .child(
                        div()
                            .px(px(7.))
                            .py(px(1.))
                            .rounded_full()
                            .bg(rgb(t.rail))
                            .text_size(px(11.))
                            .text_color(rgb(t.soft))
                            .child(a.owner.clone()),
                    ),
            );
        }

        let side = div()
            .id("side")
            .w(px(380.))
            .flex_none()
            .h_full()
            .overflow_y_scroll()
            .flex()
            .flex_col()
            .gap(px(20.))
            .px(px(20.))
            .py(px(18.))
            .bg(rgb(t.raised))
            .border_l_1()
            .border_color(rgb(t.line))
            .child(summary)
            .when(!m.actions.is_empty(), |d| d.child(actions));

        let confirming = self.confirm_delete == Some(m.id);
        let head = div()
            .flex()
            .items_start()
            .gap(px(16.))
            .px(px(24.))
            .pt(px(18.))
            .pb(px(14.))
            .border_b_1()
            .border_color(rgb(t.line))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .flex()
                    .flex_col()
                    .gap(px(6.))
                    .child(
                        div()
                            .font_family(SERIF)
                            .text_size(px(22.))
                            .text_color(rgb(t.ink))
                            .child(m.title.clone()),
                    )
                    .child(
                        div()
                            .font_family(MONO)
                            .text_size(px(11.5))
                            .text_color(rgb(t.faint))
                            .child(m.meta()),
                    ),
            )
            .when(!m.live, |d| {
                d.child(
                    div()
                        .id("delete")
                        .flex_none()
                        .px(px(10.))
                        .py(px(5.))
                        .rounded(px(7.))
                        .cursor_pointer()
                        .text_size(px(12.))
                        .when(confirming, |b| b.bg(rgb(t.record)).text_color(rgb(0xFFFFFF)))
                        .when(!confirming, |b| {
                            b.bg(rgb(t.rail)).text_color(rgb(t.soft)).hover(|h| h.text_color(rgb(t.record)))
                        })
                        .on_click(cx.listener(|this, _, _, cx| this.delete_selected(cx)))
                        .child(if confirming { "Click again to delete" } else { "Delete" }),
                )
            });

        div()
            .flex_1()
            .min_h_0()
            .flex()
            .flex_col()
            .child(head)
            .child(div().flex_1().min_h_0().flex().child(transcript).child(side))
            .into_any_element()
    }

    fn footbar(&self, t: &Theme, cx: &mut Context<Self>) -> impl IntoElement {
        let recording = self.recording.is_some();
        let label = match (recording, self.stopping) {
            (true, true) => "Stopping",
            (true, false) => "Stop",
            (false, _) => "Record",
        };
        let record_button = div()
            .id("record")
            .flex()
            .items_center()
            .gap_2()
            .px(px(12.))
            .py(px(6.))
            .rounded_full()
            .cursor_pointer()
            .bg(rgb(if recording { t.record } else { t.rail }))
            .text_size(px(12.5))
            .font_weight(FontWeight::SEMIBOLD)
            .text_color(rgb(if recording { 0xFFFFFF } else { t.record }))
            .on_click(cx.listener(|this, _, _, cx| this.toggle_recording(cx)))
            .child(
                div()
                    .size(px(9.))
                    .when(recording, |d| d.rounded(px(2.)).bg(rgb(0xFFFFFF)))
                    .when(!recording, |d| d.rounded_full().bg(rgb(t.record))),
            )
            .child(label);
        div()
            .flex_none()
            .flex()
            .items_center()
            .gap_3()
            .px(px(16.))
            .py(px(10.))
            .bg(rgb(t.raised))
            .border_t_1()
            .border_color(rgb(t.line))
            .child(record_button)
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .truncate()
                    .text_size(px(12.5))
                    .text_color(rgb(t.faint))
                    .child(self.status.clone()),
            )
    }

    fn view(&self, t: &Theme, window: &Window, cx: &mut Context<Self>) -> impl IntoElement {
        let body = if self.setup_open {
            self.setup_body(t, cx)
        } else if self.ask_open {
            self.ask_body(t, window, cx)
        } else {
            match self.meetings.get(self.selected) {
                Some(m) => self.meeting_body(m, t, cx),
                None => self.empty_body(t),
            }
        };
        div()
            .flex_1()
            .min_w_0()
            .h_full()
            .flex()
            .flex_col()
            .child(body)
            .child(self.footbar(t, cx))
    }
}

impl Render for Notary {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = Theme::for_appearance(window.appearance());
        div()
            .size_full()
            .flex()
            .flex_col()
            .bg(rgb(t.paper))
            .text_color(rgb(t.ink))
            .font_family(SANS)
            .child(self.titlebar(&t, self.recording.is_some()))
            .child(
                div()
                    .flex_1()
                    .min_h_0()
                    .flex()
                    .child(self.rail(&t, cx))
                    .child(self.list(&t, window, cx))
                    .child(self.view(&t, window, cx)),
            )
    }
}

fn main() {
    Application::new().run(|cx: &mut App| {
        let bounds = Bounds::centered(None, size(px(1180.), px(760.)), cx);
        cx.open_window(
            WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(bounds)),
                titlebar: Some(TitlebarOptions {
                    title: Some("Notary".into()),
                    appears_transparent: true,
                    traffic_light_position: Some(point(px(16.), px(15.))),
                }),
                window_min_size: Some(size(px(860.), px(520.))),
                ..Default::default()
            },
            |_, cx| cx.new(|cx| Notary::new(cx)),
        )
        .unwrap();
        cx.activate(true);
    });
}
