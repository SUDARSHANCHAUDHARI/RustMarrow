use anyhow::Result;
use rusqlite::Connection;
use std::path::PathBuf;

pub struct Database {
    pub conn: Connection,
}

impl Database {
    pub fn open(path: &PathBuf) -> Result<Self> {
        let conn = Connection::open(path)?;
        let db = Self { conn };
        db.init_schema()?;
        Ok(db)
    }

    fn init_schema(&self) -> Result<()> {
        self.conn.execute_batch(
            "
            CREATE TABLE IF NOT EXISTS memory_chunks (
                id          INTEGER PRIMARY KEY,
                source      TEXT    NOT NULL,
                source_id   TEXT    NOT NULL,
                title       TEXT    NOT NULL,
                content     TEXT    NOT NULL,
                url         TEXT,
                tags        TEXT,
                fetched_at  TEXT    NOT NULL,
                UNIQUE(source, source_id)
            );

            CREATE TABLE IF NOT EXISTS pull_log (
                id            INTEGER PRIMARY KEY,
                source        TEXT    NOT NULL,
                pulled_at     TEXT    NOT NULL,
                items_fetched INTEGER NOT NULL,
                error         TEXT
            );

            CREATE INDEX IF NOT EXISTS idx_chunks_source   ON memory_chunks(source);
            CREATE INDEX IF NOT EXISTS idx_chunks_fetched  ON memory_chunks(fetched_at);
            ",
        )?;
        Ok(())
    }
}
