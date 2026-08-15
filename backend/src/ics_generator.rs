use chrono::{NaiveDate, TimeZone, Utc};

use crate::event::EventProjection;

pub struct IcsCalendar {
    name: String,
    events: Vec<IcsEvent>,
}

pub struct IcsEvent {
    pub uid: String,
    pub summary: String,
    pub description: Option<String>,
    pub location: Option<String>,
    pub status: Option<String>,
    pub timing: IcsTiming,
    pub dtstamp: i64,
    pub sequence: u64,
}

pub enum IcsTiming {
    Timed {
        start_utc: i64,
        end_utc: i64,
        tzid: Option<String>,
    },
    AllDay {
        start_date: String,
        end_date: String,
    },
}

impl IcsCalendar {
    pub fn new(name: String) -> Self {
        Self {
            name,
            events: Vec::new(),
        }
    }

    pub fn add_event(&mut self, event: IcsEvent) {
        self.events.push(event);
    }

    pub fn serialize(&self) -> String {
        let mut output = String::new();
        output.push_str("BEGIN:VCALENDAR\r\n");
        output.push_str("VERSION:2.0\r\n");
        output.push_str("PRODID:-//CommonCal//CommonCal//EN\r\n");
        output.push_str(&format!("X-WR-CALNAME:{}\r\n", escape_text(&self.name)));
        for event in &self.events {
            output.push_str(&event.to_ics());
        }
        output.push_str("END:VCALENDAR\r\n");
        output
    }
}

impl IcsEvent {
    pub fn to_ics(&self) -> String {
        let mut output = String::new();
        output.push_str("BEGIN:VEVENT\r\n");
        output.push_str(&self.uid_line());
        output.push_str(&format!("SUMMARY:{}\r\n", escape_text(&self.summary)));
        if let Some(ref description) = self.description {
            output.push_str(&format!("DESCRIPTION:{}\r\n", escape_text(description)));
        }
        if let Some(ref location) = self.location {
            output.push_str(&format!("LOCATION:{}\r\n", escape_text(location)));
        }
        if let Some(ref status) = self.status {
            output.push_str(&format!("STATUS:{}\r\n", status));
        }
        output.push_str(&self.timing_line());
        output.push_str(&format!("DTSTAMP:{}\r\n", utc_timestamp(self.dtstamp)));
        output.push_str(&format!("SEQUENCE:{}\r\n", self.sequence));
        output.push_str("END:VEVENT\r\n");
        output
    }

    fn uid_line(&self) -> String {
        format!("UID:{}\r\n", self.uid)
    }

    fn timing_line(&self) -> String {
        match &self.timing {
            IcsTiming::Timed {
                start_utc,
                end_utc,
                tzid,
            } => {
                let mut line = String::new();
                if let Some(tzid) = tzid {
                    line.push_str(&format!(
                        "DTSTART;TZID={}:{}\r\n",
                        tzid,
                        utc_timestamp(*start_utc)
                    ));
                } else {
                    line.push_str(&format!("DTSTART:{}\r\n", utc_timestamp(*start_utc)));
                }
                line.push_str(&format!("DTEND:{}\r\n", utc_timestamp(*end_utc)));
                line
            }
            IcsTiming::AllDay {
                start_date,
                end_date,
            } => {
                format!(
                    "DTSTART;VALUE=DATE:{}\r\nDTEND;VALUE=DATE:{}\r\n",
                    start_date, end_date
                )
            }
        }
    }
}

pub fn project_events_to_ics(view_name: &str, events: &[EventProjection]) -> IcsCalendar {
    let mut calendar = IcsCalendar::new(view_name.to_owned());
    for event in events {
        let timing = match (
            &event.start_utc,
            &event.end_utc,
            &event.start_date,
            &event.end_date,
        ) {
            (Some(start_utc), Some(end_utc), None, None) => IcsTiming::Timed {
                start_utc: *start_utc,
                end_utc: *end_utc,
                tzid: event.timezone.clone(),
            },
            (None, None, Some(start_date), Some(end_date)) => {
                // RFC 5545: exclusive end for all-day events
                let end = NaiveDate::parse_from_str(end_date, "%Y-%m-%d")
                    .map(|d| {
                        d.checked_sub_signed(chrono::Duration::days(1))
                            .unwrap_or_else(|| {
                                NaiveDate::parse_from_str(start_date, "%Y-%m-%d").unwrap()
                            })
                    })
                    .unwrap_or_else(|_| NaiveDate::parse_from_str(start_date, "%Y-%m-%d").unwrap());
                IcsTiming::AllDay {
                    start_date: start_date.clone(),
                    end_date: end.format("%Y%m%d").to_string(),
                }
            }
            _ => continue,
        };
        let uid = format!(
            "{}:{}:{}",
            0,
            event.id,
            event.recurrence_id.unwrap_or(0).to_string()
        );
        let recurrence_id = event.recurrence_date.as_deref().unwrap_or("");
        let uid = if !recurrence_id.is_empty() {
            format!("{}:{}", uid, recurrence_id)
        } else {
            uid
        };
        calendar.add_event(IcsEvent {
            uid,
            summary: event.title.clone().unwrap_or_default(),
            description: event.description.clone(),
            location: event.location.clone(),
            status: Some(event.status).map(|s| match s {
                "tentative" => "TENTATIVE".to_string(),
                "confirmed" => "CONFIRMED".to_string(),
                "cancelled" => "CANCELLED".to_string(),
                _ => "CONFIRMED".to_string(),
            }),
            timing,
            dtstamp: event.created_at.unwrap_or(0),
            sequence: event.version.unwrap_or(1) as u64,
        });
    }
    calendar
}

