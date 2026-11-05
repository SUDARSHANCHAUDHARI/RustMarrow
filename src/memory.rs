use crate::db::Database;
use anyhow::Result;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

pub struct Chunk {
    pub source: String,
    pub source_id: String,
    pub title: String,
    pub content: String,
    pub url: Option<String>,
    pub tags: Vec<String>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct ExportedChunk {
    pub source: String,
    pub source_id: String,
    pub title: String,
    pub content: String,
    pub url: Option<String>,
    pub tags: Vec<String>,
    pub fetched_at: String,
}

pub struct SearchResult {
    pub id: i64,
    pub source: String,
    pub title: String,
    pub url: Option<String>,
    pub preview: String,
    pub fetched_at: String,
}

pub struct Store {
    db: Database,
    db_path: PathBuf,
    obsidian_path: Option<PathBuf>,
}

impl Store {
    pub fn new(db: Database, db_path: PathBuf, obsidian_path: Option<PathBuf>) -> Self {
        Self {
            db,
            db_path,
            obsidian_path,
        }
    }

    pub fn ingest(&self, chunk: &Chunk) -> Result<()> {
        let tags_json = serde_json::to_string(&chunk.tags)?;
        self.db.conn.execute(
            "INSERT INTO memory_chunks (source, source_id, title, content, url, tags, fetched_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)
             ON CONFLICT(source, source_id) DO UPDATE SET
               title      = excluded.title,
               content    = excluded.content,
               url        = excluded.url,
               tags       = excluded.tags,
               fetched_at = excluded.fetched_at",
            rusqlite::params![
                chunk.source,
                chunk.source_id,
                chunk.title,
                chunk.content,
                chunk.url,
                tags_json,
                Utc::now().to_rfc3339(),
            ],
        )?;

        if let Some(vault) = &self.obsidian_path {
            self.write_obsidian(vault, chunk)?;
        }

        Ok(())
    }

