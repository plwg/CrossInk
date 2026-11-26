//! Reads review logs from Anki collections and packages.

use std::fs::File;
use std::io::{self, Read};
use std::path::Path;

use anyhow::{Context, Result, bail};
use rusqlite::{Connection, OpenFlags};
use tempfile::NamedTempFile;

use crate::revlog::{ReviewKind, RevlogEntry};

const SQLITE_MAGIC: &[u8] = b"SQLite format 3\0";
const ZIP_MAGIC: &[u8] = b"PK\x03\x04";

/// Database names inside a .colpkg/.apkg, newest format first. When a newer
/// file is present, older ones are placeholders kept for old Anki versions.
const PACKAGE_MEMBERS: [(&str, bool); 3] = [
    ("collection.anki21b", true),
    ("collection.anki21", false),
    ("collection.anki2", false),
];

/// An open Anki collection, read-only. Packages are unpacked to a temporary
/// file that lives as long as this value.
pub struct Collection {
    conn: Connection,
    _unpacked: Option<NamedTempFile>,
}

impl Collection {
    /// Opens a collection file (`collection.anki2`) or a `.colpkg`/`.apkg` package.
    pub fn open(path: &Path) -> Result<Self> {
        let mut magic = [0u8; 16];
        let read = File::open(path)
            .and_then(|mut f| f.read(&mut magic))
            .with_context(|| format!("cannot read {}", path.display()))?;
        let magic = &magic[..read];

        let unpacked = if magic.starts_with(SQLITE_MAGIC) {
            None
        } else if magic.starts_with(ZIP_MAGIC) {
            Some(extract_package_db(path)?)
        } else {
            bail!(
                "{} is neither an Anki collection nor a .colpkg/.apkg package",
                path.display()
            )
        };
        let db_path = unpacked.as_ref().map_or(path, |f| f.path());
        // Opened read-write but only ever read: a read-only connection cannot
        // open a WAL-mode collection (as Anki leaves it) without its -wal file.
        let conn = Connection::open_with_flags(db_path, OpenFlags::SQLITE_OPEN_READ_WRITE)
            .or_else(|_| Connection::open_with_flags(db_path, OpenFlags::SQLITE_OPEN_READ_ONLY))
            .with_context(|| format!("cannot open {}", path.display()))?;
        // Reads WAL-mode collections without shared memory, which network
        // and WSL (/mnt/c) drives lack.
        conn.pragma_update(None, "locking_mode", "EXCLUSIVE")
            .with_context(|| format!("cannot read {} (is Anki still open?)", path.display()))?;
        Ok(Self {
            conn,
            _unpacked: unpacked,
        })
    }

    pub fn conn(&self) -> &Connection {
        &self.conn
    }

    /// Every row of the review log.
    pub fn revlog(&self) -> Result<Vec<RevlogEntry>> {
        read_revlog_from_db(&self.conn)
    }
}

/// Reads every row of the review log from a collection file
/// (`collection.anki2`) or a `.colpkg`/`.apkg` package.
pub fn read_revlog(path: &Path) -> Result<Vec<RevlogEntry>> {
    Collection::open(path)?.revlog()
}

fn extract_package_db(path: &Path) -> Result<NamedTempFile> {
    let file = File::open(path).with_context(|| format!("cannot open {}", path.display()))?;
    let mut archive = zip::ZipArchive::new(file)
        .with_context(|| format!("{} is not a valid package", path.display()))?;
    for (name, compressed) in PACKAGE_MEMBERS {
        let Ok(mut member) = archive.by_name(name) else {
            continue;
        };
        let mut out = NamedTempFile::new().context("cannot create a temporary file")?;
        if compressed {
            zstd::stream::copy_decode(&mut member, out.as_file_mut())
                .with_context(|| format!("cannot decompress {name}"))?;
        } else {
            io::copy(&mut member, out.as_file_mut())
                .with_context(|| format!("cannot extract {name}"))?;
        }
        return Ok(out);
    }
    bail!("{} contains no Anki collection", path.display())
}

