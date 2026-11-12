//! Search by meaning. The Mac's own language model turns each email into a list of
//! numbers, on the device, and similar emails get similar numbers. Nothing is sent
//! anywhere and no AI key is needed. Numbers are kept small: one byte each.

use crate::engine::Engine;
use crate::error::Result;
use crate::store::Store;
use rusqlite::params;
use serde_json::{json, Value};
use std::collections::HashSet;
use std::sync::atomic::Ordering;
use std::sync::Arc;

/// Turns text into its numbers, on this device. None if it can't.
pub type Embedder = Arc<dyn Fn(&str) -> Option<Vec<f32>> + Send + Sync>;

const PER_REFRESH: i64 = 50;
const TEXT_CHARS: usize = 600;
const CLOSE_ENOUGH: f32 = 0.45;
const MOST_FOUND: usize = 60;
/// How close an email must be to be offered as "Maybe related". Tuned on real questions.
pub(crate) const MEANING_CLOSE_ENOUGH: f32 = 0.6;
/// "Maybe related" is a few emails, never a page of them.
pub(crate) const MEANING_MOST_RELATED: usize = 5;

fn squeeze(vector: &[f32]) -> Vec<u8> {
    let length: f32 = vector.iter().map(|x| x * x).sum::<f32>().sqrt().max(1e-6);
    vector
        .iter()
        .map(|x| ((x / length) * 127.0).round().clamp(-127.0, 127.0) as i8 as u8)
        .collect()
}

fn likeness(a: &[u8], b: &[u8]) -> f32 {
    if a.len() != b.len() {
        return 0.0;
    }
    let sum: i64 = a
        .iter()
        .zip(b)
        .map(|(x, y)| i64::from(*x as i8) * i64::from(*y as i8))
        .sum();
    sum as f32 / (127.0 * 127.0)
}

impl Store {
    fn mail_wanting_meaning(&self, limit: i64) -> Result<Vec<(i64, String)>> {
        let db = self.reading_db();
        // Picks which first, as sorting with every email's text in hand would be slow.
        let mut st = db.prepare_cached(
            "SELECT m.id, m.subject || '. ' || COALESCE(b.text, m.snippet)
             FROM messages m JOIN bodies b ON b.message_id = m.id
             WHERE m.id IN (
                 SELECT n.id FROM meaning_todo t
                 JOIN messages n ON n.id = t.message_id
                 JOIN folders f ON f.id = n.folder_id
                 WHERE f.role NOT IN ('junk', 'trash', 'drafts')
                 ORDER BY n.date DESC LIMIT ?1
             )",
        )?;
        let rows = st.query_map([limit], |r| Ok((r.get(0)?, r.get(1)?)))?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    fn save_meaning(&self, id: i64, vector: &[u8]) -> Result<()> {
        let db = self.db();
        db.execute(
            "INSERT OR REPLACE INTO meanings (message_id, vector) VALUES (?1, ?2)",
            params![id, vector],
        )?;
        db.execute("DELETE FROM meaning_todo WHERE message_id = ?1", [id])?;
        Ok(())
    }

    /// With `readable`, only mail in those folders that doesn't look suspicious is looked at.
    fn closest_meanings(
        &self,
        wanted: &[u8],
        most: usize,
        readable: Option<&HashSet<i64>>,
    ) -> Result<Vec<(i64, f32)>> {
        let sql = match readable {
            None => "SELECT message_id, vector FROM meanings".to_string(),
            Some(folders) => {
                let list: Vec<String> = folders.iter().map(i64::to_string).collect();
                format!(
                    "SELECT x.message_id, x.vector FROM meanings x
                     JOIN messages m ON m.id = x.message_id
                     LEFT JOIN verdicts v ON v.message_key = m.message_key
                     WHERE m.folder_id IN ({}) AND COALESCE(v.risk, 0) = 0",
                    list.join(",")
                )
            }
        };
        let db = self.reading_db();
        let mut st = db.prepare_cached(&sql)?;
        let mut rows = st.query([])?;
        let mut found: Vec<(i64, f32)> = Vec::new();
        while let Some(r) = rows.next()? {
            let vector = r.get_ref(1)?.as_blob().unwrap_or(&[]);
            let score = likeness(wanted, vector);
            if score >= CLOSE_ENOUGH {
                found.push((r.get(0)?, score));
            }
        }
        found.sort_by(|a, b| b.1.total_cmp(&a.1));
        found.truncate(most);
        Ok(found)
    }

    pub fn meanings_saved(&self) -> Result<i64> {
        Ok(self
            .db()
            .query_row("SELECT COUNT(*) FROM meanings", [], |r| r.get(0))?)
    }
}

impl Engine {
    /// A little at a time, as each refresh finishes. Every inbox refreshes, but they all draw on
    /// the same list, so only one does this at a time rather than repeating each other's work.
    pub fn work_out_meanings(&self) -> Result<usize> {
        if self.working_out_meanings.swap(true, Ordering::AcqRel) {
            return Ok(0);
        }
        let done = self.work_out_some_meanings();
        self.working_out_meanings.store(false, Ordering::Release);
        done
    }

    fn work_out_some_meanings(&self) -> Result<usize> {
        let Some(embed) = &self.embedder else {
            return Ok(0);
        };
        if self.setting("search_meaning")? != "on" {
            return Ok(0);
        }
        let mut done = 0;
        for (id, text) in self.store.mail_wanting_meaning(PER_REFRESH)? {
            let short: String = text.chars().take(TEXT_CHARS).collect();
            // Some text has no answer, such as another language. It isn't asked about again.
            let vector = embed(&short).map(|v| squeeze(&v)).unwrap_or_default();
            self.store.save_meaning(id, &vector)?;
            done += 1;
        }
        Ok(done)
    }

    /// Which emails are about the same thing as this text.
    pub(crate) fn search_meaning(
        &self,
        text: &str,
        readable: Option<&HashSet<i64>>,
    ) -> Result<Value> {
        let Some(embed) = &self.embedder else {
            return Ok(json!({ "available": false, "ids": [] }));
        };
        if self.setting("search_meaning")? != "on" {
            return Ok(json!({ "available": false, "ids": [] }));
        }
        let Some(vector) = embed(text.trim()) else {
            return Ok(json!({ "available": true, "ids": [] }));
        };
        let close: Vec<(i64, f32)> = self
            .store
            .closest_meanings(&squeeze(&vector), MOST_FOUND, readable)?
            .into_iter()
            .filter(|x| x.1 >= MEANING_CLOSE_ENOUGH)
            .take(MEANING_MOST_RELATED)
            .collect();
        let ids: Vec<i64> = close.iter().map(|x| x.0).collect();
        let scores: Vec<f32> = close.iter().map(|x| x.1).collect();
        Ok(json!({ "available": true, "ids": ids, "scores": scores }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn closer_meanings_score_higher() {
        let a = squeeze(&[1.0, 0.0, 0.0]);
        let near = squeeze(&[0.9, 0.1, 0.0]);
        let far = squeeze(&[0.0, 1.0, 0.0]);
        assert!(likeness(&a, &near) > 0.9);
        assert!(likeness(&a, &far) < 0.1);
        assert_eq!(likeness(&a, &[]), 0.0, "an empty one matches nothing");
    }
}
