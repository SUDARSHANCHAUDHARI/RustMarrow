use crate::db::Database;
use anyhow::Result;
use chrono::{DateTime, Utc};
use std::path::{Path, PathBuf};

pub struct Chunk {
    pub source: String,
    pub source_id: String,
    pub title: String,
    pub content: String,
    pub url: Option<String>,
    pub tags: Vec<String>,
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

    pub fn log_pull(&self, source: &str, items: usize) -> Result<()> {
        self.db.conn.execute(
            "INSERT INTO pull_log (source, pulled_at, items_fetched) VALUES (?1, ?2, ?3)",
            rusqlite::params![source, Utc::now().to_rfc3339(), items as i64],
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
        if let Some(vault) = &self.obsidian_path {
            let dir = vault.join("Marrow").join(source);
            if dir.exists() {
                std::fs::remove_dir_all(dir).ok();
            }
        }
        Ok(deleted)
    }

    /// Delete one chunk by id. Also removes its Obsidian file.
    pub fn forget(&self, id: i64) -> Result<()> {
        let chunk_info: Option<(String, String)> = self
            .db
            .conn
            .query_row(
                "SELECT source, title FROM memory_chunks WHERE id = ?1",
                [id],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .ok();

        let deleted = self
            .db
            .conn
            .execute("DELETE FROM memory_chunks WHERE id = ?1", [id])?;

        if deleted == 0 {
            anyhow::bail!("No chunk with id {id}");
        }

        if let (Some(vault), Some((source, title))) = (&self.obsidian_path, chunk_info) {
            let safe = title.replace(['/', '\\', ':', '*', '?', '"', '<', '>', '|'], "-");
            let path = vault
                .join("Marrow")
                .join(&source)
                .join(format!("{safe}.md"));
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
            println!("  No data yet — run `marrow pull`");
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

        let safe = chunk
            .title
            .replace(['/', '\\', ':', '*', '?', '"', '<', '>', '|'], "-");
        let path = dir.join(format!("{safe}.md"));

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
