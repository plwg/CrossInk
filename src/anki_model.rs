//! Note types, decks and cards from an Anki collection, in both the current
//! schema (tables `notetypes`, `fields`, `templates`, `decks` with protobuf
//! configs) and the legacy one (JSON in the `col` table).

use std::collections::HashMap;

use anyhow::{Context, Result};
use rusqlite::Connection;
use serde_json::Value;

#[derive(Debug, Clone, Default)]
pub struct NoteType {
    pub field_names: Vec<String>,
    /// (question format, answer format) by template ordinal.
    pub templates: Vec<(String, String)>,
    pub is_cloze: bool,
}

#[derive(Debug, Clone)]
pub struct CardRow {
    pub id: i64,
    pub deck_id: i64,
    pub ord: u32,
    /// 0 new, 1 learning, 2 review, 3 relearning.
    pub card_type: i64,
    /// -1 suspended; other values are scheduling queues.
    pub queue: i64,
    /// For new cards, the position in the new queue.
    pub due: i64,
    pub interval_days: i64,
    pub factor_permille: i64,
    pub note_type_id: i64,
    pub fields: Vec<String>,
    /// In a filtered deck: `deck_id` and `due` are the original deck's.
    pub in_filtered_deck: bool,
    /// Card JSON data (Anki keeps FSRS memory state here).
    pub data: String,
}

fn has_table(conn: &Connection, name: &str) -> Result<bool> {
    let count: i64 = conn.query_row(
        "SELECT count(*) FROM sqlite_master WHERE type = 'table' AND name = ?1",
        [name],
        |row| row.get(0),
    )?;
    Ok(count > 0)
}

pub fn note_types(conn: &Connection) -> Result<HashMap<i64, NoteType>> {
    if has_table(conn, "notetypes")? {
        note_types_current(conn)
    } else {
        note_types_legacy(conn)
    }
}

/// Deck names by id, with `::` between levels as Anki shows them.
pub fn deck_names(conn: &Connection) -> Result<HashMap<i64, String>> {
    let mut names = HashMap::new();
    if has_table(conn, "decks")? {
        let mut stmt = conn.prepare("SELECT id, name FROM decks")?;
        let rows = stmt.query_map([], |row| {
            Ok((row.get::<_, i64>(0)?, row.get::<_, String>(1)?))
        })?;
        for row in rows {
            let (id, name) = row?;
            names.insert(id, name.replace('\x1f', "::"));
        }
    } else {
        let json: String = conn.query_row("SELECT decks FROM col", [], |row| row.get(0))?;
        let decks: HashMap<String, Value> = serde_json::from_str(&json).context("bad col.decks")?;
        for (id, deck) in decks {
            if let (Ok(id), Some(name)) = (id.parse(), deck["name"].as_str()) {
                names.insert(id, name.to_string());
            }
        }
    }
    Ok(names)
}

pub fn cards(conn: &Connection) -> Result<Vec<CardRow>> {
    let mut stmt = conn.prepare(
        "SELECT c.id, CASE WHEN c.odid != 0 THEN c.odid ELSE c.did END, c.ord, c.type, c.queue,
                CASE WHEN c.odid != 0 THEN c.odue ELSE c.due END, c.ivl, c.factor, n.mid, n.flds,
                c.odid != 0, c.data
         FROM cards c JOIN notes n ON n.id = c.nid",
    )?;
    let rows = stmt.query_map([], |row| {
        Ok(CardRow {
            id: row.get(0)?,
            deck_id: row.get(1)?,
            ord: row.get(2)?,
            card_type: row.get(3)?,
            queue: row.get(4)?,
            due: row.get(5)?,
            interval_days: row.get(6)?,
            factor_permille: row.get(7)?,
            note_type_id: row.get(8)?,
            fields: row
                .get::<_, String>(9)?
                .split('\x1f')
                .map(str::to_string)
                .collect(),
            in_filtered_deck: row.get(10)?,
            data: row.get::<_, Option<String>>(11)?.unwrap_or_default(),
        })
    })?;
    rows.collect::<Result<_, _>>().map_err(Into::into)
}

fn note_types_current(conn: &Connection) -> Result<HashMap<i64, NoteType>> {
    let mut types: HashMap<i64, NoteType> = HashMap::new();
    let mut stmt = conn.prepare("SELECT id, config FROM notetypes")?;
    for row in stmt.query_map([], |row| {
        Ok((row.get::<_, i64>(0)?, row.get::<_, Vec<u8>>(1)?))
    })? {
        let (id, config) = row?;
        // Notetype.Config: kind = 1 (0 normal, 1 cloze).
        let is_cloze = protobuf_varint(&config, 1) == Some(1);
        types.insert(
            id,
            NoteType {
                is_cloze,
                ..Default::default()
            },
        );
    }

    let mut stmt = conn.prepare("SELECT ntid, ord, name FROM fields ORDER BY ntid, ord")?;
    for row in stmt.query_map([], |row| {
        Ok((row.get::<_, i64>(0)?, row.get::<_, String>(2)?))
    })? {
        let (ntid, name) = row?;
        types.entry(ntid).or_default().field_names.push(name);
    }

    let mut stmt = conn.prepare("SELECT ntid, ord, config FROM templates ORDER BY ntid, ord")?;
    for row in stmt.query_map([], |row| {
        Ok((row.get::<_, i64>(0)?, row.get::<_, Vec<u8>>(2)?))
    })? {
        let (ntid, config) = row?;
        // Template.Config: q_format = 1, a_format = 2.
        let qfmt = protobuf_string(&config, 1).unwrap_or_default();
        let afmt = protobuf_string(&config, 2).unwrap_or_default();
        types.entry(ntid).or_default().templates.push((qfmt, afmt));
    }
    Ok(types)
}

