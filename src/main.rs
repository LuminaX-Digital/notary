mod data;
mod ask;
mod audio;
mod calendar;
mod callnames;
mod calls;
mod engine;
mod export;
mod input;
mod mcp;
mod models;
mod native;
mod search;
mod semantic;
mod settings;
mod store;
mod summarizer;
mod theme;
mod updates;
mod watch;

use data::{Action, Folder, Meeting, Speaker, line};
use engine::{Event, Recording, Source};
use input::{Outcome, TextInput};
use models::{Download, Kind, Progress};
use settings::{SUMMARY_LANGUAGES, Settings, Spoken};
use futures::StreamExt;
use gpui::{
    AnyElement, App, Application, Bounds, ClipboardItem, FocusHandle, KeyDownEvent, Context, FontWeight, IntoElement, Render, ScrollHandle,
    SharedString, TitlebarOptions, Window, WindowBounds, WindowOptions, div, point, prelude::*, px, relative,
    rgb, size,
};
use store::Store;
use theme::Theme;

const SANS: &str = ".SystemUIFont";
const SERIF: &str = "Iowan Old Style";
const MONO: &str = "Menlo";
const DEFAULT_TITLE: &str = "New note";
const TRASH_DAYS: i64 = 30;
const MIN_WORDS_TO_SUMMARIZE: usize = 25;
const STALL_HINT_SECS: u64 = 8;

enum UpdateState {
    Idle,
    Checking,
    Current,
    Available { version: SharedString, url: SharedString },
    Failed(SharedString),
}

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

/// Draws a text field's content with its cursor and selection.
fn field_text(input: &TextInput, placeholder: &'static str, max_chars: usize, size: f32, focused: bool, t: &Theme) -> AnyElement {
    let caret = || div().flex_none().w(px(1.5)).h(px(size + 3.)).bg(rgb(t.accent));
    if input.is_empty() {
        return div()
            .flex()
            .items_center()
            .text_size(px(size))
            .text_color(rgb(t.faint))
            .when(focused, |d| d.child(caret()))
            .when(!focused, |d| d.child(placeholder))
            .into_any_element();
    }
    let p = input.parts(max_chars);
    let has_selection = !p.selected.is_empty();
    div()
        .flex()
        .items_center()
        .whitespace_nowrap()
        .text_size(px(size))
        .text_color(rgb(t.ink))
        .child(p.before)
        .when(focused && p.caret_first, |d| d.child(caret()))
        .when(has_selection, |d| d.child(div().bg(rgb(t.accent_soft)).child(p.selected)))
        .when(focused && !p.caret_first, |d| d.child(caret()))
        .child(p.after)
        .into_any_element()
}

