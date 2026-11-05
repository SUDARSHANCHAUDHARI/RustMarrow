# Design: page cap and pull marker

Status: proposed, not implemented (2026-11-04). Read from the working tree with the 13 uncommitted review fixes.

## Problem

Each puller stops at `MAX_PAGES = 10` (`src/pullers/gmail.rs:14`, `github.rs:13`, `calendar.rs:14`, `slack.rs:13`), logs `warn!` and returns `Ok(partial)`. The run then logs `pulled_at = started` (`gmail.rs:67`, `github.rs:68`, `slack.rs:64`) even when the cap was hit, so items past the cap sit below the next `since` and are never fetched.

## Current behaviour

- Marker: one row per source in `pull_log` (`src/db.rs:38-44`), read as the latest `pulled_at` (`src/memory.rs:342-353`), written by `log_pull` (`src/memory.rs:356-362`). An error before `log_pull` leaves the marker in place, so errors are already safe.
- Gmail: `after:{epoch}` (`gmail.rs:79-87`), newest first, so the cap drops the oldest messages. `before:` exists.
- Slack: `conversations.history?oldest=` per channel (`slack.rs:78`, `:220-222`), newest first. `latest=` exists. A non-ok reply returns a partial list as Ok (`slack.rs:237-243`).
- GitHub issues: `?state=all&since=` with default sort `created desc` (`github.rs:219`); `since` filters on `updated_at`, so the cap drops an arbitrary slice. `sort=updated&direction=asc` exists. `Issue` has no `updated_at` (`github.rs:28-37`).
- GitHub commits: `?since=&author=` (`github.rs:229-234`), newest first. `until=` exists.
- Calendar: does not use the marker. Fixed window now to +14 days (`calendar.rs:80-82`), start time ascending (`:214`). Capped far-future events are picked up later as the window slides. The comment at `calendar.rs:240-241` saying they are "lost for good" is wrong.
- Dedupe: `UNIQUE(source, source_id)` (`db.rs:35`) plus upsert in `ingest` (`memory.rs:143-150`), so refetching is harmless.
- Rate limits: no 429 / `Retry-After` / retry handling in `src`. A 429 becomes an Err, the run fails, the marker holds.

## Design

Rule: move the marker only up to the point below which everything is known to be fetched. Overlap is fine; the upsert dedupes.

1. Signal the cap. Each fetch returns `(Vec<T>, capped: bool)`. Keep the `warn!` and keep the cap as a loop guard; do not raise it.
2. GitHub issues (oldest-first). Add `sort=updated&direction=asc` and `updated_at` on `Issue`. Capped: the repo's safe point is the last item's `updated_at`; otherwise `started`. `since` is inclusive, so the boundary item is refetched and deduped. No new state.
3. Gmail and Slack (newest-first): record a gap.
   - Add nullable `resume_before TEXT` (and the gap start) to `pull_log`. `CREATE TABLE IF NOT EXISTS` will not add it to existing DBs: run `ALTER TABLE pull_log ADD COLUMN resume_before TEXT` and ignore the duplicate-column error.
   - Capped run: log `pulled_at = started` plus `resume_before` = oldest item fetched. Gmail: min `internalDate` (add it to `Message`, `gmail.rs:29-34`; it comes in the existing `messages.get`). Slack: oldest `ts` per capped channel, max across capped channels.
   - Next run with `resume_before` set: drain only the gap. Gmail `after:{gap_from} before:{resume_before}`; Slack `oldest={gap_from}&latest={resume_before}&inclusive=true`. Fetch nothing new that run.
   - Drained: write a row with `pulled_at` unchanged and `resume_before` NULL. Capped again: lower `resume_before`.
   - Store timestamps, not page tokens (tokens are query-bound and expire).
4. GitHub commits: if capped, the safe point is the old `since` (marker holds). `shortcut: commits hold the marker on cap; switch to the until= gap if a repo ever hits it`. The GitHub row logs `min(safe point of every capped stream, started)`.
5. Calendar: no marker change; fix the comment at `calendar.rs:240-241`.
6. Slack `ok:false` with `ratelimited`: return an error, not a partial list (`slack.rs:237-243`), so the marker does not move past skipped data.

## Test plan

Reuse the canned-reply servers in each test module (`gmail.rs:248-280`, `github.rs:270-300`). Change `stops_at_the_page_cap` (`gmail.rs:316-331`, `github.rs:338-350`) to assert `capped == true`.

1. Gmail cap then drain: run 1 sets `resume_before`; run 2 sends `before:`, clears it, keeps `pulled_at`; union of ids = full set.
2. Slack, two channels, one capped: `resume_before` = capped channel's oldest ts; drain carries `latest=`.
3. GitHub issues capped: next `since` = last `updated_at`, URL has `direction=asc`; uncapped logs `started`.
4. GitHub commits capped: marker does not move.
5. Error mid-run: no new `pull_log` row.
6. Migration: old DB without the column opens, twice.

## Not verified

- API ordering and the `before:`, `latest=`, `direction=asc` parameters are from knowledge of the APIs, not checked against current docs.
- Slack's 2025 rate limits for non-Marketplace apps may make 10 pages moot; check the token's app type.