fn note_types_legacy(conn: &Connection) -> Result<HashMap<i64, NoteType>> {
    let json: String = conn.query_row("SELECT models FROM col", [], |row| row.get(0))?;
    let models: HashMap<String, Value> = serde_json::from_str(&json).context("bad col.models")?;
    let mut types = HashMap::new();
    for (id, model) in models {
        let Ok(id) = id.parse::<i64>() else { continue };
        let mut fields: Vec<(i64, String)> = model["flds"]
            .as_array()
            .into_iter()
            .flatten()
            .map(|f| {
                (
                    f["ord"].as_i64().unwrap_or(0),
                    f["name"].as_str().unwrap_or_default().to_string(),
                )
            })
            .collect();
        fields.sort_by_key(|(ord, _)| *ord);
        let mut templates: Vec<(i64, String, String)> = model["tmpls"]
            .as_array()
            .into_iter()
            .flatten()
            .map(|t| {
                (
                    t["ord"].as_i64().unwrap_or(0),
                    t["qfmt"].as_str().unwrap_or_default().to_string(),
                    t["afmt"].as_str().unwrap_or_default().to_string(),
                )
            })
            .collect();
        templates.sort_by_key(|(ord, _, _)| *ord);
        types.insert(
            id,
            NoteType {
                field_names: fields.into_iter().map(|(_, name)| name).collect(),
                templates: templates.into_iter().map(|(_, q, a)| (q, a)).collect(),
                is_cloze: model["type"].as_i64() == Some(1),
            },
        );
    }
    Ok(types)
}

fn read_varint(buf: &[u8], pos: &mut usize) -> Option<u64> {
    let mut value = 0u64;
    for shift in (0..64).step_by(7) {
        let byte = *buf.get(*pos)?;
        *pos += 1;
        value |= u64::from(byte & 0x7f) << shift;
        if byte & 0x80 == 0 {
            return Some(value);
        }
    }
    None
}

/// Visits the top-level fields of a protobuf message; returns the first
/// field `number` mapped by `pick`.
fn protobuf_field<T>(
    buf: &[u8],
    number: u64,
    pick: impl Fn(u64, &[u8], u64) -> Option<T>,
) -> Option<T> {
    let mut pos = 0;
    while pos < buf.len() {
        let key = read_varint(buf, &mut pos)?;
        let (field, wire) = (key >> 3, key & 7);
        match wire {
            0 => {
                let value = read_varint(buf, &mut pos)?;
                if field == number {
                    return pick(wire, &[], value);
                }
            }
            1 => pos += 8,
            2 => {
                let len = usize::try_from(read_varint(buf, &mut pos)?).ok()?;
                let bytes = buf.get(pos..pos.checked_add(len)?)?;
                pos += len;
                if field == number {
                    return pick(wire, bytes, 0);
                }
            }
            5 => pos += 4,
            _ => return None,
        }
    }
    None
}

fn protobuf_varint(buf: &[u8], number: u64) -> Option<u64> {
    protobuf_field(buf, number, |wire, _, value| (wire == 0).then_some(value))
}

fn protobuf_string(buf: &[u8], number: u64) -> Option<String> {
    protobuf_field(buf, number, |wire, bytes, _| {
        (wire == 2).then(|| String::from_utf8_lossy(bytes).into_owned())
    })
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    fn varint(mut v: u64, out: &mut Vec<u8>) {
        loop {
            let byte = (v & 0x7f) as u8;
            v >>= 7;
            if v == 0 {
                out.push(byte);
                return;
            }
            out.push(byte | 0x80);
        }
    }

    pub fn proto_string(field: u64, s: &str, out: &mut Vec<u8>) {
        varint(field << 3 | 2, out);
        varint(s.len() as u64, out);
        out.extend_from_slice(s.as_bytes());
    }

    pub fn proto_varint(field: u64, v: u64, out: &mut Vec<u8>) {
        varint(field << 3, out);
        varint(v, out);
    }

    #[test]
    fn reads_protobuf_fields_skipping_others() {
        let mut buf = Vec::new();
        proto_varint(5, 300, &mut buf);
        buf.extend_from_slice(&[(7 << 3) | 5, 1, 2, 3, 4]); // fixed32
        proto_string(1, "{{Front}}", &mut buf);
        proto_string(2, "{{Back}}", &mut buf);
        proto_varint(9, 1, &mut buf);
        assert_eq!(protobuf_string(&buf, 1).as_deref(), Some("{{Front}}"));
        assert_eq!(protobuf_string(&buf, 2).as_deref(), Some("{{Back}}"));
        assert_eq!(protobuf_varint(&buf, 5), Some(300));
        assert_eq!(protobuf_varint(&buf, 9), Some(1));
        assert_eq!(protobuf_varint(&buf, 3), None);
        assert_eq!(protobuf_string(&[0x0a, 0x05, b'a'], 1), None, "truncated");
    }
}
