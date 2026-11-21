//! Meetings and transcripts, saved in a SQLite file on this Mac.

use crate::data::{Action, Line, Meeting, Speaker, line};
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
        Self::open_at(&db_path()?)
    }

    pub fn open_at(path: &std::path::Path) -> Result<Self> {
        let conn = Connection::open(path).context("could not open the meetings database")?;
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
            CREATE INDEX IF NOT EXISTS lines_meeting ON lines(meeting_id);
            CREATE TABLE IF NOT EXISTS sections (
                id INTEGER PRIMARY KEY,
                meeting_id INTEGER NOT NULL REFERENCES meetings(id) ON DELETE CASCADE,
                pos INTEGER NOT NULL,
                title TEXT NOT NULL,
                body TEXT NOT NULL
            );
            CREATE TABLE IF NOT EXISTS vectors (
                key TEXT PRIMARY KEY,
                meeting_id INTEGER NOT NULL REFERENCES meetings(id) ON DELETE CASCADE,
                hash TEXT NOT NULL,
                vec BLOB NOT NULL
            );
            CREATE TABLE IF NOT EXISTS actions (
                id INTEGER PRIMARY KEY,
                meeting_id INTEGER NOT NULL REFERENCES meetings(id) ON DELETE CASCADE,
                pos INTEGER NOT NULL,
                label TEXT NOT NULL,
                owner TEXT NOT NULL,
                done INTEGER NOT NULL DEFAULT 0
            );",
        )?;
        conn.execute_batch("CREATE TABLE IF NOT EXISTS folders (id INTEGER PRIMARY KEY, name TEXT NOT NULL UNIQUE);")?;
        let has_folder_column: i64 =
            conn.query_row("SELECT COUNT(*) FROM pragma_table_info('meetings') WHERE name = 'folder_id'", [], |r| r.get(0))?;
        if has_folder_column == 0 {
            conn.execute_batch("ALTER TABLE meetings ADD COLUMN folder_id INTEGER REFERENCES folders(id) ON DELETE SET NULL;")?;
        }
        let has_people: i64 =
            conn.query_row("SELECT COUNT(*) FROM pragma_table_info('meetings') WHERE name = 'people'", [], |r| r.get(0))?;
        if has_people == 0 {
            conn.execute_batch("ALTER TABLE meetings ADD COLUMN people TEXT NOT NULL DEFAULT '';")?;
        }
        let has_deleted: i64 =
            conn.query_row("SELECT COUNT(*) FROM pragma_table_info('meetings') WHERE name = 'deleted_at'", [], |r| r.get(0))?;
        if has_deleted == 0 {
            conn.execute_batch("ALTER TABLE meetings ADD COLUMN deleted_at INTEGER;")?;
        }
        Ok(Self { conn })
    }

    /// Moves a meeting to the trash. It can be restored until it is purged.
    pub fn trash_meeting(&self, meeting_id: i64, now: i64) -> Result<()> {
        self.conn.execute("UPDATE meetings SET deleted_at = ?2 WHERE id = ?1", params![meeting_id, now])?;
        Ok(())
    }

    pub fn restore_meeting(&self, meeting_id: i64) -> Result<()> {
        self.conn.execute("UPDATE meetings SET deleted_at = NULL WHERE id = ?1", params![meeting_id])?;
        Ok(())
    }

    /// (id, title, started, deleted_at) of trashed meetings, newest deletion first.
    pub fn load_trash(&self) -> Result<Vec<(i64, String, i64, i64)>> {
        let mut stmt = self
            .conn
            .prepare("SELECT id, title, started, deleted_at FROM meetings WHERE deleted_at IS NOT NULL ORDER BY deleted_at DESC")?;
        let rows = stmt.query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)))?;
        Ok(rows.collect::<std::result::Result<Vec<_>, _>>()?)
    }

    /// Permanently deletes trashed meetings deleted before `cutoff`. Returns their ids.
    pub fn purge_trash_before(&self, cutoff: i64) -> Result<Vec<i64>> {
        let ids: Vec<i64> = {
            let mut stmt = self.conn.prepare("SELECT id FROM meetings WHERE deleted_at IS NOT NULL AND deleted_at < ?1")?;
            let rows = stmt.query_map([cutoff], |r| r.get(0))?;
            rows.collect::<std::result::Result<Vec<_>, _>>()?
        };
        for id in &ids {
            self.delete_meeting(*id)?;
        }
        Ok(ids)
    }

    /// Writes everything still in the write-ahead log into the main database file, so a copy of the file is whole.
    pub fn checkpoint(&self) -> Result<()> {
        self.conn.query_row("PRAGMA wal_checkpoint(TRUNCATE)", [], |_| Ok(()))?;
        Ok(())
    }

    pub fn set_people(&self, meeting_id: i64, people: &[String]) -> Result<()> {
        self.conn.execute("UPDATE meetings SET people = ?2 WHERE id = ?1", params![meeting_id, people.join("\n")])?;
        Ok(())
    }

    pub fn load_meetings(&self) -> Result<Vec<Meeting>> {
        let mut meetings = Vec::new();
        let mut stmt = self
            .conn
            .prepare("SELECT id, title, started, folder_id, people FROM meetings WHERE deleted_at IS NULL ORDER BY started DESC")?;
        let rows = stmt.query_map([], |r| {
            Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?, r.get::<_, i64>(2)?, r.get::<_, Option<i64>>(3)?, r.get::<_, String>(4)?))
        })?;
        for row in rows {
            let (id, title, started, folder, people) = row?;
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
            let mut meeting = Meeting::new(id, &title, started, false, lines);
            meeting.folder = folder;
            meeting.people = people.lines().filter(|p| !p.is_empty()).map(str::to_string).collect();

            let mut ss = self
                .conn
                .prepare("SELECT title, body FROM sections WHERE meeting_id = ?1 ORDER BY pos")?;
            for s in ss.query_map([id], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))? {
                let (t, b) = s?;
                meeting.summary.push((t.into(), b.into()));
            }
            let mut acts = self
                .conn
                .prepare("SELECT id, label, owner, done FROM actions WHERE meeting_id = ?1 ORDER BY pos")?;
            for a in acts.query_map([id], |r| {
                Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?, r.get::<_, String>(2)?, r.get::<_, bool>(3)?))
            })? {
                let (aid, label, owner, done) = a?;
                meeting.actions.push(Action { id: aid, label: label.into(), owner: owner.into(), done });
            }
            meetings.push(meeting);
        }
        Ok(meetings)
    }

    /// Replaces the saved summary and next steps of a meeting. Returns the new next-step ids.
    pub fn save_notes(
        &self,
        meeting_id: i64,
        title: Option<&str>,
        sections: &[(String, String)],
        actions: &[(String, String)],
    ) -> Result<Vec<i64>> {
        let tx = self.conn.unchecked_transaction()?;
        tx.execute("DELETE FROM sections WHERE meeting_id = ?1", params![meeting_id])?;
        tx.execute("DELETE FROM actions WHERE meeting_id = ?1", params![meeting_id])?;
        for (pos, (t, b)) in sections.iter().enumerate() {
            tx.execute(
                "INSERT INTO sections (meeting_id, pos, title, body) VALUES (?1, ?2, ?3, ?4)",
                params![meeting_id, pos as i64, t, b],
            )?;
        }
        let mut ids = Vec::new();
        for (pos, (label, owner)) in actions.iter().enumerate() {
            tx.execute(
                "INSERT INTO actions (meeting_id, pos, label, owner) VALUES (?1, ?2, ?3, ?4)",
                params![meeting_id, pos as i64, label, owner],
            )?;
            ids.push(tx.last_insert_rowid());
        }
        if let Some(t) = title {
            tx.execute("UPDATE meetings SET title = ?2 WHERE id = ?1", params![meeting_id, t])?;
        }
        tx.commit()?;
        Ok(ids)
    }

    /// Every saved meaning vector, by passage key, with the hash of the text it was made from.
    pub fn load_vectors(&self) -> Result<std::collections::HashMap<String, (String, Vec<f32>)>> {
        let mut stmt = self.conn.prepare("SELECT key, hash, vec FROM vectors")?;
        let rows = stmt.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?, r.get::<_, Vec<u8>>(2)?)))?;
        let mut out = std::collections::HashMap::new();
        for row in rows {
            let (key, hash, blob) = row?;
            out.insert(key, (hash, crate::semantic::from_blob(&blob)));
        }
        Ok(out)
    }

    pub fn save_vectors(&self, rows: &[(String, i64, String, Vec<f32>)]) -> Result<()> {
        let tx = self.conn.unchecked_transaction()?;
        for (key, meeting_id, hash, vec) in rows {
            tx.execute(
                "INSERT OR REPLACE INTO vectors (key, meeting_id, hash, vec) VALUES (?1, ?2, ?3, ?4)",
                params![key, meeting_id, hash, crate::semantic::to_blob(vec)],
            )?;
        }
        tx.commit()?;
        Ok(())
    }

    pub fn set_action_done(&self, action_id: i64, done: bool) -> Result<()> {
        self.conn
            .execute("UPDATE actions SET done = ?2 WHERE id = ?1", params![action_id, done])?;
        Ok(())
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

    pub fn load_folders(&self) -> Result<Vec<crate::data::Folder>> {
        let mut stmt = self.conn.prepare("SELECT id, name FROM folders ORDER BY name COLLATE NOCASE")?;
        let rows = stmt.query_map([], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?)))?;
        let mut out = Vec::new();
        for row in rows {
            let (id, name) = row?;
            out.push(crate::data::Folder { id, name: name.into() });
        }
        Ok(out)
    }

    pub fn create_folder(&self, name: &str) -> Result<i64> {
        self.conn.execute("INSERT INTO folders (name) VALUES (?1)", params![name])?;
        Ok(self.conn.last_insert_rowid())
    }

    pub fn delete_folder(&self, folder_id: i64) -> Result<()> {
        self.conn.execute("DELETE FROM folders WHERE id = ?1", params![folder_id])?;
        Ok(())
    }

    pub fn set_folder(&self, meeting_id: i64, folder_id: Option<i64>) -> Result<()> {
        self.conn
            .execute("UPDATE meetings SET folder_id = ?2 WHERE id = ?1", params![meeting_id, folder_id])?;
        Ok(())
    }

    pub fn rename_meeting(&self, meeting_id: i64, title: &str) -> Result<()> {
        self.conn
            .execute("UPDATE meetings SET title = ?2 WHERE id = ?1", params![meeting_id, title])?;
        Ok(())
    }

    pub fn delete_meeting(&self, meeting_id: i64) -> Result<()> {
        self.conn.execute("DELETE FROM meetings WHERE id = ?1", params![meeting_id])?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::data::Speaker;

    fn store() -> Store {
        Store::open_at(std::path::Path::new(":memory:")).unwrap()
    }

    #[test]
    fn trashed_meetings_leave_the_list_and_can_come_back_or_be_purged() {
        let s = store();
        let a = s.create_meeting("Keep", 100).unwrap();
        let b = s.create_meeting("Old", 200).unwrap();
        s.trash_meeting(a, 1_000).unwrap();
        s.trash_meeting(b, 5_000).unwrap();
        assert!(s.load_meetings().unwrap().is_empty());
        assert_eq!(s.load_trash().unwrap().len(), 2);
        s.restore_meeting(a).unwrap();
        assert_eq!(s.load_meetings().unwrap().len(), 1);
        assert_eq!(s.purge_trash_before(9_000).unwrap(), vec![b]);
        assert!(s.load_trash().unwrap().is_empty());
    }

    #[test]
    fn folders_hold_meetings_and_release_them_when_deleted() {
        let s = store();
        let m = s.create_meeting("Weekly", 100).unwrap();
        let f = s.create_folder("Clients").unwrap();
        s.set_folder(m, Some(f)).unwrap();
        assert_eq!(s.load_meetings().unwrap()[0].folder, Some(f));
        assert_eq!(s.load_folders().unwrap().len(), 1);
        s.delete_folder(f).unwrap();
        assert_eq!(s.load_meetings().unwrap()[0].folder, None);
        assert!(s.load_folders().unwrap().is_empty());
    }

    #[test]
    fn folder_names_are_unique_and_sorted() {
        let s = store();
        s.create_folder("b").unwrap();
        s.create_folder("A").unwrap();
        assert!(s.create_folder("b").is_err());
        let names: Vec<String> = s.load_folders().unwrap().iter().map(|f| f.name.to_string()).collect();
        assert_eq!(names, vec!["A", "b"]);
    }

    #[test]
    fn renaming_and_notes_survive_a_reload() {
        let s = store();
        let m = s.create_meeting("New note", 100).unwrap();
        s.rename_meeting(m, "Budget talk").unwrap();
        s.add_line(m, &crate::data::line("00:05", "You wrote", Speaker::Note, "Ask about Q3")).unwrap();
        let loaded = &s.load_meetings().unwrap()[0];
        assert_eq!(loaded.title.as_ref(), "Budget talk");
        assert!(loaded.lines[0].who == Speaker::Note);
    }
}
