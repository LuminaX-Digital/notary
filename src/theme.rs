use gpui::WindowAppearance;

pub struct Theme {
    pub paper: u32,
    pub raised: u32,
    pub line: u32,
    pub ink: u32,
    pub soft: u32,
    pub faint: u32,
    pub rail: u32,
    pub accent: u32,
    pub accent_soft: u32,
    pub record: u32,
    pub focus: u32,
    pub note: u32,
}

impl Theme {
    pub fn for_appearance(a: WindowAppearance) -> Self {
        match a {
            WindowAppearance::Dark | WindowAppearance::VibrantDark => Self::dark(),
            _ => Self::light(),
        }
    }

    fn light() -> Self {
        Self {
            paper: 0xF5F2EC,
            raised: 0xFBF9F4,
            line: 0xE6E0D2,
            ink: 0x17161A,
            soft: 0x4A4753,
            faint: 0x8A8597,
            rail: 0xECE7DC,
            accent: 0x1F3A68,
            accent_soft: 0xD7DEEA,
            record: 0xB8392E,
            focus: 0xC9B979,
            note: 0xF1EBD0,
        }
    }

    fn dark() -> Self {
        Self {
            paper: 0x0E0E12,
            raised: 0x16161C,
            line: 0x23232B,
            ink: 0xEDEAE2,
            soft: 0xADAAB4,
            faint: 0x70707C,
            rail: 0x1A1A21,
            accent: 0x8AA6D8,
            accent_soft: 0x1E2638,
            record: 0xD96357,
            focus: 0xC9B979,
            note: 0x25231A,
        }
    }
}
