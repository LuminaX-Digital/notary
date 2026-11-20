mod data;
mod engine;
mod theme;

use data::{Meeting, Speaker, line, sample_meetings};
use engine::{Event, Recording};
use futures::StreamExt;
use gpui::{
    App, Application, Bounds, Context, FontWeight, IntoElement, Render, ScrollHandle, SharedString,
    TitlebarOptions, Window, WindowBounds, WindowOptions, div, point, prelude::*, px, rgb, size,
};
use theme::Theme;

const SANS: &str = ".SystemUIFont";
const SERIF: &str = "Iowan Old Style";
const MONO: &str = "Menlo";

struct Notary {
    meetings: Vec<Meeting>,
    selected: usize,
    recording: Option<Recording>,
    stopping: bool,
    status: SharedString,
    scroll: ScrollHandle,
    next_id: u64,
}

fn clock(secs: f32) -> String {
    let s = secs as u32;
    format!("{:02}:{:02}", s / 60, s % 60)
}

impl Notary {
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

        let id = self.next_id;
        self.next_id += 1;
        let now = chrono::Local::now();
        self.meetings.insert(
            0,
            Meeting {
                id,
                title: "New note".into(),
                when: now.format("%H:%M").to_string().into(),
                sub: "Recording".into(),
                meta: now.format("%a %b %-d \u{b7} %H:%M \u{b7} audio stays on this Mac").to_string().into(),
                live: true,
                lines: Vec::new(),
                summary: Vec::new(),
                actions: Vec::new(),
            },
        );
        self.selected = 0;

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

    fn apply(&mut self, id: u64, event: Event, cx: &mut Context<Self>) {
        match event {
            Event::Loading => self.status = "Loading the model".into(),
            Event::Listening => self.status = "Listening to your Mac's audio".into(),
            Event::Segment { start, text } => {
                if let Some(m) = self.meetings.iter_mut().find(|m| m.id == id) {
                    m.lines.push(line(&clock(start), "Call audio", Speaker::Other, &text));
                    m.sub = format!("{} lines", m.lines.len()).into();
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
                    m.sub = format!("{} lines \u{b7} just now", m.lines.len()).into();
                }
                let routine = ["Finishing", "Listening", "Loading"];
                if routine.iter().any(|p| self.status.starts_with(p)) {
                    self.status = "Saved in this session".into();
                }
            }
        }
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
            .child(
                div()
                    .text_size(px(13.))
                    .text_color(rgb(t.soft))
                    .child("Notary"),
            )
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
                        .child(
                            div()
                                .text_size(px(12.))
                                .text_color(rgb(t.soft))
                                .child("Recording"),
                        ),
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
        let mut items = div().flex().flex_col().px(px(6.)).pb(px(10.));
        for (i, m) in self.meetings.iter().enumerate() {
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
                                    .child(m.when.clone()),
                            ),
                    )
                    .child(
                        div()
                            .truncate()
                            .text_size(px(12.))
                            .text_color(rgb(t.soft))
                            .child(m.sub.clone()),
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
            .child(
                div()
                    .px(px(14.))
                    .pt(px(14.))
                    .pb(px(4.))
                    .text_size(px(11.))
                    .text_color(rgb(t.faint))
                    .child("TODAY"),
            )
            .child(items)
    }

    fn view(&self, t: &Theme, cx: &mut Context<Self>) -> impl IntoElement {
        let m = &self.meetings[self.selected];

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
        for line in &m.lines {
            let (who_color, note) = match line.who {
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
                .child(line.text.clone());
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
                            .child(line.ts.clone()),
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
                                    .child(line.name.clone()),
                            )
                            .child(text),
                    ),
            );
        }
        if m.lines.is_empty() {
            transcript = transcript.child(
                div()
                    .py(px(24.))
                    .text_size(px(13.5))
                    .text_color(rgb(t.faint))
                    .child("Nothing yet. Play a video or join a call and the words appear here."),
            );
        }

        let heading = |s: &'static str| {
            div()
                .font_family(SERIF)
                .text_size(px(14.))
                .text_color(rgb(t.ink))
                .child(s)
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

        let head = div()
            .flex()
            .flex_col()
            .gap(px(6.))
            .px(px(24.))
            .pt(px(18.))
            .pb(px(14.))
            .border_b_1()
            .border_color(rgb(t.line))
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
                    .child(m.meta.clone()),
            );

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
        let footbar = div()
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
            );

        div()
            .flex_1()
            .min_w_0()
            .h_full()
            .flex()
            .flex_col()
            .child(head)
            .child(
                div()
                    .flex_1()
                    .min_h_0()
                    .flex()
                    .child(transcript)
                    .child(side),
            )
            .child(footbar)
    }
}

impl Render for Notary {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = Theme::for_appearance(window.appearance());
        let live = self.recording.is_some();
        div()
            .size_full()
            .flex()
            .flex_col()
            .bg(rgb(t.paper))
            .text_color(rgb(t.ink))
            .font_family(SANS)
            .child(self.titlebar(&t, live))
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
            |_, cx| {
                cx.new(|_| Notary {
                    meetings: sample_meetings(),
                    selected: 0,
                    recording: None,
                    stopping: false,
                    status: "Ready".into(),
                    scroll: ScrollHandle::new(),
                    next_id: 100,
                })
            },
        )
        .unwrap();
        cx.activate(true);
    });
}
