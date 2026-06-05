use chrono::{Datelike, DateTime, NaiveDate, NaiveTime, Utc};
use log::{debug, info, warn};
use serde::Deserialize;
use std::sync::Arc;
use tokio::sync::Mutex;

/// Finnhub economic calendar event
#[derive(Debug, Clone, Deserialize)]
pub struct EconomicEvent {
    pub country: Option<String>,
    pub event: Option<String>,
    pub impact: Option<String>,
    pub date: Option<String>,    // "2026-06-05"
    pub time: Option<String>,    // "12:30 PM"
    #[serde(rename = "actual")]
    pub actual: Option<f64>,
    #[serde(rename = "estimate")]
    pub estimate: Option<f64>,
    #[serde(rename = "prev")]
    pub prev: Option<f64>,
}

#[derive(Debug, Clone, Deserialize)]
struct FinnhubCalendarResponse {
    #[serde(rename = "economicCalendar")]
    economic_calendar: Option<Vec<EconomicEvent>>,
}

/// Macro news filter — fetches economic calendar from Finnhub
/// and blocks trading around high-impact events
pub struct MacroFilter {
    api_key: String,
    client: reqwest::Client,
    /// Cached events for current window
    events: Arc<Mutex<Vec<EconomicEvent>>>,
    /// How many hours before/after event to block (high impact)
    block_hours_high: i64,
    /// How many hours before/after event to block (medium impact)
    block_hours_medium: i64,
    /// Last fetch date (to avoid re-fetching same day)
    last_fetch_date: Arc<Mutex<Option<NaiveDate>>>,
}

impl MacroFilter {
    pub fn new(api_key: String, block_hours_high: i64, block_hours_medium: i64) -> Self {
        let client = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(10))
            .build()
            .expect("Failed to build HTTP client");

        Self {
            api_key,
            client,
            events: Arc::new(Mutex::new(Vec::new())),
            block_hours_high,
            block_hours_medium,
            last_fetch_date: Arc::new(Mutex::new(None)),
        }
    }

    /// Fetch economic calendar from Finnhub for today + tomorrow
    pub async fn fetch_events(&self) -> Result<usize, String> {
        let today = Utc::now().date_naive();
        let tomorrow = today + chrono::Duration::days(1);

        let url = format!(
            "https://finnhub.io/api/v1/calendar/economic?from={}&to={}&token={}",
            today, tomorrow, self.api_key
        );

        let resp = self.client
            .get(&url)
            .send()
            .await
            .map_err(|e| format!("Finnhub request failed: {}", e))?;

        if !resp.status().is_success() {
            return Err(format!("Finnhub returned status {}", resp.status()));
        }

        let body: FinnhubCalendarResponse = resp
            .json()
            .await
            .map_err(|e| format!("Finnhub parse error: {}", e))?;

        let all_events = body.economic_calendar.unwrap_or_default();

        // Filter: only US events with High or Medium impact
        let relevant: Vec<EconomicEvent> = all_events
            .into_iter()
            .filter(|e| {
                let country = e.country.as_deref().unwrap_or("");
                let impact = e.impact.as_deref().unwrap_or("");
                (country == "US" || country == "GLOBAL")
                    && (impact == "High" || impact == "Medium")
            })
            .collect();

        let count = relevant.len();
        info!("Macro filter: fetched {} relevant US events (High/Medium impact)", count);

        for ev in &relevant {
            info!(
                "  📅 {} | {} | impact={} | {}",
                ev.date.as_deref().unwrap_or("?"),
                ev.event.as_deref().unwrap_or("unknown"),
                ev.impact.as_deref().unwrap_or("?"),
                ev.time.as_deref().unwrap_or("?"),
            );
        }

        let mut events = self.events.lock().await;
        *events = relevant;

        let mut last = self.last_fetch_date.lock().await;
        *last = Some(today);

        Ok(count)
    }

    /// Auto-fetch once per day (call from poller)
    pub async fn maybe_refresh(&self) {
        let today = Utc::now().date_naive();
        let last = self.last_fetch_date.lock().await;
        if *last == Some(today) {
            return; // already fetched today
        }
        drop(last);

        match self.fetch_events().await {
            Ok(n) => debug!("Macro filter refreshed: {} events", n),
            Err(e) => warn!("Macro filter refresh failed: {}", e),
        }
    }

    /// Check if trading should be blocked right now due to macro event
    /// Returns Some(event_name) if blocked, None if OK to trade
    pub async fn should_block(&self) -> Option<String> {
        let now = Utc::now();
        let events = self.events.lock().await;

        for ev in events.iter() {
            let event_time = match parse_event_datetime(ev) {
                Some(t) => t,
                None => continue,
            };

            let impact = ev.impact.as_deref().unwrap_or("");
            let block_hours = match impact {
                "High" => self.block_hours_high,
                "Medium" => self.block_hours_medium,
                _ => continue,
            };

            // Use seconds for precise comparison instead of truncated hours
            let diff_seconds = (now - event_time).num_seconds().unsigned_abs();
            let block_seconds = (block_hours as u64) * 3600;
            if diff_seconds <= block_seconds {
                let diff_hours = diff_seconds / 3600;
                let name = ev.event.as_deref().unwrap_or("unknown event");
                let reason = format!(
                    "{} ({}) — {}h away (block={}h)",
                    name, impact, diff_hours, block_hours
                );
                info!("Macro BLOCK: {}", reason);
                return Some(reason);
            }
        }

        None
    }
}

