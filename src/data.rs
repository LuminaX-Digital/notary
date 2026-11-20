use gpui::SharedString;

pub enum Speaker {
    Me,
    Other,
    Note,
}

pub struct Line {
    pub ts: SharedString,
    pub name: SharedString,
    pub who: Speaker,
    pub text: SharedString,
}

pub struct Action {
    pub label: &'static str,
    pub owner: &'static str,
    pub done: bool,
}

pub struct Meeting {
    pub id: u64,
    pub title: SharedString,
    pub when: SharedString,
    pub sub: SharedString,
    pub meta: SharedString,
    pub live: bool,
    pub lines: Vec<Line>,
    pub summary: Vec<(&'static str, &'static str)>,
    pub actions: Vec<Action>,
}

pub fn line(ts: &str, name: &str, who: Speaker, text: &str) -> Line {
    Line {
        ts: ts.to_string().into(),
        name: name.to_string().into(),
        who,
        text: text.to_string().into(),
    }
}

fn action(label: &'static str, owner: &'static str, done: bool) -> Action {
    Action { label, owner, done }
}

pub fn sample_meetings() -> Vec<Meeting> {
    vec![
        Meeting {
            id: 1,
            title: "Launch planning \u{2014} v0.3".into(),
            when: "14:02".into(),
            sub: "Fabian, Lena \u{b7} Zoom \u{b7} recording".into(),
            meta: "Thu Nov 20 \u{b7} 14:02 \u{b7} Zoom \u{b7} audio stays on this Mac".into(),
            live: true,
            lines: vec![
                line("00:12", "Lena", Speaker::Other, "Let's keep v0.3 to the Mac audio tap and the summary panel. Windows can wait another two weeks."),
                line("00:38", "Fabian", Speaker::Me, "Agreed. The ScreenCaptureKit tap works on 14.4 and above, so I'll mark older macOS as a known limit in the release notes."),
                line("01:02", "You wrote", Speaker::Note, "Ship with the multilingual turbo model by default. English and German in one download."),
                line("01:40", "Lena", Speaker::Other, "Can we name the speakers from the names shown in the Zoom window?"),
                line("02:05", "Fabian", Speaker::Me, "Yes, but through the accessibility tree instead of screenshots. My spike stays under two percent CPU during a call."),
                line("03:11", "Lena", Speaker::Other, "If the beta isn't out by December 4, the MCP server moves to v0.4."),
                line("03:44", "Fabian", Speaker::Me, "Fair. A signed DMG by Monday."),
            ],
            summary: vec![
                ("Plan", "Ship v0.3 on Mac only: system audio capture, local transcription and the summary panel. Older macOS is a documented limit."),
                ("Decision", "If the beta is not signed and out by December 4, the MCP server moves to v0.4. One multilingual model covers English and German."),
            ],
            actions: vec![
                action("Confirm minimum macOS for the audio tap", "Fabian", true),
                action("Signed DMG of the v0.3 beta by Monday", "Fabian", false),
                action("Write the speaker naming note for the README", "Lena", false),
                action("Decide the MCP cutoff by December 4", "Both", false),
            ],
        },
        Meeting {
            id: 2,
            title: "Weekly 1:1 with Lena".into(),
            when: "10:30".into(),
            sub: "Lena \u{b7} Google Meet \u{b7} 32 min".into(),
            meta: "Thu Nov 20 \u{b7} 10:30 \u{b7} Google Meet \u{b7} 32 min".into(),
            live: false,
            lines: vec![
                line("00:05", "Lena", Speaker::Other, "The onboarding flow still asks for three permissions on the first screen. That feels like a lot."),
                line("00:31", "Fabian", Speaker::Me, "We can ask for the microphone later and explain screen recording first, with the sound it controls."),
                line("01:12", "Lena", Speaker::Other, "I'd also like the reminder to be a plain notification, nothing fancy."),
            ],
            summary: vec![
                ("Onboarding", "Ask for screen recording first and the microphone later. Each permission shows what it controls."),
                ("Reminders", "Recording reminders stay a plain system notification."),
            ],
            actions: vec![
                action("Reorder the permission screens", "Fabian", false),
                action("Draft the reminder copy", "Lena", true),
            ],
        },
        Meeting {
            id: 3,
            title: "Kantwerk \u{2014} AGB Durchgang".into(),
            when: "Gestern".into(),
            sub: "Fabian, Rechtsberatung \u{b7} 47 min".into(),
            meta: "Wed Nov 19 \u{b7} 16:15 \u{b7} Deutsch \u{b7} 47 min".into(),
            live: false,
            lines: vec![
                line("00:08", "Beratung", Speaker::Other, "Der Auftragsverarbeitungsvertrag wird Teil jedes Vertrags, das regeln wir in Paragraph zehn der AGB."),
                line("00:52", "Fabian", Speaker::Me, "Dann ver\u{f6}ffentlichen wir den AVV unter einer eigenen Adresse und verlinken ihn im Checkout."),
                line("01:30", "Beratung", Speaker::Other, "Wichtig ist, dass der Kunde das Kontrollk\u{e4}stchen aktiv setzt. Nur dann gilt er als vereinbart."),
            ],
            summary: vec![
                ("Ergebnis", "Der AVV wird Teil jedes Vertrags (AGB \u{a7} 10), liegt unter eigener Adresse und wird im Checkout verlinkt."),
                ("Offen", "Das Kontrollk\u{e4}stchen im Checkout muss aktiv gesetzt werden."),
            ],
            actions: vec![
                action("AVV-Seite ver\u{f6}ffentlichen", "Fabian", false),
                action("Checkbox im Checkout einbauen", "Fabian", false),
            ],
        },
    ]
}
