use crate::{
    config::Config,
    google_auth,
    memory::{Chunk, Store},
};
use anyhow::{Context, Result};
use chrono::Utc;
use serde::Deserialize;
use tracing::info;

const CALENDAR_API: &str = "https://www.googleapis.com/calendar/v3";

#[derive(Deserialize)]
struct CalendarList {
    #[serde(default)]
    items: Vec<Calendar>,
}

#[derive(Deserialize)]
struct Calendar {
    id: String,
    summary: String,
    #[serde(default)]
    primary: bool,
}

#[derive(Deserialize)]
struct EventList {
    #[serde(default)]
    items: Vec<Event>,
}

#[derive(Deserialize)]
struct Event {
    id: String,
    summary: Option<String>,
    description: Option<String>,
    location: Option<String>,
    #[serde(rename = "htmlLink")]
    html_link: Option<String>,
    start: EventTime,
    end: EventTime,
    #[serde(default)]
    attendees: Vec<Attendee>,
}

#[derive(Deserialize)]
struct EventTime {
    #[serde(rename = "dateTime")]
    date_time: Option<String>,
    date: Option<String>,
}

#[derive(Deserialize)]
struct Attendee {
    email: String,
    #[serde(rename = "responseStatus", default)]
    response_status: String,
}

pub async fn pull(cfg: &Config, store: &Store) -> Result<()> {
    let (client_id, client_secret, refresh_token) = cfg
        .google_creds()
        .context("Google credentials not set — run `marrow auth google` first")?;

    let client = reqwest::Client::builder()
        .user_agent("marrow/0.1")
        .build()?;

    let token = google_auth::access_token(&client, client_id, client_secret, refresh_token).await?;

    // Pull next 14 days of events from primary calendar
    let time_min = Utc::now().to_rfc3339();
    let time_max = (Utc::now() + chrono::Duration::days(14)).to_rfc3339();

    info!("Pulling Calendar — next 14 days");

    let calendars = fetch_calendars(&client, &token).await?;
    let primary_cal = calendars.iter().find(|c| c.primary);
    let primary_id = primary_cal.map(|c| c.id.as_str()).unwrap_or("primary");
    let primary_name = primary_cal.map(|c| c.summary.as_str()).unwrap_or("primary");

    info!("Pulling calendar '{primary_name}'");

    let events = fetch_events(&client, &token, primary_id, &time_min, &time_max).await?;
    info!("{} events found", events.len());

    let mut total = 0usize;
    for event in &events {
        let title = event.summary.as_deref().unwrap_or("(no title)");
        let start = event
            .start
            .date_time
            .as_deref()
            .or(event.start.date.as_deref())
            .unwrap_or("?");
        let end = event
            .end
            .date_time
            .as_deref()
            .or(event.end.date.as_deref())
            .unwrap_or("?");

        let attendee_list: Vec<_> = event
            .attendees
            .iter()
            .filter(|a| a.response_status != "declined")
            .map(|a| a.email.as_str())
            .collect();

        let content = format!(
            "Start: {start}\nEnd: {end}\nLocation: {}\nAttendees: {}\n\n{}",
            event.location.as_deref().unwrap_or("—"),
            if attendee_list.is_empty() {
                "none".into()
            } else {
                attendee_list.join(", ")
            },
            event
                .description
                .as_deref()
                .unwrap_or("")
                .chars()
                .take(400)
                .collect::<String>()
        );

        store.ingest(&Chunk {
            source: "calendar".into(),
            source_id: event.id.clone(),
            title: format!("Event: {title} ({start})"),
            content,
            url: event.html_link.clone(),
            tags: vec!["calendar".into(), "event".into()],
        })?;
        total += 1;
    }

    store.log_pull("calendar", total)?;
    info!("Calendar pull complete: {} events", total);
    Ok(())
}

async fn fetch_calendars(client: &reqwest::Client, token: &str) -> Result<Vec<Calendar>> {
    Ok(client
        .get(format!("{CALENDAR_API}/users/me/calendarList"))
        .bearer_auth(token)
        .send()
        .await?
        .error_for_status()?
        .json::<CalendarList>()
        .await?
        .items)
}

async fn fetch_events(
    client: &reqwest::Client,
    token: &str,
    calendar_id: &str,
    time_min: &str,
    time_max: &str,
) -> Result<Vec<Event>> {
    let url = format!(
        "{CALENDAR_API}/calendars/{}/events?singleEvents=true&orderBy=startTime&timeMin={}&timeMax={}&maxResults=50",
        urlencoding::encode(calendar_id),
        urlencoding::encode(time_min),
        urlencoding::encode(time_max)
    );
    Ok(client
        .get(&url)
        .bearer_auth(token)
        .send()
        .await?
        .error_for_status()?
        .json::<EventList>()
        .await?
        .items)
}
