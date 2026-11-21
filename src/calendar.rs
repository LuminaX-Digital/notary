//! Calendar logic: call links, what is coming up, and which event a recording belongs to.

use crate::native::CalendarEvent;
use chrono::{DateTime, FixedOffset};

const CALL_HOSTS: [&str; 10] = [
    "zoom.us/",
    "zoom.com/",
    "meet.google.com/",
    "teams.microsoft.com/",
    "teams.live.com/",
    "webex.com/",
    "whereby.com/",
    "facetime.apple.com/",
    "discord.gg/",
    "gotomeeting.com/",
];

/// The first call link in an event's link, location or notes.
pub fn meeting_link(e: &CalendarEvent) -> Option<String> {
    [e.url.as_deref(), e.location.as_deref(), e.notes.as_deref()].into_iter().flatten().find_map(find_link)
}

fn find_link(text: &str) -> Option<String> {
    let mut rest = text;
    while let Some(i) = rest.find("https://") {
        let tail = &rest[i..];
        let end = tail.find(|c: char| c.is_whitespace() || "\"'<>)".contains(c)).unwrap_or(tail.len());
        let url = &tail[..end];
        if CALL_HOSTS.iter().any(|h| url.contains(h)) {
            return Some(url.trim_end_matches(['.', ',', ';']).to_string());
        }
        rest = &tail[8..];
    }
    None
}

pub fn start_ts(e: &CalendarEvent) -> Option<i64> {
    DateTime::<FixedOffset>::parse_from_rfc3339(&e.start).ok().map(|d| d.timestamp())
}

pub fn end_ts(e: &CalendarEvent) -> Option<i64> {
    DateTime::<FixedOffset>::parse_from_rfc3339(&e.end).ok().map(|d| d.timestamp())
}

/// Events worth showing: not declined, not in a hidden calendar, optionally only those with a call link.
pub fn visible<'a>(events: &'a [CalendarEvent], hidden_calendars: &[String], only_with_link: bool) -> Vec<&'a CalendarEvent> {
    events
        .iter()
        .filter(|e| !e.declined && !hidden_calendars.contains(&e.calendar_id))
        .filter(|e| !only_with_link || meeting_link(e).is_some())
        .collect()
}

/// Events that have not ended yet, soonest first.
pub fn upcoming<'a>(events: &[&'a CalendarEvent], now: i64, limit: usize) -> Vec<&'a CalendarEvent> {
    let mut v: Vec<&CalendarEvent> = events.iter().copied().filter(|e| end_ts(e).is_some_and(|end| end > now)).collect();
    v.sort_by_key(|e| start_ts(e).unwrap_or(i64::MAX));
    v.truncate(limit);
    v
}

/// The event a call at `now` most likely belongs to: started up to 10 minutes ago (or starts within 10),
/// not over yet; one with a call link wins, then the nearest start.
pub fn match_event<'a>(events: &[&'a CalendarEvent], now: i64) -> Option<&'a CalendarEvent> {
    const WINDOW: i64 = 10 * 60;
    events
        .iter()
        .copied()
        .filter_map(|e| {
            let (start, end) = (start_ts(e)?, end_ts(e)?);
            (start - WINDOW <= now && now < end).then_some((e, (start - now).abs()))
        })
        .min_by_key(|(e, distance)| (meeting_link(e).is_none(), *distance))
        .map(|(e, _)| e)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::native::Attendee;

    fn event(title: &str, start: &str, end: &str) -> CalendarEvent {
        CalendarEvent {
            id: title.into(),
            title: title.into(),
            start: start.into(),
            end: end.into(),
            calendar: "Work".into(),
            calendar_id: "c1".into(),
            location: None,
            notes: None,
            url: None,
            me: None,
            declined: false,
            attendees: vec![Attendee { name: "Lena".into(), email: "l@example.com".into() }],
        }
    }

    fn ts(s: &str) -> i64 {
        DateTime::<FixedOffset>::parse_from_rfc3339(s).unwrap().timestamp()
    }

    #[test]
    fn finds_call_links_in_url_location_or_notes() {
        let mut e = event("A", "2026-11-21T10:00:00+01:00", "2026-11-21T11:00:00+01:00");
        assert!(meeting_link(&e).is_none());
        e.notes = Some("Join: https://us02web.zoom.us/j/12345?pwd=abc. See you".into());
        assert_eq!(meeting_link(&e).unwrap(), "https://us02web.zoom.us/j/12345?pwd=abc");
        e.url = Some("https://meet.google.com/abc-defg-hij".into());
        assert_eq!(meeting_link(&e).unwrap(), "https://meet.google.com/abc-defg-hij");
        e.url = None;
        e.notes = Some("Agenda at https://docs.example.com/x".into());
        e.location = Some("<https://teams.microsoft.com/l/meetup-join/xyz>".into());
        assert_eq!(meeting_link(&e).unwrap(), "https://teams.microsoft.com/l/meetup-join/xyz");
    }

    #[test]
    fn hides_declined_and_hidden_calendar_events() {
        let a = event("A", "2026-11-21T10:00:00+01:00", "2026-11-21T11:00:00+01:00");
        let mut b = a.clone();
        b.declined = true;
        let mut c = a.clone();
        c.calendar_id = "private".into();
        let all = vec![a, b, c];
        assert_eq!(visible(&all, &["private".to_string()], false).len(), 1);
    }

    #[test]
    fn upcoming_skips_finished_events_and_sorts() {
        let early = event("early", "2026-11-21T09:00:00+01:00", "2026-11-21T09:30:00+01:00");
        let later = event("later", "2026-11-21T15:00:00+01:00", "2026-11-21T16:00:00+01:00");
        let soon = event("soon", "2026-11-21T11:00:00+01:00", "2026-11-21T12:00:00+01:00");
        let all = vec![later, early, soon];
        let refs: Vec<&CalendarEvent> = all.iter().collect();
        let now = ts("2026-11-21T10:00:00+01:00");
        let titles: Vec<&str> = upcoming(&refs, now, 5).iter().map(|e| e.title.as_str()).collect();
        assert_eq!(titles, vec!["soon", "later"]);
    }

    #[test]
    fn matches_the_event_around_now_and_prefers_a_call_link() {
        let mut with_link = event("With link", "2026-11-21T10:05:00+01:00", "2026-11-21T11:00:00+01:00");
        with_link.url = Some("https://zoom.us/j/1".into());
        let nearer = event("Nearer", "2026-11-21T10:00:00+01:00", "2026-11-21T11:00:00+01:00");
        let over = event("Over", "2026-11-21T08:00:00+01:00", "2026-11-21T09:00:00+01:00");
        let all = vec![nearer, with_link, over];
        let refs: Vec<&CalendarEvent> = all.iter().collect();
        let now = ts("2026-11-21T10:01:00+01:00");
        assert_eq!(match_event(&refs, now).unwrap().title, "With link");
        assert!(match_event(&refs, ts("2026-11-21T13:00:00+01:00")).is_none());
    }
}
