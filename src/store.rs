//! Meetings and transcripts, saved in a SQLite file on this Mac.

use crate::data::{Line, Meeting, Speaker, line};
use anyhow::{Context, Result};
use rusqlite::{Connection, params};
use std::path::PathBuf;

pub struct Store {
    conn: Connection,
}

fn db_path() -> Result<PathBuf> {
    if let Ok(p) = std::env::var("NOTARY_DB") {
        return Ok(PathBuf::from(p));
    }
    let home = std::env::var("HOME").context("HOME is not set")?;
    let dir = PathBuf::from(home).join("Library/Application Support/Notary");
    std::fs::create_dir_all(&dir)?;
    Ok(dir.join("notary.db"))
}

impl Store {
    pub fn open() -> Result<Self> {
        let conn = Connection::open(db_path()?).context("could not open the meetings database")?;
        conn.pragma_update(None, "journal_mode", "WAL")?;
        conn.pragma_update(None, "foreign_keys", "ON")?;
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS meetings (
                id INTEGER PRIMARY KEY,
                title TEXT NOT NULL,
                started INTEGER NOT NULL,
                ended INTEGER
            );
            CREATE TABLE IF NOT EXISTS lines (
                id INTEGER PRIMARY KEY,
                meeting_id INTEGER NOT NULL REFERENCES meetings(id) ON DELETE CASCADE,
                ts TEXT NOT NULL,
                name TEXT NOT NULL,
                who INTEGER NOT NULL,
                text TEXT NOT NULL
            );
            CREATE INDEX IF NOT EXISTS lines_meeting ON lines(meeting_id);",
        )?;
        Ok(Self { conn })
    }

    pub fn load_meetings(&self) -> Result<Vec<Meeting>> {
        let mut meetings = Vec::new();
        let mut stmt = self
            .conn
            .prepare("SELECT id, title, started FROM meetings ORDER BY started DESC")?;
        let rows = stmt.query_map([], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?, r.get::<_, i64>(2)?)))?;
        for row in rows {
            let (id, title, started) = row?;
            let mut lines = Vec::new();
            let mut ls = self
                .conn
                .prepare("SELECT ts, name, who, text FROM lines WHERE meeting_id = ?1 ORDER BY id")?;
            let line_rows = ls.query_map([id], |r| {
                Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?, r.get::<_, i64>(2)?, r.get::<_, String>(3)?))
            })?;
            for l in line_rows {
                let (ts, name, who, text) = l?;
                lines.push(line(&ts, &name, Speaker::from_db(who), &text));
            }
            meetings.push(Meeting::new(id, &title, started, false, lines));
        }
        Ok(meetings)
    }

    pub fn create_meeting(&self, title: &str, started: i64) -> Result<i64> {
        self.conn
            .execute("INSERT INTO meetings (title, started) VALUES (?1, ?2)", params![title, started])?;
        Ok(self.conn.last_insert_rowid())
    }

    pub fn add_line(&self, meeting_id: i64, l: &Line) -> Result<()> {
        self.conn.execute(
            "INSERT INTO lines (meeting_id, ts, name, who, text) VALUES (?1, ?2, ?3, ?4, ?5)",
            params![meeting_id, l.ts.as_ref(), l.name.as_ref(), l.who.to_db(), l.text.as_ref()],
        )?;
        Ok(())
    }

    pub fn finish(&self, meeting_id: i64, ended: i64) -> Result<()> {
        self.conn
            .execute("UPDATE meetings SET ended = ?2 WHERE id = ?1", params![meeting_id, ended])?;
        Ok(())
    }

    pub fn delete_meeting(&self, meeting_id: i64) -> Result<()> {
        self.conn.execute("DELETE FROM meetings WHERE id = ?1", params![meeting_id])?;
        Ok(())
    }
}