/// Parse event date + time into UTC DateTime
/// Finnhub time format: "12:30 PM" (ET/EST) or "All Day"
/// We approximate: US Eastern = UTC-5 (EST) or UTC-4 (EDT)
/// For simplicity, assume EDT (UTC-4) during summer, EST (UTC-5) during winter
fn parse_event_datetime(ev: &EconomicEvent) -> Option<DateTime<Utc>> {
    let date_str = ev.date.as_deref()?;
    let time_str = ev.time.as_deref()?;

    if time_str == "All Day" || time_str.is_empty() {
        // For all-day events, use midnight UTC — effectively blocks the whole day
        let date = NaiveDate::parse_from_str(date_str, "%Y-%m-%d").ok()?;
        return Some(date.and_hms_opt(0, 0, 0)?.and_utc());
    }

    let date = NaiveDate::parse_from_str(date_str, "%Y-%m-%d").ok()?;

    // Parse time like "12:30 PM" or "8:30 AM"
    let time = NaiveTime::parse_from_str(time_str.trim(), "%I:%M %P")
        .or_else(|_| NaiveTime::parse_from_str(time_str.trim(), "%H:%M"))
        .ok()?;

    // Finnhub times are in US Eastern time
    // US DST: starts 2nd Sunday of March, ends 1st Sunday of November
    // EDT = UTC-4, EST = UTC-5
    let month = date.month();
    let day = date.day();
    let utc_offset_hours = match month {
        1 | 2 => 5,                                          // Jan-Feb: EST
        12 => 5,                                              // Dec: EST
        4..=10 => 4,                                         // Apr-Oct: EDT
        3 => {
            // March: EDT starts on 2nd Sunday
            // 2nd Sunday = day 8-14
            if day >= 8 && date.weekday() == chrono::Weekday::Sun {
                4 // DST starts this day
            } else if day > 14 {
                4
            } else {
                5 // Still EST
            }
        }
        11 => {
            // November: EST starts on 1st Sunday
            // 1st Sunday = day 1-7
            if day <= 7 && date.weekday() == chrono::Weekday::Sun {
                5 // DST ends this day
            } else if day > 7 {
                5
            } else {
                4 // Still EDT
            }
        }
        _ => 5, // fallback
    };

    let naive_dt = date.and_time(time);
    let utc_dt = naive_dt + chrono::Duration::hours(utc_offset_hours);

    Some(utc_dt.and_utc())
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Timelike;

    #[test]
    fn test_parse_event_datetime() {
        let ev = EconomicEvent {
            country: Some("US".into()),
            event: Some("Non Farm Payrolls".into()),
            impact: Some("High".into()),
            date: Some("2026-06-05".into()),
            time: Some("8:30 AM".into()),
            actual: None,
            estimate: None,
            prev: None,
        };
        let dt = parse_event_datetime(&ev).unwrap();
        // June = EDT (UTC-4), 8:30 AM ET = 12:30 UTC
        assert_eq!(dt.hour(), 12);
        assert_eq!(dt.minute(), 30);
    }
}
