//! Parses the `--deadline` flag: either a full RFC 3339 timestamp, or a bare
//! `HH:MM` local time (Europe/Oslo) meaning "the next occurrence of that
//! clock time" — mirroring the dashboard's datetime picker, which is always
//! local time too.

use anyhow::{anyhow, Result};
use chrono::{DateTime, Duration, NaiveTime, TimeZone, Utc};
use chrono_tz::Europe::Oslo;

pub fn parse(input: &str) -> Result<DateTime<Utc>> {
    let input = input.trim();

    if let Ok(dt) = DateTime::parse_from_rfc3339(input) {
        return Ok(dt.with_timezone(&Utc));
    }

    if let Ok(time) = NaiveTime::parse_from_str(input, "%H:%M") {
        let now_local = Utc::now().with_timezone(&Oslo);
        let today = now_local.date_naive();
        let candidate = Oslo
            .from_local_datetime(&today.and_time(time))
            .single()
            .ok_or_else(|| anyhow!("that local time is ambiguous around a DST change — use a full RFC 3339 timestamp instead"))?;
        let candidate = if candidate <= now_local {
            candidate + Duration::days(1)
        } else {
            candidate
        };
        return Ok(candidate.with_timezone(&Utc));
    }

    Err(anyhow!(
        "couldn't parse deadline {input:?} — use HH:MM (Europe/Oslo, next occurrence) or a full RFC 3339 timestamp like 2026-09-05T07:00:00+02:00"
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_garbage() {
        assert!(parse("not a time").is_err());
    }

    #[test]
    fn accepts_rfc3339() {
        let dt = parse("2030-01-01T12:00:00Z").unwrap();
        assert_eq!(dt.to_rfc3339(), "2030-01-01T12:00:00+00:00");
    }

    #[test]
    fn accepts_hh_mm_and_lands_in_the_future() {
        let dt = parse("23:59").unwrap();
        assert!(dt > Utc::now());
    }
}
