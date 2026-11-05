use crate::{
    config::Config,
    google_auth,
    memory::{Chunk, Store},
};
use anyhow::{Context, Result};
use chrono::Utc;
use serde::Deserialize;
use tracing::{info, warn};

const CALENDAR_API: &str = "https://www.googleapis.com/calendar/v3";
const PER_PAGE: u32 = 100;
/// Safety cap so a misbehaving API cannot spin the page loop forever.
const MAX_PAGES: u32 = 10;

#[derive(Deserialize)]
struct CalendarList {
    #[serde(default)]
    items: Vec<Calendar>,
    #[serde(rename = "nextPageToken")]
    next_page_token: Option<String>,
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
    #[serde(rename = "nextPageToken")]
    next_page_token: Option<String>,
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
    let started = Utc::now();
    let (client_id, client_secret, refresh_token) = cfg
        .google_creds()
        .context("Google credentials not set — run `rustmarrow auth google` first")?;

    let client = reqwest::Client::builder()
        .user_agent("marrow/0.1")
        .build()?;

    let token = google_auth::access_token(&client, client_id, client_secret, refresh_token).await?;

    // Pull next 14 days of events from primary calendar
    let time_min = Utc::now().to_rfc3339();
    let time_max = (Utc::now() + chrono::Duration::days(14)).to_rfc3339();

    info!("Pulling Calendar — next 14 days");

    let calendars = fetch_calendars(&client, &token, CALENDAR_API).await?;
    let primary_cal = calendars.iter().find(|c| c.primary);
    let primary_id = primary_cal.map(|c| c.id.as_str()).unwrap_or("primary");
    let primary_name = primary_cal.map(|c| c.summary.as_str()).unwrap_or("primary");

    info!("Pulling calendar '{primary_name}'");

    let events = fetch_events(
        &client,
        &token,
        CALENDAR_API,
        primary_id,
        &time_min,
        &time_max,
    )
    .await?;
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

    store.log_pull("calendar", total, started)?;
    info!("Calendar pull complete: {} events", total);
    Ok(())
}

/// Follows `nextPageToken` until Google stops sending one, so a list longer than one page
/// is not dropped. `api_base` is a parameter so a test can point it at a local server.
async fn fetch_calendars(
    client: &reqwest::Client,
    token: &str,
    api_base: &str,
) -> Result<Vec<Calendar>> {
    let mut all: Vec<Calendar> = Vec::new();
    let mut page_token: Option<String> = None;

    for _ in 0..MAX_PAGES {
        let mut url = format!("{api_base}/users/me/calendarList?maxResults={PER_PAGE}");
        if let Some(t) = &page_token {
            url.push_str(&format!("&pageToken={}", urlencoding::encode(t)));
        }

        let list = client
            .get(&url)
            .bearer_auth(token)
            .send()
            .await?
            .error_for_status()?
            .json::<CalendarList>()
            .await?;

        all.extend(list.items);

        match list.next_page_token {
            Some(t) => page_token = Some(t),
            None => return Ok(all),
        }
    }

    warn!(
        "Calendar list stopped at the {MAX_PAGES}-page cap ({} calendars); later calendars were not fetched",
        all.len()
    );
    Ok(all)
}