/// Every row of the review log of an open collection database.
pub fn read_revlog_from_db(conn: &Connection) -> Result<Vec<RevlogEntry>> {
    let mut stmt = conn
        .prepare("SELECT id, cid, ease, type, factor FROM revlog")
        .context("cannot read the review log (is Anki still open?)")?;
    let rows = stmt.query_map([], |row| {
        Ok((
            row.get::<_, i64>(0)?,
            row.get::<_, i64>(1)?,
            row.get::<_, i64>(2)?,
            row.get::<_, i64>(3)?,
            row.get::<_, i64>(4)?,
        ))
    })?;

    let mut entries = Vec::new();
    for row in rows {
        let (id, card_id, ease, kind, factor) = row?;
        let (Some(kind), Ok(ease)) = (ReviewKind::from_anki(kind), u8::try_from(ease)) else {
            continue;
        };
        entries.push(RevlogEntry {
            id,
            card_id,
            ease,
            kind,
            factor,
        });
    }
    Ok(entries)
}

#[cfg(test)]
mod tests {
    use std::io::Write;

    use super::*;

    fn make_collection(path: &Path) {
        let conn = Connection::open(path).unwrap();
        conn.execute_batch(
            "CREATE TABLE revlog (id INTEGER PRIMARY KEY, cid INTEGER, usn INTEGER,
                 ease INTEGER, ivl INTEGER, lastIvl INTEGER, factor INTEGER,
                 time INTEGER, type INTEGER);
             INSERT INTO revlog VALUES (1000, 7, -1, 3, -600, 0, 0, 5000, 0);
             INSERT INTO revlog VALUES (2000, 7, -1, 4, 3, -600, 2500, 4000, 1);
             INSERT INTO revlog VALUES (3000, 7, -1, 3, 3, 3, 2500, 4000, 9);",
        )
        .unwrap();
    }

    fn expected() -> Vec<RevlogEntry> {
        vec![
            RevlogEntry {
                id: 1000,
                card_id: 7,
                ease: 3,
                kind: ReviewKind::Learning,
                factor: 0,
            },
            RevlogEntry {
                id: 2000,
                card_id: 7,
                ease: 4,
                kind: ReviewKind::Review,
                factor: 2500,
            },
        ]
    }

    #[test]
    fn reads_plain_collection_and_skips_unknown_types() {
        let dir = tempfile::tempdir().unwrap();
        let db = dir.path().join("collection.anki2");
        make_collection(&db);
        assert_eq!(read_revlog(&db).unwrap(), expected());
    }

    fn package(members: &[(&str, Vec<u8>)]) -> NamedTempFile {
        let out = NamedTempFile::new().unwrap();
        let mut zip = zip::ZipWriter::new(out.reopen().unwrap());
        for (name, bytes) in members {
            zip.start_file(*name, zip::write::SimpleFileOptions::default())
                .unwrap();
            zip.write_all(bytes).unwrap();
        }
        zip.finish().unwrap();
        out
    }

    #[test]
    fn reads_zstd_package_before_legacy_placeholder() {
        let dir = tempfile::tempdir().unwrap();
        let db = dir.path().join("c.db");
        make_collection(&db);
        let bytes = std::fs::read(&db).unwrap();
        let pkg = package(&[
            ("collection.anki2", b"placeholder".to_vec()),
            (
                "collection.anki21b",
                zstd::encode_all(bytes.as_slice(), 3).unwrap(),
            ),
        ]);
        assert_eq!(read_revlog(pkg.path()).unwrap(), expected());
    }

    #[test]
    fn reads_legacy_package() {
        let dir = tempfile::tempdir().unwrap();
        let db = dir.path().join("c.db");
        make_collection(&db);
        let pkg = package(&[("collection.anki2", std::fs::read(&db).unwrap())]);
        assert_eq!(read_revlog(pkg.path()).unwrap(), expected());
    }

    #[test]
    fn rejects_other_files() {
        let mut file = NamedTempFile::new().unwrap();
        file.write_all(b"hello").unwrap();
        assert!(read_revlog(file.path()).is_err());
        let empty = package(&[("media", b"{}".to_vec())]);
        assert!(read_revlog(empty.path()).is_err());
    }
}