    pub fn export_json(&self, path: &Path) -> Result<usize> {
        let mut stmt = self.db.conn.prepare(
            "SELECT source, source_id, title, content, url, tags, fetched_at
             FROM memory_chunks
             ORDER BY fetched_at DESC, id DESC",
        )?;
        let chunks = stmt
            .query_map([], |row| {
                let tags_json: String = row.get(5)?;
                let tags = serde_json::from_str(&tags_json).unwrap_or_default();
                Ok(ExportedChunk {
                    source: row.get(0)?,
                    source_id: row.get(1)?,
                    title: row.get(2)?,
                    content: row.get(3)?,
                    url: row.get(4)?,
                    tags,
                    fetched_at: row.get(6)?,
                })
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;

        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let json = serde_json::to_string_pretty(&chunks)?;
        std::fs::write(path, json)?;
        Ok(chunks.len())
    }

    pub fn import_json(&self, path: &Path) -> Result<usize> {
        let content = std::fs::read_to_string(path)?;
        let chunks: Vec<ExportedChunk> = serde_json::from_str(&content)?;

        for chunk in &chunks {
            let tags_json = serde_json::to_string(&chunk.tags)?;
            self.db.conn.execute(
                "INSERT INTO memory_chunks (source, source_id, title, content, url, tags, fetched_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)
                 ON CONFLICT(source, source_id) DO UPDATE SET
                   title      = excluded.title,
                   content    = excluded.content,
                   url        = excluded.url,
                   tags       = excluded.tags,
                   fetched_at = excluded.fetched_at",
                rusqlite::params![
                    chunk.source,
                    chunk.source_id,
                    chunk.title,
                    chunk.content,
                    chunk.url,
                    tags_json,
                    chunk.fetched_at,
                ],
            )?;
        }

        Ok(chunks.len())
    }

    /// For agent context — single-phrase LIKE, returns formatted markdown
    pub fn search(&self, query: &str) -> Result<Vec<String>> {
        let pattern = format!("%{}%", query.to_lowercase());
        let mut stmt = self.db.conn.prepare(
            "SELECT title, content FROM memory_chunks
             WHERE lower(title) LIKE ?1 OR lower(content) LIKE ?1
             ORDER BY fetched_at DESC
             LIMIT 10",
        )?;
        let results = stmt
            .query_map([&pattern], |row| {
                Ok(format!(
                    "## {}\n{}",
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?
                ))
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(results)
    }

    /// For CLI display — multi-word AND search, returns structured results with id
    pub fn search_display(&self, query: &str, limit: usize) -> Result<Vec<SearchResult>> {
        let words: Vec<String> = query
            .split_whitespace()
            .map(|w| format!("%{}%", w.to_lowercase()))
            .collect();

        if words.is_empty() {
            return Ok(vec![]);
        }

        // Build: WHERE (title LIKE ? OR content LIKE ?) AND (title LIKE ? OR content LIKE ?) ...
        let conditions: String = words
            .iter()
            .map(|_| "(lower(title) LIKE ? OR lower(content) LIKE ?)")
            .collect::<Vec<_>>()
            .join(" AND ");

        let sql = format!(
            "SELECT id, source, title, url, content, fetched_at FROM memory_chunks
             WHERE {conditions}
             ORDER BY fetched_at DESC LIMIT ?"
        );

        // Each word appears twice (title + content), then limit at end
        let mut params: Vec<Box<dyn rusqlite::ToSql>> = words
            .iter()
            .flat_map(|w| {
                [
                    Box::new(w.clone()) as Box<dyn rusqlite::ToSql>,
                    Box::new(w.clone()) as Box<dyn rusqlite::ToSql>,
                ]
            })
            .collect();
        params.push(Box::new(limit as i64));

        let mut stmt = self.db.conn.prepare(&sql)?;
        let results = stmt
            .query_map(rusqlite::params_from_iter(params.iter()), |row| {
                let content: String = row.get(4)?;
                let preview: String = content.chars().take(120).collect();
                Ok(SearchResult {
                    id: row.get(0)?,
                    source: row.get(1)?,
                    title: row.get(2)?,
                    url: row.get(3)?,
                    preview,
                    fetched_at: row.get(5)?,
                })
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(results)
    }

    pub fn recent_context(&self, limit: usize) -> Result<Vec<String>> {
        let mut stmt = self.db.conn.prepare(
            "SELECT title, content, source FROM memory_chunks
             ORDER BY fetched_at DESC LIMIT ?1",
        )?;
        let results = stmt
            .query_map([limit as i64], |row| {
                Ok(format!(
                    "## {} [{}]\n{}",
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, String>(1)?
                ))
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(results)
    }

    /// All chunks for a source, newest first — used by digest
    pub fn chunks_by_source(&self, source: &str, limit: usize) -> Result<Vec<String>> {
        let mut stmt = self.db.conn.prepare(
            "SELECT title, content FROM memory_chunks
             WHERE source = ?1
             ORDER BY fetched_at DESC LIMIT ?2",
        )?;
        let results = stmt
            .query_map(rusqlite::params![source, limit as i64], |row| {
                Ok(format!(
                    "## {}\n{}",
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?
                ))
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(results)
    }

    /// Returns the timestamp of the last successful pull for a source
    pub fn last_pull_at(&self, source: &str) -> Option<DateTime<Utc>> {
        self.db
            .conn
            .query_row(
                "SELECT pulled_at FROM pull_log WHERE source = ?1 ORDER BY id DESC LIMIT 1",
                [source],
                |r| r.get::<_, String>(0),
            )
            .ok()
            .and_then(|s| DateTime::parse_from_rfc3339(&s).ok())
            .map(|dt| dt.with_timezone(&Utc))
    }

    /// The `(gap_from, resume_before)` range a capped pull left unfetched, if the latest
    /// row for the source has one.
    pub fn pull_gap(&self, source: &str) -> Option<(DateTime<Utc>, DateTime<Utc>)> {
        let (from, before): (Option<String>, Option<String>) = self
            .db
            .conn
            .query_row(
                "SELECT gap_from, resume_before FROM pull_log WHERE source = ?1 ORDER BY id DESC LIMIT 1",
                [source],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .ok()?;
        let parse = |s: String| {
            DateTime::parse_from_rfc3339(&s)
                .ok()
                .map(|dt| dt.with_timezone(&Utc))
        };
        Some((parse(from?)?, parse(before?)?))
    }

    /// `pulled_at` is when the pull started, which becomes the next pull's `since`.
    pub fn log_pull(&self, source: &str, items: usize, pulled_at: DateTime<Utc>) -> Result<()> {
        self.log_pull_gap(source, items, pulled_at, None)
    }

    /// As `log_pull`, plus the `(gap_from, resume_before)` range still to drain, if any.
    pub fn log_pull_gap(
        &self,
        source: &str,
        items: usize,
        pulled_at: DateTime<Utc>,
        gap: Option<(DateTime<Utc>, DateTime<Utc>)>,
    ) -> Result<()> {
        self.db.conn.execute(
            "INSERT INTO pull_log (source, pulled_at, items_fetched, gap_from, resume_before)
             VALUES (?1, ?2, ?3, ?4, ?5)",
            rusqlite::params![
                source,
                pulled_at.to_rfc3339(),
                items as i64,
                gap.map(|g| g.0.to_rfc3339()),
                gap.map(|g| g.1.to_rfc3339()),
            ],
        )?;
        Ok(())
    }

    /// Delete all memory chunks + pull history for a source. Also cleans Obsidian folder.
    pub fn clear(&self, source: &str) -> Result<usize> {
        let deleted = self
            .db
            .conn
            .execute("DELETE FROM memory_chunks WHERE source = ?1", [source])?;
        self.db
            .conn
            .execute("DELETE FROM pull_log WHERE source = ?1", [source])?;
        // Only a plain folder name may reach remove_dir_all: `..` or an absolute path
        // would point the recursive delete outside the Marrow folder.
        let plain_name = matches!(
            Path::new(source)
                .components()
                .collect::<Vec<_>>()
                .as_slice(),
            [std::path::Component::Normal(_)]
        );
        if let (Some(vault), true) = (&self.obsidian_path, plain_name) {
            let dir = vault.join("Marrow").join(source);
            if dir.exists() {
                std::fs::remove_dir_all(dir).ok();
            }
        }
        Ok(deleted)
    }

    /// Delete one chunk by id. Also removes its Obsidian file.
    pub fn forget(&self, id: i64) -> Result<()> {
        let chunk_info: Option<(String, String, String)> = self
            .db
            .conn
            .query_row(
                "SELECT source, source_id, title FROM memory_chunks WHERE id = ?1",
                [id],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .ok();

        let deleted = self
            .db
            .conn
            .execute("DELETE FROM memory_chunks WHERE id = ?1", [id])?;

        if deleted == 0 {
            anyhow::bail!("No chunk with id {id}");
        }

        if let (Some(vault), Some((source, source_id, title))) = (&self.obsidian_path, chunk_info) {
            let path = vault
                .join("Marrow")
                .join(&source)
                .join(note_file_name(&title, &source_id));
            if path.exists() {
                std::fs::remove_file(path).ok();
            }
        }

        Ok(())
    }

    pub fn print_stats(&self) -> Result<()> {
        let total: i64 = self
            .db
            .conn
            .query_row("SELECT COUNT(*) FROM memory_chunks", [], |r| r.get(0))?;

        let mut stmt = self.db.conn.prepare(
            "SELECT
               c.source,
               COUNT(c.id)      AS chunks,
               MAX(l.pulled_at) AS last_pull,
               l2.items_fetched AS last_count
             FROM memory_chunks c
             LEFT JOIN pull_log l  ON l.source = c.source
             LEFT JOIN pull_log l2 ON l2.id = (
               SELECT MAX(id) FROM pull_log WHERE source = c.source
             )
             GROUP BY c.source
             ORDER BY chunks DESC",
        )?;
        let by_source: Vec<(String, i64, Option<String>, Option<i64>)> = stmt
            .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)))?
            .collect::<rusqlite::Result<_>>()?;

        let db_size = std::fs::metadata(&self.db_path)
            .map(|m| format!("{:.1} MB", m.len() as f64 / 1_048_576.0))
            .unwrap_or_else(|_| "?".into());

        println!("Marrow — {total} chunks | DB: {db_size}");
        println!();
        if by_source.is_empty() {
            println!("  No data yet — run `rustmarrow pull`");
        } else {
            for (source, count, last_pull, last_count) in &by_source {
                let pull_info = match (last_pull, last_count) {
                    (Some(t), Some(n)) => format!("last pull: {t} ({n} items)"),
                    (Some(t), None) => format!("last pull: {t}"),
                    _ => "never pulled".into(),
                };
                println!("  {source:<12} {count:>4} chunks   {pull_info}");
            }
        }
        Ok(())
    }

    fn write_obsidian(&self, vault: &Path, chunk: &Chunk) -> Result<()> {
        let dir = vault.join("Marrow").join(&chunk.source);
        std::fs::create_dir_all(&dir)?;

        let path = dir.join(note_file_name(&chunk.title, &chunk.source_id));

        let tags_yaml = chunk
            .tags
            .iter()
            .map(|t| format!("  - {t}"))
            .collect::<Vec<_>>()
            .join("\n");

        let md = format!(
            "---\nsource: {}\nid: {}\nurl: {}\ntags:\n{}\n---\n\n# {}\n\n{}",
            chunk.source,
            chunk.source_id,
            chunk.url.as_deref().unwrap_or(""),
            tags_yaml,
            chunk.title,
            chunk.content
        );

        std::fs::write(path, md)?;
        Ok(())
    }
}

/// Obsidian note name: title plus source id, so two chunks with the same title do not
/// share a file. The title is cut to 150 bytes to stay under the 255-byte filename limit.
fn note_file_name(title: &str, source_id: &str) -> String {
    let mut short = String::new();
    for c in title.chars() {
        if short.len() + c.len_utf8() > 150 {
            break;
        }
        short.push(c);
    }
    format!("{short} ({source_id}).md").replace(['/', '\\', ':', '*', '?', '"', '<', '>', '|'], "-")
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    pub(crate) fn temp_db_path(name: &str) -> PathBuf {
        let mut path = std::env::temp_dir();
        path.push(format!(
            "marrow-{name}-{}-{}.db",
            std::process::id(),
            Utc::now().timestamp_nanos_opt().unwrap_or_default()
        ));
        path
    }

    pub(crate) fn store_at(path: &PathBuf) -> Store {
        let db = Database::open(path).unwrap();
        Store::new(db, path.clone(), None)
    }

    #[test]
    fn an_old_db_without_the_gap_columns_opens_twice() {
        let path = temp_db_path("migrate");
        // The pull_log shape before the gap columns existed, with one marker row.
        let old = rusqlite::Connection::open(&path).unwrap();
        old.execute_batch(
            "CREATE TABLE pull_log (
                id INTEGER PRIMARY KEY, source TEXT NOT NULL, pulled_at TEXT NOT NULL,
                items_fetched INTEGER NOT NULL, error TEXT);
             INSERT INTO pull_log (source, pulled_at, items_fetched)
                VALUES ('gmail', '2026-11-01T00:00:00+00:00', 3);",
        )
        .unwrap();
        drop(old);

        // Second open hits the duplicate-column path and must not fail.
        drop(store_at(&path));
        let store = store_at(&path);

        let marker = DateTime::parse_from_rfc3339("2026-11-01T00:00:00+00:00").unwrap();
        assert_eq!(
            store.last_pull_at("gmail"),
            Some(marker.with_timezone(&Utc))
        );
        assert_eq!(store.pull_gap("gmail"), None);

        let gap = (marker.with_timezone(&Utc), Utc::now());
        store.log_pull_gap("gmail", 0, gap.0, Some(gap)).unwrap();
        assert_eq!(store.pull_gap("gmail"), Some(gap));

        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn export_import_json_round_trips_memory_chunks() {
        let source_path = temp_db_path("source");
        let target_path = temp_db_path("target");
        let export_path = temp_db_path("backup").with_extension("json");

        let source = store_at(&source_path);
        source
            .ingest(&Chunk {
                source: "manual".to_string(),
                source_id: "note-1".to_string(),
                title: "Production note".to_string(),
                content: "Remember the release checklist".to_string(),
                url: Some("https://example.com/note".to_string()),
                tags: vec!["release".to_string(), "rust".to_string()],
            })
            .unwrap();

        let exported = source.export_json(&export_path).unwrap();
        assert_eq!(exported, 1);

        let target = store_at(&target_path);
        let imported = target.import_json(&export_path).unwrap();
        assert_eq!(imported, 1);

        let results = target.search_display("release checklist", 10).unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].title, "Production note");

        let _ = std::fs::remove_file(source_path);
        let _ = std::fs::remove_file(target_path);
        let _ = std::fs::remove_file(export_path);
    }

    #[test]
    fn clear_never_deletes_outside_the_marrow_folder() {
        let db_path = temp_db_path("clear");
        let vault = temp_db_path("vault").with_extension("");
        let outside = vault.join("keep");
        let inside = vault.join("Marrow").join("gmail");
        std::fs::create_dir_all(&outside).unwrap();
        std::fs::create_dir_all(&inside).unwrap();
        let store = Store::new(
            Database::open(&db_path).unwrap(),
            db_path.clone(),
            Some(vault.clone()),
        );

        store.clear("..").unwrap();
        store.clear(outside.to_str().unwrap()).unwrap();
        assert!(outside.exists() && inside.exists());

        store.clear("gmail").unwrap();
        assert!(!inside.exists());

        let _ = std::fs::remove_dir_all(vault);
        let _ = std::fs::remove_file(db_path);
    }

    #[test]
    fn note_file_name_is_unique_safe_and_short() {
        let a = note_file_name("Email: Hello", "msg-1");
        let b = note_file_name("Email: Hello", "msg-2");
        assert_ne!(a, b);
        assert!(!a.contains(['/', ':']));
        let long = note_file_name(&"é".repeat(500), "commit:o/r:abcdef12");
        assert!(long.len() < 255, "{} bytes", long.len());
    }
}