/// Same token walk as `fetch_calendars`, for one calendar's events.
async fn fetch_events(
    client: &reqwest::Client,
    token: &str,
    api_base: &str,
    calendar_id: &str,
    time_min: &str,
    time_max: &str,
) -> Result<Vec<Event>> {
    let mut all: Vec<Event> = Vec::new();
    let mut page_token: Option<String> = None;

    for _ in 0..MAX_PAGES {
        let mut url = format!(
            "{api_base}/calendars/{}/events?singleEvents=true&orderBy=startTime&timeMin={}&timeMax={}&maxResults={PER_PAGE}",
            urlencoding::encode(calendar_id),
            urlencoding::encode(time_min),
            urlencoding::encode(time_max)
        );
        if let Some(t) = &page_token {
            url.push_str(&format!("&pageToken={}", urlencoding::encode(t)));
        }

        let list = client
            .get(&url)
            .bearer_auth(token)
            .send()
            .await?
            .error_for_status()?
            .json::<EventList>()
            .await?;

        all.extend(list.items);

        match list.next_page_token {
            Some(t) => page_token = Some(t),
            None => return Ok(all),
        }
    }

    // Events are in start-time order, so the cap drops the furthest-out ones. Calendar does
    // not use the pull marker: a later run picks them up as the 14-day window slides
    // forward. Loud on purpose.
    warn!(
        "Calendar events stopped at the {MAX_PAGES}-page cap ({} events); later events were not fetched",
        all.len()
    );
    Ok(all)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpListener;

    /// An events page: `count` items, and `next` as the nextPageToken when present.
    fn events_page(start: usize, count: usize, next: Option<&str>) -> String {
        let items: Vec<String> = (start..start + count)
            .map(|i| {
                format!(
                    "{{\"id\":\"e{i}\",\"start\":{{\"dateTime\":\"2026-11-02T10:00:00Z\"}},\"end\":{{\"dateTime\":\"2026-11-02T11:00:00Z\"}}}}"
                )
            })
            .collect();
        match next {
            Some(t) => format!(
                "{{\"items\":[{}],\"nextPageToken\":\"{t}\"}}",
                items.join(",")
            ),
            None => format!("{{\"items\":[{}]}}", items.join(",")),
        }
    }

    /// A calendarList page.
    fn calendars_page(start: usize, count: usize, next: Option<&str>) -> String {
        let items: Vec<String> = (start..start + count)
            .map(|i| format!("{{\"id\":\"c{i}\",\"summary\":\"cal {i}\"}}"))
            .collect();
        match next {
            Some(t) => format!(
                "{{\"items\":[{}],\"nextPageToken\":\"{t}\"}}",
                items.join(",")
            ),
            None => format!("{{\"items\":[{}]}}", items.join(",")),
        }
    }

    /// Serves one canned reply per connection and records the `pageToken` each request
    /// carried, so a test can prove the token is actually sent back.
    async fn serve(replies: Vec<String>) -> (String, tokio::task::JoinHandle<Vec<String>>) {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let handle = tokio::spawn(async move {
            let mut tokens = Vec::new();
            for body in replies {
                let (mut sock, _) = listener.accept().await.unwrap();
                let mut buf = [0u8; 4096];
                let n = sock.read(&mut buf).await.unwrap();
                let req = String::from_utf8_lossy(&buf[..n]).to_string();
                let sent = req
                    .split("pageToken=")
                    .nth(1)
                    .map(|s| {
                        s.split(|c: char| c == '&' || c.is_whitespace())
                            .next()
                            .unwrap_or("")
                            .to_string()
                    })
                    .unwrap_or_default();
                tokens.push(sent);
                let resp = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                );
                sock.write_all(resp.as_bytes()).await.unwrap();
                sock.flush().await.unwrap();
            }
            tokens
        });
        (format!("http://{addr}"), handle)
    }

    #[tokio::test]
    async fn events_follow_the_next_page_token() {
        let (base, server) = serve(vec![
            events_page(0, PER_PAGE as usize, Some("tok-2")),
            events_page(100, 9, None),
        ])
        .await;
        let client = reqwest::Client::new();

        let events = fetch_events(&client, "t", &base, "primary", "min", "max")
            .await
            .unwrap();

        // Before the fix only the first page was read, so this was 100, not 109.
        assert_eq!(events.len(), PER_PAGE as usize + 9);
        assert_eq!(server.await.unwrap(), vec!["".to_string(), "tok-2".into()]);
    }

    #[tokio::test]
    async fn calendars_follow_the_next_page_token() {
        let (base, server) = serve(vec![
            calendars_page(0, PER_PAGE as usize, Some("tok-2")),
            calendars_page(100, 2, None),
        ])
        .await;
        let client = reqwest::Client::new();

        let cals = fetch_calendars(&client, "t", &base).await.unwrap();

        assert_eq!(cals.len(), PER_PAGE as usize + 2);
        assert_eq!(server.await.unwrap(), vec!["".to_string(), "tok-2".into()]);
    }

    #[tokio::test]
    async fn events_stop_when_no_token_comes_back() {
        let (base, server) = serve(vec![events_page(0, 4, None)]).await;
        let client = reqwest::Client::new();

        let events = fetch_events(&client, "t", &base, "primary", "min", "max")
            .await
            .unwrap();

        assert_eq!(events.len(), 4);
        assert_eq!(server.await.unwrap(), vec!["".to_string()]);
    }

    #[tokio::test]
    async fn events_stop_at_the_page_cap() {
        // Every page keeps handing back a token, so only the cap ends the loop.
        let replies = (0..MAX_PAGES)
            .map(|p| events_page(p as usize * 100, 100, Some("always-more")))
            .collect();
        let (base, server) = serve(replies).await;
        let client = reqwest::Client::new();

        // The server accepts exactly MAX_PAGES connections, so an 11th request would
        // be refused and this would be an Err.
        let events = fetch_events(&client, "t", &base, "primary", "min", "max")
            .await
            .unwrap();

        assert_eq!(events.len(), (MAX_PAGES * 100) as usize);
        assert_eq!(server.await.unwrap().len(), MAX_PAGES as usize);
    }
}