fn utc_timestamp(seconds: i64) -> String {
    Utc.timestamp_opt(seconds, 0)
        .single()
        .map(|dt| dt.format("%Y%m%dT%H%M%SZ").to_string())
        .unwrap_or_default()
}

fn escape_text(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for ch in text.chars() {
        match ch {
            '\\' => out.push_str("\\\\"),
            ';' => out.push_str("\\;"),
            ',' => out.push_str("\\,"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\n"),
            _ => out.push(ch),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ics_timed_event_output() {
        let mut calendar = IcsCalendar::new("Test".to_string());
        calendar.add_event(IcsEvent {
            uid: "test-1".to_string(),
            summary: "Test Event".to_string(),
            description: None,
            location: None,
            status: None,
            timing: IcsTiming::Timed {
                start_utc: 1768435200,
                end_utc: 1768438800,
                tzid: None,
            },
            dtstamp: 1768435200,
            sequence: 1,
        });
        let ics = calendar.serialize();
        assert!(ics.contains("BEGIN:VCALENDAR"));
        assert!(ics.contains("BEGIN:VEVENT"));
        assert!(ics.contains("UID:test-1"));
        assert!(ics.contains("SUMMARY:Test Event"));
        assert!(ics.contains("DTSTART:20260115T000000Z"));
        assert!(ics.contains("DTEND:20260115T010000Z"));
        assert!(ics.contains("END:VEVENT"));
        assert!(ics.contains("END:VCALENDAR"));
    }

    #[test]
    fn ics_all_day_event_output() {
        let mut calendar = IcsCalendar::new("Test".to_string());
        calendar.add_event(IcsEvent {
            uid: "test-1".to_string(),
            summary: "All Day".to_string(),
            description: None,
            location: None,
            status: None,
            timing: IcsTiming::AllDay {
                start_date: "20260115".to_string(),
                end_date: "20260116".to_string(),
            },
            dtstamp: 1768454400,
            sequence: 1,
        });
        let ics = calendar.serialize();
        assert!(ics.contains("DTSTART;VALUE=DATE:20260115"));
        assert!(ics.contains("DTEND;VALUE=DATE:20260116"));
    }

    #[test]
    fn ics_all_day_end_date_exclusive() {
        let mut calendar = IcsCalendar::new("Test".to_string());
        calendar.add_event(IcsEvent {
            uid: "test-1".to_string(),
            summary: "3 Day".to_string(),
            description: None,
            location: None,
            status: None,
            timing: IcsTiming::AllDay {
                start_date: "20260101".to_string(),
                end_date: "20260103".to_string(),
            },
            dtstamp: 1768454400,
            sequence: 1,
        });
        let ics = calendar.serialize();
        assert!(ics.contains("DTEND;VALUE=DATE:20260103"));
    }

    #[test]
    fn ics_empty_calendar() {
        let calendar = IcsCalendar::new("Empty".to_string());
        let ics = calendar.serialize();
        assert!(ics.contains("BEGIN:VCALENDAR"));
        assert!(ics.contains("END:VCALENDAR"));
        assert!(!ics.contains("BEGIN:VEVENT"));
    }

    #[test]
    fn ics_description_escaping() {
        let mut calendar = IcsCalendar::new("Test".to_string());
        calendar.add_event(IcsEvent {
            uid: "test-1".to_string(),
            summary: "Escaped".to_string(),
            description: Some("line1\\nline2;semi,comma".to_string()),
            location: None,
            status: None,
            timing: IcsTiming::Timed {
                start_utc: 1768454400,
                end_utc: 1768458000,
                tzid: None,
            },
            dtstamp: 1768454400,
            sequence: 1,
        });
        let ics = calendar.serialize();
        assert!(ics.contains("DESCRIPTION:line1"));
        assert!(ics.contains("line2"));
        assert!(ics.contains("\\;semi"));
        assert!(ics.contains("\\,comma"));
    }

    #[test]
    fn ics_timezone_preserved() {
        let mut calendar = IcsCalendar::new("Test".to_string());
        calendar.add_event(IcsEvent {
            uid: "test-1".to_string(),
            summary: "TZ Event".to_string(),
            description: None,
            location: None,
            status: None,
            timing: IcsTiming::Timed {
                start_utc: 1768483200,
                end_utc: 1768486800,
                tzid: Some("America/New_York".to_string()),
            },
            dtstamp: 1768483200,
            sequence: 1,
        });
        let ics = calendar.serialize();
        assert!(ics.contains("DTSTART;TZID=America/New_York"));
    }
}
