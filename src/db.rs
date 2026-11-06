use anyhow::Result;
use rusqlite::Connection;
use std::path::PathBuf;

pub struct Database {
    pub conn: Connection,
}

impl Database {
    pub fn open(path: &PathBuf) -> Result<Self> {
        let conn = Connection::open(path)?;
        // Holds emails and Slack DMs: owner-only, whatever the umask.
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))?;
        }
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
        // The gap a capped pull left. CREATE TABLE IF NOT EXISTS does not add columns to an
        // existing DB, so add them here and treat "already there" as done.
        for col in ["gap_from", "resume_before"] {
            match self
                .conn
                .execute(&format!("ALTER TABLE pull_log ADD COLUMN {col} TEXT"), [])
            {
                Err(rusqlite::Error::SqliteFailure(_, Some(msg)))
                    if msg.starts_with("duplicate column name") => {}
                other => {
                    other?;
                }
            }
        }
        Ok(())
    }
}