struct Notary {
    settings: Settings,
    name_input: TextInput,
    focus_name: FocusHandle,
    words_input: TextInput,
    focus_words: FocusHandle,
    folders: Vec<Folder>,
    folder_filter: Option<i64>,
    adding_folder: bool,
    new_folder: TextInput,
    focus_folder: FocusHandle,
    confirm_folder_delete: bool,
    moving: bool,
    renaming: bool,
    rename_input: TextInput,
    focus_rename: FocusHandle,
    note_input: TextInput,
    focus_note: FocusHandle,
    update_state: UpdateState,
    ask_open: bool,
    search: TextInput,
    focus_search: FocusHandle,
    ask_input: TextInput,
    focus_ask: FocusHandle,
    qa: Vec<Qa>,
    asking: bool,
    ask_scroll: ScrollHandle,
    setup_open: bool,
    downloads: [Dl; 3],
    store: Option<Store>,
    meetings: Vec<Meeting>,
    selected: usize,
    recording: Option<Recording>,
    stopping: bool,
    confirm_delete: Option<i64>,
    status: SharedString,
    scroll: ScrollHandle,
    detector: calls::Detector,
    pending_bound: Option<calls::CallApp>,
    call_bound: Option<calls::CallApp>,
    rec_started_ms: i64,
    timeline: Option<callnames::Timeline>,
    call_watch: Option<watch::CallWatch>,
    last_speech: std::time::Instant,
    offer: Option<calls::CallApp>,
    ask_stop: bool,
    warned_accessibility: bool,
    perms: Option<native::Status>,
    perms_busy: bool,
    events: Vec<native::CalendarEvent>,
    pending_event: Option<String>,
    event_people: Vec<String>,
    trash_open: bool,
    trash: Vec<(i64, String, i64, i64)>,
    find_open: bool,
    find_input: TextInput,
    focus_find: FocusHandle,
    instruction_input: TextInput,
    focus_instruction: FocusHandle,
    player: Option<(i64, watch::Player)>,
    play_gen: u64,
    paused_at: Option<std::time::Instant>,
    importing: bool,
    menu_bar: Option<watch::MenuBar>,
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
        if let Some(s) = &store {
            let cutoff = chrono::Local::now().timestamp() - TRASH_DAYS * 86_400;
            for id in s.purge_trash_before(cutoff).unwrap_or_default() {
                audio::delete_audio(id);
            }
        }
        let trash = store.as_ref().and_then(|s| s.load_trash().ok()).unwrap_or_default();
        let folders = store.as_ref().and_then(|s| s.load_folders().ok()).unwrap_or_default();
        let settings = Settings::load();
        let onboarded = settings.onboarded;
        let mut name_input = TextInput::default();
        name_input.set(&settings.your_name);
        let mut words_input = TextInput::default();
        words_input.set(&settings.custom_words);
        Self::watch_microphone(cx);
        Self::watch_calendar(cx);
        Self::start_menu_bar(cx);
        Self::load_permissions(cx);
        Self {
            settings,
            name_input,
            focus_name: cx.focus_handle(),
            words_input,
            focus_words: cx.focus_handle(),
            folders,
            folder_filter: None,
            adding_folder: false,
            new_folder: TextInput::default(),
            focus_folder: cx.focus_handle(),
            confirm_folder_delete: false,
            moving: false,
            renaming: false,
            rename_input: TextInput::default(),
            focus_rename: cx.focus_handle(),
            note_input: TextInput::default(),
            focus_note: cx.focus_handle(),
            update_state: UpdateState::Idle,
            ask_open: false,
            search: TextInput::default(),
            focus_search: cx.focus_handle(),
            ask_input: TextInput::default(),
            focus_ask: cx.focus_handle(),
            qa: Vec::new(),
            asking: false,
            ask_scroll: ScrollHandle::new(),
            setup_open: models::find(Kind::Speech).is_none() || !onboarded,
            downloads: [Dl::Idle, Dl::Idle, Dl::Idle],
            store,
            meetings,
            selected: 0,
            recording: None,
            stopping: false,
            confirm_delete: None,
            status,
            scroll: ScrollHandle::new(),
            detector: calls::Detector::default(),
            pending_bound: None,
            call_bound: None,
            rec_started_ms: 0,
            timeline: None,
            call_watch: None,
            last_speech: std::time::Instant::now(),
            offer: None,
            ask_stop: false,
            warned_accessibility: false,
            perms: None,
            perms_busy: false,
            events: Vec::new(),
            pending_event: None,
            event_people: Vec::new(),
            trash_open: false,
            trash,
            find_open: false,
            find_input: TextInput::default(),
            focus_find: cx.focus_handle(),
            instruction_input: TextInput::default(),
            focus_instruction: cx.focus_handle(),
            player: None,
            play_gen: 0,
            paused_at: None,
            importing: false,
            menu_bar: None,
        }
    }

    /// Looks every few seconds at which call apps use the microphone.
    fn watch_microphone(cx: &mut Context<Self>) {
        let mut apps = watch::mic_poll();
        cx.spawn(async move |this, cx| {
            while let Some(on_mic) = apps.next().await {
                if this.update(cx, |this, cx| this.on_mic(on_mic, cx)).is_err() {
                    break;
                }
            }
        })
        .detach();
    }

    /// Reads the next day of calendar events now and every few minutes, when the Calendar permission is given.
    fn watch_calendar(cx: &mut Context<Self>) {
        cx.spawn(async move |this, cx| {
            loop {
                let from = chrono::Local::now() - chrono::Duration::minutes(15);
                let to = from + chrono::Duration::hours(26);
                let fmt = |d: chrono::DateTime<chrono::Local>| d.to_rfc3339_opts(chrono::SecondsFormat::Secs, false);
                let (from, to) = (fmt(from), fmt(to));
                let rx = native::in_background(move || {
                    if native::status()?.calendar != "fullAccess" {
                        anyhow::bail!("calendar-not-authorized");
                    }
                    native::events(&from, &to)
                });
                if let Ok(Ok(events)) = rx.await {
                    if this.update(cx, |this, cx| {
                        this.events = events;
                        cx.notify();
                    }).is_err() {
                        break;
                    }
                }
                cx.background_executor().timer(std::time::Duration::from_secs(240)).await;
            }
        })
        .detach();
    }

    /// A menu bar item to open Notary and start or stop recording from anywhere.
    fn start_menu_bar(cx: &mut Context<Self>) {
        let Some((bar, mut commands)) = watch::menu_bar() else { return };
        cx.spawn(async move |this, cx| {
            let _ = this.update(cx, |this, _| this.menu_bar = Some(bar));
            while let Some(command) = commands.next().await {
                let alive = this.update(cx, |this, cx| match command.as_str() {
                    "open" => cx.activate(true),
                    "toggle" => {
                        if this.recording.is_some() {
                            this.toggle_recording(cx);
                        } else {
                            this.toggle_recording(cx);
                            cx.activate(true);
                        }
                    }
                    "quit" => cx.quit(),
                    _ => {}
                });
                if alive.is_err() {
                    break;
                }
            }
        })
        .detach();
    }

    fn sync_menu_bar(&self) {
        if let Some(bar) = &self.menu_bar {
            bar.set_state(match (&self.recording, self.paused_at.is_some()) {
                (None, _) => "idle",
                (Some(_), true) => "paused",
                (Some(_), false) => "recording",
            });
        }
    }

    /// Events to show and to match: not declined, not in a hidden calendar.
    fn shown_events(&self) -> Vec<&native::CalendarEvent> {
        calendar::visible(&self.events, &self.settings.hidden_calendars, self.settings.hide_events_without_link)
    }

    /// The call app a calendar event's link belongs to, so its speaker names can be read.
    fn app_for_event(e: &native::CalendarEvent) -> Option<calls::CallApp> {
        let link = calendar::meeting_link(e)?;
        if link.contains("zoom.") {
            Some(calls::CallApp::Zoom)
        } else if link.contains("teams.") {
            Some(calls::CallApp::Teams)
        } else if link.contains("meet.google.") {
            Some(calls::CallApp::Browser)
        } else {
            None
        }
    }

    fn on_mic(&mut self, on_mic: Vec<calls::CallApp>, cx: &mut Context<Self>) {
        let rec = self.recording.as_ref().map(|_| calls::Recording { started_ms: self.rec_started_ms, bound: self.call_bound });
        let poll = calls::Poll {
            now_ms: chrono::Local::now().timestamp_millis(),
            on_mic: &on_mic,
            recording: rec.as_ref(),
            quiet_secs: self.last_speech.elapsed().as_secs(),
        };
        let rules = calls::Rules { offer_to_record: self.settings.offer_to_record, stop_when_call_ends: self.settings.stop_when_call_ends };
        for action in self.detector.step(&poll, &rules) {
            match action {
                calls::Action::Offer(app) => self.offer = Some(app),
                calls::Action::Bind(app) => {
                    self.call_bound = Some(app);
                    if let Some(id) = self.meetings.first().filter(|m| m.live).map(|m| m.id) {
                        self.begin_call_watch(app, id, cx);
                    }
                }
                calls::Action::AutoStop => {
                    if self.recording.is_some() && !self.stopping {
                        self.status = "The call ended".into();
                        self.toggle_recording(cx);
                    }
                }
                calls::Action::AskStop => self.ask_stop = true,
            }
        }
        cx.notify();
    }

    /// Starts reading who speaks from the window of the call app.
    fn begin_call_watch(&mut self, app: calls::CallApp, id: i64, cx: &mut Context<Self>) {
        let zero = self.rec_started_ms;
        self.timeline.get_or_insert_with(|| callnames::Timeline::new(zero));
        let Some((watcher, mut readings)) = watch::call_watch(app) else { return };
        self.call_watch = Some(watcher);
        cx.spawn(async move |this, cx| {
            while let Some(reading) = readings.next().await {
                if this.update(cx, |this, cx| this.on_reading(id, app, reading, cx)).is_err() {
                    break;
                }
            }
        })
        .detach();
    }

    fn on_reading(&mut self, id: i64, app: calls::CallApp, reading: callnames::Reading, cx: &mut Context<Self>) {
        if self.recording.is_none() {
            return;
        }
        if reading.error.as_deref() == Some("not-trusted") {
            if !self.warned_accessibility {
                self.warned_accessibility = true;
                self.status = "Speaker names need the Accessibility permission for Notary".into();
                cx.notify();
            }
            return;
        }
        let sample = callnames::read(app, &reading);
        let Some(timeline) = self.timeline.as_mut() else { return };
        let before = timeline.seen.len();
        timeline.push(sample);
        if timeline.seen.len() != before {
            let mine = self.settings.your_name.clone();
            let mut people = self.event_people.clone();
            for p in timeline.seen.iter().filter(|p| **p != mine) {
                if !people.contains(p) {
                    people.push(p.clone());
                }
            }
            if let Some(m) = self.meetings.iter_mut().find(|m| m.id == id) {
                m.people = people.clone();
            }
            if let Some(store) = &self.store {
                let result = store.set_people(id, &people);
                self.save(result);
            }
            cx.notify();
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

        let (rec, mut events) = engine::start(
            model,
            engine::Config {
                language: self.settings.whisper_language().to_string(),
                prompt: self.settings.prompt_words(),
                audio_dir: self.settings.keep_audio.then(|| audio::meeting_dir(id)),
            },
        );
        self.recording = Some(rec);
        self.status = "Loading the model".into();
        self.rec_started_ms = chrono::Local::now().timestamp_millis();
        self.detector.recording_started();
        self.last_speech = std::time::Instant::now();
        self.offer = None;
        self.ask_stop = false;
        self.timeline = None;
        self.call_watch = None;
        self.warned_accessibility = false;
        let now_s = chrono::Local::now().timestamp();
        let wanted = self.pending_event.take();
        let event = {
            let shown = self.shown_events();
            match wanted {
                Some(id) => shown.iter().copied().find(|e| e.id == id),
                None => calendar::match_event(&shown, now_s),
            }
            .cloned()
        };
        self.event_people.clear();
        if let Some(e) = &event {
            if self.pending_bound.is_none() {
                self.pending_bound = Self::app_for_event(e);
            }
            let me = e.me.clone().unwrap_or_default();
            self.event_people = e
                .attendees
                .iter()
                .filter(|a| a.name != self.settings.your_name && (me.is_empty() || a.email != me))
                .map(|a| if a.name.is_empty() { a.email.split('@').next().unwrap_or("").to_string() } else { a.name.clone() })
                .filter(|n| !n.is_empty())
                .take(50)
                .collect();
            if let Some(m) = self.meetings.iter_mut().find(|m| m.id == id) {
                m.title = e.title.clone().into();
                m.people = self.event_people.clone();
            }
            if let Some(store) = &self.store {
                let renamed = store.rename_meeting(id, &e.title);
                let people = store.set_people(id, &self.event_people);
                self.save(renamed);
                self.save(people);
            }
        }
        self.call_bound = self.pending_bound.take();
        if let Some(app) = self.call_bound {
            self.begin_call_watch(app, id, cx);
        }

        cx.spawn(async move |this, cx| {
            while let Some(event) = events.next().await {
                let alive = this.update(cx, |this, cx| this.apply(id, event, cx));
                if alive.is_err() {
                    break;
                }
            }
        })
        .detach();
        self.sync_menu_bar();
        cx.notify();
    }

    fn apply(&mut self, id: i64, event: Event, cx: &mut Context<Self>) {
        match event {
            Event::Loading => self.status = "Loading the model".into(),
            Event::Listening { mic } => {
                let now = chrono::Local::now().timestamp_millis();
                self.rec_started_ms = now;
                if let Some(t) = self.timeline.as_mut() {
                    t.set_zero(now);
                }
                self.status = if mic {
                    "Listening to your Mac's audio and your microphone".into()
                } else {
                    "Listening to your Mac's audio (microphone needs macOS 15)".into()
                };
            }
            Event::Segment { source, start, end, text } => {
                self.last_speech = std::time::Instant::now();
                let (name, who) = match source {
                    Source::System if self.importing => ("Audio".to_string(), Speaker::Other),
                    Source::System => {
                        let named = self.timeline.as_ref().and_then(|t| {
                            let mut mine = vec![self.settings.your_name.clone()];
                            mine.extend(t.me());
                            t.who((start * 1000.0) as i64, (end * 1000.0) as i64, &mine)
                        });
                        (named.unwrap_or_else(|| "Call audio".to_string()), Speaker::Other)
                    }
                    Source::Mic => (self.settings.your_name.clone(), Speaker::Me),
                };
                let l = line(&clock(start), &name, who, &text);
                let saved = self.store.as_ref().map(|s| s.add_line(id, &l));
                if let Some(m) = self.meetings.iter_mut().find(|m| m.id == id) {
                    m.lines.push(l);
                }
                if let Some(result) = saved {
                    self.save(result);
                }
                self.scroll.scroll_to_bottom();
            }
            Event::Progress(share) => {
                self.status = format!("Transcribing the file: {}%", (share * 100.0).round() as u32).into();
            }
            Event::Failed(message) => {
                self.recording = None;
                self.stopping = false;
                self.importing = false;
                self.status = message.into();
            }
            Event::Stopped => {
                self.recording = None;
                self.stopping = false;
                self.call_watch = None;
                self.timeline = None;
                self.call_bound = None;
                self.ask_stop = false;
                self.paused_at = None;
                self.importing = false;
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
        self.sync_menu_bar();
        cx.notify();
    }

    fn copy_notes(&mut self, id: i64, cx: &mut Context<Self>) {
        if let Some(m) = self.meetings.iter().find(|m| m.id == id) {
            cx.write_to_clipboard(ClipboardItem::new_string(export::notes_markdown(m)));
            self.status = "Notes copied".into();
            cx.notify();
        }
    }

    fn copy_transcript(&mut self, id: i64, cx: &mut Context<Self>) {
        if let Some(m) = self.meetings.iter().find(|m| m.id == id) {
            cx.write_to_clipboard(ClipboardItem::new_string(export::transcript_text(m)));
            self.status = "Transcript copied".into();
            cx.notify();
        }
    }

    fn save_markdown(&mut self, id: i64, cx: &mut Context<Self>) {
        let Some(m) = self.meetings.iter().find(|m| m.id == id) else { return };
        let (name, text) = (export::file_name(m), export::full_markdown(m));
        let dir = std::path::PathBuf::from(std::env::var("HOME").unwrap_or_default()).join("Documents");
        let rx = cx.prompt_for_new_path(&dir, Some(&name));
        cx.spawn(async move |this, cx| {
            let Ok(Ok(Some(path))) = rx.await else { return };
            let message = match std::fs::write(&path, text) {
                Ok(()) => format!("Saved {}", path.display()),
                Err(e) => format!("Could not save: {e}"),
            };
            let _ = this.update(cx, |this, cx| {
                this.status = message.into();
                cx.notify();
            });
        })
        .detach();
    }

    fn check_updates(&mut self, cx: &mut Context<Self>) {
        if matches!(self.update_state, UpdateState::Checking) {
            return;
        }
        self.update_state = UpdateState::Checking;
        let rx = updates::check();
        cx.spawn(async move |this, cx| {
            let result = rx.await;
            let _ = this.update(cx, |this, cx| {
                this.update_state = match result {
                    Ok(Ok(Some(r))) => UpdateState::Available { version: r.version.into(), url: r.url.into() },
                    Ok(Ok(None)) => UpdateState::Current,
                    Ok(Err(e)) => UpdateState::Failed(format!("{e:#}").into()),
                    Err(_) => UpdateState::Failed("The check was interrupted".into()),
                };
                cx.notify();
            });
        })
        .detach();
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
        let mut transcript = m.transcript_text();
        let instruction = self.instruction_input.text().trim().to_string();
        if !instruction.is_empty() {
            transcript = format!("Instruction from the user for these notes: {instruction}\n\n{transcript}");
            self.instruction_input.clear();
        }
        self.status = "Writing the summary on this Mac".into();
        let rx = summarizer::summarize(model, transcript, self.settings.forced_summary_language().map(str::to_string));
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
        let now = chrono::Local::now().timestamp();
        if let Some(store) = &self.store {
            let result = store.trash_meeting(id, now);
            self.save(result);
        }
        let gone = self.meetings.remove(self.selected);
        self.trash.insert(0, (gone.id, gone.title.to_string(), gone.started, now));
        self.selected = self.selected.min(self.meetings.len().saturating_sub(1));
        self.confirm_delete = None;
        self.status = format!("Moved to the trash. It is deleted for good after {TRASH_DAYS} days.").into();
        cx.notify();
    }

    fn restore_from_trash(&mut self, id: i64, cx: &mut Context<Self>) {
        let loaded = self.store.as_ref().map(|store| store.restore_meeting(id).and_then(|_| store.load_meetings()));
        match loaded {
            Some(Ok(list)) => self.meetings = list,
            Some(Err(e)) => self.status = format!("Could not restore: {e:#}").into(),
            None => {}
        }
        self.trash.retain(|t| t.0 != id);
        self.selected = self.meetings.iter().position(|m| m.id == id).unwrap_or(0);
        self.status = "Restored".into();
        cx.notify();
    }

    fn delete_for_good(&mut self, id: i64, cx: &mut Context<Self>) {
        if let Some(store) = &self.store {
            let result = store.delete_meeting(id);
            self.save(result);
        }
        audio::delete_audio(id);
        self.trash.retain(|t| t.0 != id);
        self.status = "Deleted for good".into();
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
                        this.refresh_permissions(cx);
                        cx.notify();
                    })),
            )
    }

    fn search_field(&self, t: &Theme, window: &Window, cx: &mut Context<Self>) -> impl IntoElement {
        let focused = self.focus_search.is_focused(window);
        let empty = self.search.is_empty();
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
                    .overflow_hidden()
                    .child(field_text(&self.search, "Search meetings", 24, 13., focused, t)),
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

    fn key_into(input: &mut TextInput, ev: &KeyDownEvent, cx: &mut Context<Self>) -> Outcome {
        let Some(action) = input::action_for(ev, || cx.read_from_clipboard().and_then(|c| c.text())) else {
            return Outcome::Nothing;
        };
        let outcome = input.apply(action);
        if let Outcome::Copied(text) = &outcome {
            cx.write_to_clipboard(ClipboardItem::new_string(text.clone()));
        }
        outcome
    }

    fn on_search_key(&mut self, ev: &KeyDownEvent, cx: &mut Context<Self>) {
        match Self::key_into(&mut self.search, ev, cx) {
            Outcome::Nothing => return,
            Outcome::Escape => self.search.clear(),
            _ => {}
        }
        cx.notify();
    }

    fn load_permissions(cx: &mut Context<Self>) {
        let rx = native::in_background(native::status);
        cx.spawn(async move |this, cx| {
            if let Ok(Ok(status)) = rx.await {
                let _ = this.update(cx, |this, cx| {
                    this.perms = Some(status);
                    cx.notify();
                });
            }
        })
        .detach();
    }

    /// Reads the macOS permissions the helper needs, without asking for any.
    fn refresh_permissions(&mut self, cx: &mut Context<Self>) {
        let rx = native::in_background(native::status);
        cx.spawn(async move |this, cx| {
            if let Ok(Ok(status)) = rx.await {
                let _ = this.update(cx, |this, cx| {
                    this.perms = Some(status);
                    cx.notify();
                });
            }
        })
        .detach();
    }

    /// Shows the macOS permission prompt for `kind`, then reads the permissions again.
    fn ask_permission(&mut self, kind: &'static str, cx: &mut Context<Self>) {
        if self.perms_busy {
            return;
        }
        self.perms_busy = true;
        let rx = native::in_background(move || native::request(kind));
        cx.spawn(async move |this, cx| {
            let answer = rx.await;
            let _ = this.update(cx, |this, cx| {
                this.perms_busy = false;
                match answer {
                    Ok(Ok(status)) => this.perms = Some(status),
                    Ok(Err(e)) => this.status = format!("Could not ask for permission: {e:#}").into(),
                    Err(_) => {}
                }
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    /// Saves a ZIP with the database, the settings and the audio of every meeting.
    fn back_up(&mut self, cx: &mut Context<Self>) {
        if let Some(store) = &self.store {
            let result = store.checkpoint();
            self.save(result);
        }
        let day = chrono::Local::now().format("%Y-%m-%d");
        let name = format!("Notary backup {day}.zip");
        let dir = std::path::PathBuf::from(std::env::var("HOME").unwrap_or_default()).join("Documents");
        let rx = cx.prompt_for_new_path(&dir, Some(&name));
        let source = settings::data_dir();
        cx.spawn(async move |this, cx| {
            let Ok(Ok(Some(target))) = rx.await else { return };
            let job = native::in_background(move || {
                let _ = std::fs::remove_file(&target);
                let out = std::process::Command::new("/usr/bin/ditto")
                    .args(["-c", "-k", "--keepParent", "--norsrc", "--noextattr", "--noqtn"])
                    .arg(&source)
                    .arg(&target)
                    .output()?;
                if !out.status.success() {
                    anyhow::bail!("{}", String::from_utf8_lossy(&out.stderr).trim());
                }
                Ok(target)
            });
            let message = match job.await {
                Ok(Ok(path)) => format!("Backup saved to {}", path.display()),
                Ok(Err(e)) => format!("Backup failed: {e:#}"),
                Err(_) => "Backup was interrupted".to_string(),
            };
            let _ = this.update(cx, |this, cx| {
                this.status = message.into();
                cx.notify();
            });
        })
        .detach();
    }

    fn persist_settings(&mut self) {
        if let Err(e) = self.settings.save() {
            self.status = format!("Could not save settings: {e:#}").into();
        }
    }

    fn on_name_key(&mut self, ev: &KeyDownEvent, cx: &mut Context<Self>) {
        match Self::key_into(&mut self.name_input, ev, cx) {
            Outcome::Nothing => return,
            Outcome::Escape => self.name_input.set(&self.settings.your_name.clone()),
            _ => {}
        }
        let name = self.name_input.text().trim().to_string();
        if !name.is_empty() && name != self.settings.your_name {
            self.settings.your_name = name;
            self.persist_settings();
        }
        cx.notify();
    }

    fn on_words_key(&mut self, ev: &KeyDownEvent, cx: &mut Context<Self>) {
        match Self::key_into(&mut self.words_input, ev, cx) {
            Outcome::Nothing => return,
            Outcome::Escape => self.words_input.set(&self.settings.custom_words.clone()),
            _ => {}
        }
        let words = self.words_input.text().to_string();
        if words != self.settings.custom_words {
            self.settings.custom_words = words;
            self.persist_settings();
        }
        cx.notify();
    }

    fn on_folder_key(&mut self, ev: &KeyDownEvent, cx: &mut Context<Self>) {
        match Self::key_into(&mut self.new_folder, ev, cx) {
            Outcome::Nothing => return,
            Outcome::Submit => return self.create_folder(cx),
            Outcome::Escape => {
                self.new_folder.clear();
                self.adding_folder = false;
            }
            _ => {}
        }
        cx.notify();
    }

    fn create_folder(&mut self, cx: &mut Context<Self>) {
        let name = self.new_folder.text().trim().to_string();
        if name.is_empty() {
            return;
        }
        if self.folders.iter().any(|f| f.name.to_lowercase() == name.to_lowercase()) {
            self.status = "A folder with that name exists already".into();
            cx.notify();
            return;
        }
        let id = match &self.store {
            Some(store) => match store.create_folder(&name) {
                Ok(id) => id,
                Err(e) => {
                    self.status = format!("Could not save: {e:#}").into();
                    cx.notify();
                    return;
                }
            },
            None => self.folders.iter().map(|f| f.id).max().unwrap_or(0) + 1,
        };
        self.folders.push(Folder { id, name: name.into() });
        self.folders.sort_by_key(|f| f.name.to_lowercase());
        self.folder_filter = Some(id);
        self.adding_folder = false;
        self.new_folder.clear();
        cx.notify();
    }

    fn delete_folder_filter(&mut self, cx: &mut Context<Self>) {
        let Some(id) = self.folder_filter else { return };
        if !self.confirm_folder_delete {
            self.confirm_folder_delete = true;
            cx.notify();
            return;
        }
        if let Some(store) = &self.store {
            let result = store.delete_folder(id);
            self.save(result);
        }
        self.folders.retain(|f| f.id != id);
        for m in self.meetings.iter_mut().filter(|m| m.folder == Some(id)) {
            m.folder = None;
        }
        self.folder_filter = None;
        self.confirm_folder_delete = false;
        self.status = "Folder deleted. Its meetings are still in All.".into();
        cx.notify();
    }

    fn move_selected(&mut self, folder: Option<i64>, cx: &mut Context<Self>) {
        let Some(m) = self.meetings.get_mut(self.selected) else { return };
        m.folder = folder;
        let id = m.id;
        let saved = self.store.as_ref().map(|s| s.set_folder(id, folder));
        if let Some(result) = saved {
            self.save(result);
        }
        self.moving = false;
        cx.notify();
    }

    fn on_rename_key(&mut self, ev: &KeyDownEvent, cx: &mut Context<Self>) {
        match Self::key_into(&mut self.rename_input, ev, cx) {
            Outcome::Nothing => return,
            Outcome::Submit => return self.finish_rename(cx),
            Outcome::Escape => self.renaming = false,
            _ => {}
        }
        cx.notify();
    }

    fn finish_rename(&mut self, cx: &mut Context<Self>) {
        let title = self.rename_input.text().trim().to_string();
        self.renaming = false;
        if !title.is_empty() {
            if let Some(m) = self.meetings.get_mut(self.selected) {
                m.title = title.clone().into();
                let id = m.id;
                let saved = self.store.as_ref().map(|s| s.rename_meeting(id, &title));
                if let Some(result) = saved {
                    self.save(result);
                }
            }
        }
        cx.notify();
    }

    fn on_find_key(&mut self, ev: &KeyDownEvent, cx: &mut Context<Self>) {
        match Self::key_into(&mut self.find_input, ev, cx) {
            Outcome::Nothing => return,
            Outcome::Escape => {
                self.find_input.clear();
                self.find_open = false;
            }
            _ => {}
        }
        cx.notify();
    }

    fn on_instruction_key(&mut self, ev: &KeyDownEvent, cx: &mut Context<Self>) {
        match Self::key_into(&mut self.instruction_input, ev, cx) {
            Outcome::Nothing => return,
            Outcome::Submit => {
                if let Some(id) = self.meetings.get(self.selected).map(|m| m.id) {
                    return self.start_summary(id, cx);
                }
            }
            Outcome::Escape => self.instruction_input.clear(),
            _ => {}
        }
        cx.notify();
    }

    /// Plays the saved audio of a meeting from `start` seconds, or stops it when it is already playing.
    fn toggle_play(&mut self, id: i64, start: f32, cx: &mut Context<Self>) {
        if self.player.as_ref().is_some_and(|p| p.0 == id) && start == 0.0 {
            self.player = None;
            cx.notify();
            return;
        }
        self.player = None;
        let dir = audio::meeting_dir(id);
        let files: Vec<std::path::PathBuf> = ["them.m4a", "me.m4a"].iter().map(|n| dir.join(n)).filter(|p| p.exists()).collect();
        if files.is_empty() {
            return;
        }
        match watch::play(start, &files) {
            Some((player, ended)) => {
                self.play_gen += 1;
                let generation = self.play_gen;
                self.player = Some((id, player));
                cx.spawn(async move |this, cx| {
                    let _ = ended.await;
                    let _ = this.update(cx, |this, cx| {
                        if this.play_gen == generation {
                            this.player = None;
                            cx.notify();
                        }
                    });
                })
                .detach();
            }
            None => self.status = "Playback needs the notary-mac helper next to the app".into(),
        }
        cx.notify();
    }

    /// Asks for an audio or video file and turns its speech into a new meeting.
    fn import_audio(&mut self, cx: &mut Context<Self>) {
        if self.recording.is_some() {
            return;
        }
        let Some(model) = engine::model_path() else {
            self.setup_open = true;
            self.status = "Download the speech model first".into();
            cx.notify();
            return;
        };
        let rx = cx.prompt_for_paths(gpui::PathPromptOptions {
            files: true,
            directories: false,
            multiple: false,
            prompt: Some("Transcribe".into()),
        });
        cx.spawn(async move |this, cx| {
            let Ok(Ok(Some(paths))) = rx.await else { return };
            let Some(file) = paths.into_iter().next() else { return };
            let _ = this.update(cx, |this, cx| this.start_import(model, file, cx));
        })
        .detach();
    }

    fn start_import(&mut self, model: std::path::PathBuf, file: std::path::PathBuf, cx: &mut Context<Self>) {
        if self.recording.is_some() {
            return;
        }
        let started = chrono::Local::now().timestamp();
        let title = file.file_stem().map(|s| s.to_string_lossy().to_string()).filter(|s| !s.is_empty()).unwrap_or_else(|| DEFAULT_TITLE.to_string());
        let id = match &self.store {
            Some(store) => match store.create_meeting(&title, started) {
                Ok(id) => id,
                Err(e) => {
                    self.status = format!("Could not save: {e:#}").into();
                    cx.notify();
                    return;
                }
            },
            None => self.meetings.iter().map(|m| m.id).max().unwrap_or(0) + 1,
        };
        self.meetings.insert(0, Meeting::new(id, &title, started, true, Vec::new()));
        self.selected = 0;
        self.trash_open = false;
        self.importing = true;
        let (rec, mut events) = engine::import(
            model,
            engine::Config {
                language: self.settings.whisper_language().to_string(),
                prompt: self.settings.prompt_words(),
                audio_dir: self.settings.keep_audio.then(|| audio::meeting_dir(id)),
            },
            file,
        );
        self.recording = Some(rec);
        self.status = "Reading the file".into();
        cx.spawn(async move |this, cx| {
            while let Some(event) = events.next().await {
                if this.update(cx, |this, cx| this.apply(id, event, cx)).is_err() {
                    break;
                }
            }
        })
        .detach();
        cx.notify();
    }

    fn toggle_pause(&mut self, cx: &mut Context<Self>) {
        let Some(rec) = &self.recording else { return };
        if self.stopping {
            return;
        }
        match self.paused_at.take() {
            None => {
                rec.set_paused(true);
                self.paused_at = Some(std::time::Instant::now());
                self.status = "Paused. Nothing is recorded until you resume.".into();
            }
            Some(since) => {
                rec.set_paused(false);
                // The call window kept moving while the audio stood still: shift the timeline by the pause.
                self.rec_started_ms += since.elapsed().as_millis() as i64;
                if let Some(t) = self.timeline.as_mut() {
                    t.set_zero(self.rec_started_ms);
                }
                self.last_speech = std::time::Instant::now();
                self.status = "Listening".into();
            }
        }
        self.sync_menu_bar();
        cx.notify();
    }

    fn on_note_key(&mut self, ev: &KeyDownEvent, cx: &mut Context<Self>) {
        match Self::key_into(&mut self.note_input, ev, cx) {
            Outcome::Nothing => return,
            Outcome::Submit => return self.add_note(cx),
            Outcome::Escape => self.note_input.clear(),
            _ => {}
        }
        cx.notify();
    }

    /// Saves the typed note as a line of the selected meeting, stamped with the time into the recording.
    fn add_note(&mut self, cx: &mut Context<Self>) {
        let text = self.note_input.text().trim().to_string();
        let Some(m) = self.meetings.get_mut(self.selected) else { return };
        if text.is_empty() {
            return;
        }
        let ts = if m.live { clock((chrono::Local::now().timestamp() - m.started).max(0) as f32) } else { "--:--".to_string() };
        let l = line(&ts, "You wrote", Speaker::Note, &text);
        let id = m.id;
        let saved = self.store.as_ref().map(|s| s.add_line(id, &l));
        m.lines.push(l);
        self.note_input.clear();
        if let Some(result) = saved {
            self.save(result);
        }
        self.scroll.scroll_to_bottom();
        cx.notify();
    }

    fn on_ask_key(&mut self, ev: &KeyDownEvent, cx: &mut Context<Self>) {
        match Self::key_into(&mut self.ask_input, ev, cx) {
            Outcome::Nothing => return,
            Outcome::Submit => return self.submit_question(cx),
            Outcome::Escape => self.ask_input.clear(),
            _ => {}
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
        let question = self.ask_input.text().trim().to_string();
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
        let slot = self.qa.len();
        self.qa.push(Qa { question: question.clone().into(), answer: QaState::Pending });
        self.asking = true;
        self.ask_scroll.scroll_to_bottom();

        let existing = self.store.as_ref().and_then(|s| s.load_vectors().ok()).unwrap_or_default();
        let stale: Vec<ask::IndexChunk> = ask::index_chunks(&self.meetings)
            .into_iter()
            .filter(|c| existing.get(&c.key).map(|(h, _)| h.as_str()) != Some(c.hash.as_str()))
            .collect();
        match models::find(Kind::Embedding) {
            Some(embedding_model) => {
                let rx = semantic::embed(embedding_model, question.clone(), stale.iter().map(|c| c.text.clone()).collect());
                cx.spawn(async move |this, cx| {
                    let result = rx.await;
                    let _ = this.update(cx, |this, cx| this.continue_ask(slot, question, model, existing, stale, Some(result), cx));
                })
                .detach();
            }
            None => self.continue_ask(slot, question, model, existing, stale, None, cx),
        }
        cx.notify();
    }

    /// Second step of a question: keep the new meaning vectors, pick the passages, ask the model.
    fn continue_ask(
        &mut self,
        slot: usize,
        question: String,
        model: std::path::PathBuf,
        mut vectors: std::collections::HashMap<String, (String, Vec<f32>)>,
        stale: Vec<ask::IndexChunk>,
        embedded: Option<Result<anyhow::Result<semantic::Embedded>, futures::channel::oneshot::Canceled>>,
        cx: &mut Context<Self>,
    ) {
        let mut query = None;
        if let Some(Ok(Ok(e))) = embedded {
            let rows: Vec<(String, i64, String, Vec<f32>)> = stale
                .iter()
                .zip(e.passages)
                .map(|(c, v)| (c.key.clone(), c.meeting_id, c.hash.clone(), v))
                .collect();
            let saved = self.store.as_ref().map(|s| s.save_vectors(&rows));
            if let Some(result) = saved {
                self.save(result);
            }
            for (key, _, hash, vec) in rows {
                vectors.insert(key, (hash, vec));
            }
            query = Some(e.query);
        }
        let by_meaning = query.map(|q| ask::Semantic {
            query: q,
            vectors: vectors.into_iter().map(|(k, (_, v))| (k, v)).collect(),
        });
        let sources = ask::retrieve(&self.meetings, &question, 6, by_meaning.as_ref());
        if sources.is_empty() {
            let text = if summarizer::detect_language(&question) == "German" {
                "Dazu habe ich in deinen Meetings nichts gefunden. Versuche andere Worte oder nimm das Meeting zuerst auf."
            } else {
                "I couldn't find anything about that in your meetings. Try other words, or record the meeting first."
            };
            if let Some(qa) = self.qa.get_mut(slot) {
                qa.answer = QaState::Done { text: text.into(), sources: Vec::new() };
            }
            self.asking = false;
            self.ask_scroll.scroll_to_bottom();
            cx.notify();
            return;
        }
        let refs: Vec<(i64, String)> = sources.iter().map(|s| (s.meeting_id, format!("{} \u{b7} {}", s.title, s.when))).collect();
        let rx = ask::answer(model, question, &sources);
        cx.spawn(async move |this, cx| {
            let result = rx.await;
            let _ = this.update(cx, |this, cx| this.apply_answer(slot, refs, result, cx));
        })
        .detach();
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
        let terms = search::terms(self.search.text());
        let mut items = div().id("meetings").flex_1().min_h_0().overflow_y_scroll().flex().flex_col().px(px(6.)).pb(px(10.));
        if self.trash_open {
            items = items.child(
                div()
                    .px(px(8.))
                    .py(px(10.))
                    .text_size(px(12.))
                    .line_height(px(18.))
                    .text_color(rgb(t.faint))
                    .child(if self.trash.is_empty() { "The trash is empty." } else { "Meetings here are deleted for good after 30 days." }),
            );
            for (id, title, started, deleted) in self.trash.clone() {
                let left = (TRASH_DAYS - (chrono::Local::now().timestamp() - deleted) / 86_400).max(0);
                let day = chrono::DateTime::from_timestamp(started, 0)
                    .map(|d| d.with_timezone(&chrono::Local).format("%b %-d").to_string())
                    .unwrap_or_default();
                items = items.child(
                    div()
                        .flex()
                        .flex_col()
                        .gap(px(6.))
                        .px(px(10.))
                        .py(px(9.))
                        .child(div().truncate().text_size(px(13.5)).text_color(rgb(t.ink)).child(title))
                        .child(div().text_size(px(12.)).text_color(rgb(t.faint)).child(format!("{day} \u{b7} {left} days left")))
                        .child(
                            div()
                                .flex()
                                .gap(px(6.))
                                .child(Self::pill(format!("restore-{id}").into(), "Restore".into(), true, t).on_click(cx.listener(move |this, _, _, cx| this.restore_from_trash(id, cx))))
                                .child(Self::pill(format!("purge-{id}").into(), "Delete for good".into(), false, t).on_click(cx.listener(move |this, _, _, cx| this.delete_for_good(id, cx)))),
                        ),
                );
            }
        }
        let mut last_day = String::new();
        let mut shown = 0;
        let coming: Vec<native::CalendarEvent> = if terms.is_empty() && self.recording.is_none() && !self.trash_open {
            let shown = self.shown_events();
            calendar::upcoming(&shown, chrono::Local::now().timestamp(), 3).into_iter().cloned().collect()
        } else {
            Vec::new()
        };
        if !coming.is_empty() {
            items = items.child(div().px(px(8.)).pt(px(14.)).pb(px(4.)).text_size(px(11.)).text_color(rgb(t.faint)).child("COMING UP"));
            for e in coming {
                let time = calendar::start_ts(&e)
                    .and_then(|s| chrono::DateTime::from_timestamp(s, 0))
                    .map(|d| d.with_timezone(&chrono::Local).format("%H:%M").to_string())
                    .unwrap_or_default();
                let id = e.id.clone();
                items = items.child(
                    div()
                        .flex()
                        .items_center()
                        .gap_2()
                        .px(px(10.))
                        .py(px(7.))
                        .child(div().font_family(MONO).text_size(px(11.)).text_color(rgb(t.faint)).child(time))
                        .child(div().flex_1().min_w_0().truncate().text_size(px(13.)).text_color(rgb(t.soft)).child(e.title.clone()))
                        .child(
                            Self::pill(format!("event-{}", e.id).into(), "Record".into(), false, t).on_click(cx.listener(move |this, _, _, cx| {
                                this.pending_event = Some(id.clone());
                                this.toggle_recording(cx);
                            })),
                        ),
                );
            }
        }
        for (i, m) in self.meetings.iter().enumerate().filter(|_| !self.trash_open) {
            if self.folder_filter.is_some() && m.folder != self.folder_filter {
                continue;
            }
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
        if shown == 0 && !terms.is_empty() && !self.trash_open {
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
            .child(self.folder_bar(t, window, cx))
            .child(items)
    }

    fn folder_bar(&self, t: &Theme, window: &Window, cx: &mut Context<Self>) -> impl IntoElement {
        let chip = |id: SharedString, label: SharedString, active: bool| {
            div()
                .id(id)
                .flex_none()
                .px(px(9.))
                .py(px(3.))
                .rounded_full()
                .cursor_pointer()
                .text_size(px(11.5))
                .when(active, |d| d.bg(rgb(t.accent_soft)).text_color(rgb(t.accent)))
                .when(!active, |d| d.bg(rgb(t.rail)).text_color(rgb(t.soft)).hover(|h| h.text_color(rgb(t.accent))))
                .child(label)
        };
        let mut row = div().flex().flex_wrap().items_center().gap(px(6.)).px(px(12.)).py(px(8.)).border_b_1().border_color(rgb(t.line));
        row = row.child(chip("folder-all".into(), "All".into(), self.folder_filter.is_none()).on_click(cx.listener(|this, _, _, cx| {
            this.folder_filter = None;
            this.confirm_folder_delete = false;
            cx.notify();
        })));
        for f in &self.folders {
            let id = f.id;
            row = row.child(
                chip(format!("folder-{id}").into(), f.name.clone(), self.folder_filter == Some(id)).on_click(cx.listener(move |this, _, _, cx| {
                    this.folder_filter = Some(id);
                    this.confirm_folder_delete = false;
                    cx.notify();
                })),
            );
        }
        row = row.child(chip("folder-new".into(), "+ Folder".into(), self.adding_folder).on_click(cx.listener(|this, _, window, cx| {
            this.adding_folder = true;
            window.focus(&this.focus_folder);
            cx.notify();
        })));
        row = row.child(chip("trash-chip".into(), format!("Trash ({})", self.trash.len()).into(), self.trash_open).on_click(cx.listener(|this, _, _, cx| {
            this.trash_open = !this.trash_open;
            cx.notify();
        })));
        if self.folder_filter.is_some() {
            row = row.child(
                div()
                    .id("folder-delete")
                    .cursor_pointer()
                    .text_size(px(11.5))
                    .text_color(rgb(if self.confirm_folder_delete { t.record } else { t.faint }))
                    .on_click(cx.listener(|this, _, _, cx| this.delete_folder_filter(cx)))
                    .child(if self.confirm_folder_delete { "Click again to delete folder" } else { "Delete folder" }),
            );
        }
        let focused = self.focus_folder.is_focused(window);
        div().flex_none().flex().flex_col().child(row).when(self.adding_folder, |d| {
            d.child(
                div().px(px(12.)).pb(px(8.)).child(
                    div()
                        .id("new-folder-input")
                        .track_focus(&self.focus_folder)
                        .on_key_down(cx.listener(|this, ev: &KeyDownEvent, _, cx| this.on_folder_key(ev, cx)))
                        .on_click(cx.listener(|this, _, window, cx| {
                            window.focus(&this.focus_folder);
                            cx.notify();
                        }))
                        .px(px(10.))
                        .py(px(6.))
                        .rounded(px(8.))
                        .bg(rgb(t.rail))
                        .border_1()
                        .border_color(rgb(if focused { t.accent } else { t.rail }))
                        .overflow_hidden()
                        .child(field_text(&self.new_folder, "Folder name, then Enter", 28, 13., focused, t)),
                ),
            )
        })
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
                            this.ask_input.set(example);
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
                    .overflow_hidden()
                    .child(field_text(&self.ask_input, "Ask something about your meetings", 64, 14., focused, t)),
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

    fn settings_card(t: &Theme) -> gpui::Div {
        div()
            .flex()
            .flex_col()
            .gap(px(14.))
            .p(px(16.))
            .rounded(px(12.))
            .bg(rgb(t.raised))
            .border_1()
            .border_color(rgb(t.line))
    }

    fn input_shell(id: &'static str, focus: &FocusHandle, focused: bool, t: &Theme, cx: &mut Context<Self>) -> gpui::Stateful<gpui::Div> {
        let handle = focus.clone();
        div()
            .id(id)
            .track_focus(focus)
            .on_click(cx.listener(move |_, _, window, cx| {
                window.focus(&handle);
                cx.notify();
            }))
            .px(px(10.))
            .py(px(7.))
            .rounded(px(8.))
            .bg(rgb(t.rail))
            .border_1()
            .border_color(rgb(if focused { t.accent } else { t.rail }))
            .cursor_pointer()
            .overflow_hidden()
    }

    fn pill(id: SharedString, label: SharedString, active: bool, t: &Theme) -> gpui::Stateful<gpui::Div> {
        div()
            .id(id)
            .flex_none()
            .px(px(10.))
            .py(px(4.))
            .rounded_full()
            .cursor_pointer()
            .text_size(px(12.))
            .when(active, |d| d.bg(rgb(t.accent_soft)).text_color(rgb(t.accent)))
            .when(!active, |d| d.bg(rgb(t.rail)).text_color(rgb(t.soft)).hover(|h| h.text_color(rgb(t.accent))))
            .child(label)
    }

    fn settings_cards(&self, t: &Theme, window: &Window, cx: &mut Context<Self>) -> impl IntoElement {
        let label = |s: &'static str| div().text_size(px(12.5)).font_weight(FontWeight::SEMIBOLD).text_color(rgb(t.ink)).child(s);
        let hint = |s: &'static str| div().text_size(px(12.)).line_height(px(18.)).text_color(rgb(t.faint)).child(s);

        let name_focused = self.focus_name.is_focused(window);
        let words_focused = self.focus_words.is_focused(window);

        let you = Self::settings_card(t)
            .child(div().font_family(SERIF).text_size(px(16.)).text_color(rgb(t.ink)).child("You"))
            .child(
                div().flex().flex_col().gap(px(6.)).child(label("Your name")).child(
                    Self::input_shell("name-input", &self.focus_name, name_focused, t, cx)
                        .on_key_down(cx.listener(|this, ev: &KeyDownEvent, _, cx| this.on_name_key(ev, cx)))
                        .child(field_text(&self.name_input, "Your name", 40, 13., name_focused, t)),
                ).child(hint("Shown next to the lines your microphone picks up.")),
            );

        let mut summary_pills = div().flex().flex_wrap().gap(px(6.));
        for (i, language) in SUMMARY_LANGUAGES.iter().enumerate() {
            let active = self.settings.summary_language == *language;
            summary_pills = summary_pills.child(
                Self::pill(format!("summary-lang-{i}").into(), (*language).into(), active, t).on_click(cx.listener(move |this, _, _, cx| {
                    this.settings.summary_language = SUMMARY_LANGUAGES[i].to_string();
                    this.persist_settings();
                    cx.notify();
                })),
            );
        }
        let spoken = div()
            .flex()
            .flex_col()
            .gap(px(6.))
            .child(label("Spoken language"))
            .child(
                div()
                    .flex()
                    .gap(px(6.))
                    .child(Self::pill("spoken-multi".into(), "Multi-language".into(), self.settings.spoken == Spoken::Multi, t).on_click(
                        cx.listener(|this, _, _, cx| {
                            this.settings.spoken = Spoken::Multi;
                            this.persist_settings();
                            cx.notify();
                        }),
                    ))
                    .child(Self::pill("spoken-en".into(), "English only".into(), self.settings.spoken == Spoken::EnglishOnly, t).on_click(
                        cx.listener(|this, _, _, cx| {
                            this.settings.spoken = Spoken::EnglishOnly;
                            this.persist_settings();
                            cx.notify();
                        }),
                    )),
            )
            .child(hint("English only is faster for English meetings. Multi-language detects the language of every phrase, so a meeting can switch between languages."));
        let language = Self::settings_card(t)
            .child(div().font_family(SERIF).text_size(px(16.)).text_color(rgb(t.ink)).child("Language"))
            .child(spoken)
            .child(div().flex().flex_col().gap(px(6.)).child(label("Summary language")).child(summary_pills))
            .child(
                div().flex().flex_col().gap(px(6.)).child(label("Custom words")).child(
                    Self::input_shell("words-input", &self.focus_words, words_focused, t, cx)
                        .on_key_down(cx.listener(|this, ev: &KeyDownEvent, _, cx| this.on_words_key(ev, cx)))
                        .child(field_text(&self.words_input, "Names and jargon, separated by commas", 60, 13., words_focused, t)),
                ).child(hint("Helps Notary spell names, products and technical terms the way you do.")),
            );
        let toggle = |id: &'static str, title: &'static str, text: &'static str, on: bool, field: fn(&mut Settings) -> &mut bool, cx: &mut Context<Self>| {
            div()
                .flex()
                .items_center()
                .gap(px(16.))
                .child(div().flex_1().min_w_0().flex().flex_col().gap(px(2.)).child(label(title)).child(hint(text)))
                .child(Self::pill(id.into(), if on { "On" } else { "Off" }.into(), on, t).on_click(cx.listener(move |this, _, _, cx| {
                    let value = field(&mut this.settings);
                    *value = !*value;
                    this.persist_settings();
                    cx.notify();
                })))
        };
        let s = &self.settings;
        let recording = Self::settings_card(t)
            .child(div().font_family(SERIF).text_size(px(16.)).text_color(rgb(t.ink)).child("Recording"))
            .child(toggle("t-audio", "Keep the audio", "Saves what was said and heard next to the transcript, so you can play it back. Stays on this Mac.", s.keep_audio, |s| &mut s.keep_audio, cx))
            .child(toggle("t-offer", "Offer to record calls", "A card appears when Zoom, Teams, Meet, FaceTime and others start using your microphone.", s.offer_to_record, |s| &mut s.offer_to_record, cx))
            .child(toggle("t-stop", "Stop when the call ends", "Stops a recording that was started from a call once the call is over and it has been quiet.", s.stop_when_call_ends, |s| &mut s.stop_when_call_ends, cx));

        let perm_row = |title: &'static str, text: &'static str, state: Option<bool>, kind: &'static str, cx: &mut Context<Self>| {
            let action: AnyElement = match state {
                Some(true) => div().text_size(px(12.5)).text_color(rgb(t.accent)).child("Allowed").into_any_element(),
                _ => Self::pill(format!("perm-{kind}").into(), if state.is_none() { "Checking" } else { "Allow" }.into(), true, t)
                    .on_click(cx.listener(move |this, _, _, cx| this.ask_permission(kind, cx)))
                    .into_any_element(),
            };
            div()
                .flex()
                .items_center()
                .gap(px(16.))
                .child(div().flex_1().min_w_0().flex().flex_col().gap(px(2.)).child(label(title)).child(hint(text)))
                .child(action)
        };
        let p = self.perms.as_ref();
        let permissions = Self::settings_card(t)
            .child(div().font_family(SERIF).text_size(px(16.)).text_color(rgb(t.ink)).child("Permissions"))
            .child(perm_row("Screen recording", "Lets Notary hear your Mac's audio. No picture is ever recorded.", p.map(|p| p.screen), "screen", cx))
            .child(perm_row("Microphone", "Picks up your own voice.", p.map(|p| p.microphone == "authorized"), "microphone", cx))
            .child(perm_row("Accessibility", "Reads who is speaking from the Zoom, Teams or Meet window. Nothing else is read.", p.map(|p| p.accessibility), "accessibility", cx))
            .child(perm_row("Calendar", "Shows your next meetings and names recordings after them.", p.map(|p| p.calendar == "fullAccess"), "calendar", cx));
        let command = std::env::current_exe().map(|p| format!("\"{}\" mcp", p.display())).unwrap_or_else(|_| "Notary mcp".to_string());
        let ai = Self::settings_card(t)
            .child(div().font_family(SERIF).text_size(px(16.)).text_color(rgb(t.ink)).child("AI apps"))
            .child(toggle("t-mcp", "Let AI apps read your meetings", "Lets an app you connect, such as Claude or Cursor, search and read your notes and transcripts. Read only. Off by default.", s.mcp_enabled, |s| &mut s.mcp_enabled, cx))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(16.))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .flex()
                            .flex_col()
                            .gap(px(2.))
                            .child(label("Command to add to your AI app"))
                            .child(div().font_family(MONO).text_size(px(11.5)).line_height(px(17.)).text_color(rgb(t.soft)).child(command.clone())),
                    )
                    .child(Self::pill("copy-mcp".into(), "Copy".into(), false, t).on_click(cx.listener(move |this, _, _, cx| {
                        cx.write_to_clipboard(ClipboardItem::new_string(command.clone()));
                        this.status = "Command copied".into();
                        cx.notify();
                    }))),
            );
        let data = Self::settings_card(t)
            .child(div().font_family(SERIF).text_size(px(16.)).text_color(rgb(t.ink)).child("Your data"))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(16.))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .flex()
                            .flex_col()
                            .gap(px(2.))
                            .child(label("Back up everything"))
                            .child(hint("Saves a ZIP with your meetings, settings and audio. To restore it, quit Notary and unzip it over the Notary folder in Application Support.")),
                    )
                    .child(Self::pill("backup".into(), "Back up\u{2026}".into(), true, t).on_click(cx.listener(|this, _, _, cx| this.back_up(cx)))),
            );
        div().flex().flex_col().gap(px(16.)).child(you).child(recording).child(language).child(permissions).child(ai).child(data)
    }

    fn model_row(&self, kind: Kind, t: &Theme, cx: &mut Context<Self>) -> impl IntoElement {
        let spec = models::spec(kind);
        let installed = models::find(kind).is_some();
        let state = &self.downloads[kind as usize];
        let active = matches!(state, Dl::Active { .. });

        let button = |id: SharedString, label: &'static str, primary: bool| {
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
            button(format!("cancel-{}", spec.id).into(), "Cancel", false)
                .on_click(cx.listener(move |this, _, _, cx| this.cancel_download(kind, cx)))
                .into_any_element()
        } else {
            let label = if matches!(state, Dl::Failed(_)) { "Retry" } else { "Download" };
            button(format!("get-{}", spec.id).into(), label, true)
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

    fn setup_body(&self, t: &Theme, window: &Window, cx: &mut Context<Self>) -> AnyElement {
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
                    .child(if ready { "Settings" } else { "Set up Notary" }),
            )
            .child(self.settings_cards(t, window, cx))
            .child(div().font_family(SERIF).text_size(px(18.)).text_color(rgb(t.ink)).child("Models"))
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
        let (update_text, update_color): (SharedString, u32) = match &self.update_state {
            UpdateState::Idle => ("".into(), t.faint),
            UpdateState::Checking => ("Checking github.com".into(), t.faint),
            UpdateState::Current => ("You have the latest version.".into(), t.soft),
            UpdateState::Available { version, .. } => (format!("Version {version} is available.").into(), t.accent),
            UpdateState::Failed(message) => (message.clone(), t.record),
        };
        let open_url: Option<SharedString> = match &self.update_state {
            UpdateState::Available { url, .. } => Some(url.clone()),
            _ => None,
        };
        let update_button = div()
            .id("check-updates")
            .flex_none()
            .px(px(14.))
            .py(px(6.))
            .rounded(px(8.))
            .cursor_pointer()
            .bg(rgb(if open_url.is_some() { t.accent } else { t.rail }))
            .text_color(rgb(if open_url.is_some() { t.raised } else { t.soft }))
            .text_size(px(12.5))
            .font_weight(FontWeight::SEMIBOLD)
            .on_click(cx.listener(move |this, _, _, cx| match &open_url {
                Some(url) => cx.open_url(url),
                None => this.check_updates(cx),
            }))
            .child(if matches!(self.update_state, UpdateState::Available { .. }) { "Open download page" } else { "Check for updates" });
        column = column.child(
            div()
                .flex()
                .flex_col()
                .gap(px(10.))
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
                                        .font_family(SERIF)
                                        .text_size(px(16.))
                                        .text_color(rgb(t.ink))
                                        .child(format!("Notary {}", updates::CURRENT)),
                                )
                                .child(
                                    div()
                                        .text_size(px(12.5))
                                        .line_height(px(19.))
                                        .text_color(rgb(t.soft))
                                        .child("Notary contacts github.com only when you press Check for updates."),
                                ),
                        )
                        .child(update_button),
                )
                .when(!update_text.is_empty(), |d| {
                    d.child(div().text_size(px(12.5)).text_color(rgb(update_color)).child(update_text))
                }),
        );
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
                                    if !this.settings.onboarded {
                                        this.settings.onboarded = true;
                                        this.persist_settings();
                                    }
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

    fn meeting_body(&self, m: &Meeting, t: &Theme, window: &Window, cx: &mut Context<Self>) -> AnyElement {
        let mut transcript = div()
            .id("transcript")
            .track_scroll(&self.scroll)
            .flex_1()
            .min_w_0()
            .min_h_0()
            .overflow_y_scroll()
            .flex()
            .flex_col()
            .px(px(24.))
            .py(px(8.));
        let find_terms = if self.find_open { search::terms(self.find_input.text()) } else { Vec::new() };
        let mut found = 0;
        let has_audio = audio::has_audio(m.id);
        let live_id = m.id;
        for (line_index, l) in m.lines.iter().enumerate() {
            if !find_terms.is_empty() {
                let hay = format!("{} {}", l.name, l.text).to_lowercase();
                if !find_terms.iter().all(|w| hay.contains(w.as_str())) {
                    continue;
                }
                found += 1;
            }
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
                            .id(("ts", line_index))
                            .w(px(50.))
                            .flex_none()
                            .pt(px(3.))
                            .font_family(MONO)
                            .text_size(px(11.))
                            .text_color(rgb(t.faint))
                            .child(l.ts.clone())
                            .when(has_audio && !m.live && l.who != Speaker::Note, |d| {
                                let secs = l.ts.split(':').fold(0.0f32, |acc, p| acc * 60.0 + p.parse::<f32>().unwrap_or(0.0));
                                d.cursor_pointer()
                                    .hover(|h| h.text_color(rgb(t.accent)))
                                    .on_click(cx.listener(move |this, _, _, cx| this.toggle_play(live_id, secs.max(0.001), cx)))
                            }),
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
        if can_summarize {
            let instr_focused = self.focus_instruction.is_focused(window);
            summary = summary.child(
                div()
                    .id("instruction-input")
                    .track_focus(&self.focus_instruction)
                    .on_key_down(cx.listener(|this, ev: &KeyDownEvent, _, cx| this.on_instruction_key(ev, cx)))
                    .on_click(cx.listener(|this, _, window, cx| {
                        window.focus(&this.focus_instruction);
                        cx.notify();
                    }))
                    .px(px(10.))
                    .py(px(5.))
                    .rounded(px(8.))
                    .bg(rgb(t.rail))
                    .border_1()
                    .border_color(rgb(if instr_focused { t.focus } else { t.rail }))
                    .overflow_hidden()
                    .child(field_text(&self.instruction_input, "Optional: what to focus on, then Enter", 36, 12.5, instr_focused, t)),
            );
        }
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

        let note_focused = self.focus_note.is_focused(window);
        let note_bar = div().flex_none().px(px(24.)).py(px(10.)).border_t_1().border_color(rgb(t.line)).child(
            div()
                .id("note-input")
                .track_focus(&self.focus_note)
                .on_key_down(cx.listener(|this, ev: &KeyDownEvent, _, cx| this.on_note_key(ev, cx)))
                .on_click(cx.listener(|this, _, window, cx| {
                    window.focus(&this.focus_note);
                    cx.notify();
                }))
                .flex()
                .items_center()
                .px(px(12.))
                .py(px(7.))
                .rounded(px(8.))
                .bg(rgb(t.rail))
                .border_1()
                .border_color(rgb(if note_focused { t.focus } else { t.rail }))
                .cursor_pointer()
                .overflow_hidden()
                .child(field_text(&self.note_input, "Write your own note and press Enter", 70, 13., note_focused, t)),
        );
        let playing = self.player.as_ref().is_some_and(|p| p.0 == m.id);
        let find_focused = self.focus_find.is_focused(window);
        let tool = |id: &'static str, label: &'static str, active: bool| {
            div()
                .id(id)
                .flex_none()
                .px(px(10.))
                .py(px(4.))
                .rounded_full()
                .cursor_pointer()
                .text_size(px(12.))
                .when(active, |d| d.bg(rgb(t.accent_soft)).text_color(rgb(t.accent)))
                .when(!active, |d| d.bg(rgb(t.rail)).text_color(rgb(t.soft)).hover(|h| h.text_color(rgb(t.accent))))
                .child(label)
        };
        let tools = div()
            .flex_none()
            .flex()
            .items_center()
            .gap(px(8.))
            .px(px(24.))
            .py(px(8.))
            .border_b_1()
            .border_color(rgb(t.line))
            .when(has_audio && !m.live, |d| {
                d.child(
                    tool("play", if playing { "Stop" } else { "Play" }, playing)
                        .on_click(cx.listener(move |this, _, _, cx| this.toggle_play(meeting_id, 0.0, cx))),
                )
            })
            .child(tool("find", "Find", self.find_open).on_click(cx.listener(|this, _, window, cx| {
                this.find_open = !this.find_open;
                if this.find_open {
                    window.focus(&this.focus_find);
                } else {
                    this.find_input.clear();
                }
                cx.notify();
            })))
            .when(self.find_open, |d| {
                d.child(
                    div()
                        .id("find-input")
                        .track_focus(&self.focus_find)
                        .on_key_down(cx.listener(|this, ev: &KeyDownEvent, _, cx| this.on_find_key(ev, cx)))
                        .on_click(cx.listener(|this, _, window, cx| {
                            window.focus(&this.focus_find);
                            cx.notify();
                        }))
                        .flex_1()
                        .min_w_0()
                        .px(px(10.))
                        .py(px(4.))
                        .rounded(px(8.))
                        .bg(rgb(t.rail))
                        .border_1()
                        .border_color(rgb(if find_focused { t.focus } else { t.rail }))
                        .overflow_hidden()
                        .child(field_text(&self.find_input, "Find in this transcript", 40, 13., find_focused, t)),
                )
                .when(!find_terms.is_empty(), |d| d.child(div().flex_none().text_size(px(12.)).text_color(rgb(t.faint)).child(format!("{found} found"))))
            });
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
                    .child(if self.renaming {
                        div()
                            .id("rename-input")
                            .track_focus(&self.focus_rename)
                            .on_key_down(cx.listener(|this, ev: &KeyDownEvent, _, cx| this.on_rename_key(ev, cx)))
                            .px(px(8.))
                            .py(px(2.))
                            .rounded(px(8.))
                            .bg(rgb(t.rail))
                            .border_1()
                            .border_color(rgb(t.accent))
                            .overflow_hidden()
                            .child(field_text(&self.rename_input, "Title", 46, 20., self.focus_rename.is_focused(window), t))
                            .into_any_element()
                    } else {
                        div()
                            .font_family(SERIF)
                            .text_size(px(22.))
                            .text_color(rgb(t.ink))
                            .child(m.title.clone())
                            .into_any_element()
                    })
                    .child(
                        div()
                            .font_family(MONO)
                            .text_size(px(11.5))
                            .text_color(rgb(t.faint))
                            .child(m.meta()),
                    )
                    .when(!m.people.is_empty(), |d| {
                        d.child(div().text_size(px(12.)).text_color(rgb(t.soft)).child(format!("With {}", m.people.join(", "))))
                    })
                    .when(self.moving, |d| {
                        let mut chips = div().flex().flex_wrap().gap(px(6.)).pt(px(4.));
                        let pick = |id: SharedString, label: SharedString, active: bool| {
                            div()
                                .id(id)
                                .px(px(9.))
                                .py(px(3.))
                                .rounded_full()
                                .cursor_pointer()
                                .text_size(px(11.5))
                                .when(active, |c| c.bg(rgb(t.accent_soft)).text_color(rgb(t.accent)))
                                .when(!active, |c| c.bg(rgb(t.rail)).text_color(rgb(t.soft)).hover(|h| h.text_color(rgb(t.accent))))
                                .child(label)
                        };
                        chips = chips.child(
                            pick("move-none".into(), "No folder".into(), m.folder.is_none())
                                .on_click(cx.listener(|this, _, _, cx| this.move_selected(None, cx))),
                        );
                        for f in &self.folders {
                            let fid = f.id;
                            chips = chips.child(
                                pick(format!("move-{fid}").into(), f.name.clone(), m.folder == Some(fid))
                                    .on_click(cx.listener(move |this, _, _, cx| this.move_selected(Some(fid), cx))),
                            );
                        }
                        if self.folders.is_empty() {
                            chips = chips.child(div().text_size(px(11.5)).text_color(rgb(t.faint)).child("Create a folder first with + Folder in the list."));
                        }
                        d.child(chips)
                    }),
            )
            .when(!m.live, |d| {
                let chip = |id: &'static str, label: &'static str| {
                    div()
                        .id(id)
                        .flex_none()
                        .px(px(10.))
                        .py(px(5.))
                        .rounded(px(7.))
                        .cursor_pointer()
                        .text_size(px(12.))
                        .bg(rgb(t.rail))
                        .text_color(rgb(t.soft))
                        .hover(|h| h.text_color(rgb(t.accent)))
                        .child(label)
                };
                d.child(
                    div()
                        .flex()
                        .flex_none()
                        .items_center()
                        .gap(px(6.))
                        .child(chip("copy-notes", "Copy notes").on_click(cx.listener(move |this, _, _, cx| this.copy_notes(meeting_id, cx))))
                        .child(
                            chip("copy-transcript", "Copy transcript")
                                .on_click(cx.listener(move |this, _, _, cx| this.copy_transcript(meeting_id, cx))),
                        )
                        .child(chip("export-md", "Export").on_click(cx.listener(move |this, _, _, cx| this.save_markdown(meeting_id, cx))))
                        .when(audio::has_audio(meeting_id), |row| {
                            row.child(chip("audio", "Audio").on_click(cx.listener(move |_, _, _, cx| {
                                let dir = audio::meeting_dir(meeting_id);
                                let file = ["them.m4a", "me.m4a"].iter().map(|n| dir.join(n)).find(|p| p.exists());
                                cx.reveal_path(&file.unwrap_or(dir));
                            })))
                        })
                        .child(chip("move", "Move").on_click(cx.listener(|this, _, _, cx| {
                            this.moving = !this.moving;
                            cx.notify();
                        })))
                        .child(chip("rename", "Rename").on_click(cx.listener(|this, _, window, cx| {
                            if let Some(m) = this.meetings.get(this.selected) {
                                this.rename_input.set(m.title.as_ref());
                                this.renaming = true;
                                window.focus(&this.focus_rename);
                                cx.notify();
                            }
                        })))
                        .child(
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
                        ),
                )
            });

        div()
            .flex_1()
            .min_h_0()
            .flex()
            .flex_col()
            .child(head)
            .child(
                div()
                    .flex_1()
                    .min_h_0()
                    .flex()
                    .child(div().flex_1().min_w_0().h_full().flex().flex_col().child(tools).child(transcript).child(note_bar))
                    .child(side),
            )
            .into_any_element()
    }

    /// Cards above the footbar: an offer to record a call, or a question about stopping.
    fn cards(&self, t: &Theme, cx: &mut Context<Self>) -> Vec<gpui::AnyElement> {
        let button = |id: &'static str, label: &'static str, primary: bool| {
            div()
                .id(id)
                .px(px(12.))
                .py(px(5.))
                .rounded_full()
                .cursor_pointer()
                .text_size(px(12.5))
                .font_weight(FontWeight::SEMIBOLD)
                .when(primary, |b| b.bg(rgb(t.accent)).text_color(rgb(t.paper)))
                .when(!primary, |b| b.bg(rgb(t.rail)).text_color(rgb(t.soft)).hover(|h| h.text_color(rgb(t.ink))))
                .child(label)
        };
        let card = |text: String| {
            div()
                .flex_none()
                .flex()
                .items_center()
                .gap_3()
                .px(px(16.))
                .py(px(10.))
                .bg(rgb(t.accent_soft))
                .border_t_1()
                .border_color(rgb(t.line))
                .text_size(px(13.))
                .text_color(rgb(t.ink))
                .child(div().flex_1().min_w_0().child(text))
        };
        let mut out = Vec::new();
        if let (Some(app), None) = (self.offer, &self.recording) {
            out.push(
                card(format!("{} is using your microphone. Record this call?", app.name()))
                    .child(button("offer-yes", "Record", true).on_click(cx.listener(move |this, _, _, cx| {
                        this.offer = None;
                        this.pending_bound = Some(app);
                        this.toggle_recording(cx);
                    })))
                    .child(button("offer-no", "Not now", false).on_click(cx.listener(|this, _, _, cx| {
                        this.offer = None;
                        cx.notify();
                    })))
                    .into_any_element(),
            );
        }
        if self.ask_stop && self.recording.is_some() && !self.stopping {
            out.push(
                card("Nothing has been said for a minute. Stop recording?".to_string())
                    .child(button("stop-yes", "Stop", true).on_click(cx.listener(|this, _, _, cx| {
                        this.ask_stop = false;
                        this.toggle_recording(cx);
                    })))
                    .child(button("stop-no", "Keep going", false).on_click(cx.listener(|this, _, _, cx| {
                        this.ask_stop = false;
                        this.detector.snooze(chrono::Local::now().timestamp_millis());
                        cx.notify();
                    })))
                    .into_any_element(),
            );
        }
        out
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
            .when(!recording, |d| {
                d.child(
                    div()
                        .id("import")
                        .px(px(12.))
                        .py(px(6.))
                        .rounded_full()
                        .cursor_pointer()
                        .bg(rgb(t.rail))
                        .text_size(px(12.5))
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(rgb(t.soft))
                        .hover(|h| h.text_color(rgb(t.ink)))
                        .on_click(cx.listener(|this, _, _, cx| this.import_audio(cx)))
                        .child("Import audio"),
                )
            })
            .when(recording && !self.importing && !self.stopping, |d| {
                d.child(
                    div()
                        .id("pause")
                        .px(px(12.))
                        .py(px(6.))
                        .rounded_full()
                        .cursor_pointer()
                        .bg(rgb(t.rail))
                        .text_size(px(12.5))
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(rgb(t.soft))
                        .hover(|h| h.text_color(rgb(t.ink)))
                        .on_click(cx.listener(|this, _, _, cx| this.toggle_pause(cx)))
                        .child(if self.paused_at.is_some() { "Resume" } else { "Pause" }),
                )
            })
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
            self.setup_body(t, window, cx)
        } else if self.ask_open {
            self.ask_body(t, window, cx)
        } else {
            match self.meetings.get(self.selected) {
                Some(m) => self.meeting_body(m, t, window, cx),
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
            .children(self.cards(t, cx))
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
    if std::env::args().nth(1).as_deref() == Some("mcp") {
        mcp::serve();
        return;
    }
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
