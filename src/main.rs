mod data;
mod engine;
mod store;
mod theme;

use data::{Meeting, Speaker, line};
use engine::{Event, Recording};
use futures::StreamExt;
use gpui::{
    AnyElement, App, Application, Bounds, Context, FontWeight, IntoElement, Render, ScrollHandle,
    SharedString, TitlebarOptions, Window, WindowBounds, WindowOptions, div, point, prelude::*, px,
    rgb, size,
};
use store::Store;
use theme::Theme;

const SANS: &str = ".SystemUIFont";
const SERIF: &str = "Iowan Old Style";
const MONO: &str = "Menlo";

struct Notary {
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
    fn new() -> Self {
        let mut status: SharedString = "Ready".into();
        let (store, meetings) = match Store::open().and_then(|s| s.load_meetings().map(|m| (s, m))) {
            Ok((s, m)) => (Some(s), m),
            Err(e) => {
                status = format!("Meetings will not be saved: {e:#}").into();
                (None, Vec::new())
            }
        };
        Self {
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
            self.status = "Model not found. Put ggml-large-v3-turbo-q5_0.bin in ./models or set NOTARY_MODEL.".into();
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
            Event::Listening => self.status = "Listening to your Mac's audio".into(),
            Event::Segment { start, text } => {
                let l = line(&clock(start), "Call audio", Speaker::Other, &text);
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
            }
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

    fn rail(&self, t: &Theme) -> impl IntoElement {
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
            .child(glyph("\u{25a4}", true))
            .child(glyph("\u{2315}", false))
            .child(glyph("\u{25a2}", false))
            .child(div().flex_1())
            .child(glyph("\u{2699}", false))
    }

    fn list(&self, t: &Theme, cx: &mut Context<Self>) -> impl IntoElement {
        let mut items = div().id("meetings").flex_1().min_h_0().overflow_y_scroll().flex().flex_col().px(px(6.)).pb(px(10.));
        let mut last_day = String::new();
        for (i, m) in self.meetings.iter().enumerate() {
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
            let active = i == self.selected;
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
                    .child(
                        div()
                            .truncate()
                            .text_size(px(12.))
                            .text_color(rgb(t.soft))
                            .child(m.sub()),
                    ),
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
                    .child(
                        div()
                            .px(px(10.))
                            .py(px(7.))
                            .rounded(px(8.))
                            .bg(rgb(t.rail))
                            .text_size(px(13.))
                            .text_color(rgb(t.faint))
                            .child("Search meetings, notes, transcript"),
                    ),
            )
            .child(items)
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
        let mut summary = div().flex().flex_col().gap(px(10.)).child(heading("Summary"));
        if m.summary.is_empty() {
            summary = summary.child(
                div()
                    .text_size(px(13.))
                    .line_height(px(20.))
                    .text_color(rgb(t.faint))
                    .child("Summaries arrive with the local language model, which is not built yet."),
            );
        }
        for (lead, body) in &m.summary {
            summary = summary.child(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(2.))
                    .child(
                        div()
                            .text_size(px(12.))
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(rgb(t.ink))
                            .child(*lead),
                    )
                    .child(
                        div()
                            .text_size(px(13.5))
                            .line_height(px(21.))
                            .text_color(rgb(t.soft))
                            .child(*body),
                    ),
            );
        }

        let mut actions = div().flex().flex_col().gap(px(8.)).child(heading("Next steps"));
        for a in &m.actions {
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
                            .mt(px(2.))
                            .size(px(15.))
                            .flex_none()
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
                            .child(a.label),
                    )
                    .child(
                        div()
                            .px(px(7.))
                            .py(px(1.))
                            .rounded_full()
                            .bg(rgb(t.rail))
                            .text_size(px(11.))
                            .text_color(rgb(t.soft))
                            .child(a.owner),
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

    fn view(&self, t: &Theme, cx: &mut Context<Self>) -> impl IntoElement {
        let body = match self.meetings.get(self.selected) {
            Some(m) => self.meeting_body(m, t, cx),
            None => self.empty_body(t),
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
                    .child(self.rail(&t))
                    .child(self.list(&t, cx))
                    .child(self.view(&t, cx)),
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
            |_, cx| cx.new(|_| Notary::new()),
        )
        .unwrap();
        cx.activate(true);
    });
}
