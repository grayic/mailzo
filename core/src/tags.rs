//! Tags the person makes up themselves, such as "Paid" or "Waiting on client", each with
//! a colour, put on conversations with an amount if they like. No AI is involved.

use crate::engine::Engine;
use crate::error::{CoreError, Result};
use crate::model::ThreadSummary;
use crate::store::Store;
use rusqlite::{params, OptionalExtension};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

/// The colours a tag can have. The screens turn each name into the app's own colours.
pub const TAG_COLOURS: &[&str] = &[
    "gray", "red", "orange", "yellow", "green", "teal", "blue", "purple", "pink",
];
const MOST_TAGS: i64 = 200;
const LONGEST_NAME: usize = 40;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Tag {
    pub id: i64,
    pub name: String,
    pub colour: String,
    /// How many conversations have it.
    #[serde(default)]
    pub count: i64,
}

/// A tag on one conversation, with its amount if one was given.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ThreadTag {
    pub tag_id: i64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub amount: Option<f64>,
}

/// Reads what the email list's query packs into one column: `id:amount` pairs.
pub(crate) fn unpack(packed: Option<String>) -> Vec<ThreadTag> {
    let Some(packed) = packed else {
        return vec![];
    };
    let mut out: Vec<ThreadTag> = packed
        .split(',')
        .filter_map(|one| {
            let (id, amount) = one.split_once(':')?;
            Some(ThreadTag {
                tag_id: id.parse().ok()?,
                amount: amount.parse().ok(),
            })
        })
        .collect();
    out.sort_by_key(|t| t.tag_id);
    out.dedup_by_key(|t| t.tag_id);
    out
}

fn clean_name(name: &str) -> Result<String> {
    let name: String = name.split_whitespace().collect::<Vec<_>>().join(" ");
    if name.is_empty() {
        return Err(CoreError::Invalid("Give the tag a name.".into()));
    }
    if name.chars().count() > LONGEST_NAME {
        return Err(CoreError::Invalid(format!(
            "Keep the name under {LONGEST_NAME} letters."
        )));
    }
    Ok(name)
}

fn clean_colour(colour: &str) -> Result<String> {
    if TAG_COLOURS.contains(&colour) {
        Ok(colour.to_string())
    } else {
        Err(CoreError::Invalid("Pick one of the colours.".into()))
    }
}

impl Store {
    pub fn list_tags(&self) -> Result<Vec<Tag>> {
        let db = self.db();
        let mut st = db.prepare_cached(
            "SELECT t.id, t.name, t.colour,
                    (SELECT COUNT(*) FROM thread_tags x WHERE x.tag_id = t.id)
             FROM tags t ORDER BY t.name COLLATE NOCASE",
        )?;
        let rows = st.query_map([], |r| {
            Ok(Tag {
                id: r.get(0)?,
                name: r.get(1)?,
                colour: r.get(2)?,
                count: r.get(3)?,
            })
        })?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    pub fn save_tag(&self, id: Option<i64>, name: &str, colour: &str) -> Result<Tag> {
        let name = clean_name(name)?;
        let colour = clean_colour(colour)?;
        let db = self.db();
        let same: Option<i64> = db
            .query_row(
                "SELECT id FROM tags WHERE name = ?1 COLLATE NOCASE",
                [&name],
                |r| r.get(0),
            )
            .optional()?;
        if same.is_some() && same != id {
            return Err(CoreError::Invalid(format!(
                "There's already a tag called {name}."
            )));
        }
        let id = match id {
            Some(id) => {
                let changed = db.execute(
                    "UPDATE tags SET name = ?2, colour = ?3 WHERE id = ?1",
                    params![id, name, colour],
                )?;
                if changed == 0 {
                    return Err(CoreError::NotFound("That tag is gone.".into()));
                }
                id
            }
            None => {
                let have: i64 = db.query_row("SELECT COUNT(*) FROM tags", [], |r| r.get(0))?;
                if have >= MOST_TAGS {
                    return Err(CoreError::Invalid(format!(
                        "You can have up to {MOST_TAGS} tags."
                    )));
                }
                db.execute(
                    "INSERT INTO tags (name, colour) VALUES (?1, ?2)",
                    params![name, colour],
                )?;
                db.last_insert_rowid()
            }
        };
        drop(db);
        self.list_tags()?
            .into_iter()
            .find(|t| t.id == id)
            .ok_or_else(|| CoreError::NotFound("That tag is gone.".into()))
    }

    /// Takes the tag off every conversation too. The mail itself is untouched.
    pub fn delete_tag(&self, id: i64) -> Result<()> {
        self.db().execute("DELETE FROM tags WHERE id = ?1", [id])?;
        Ok(())
    }

    pub fn set_thread_tag(
        &self,
        account_id: i64,
        thread_id: &str,
        tag_id: i64,
        on: bool,
        amount: Option<f64>,
    ) -> Result<()> {
        let db = self.db();
        if !on {
            db.execute(
                "DELETE FROM thread_tags WHERE account_id = ?1 AND thread_id = ?2 AND tag_id = ?3",
                params![account_id, thread_id, tag_id],
            )?;
            return Ok(());
        }
        if amount.is_some_and(|a| !a.is_finite()) {
            return Err(CoreError::Invalid("That amount isn't a number.".into()));
        }
        let known: bool = db.query_row(
            "SELECT EXISTS (SELECT 1 FROM tags WHERE id = ?1)",
            [tag_id],
            |r| r.get(0),
        )?;
        if !known {
            return Err(CoreError::NotFound("That tag is gone.".into()));
        }
        db.execute(
            "INSERT INTO thread_tags (account_id, thread_id, tag_id, amount, tagged_at)
             VALUES (?1, ?2, ?3, ?4, ?5)
             ON CONFLICT (account_id, thread_id, tag_id) DO UPDATE SET amount = ?4",
            params![
                account_id,
                thread_id,
                tag_id,
                amount,
                crate::identity::unix_now()
            ],
        )?;
        Ok(())
    }

    pub fn thread_tags(&self, account_id: i64, thread_id: &str) -> Result<Vec<ThreadTag>> {
        let db = self.db();
        let mut st = db.prepare_cached(
            "SELECT tag_id, amount FROM thread_tags WHERE account_id = ?1 AND thread_id = ?2
             ORDER BY tag_id",
        )?;
        let rows = st.query_map(params![account_id, thread_id], |r| {
            Ok(ThreadTag {
                tag_id: r.get(0)?,
                amount: r.get(1)?,
            })
        })?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    /// Conversations with this tag, newest tagged first.
    pub fn tagged(&self, tag_id: i64) -> Result<Vec<(i64, String)>> {
        let db = self.db();
        let mut st = db.prepare_cached(
            "SELECT account_id, thread_id FROM thread_tags WHERE tag_id = ?1
             ORDER BY tagged_at DESC LIMIT 500",
        )?;
        let rows = st.query_map([tag_id], |r| Ok((r.get(0)?, r.get(1)?)))?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }
}

impl Engine {
    pub(crate) fn tag_save(&self, id: Option<i64>, name: &str, colour: &str) -> Result<Value> {
        Ok(json!(self.store.save_tag(id, name, colour)?))
    }

    pub(crate) fn thread_tag(
        &self,
        account_id: i64,
        thread_id: &str,
        tag_id: i64,
        on: bool,
        amount: Option<f64>,
    ) -> Result<Value> {
        let thread = self.store.real_thread(thread_id)?;
        self.store
            .set_thread_tag(account_id, &thread, tag_id, on, amount)?;
        Ok(json!(self.store.thread_tags(account_id, &thread)?))
    }

    /// Every conversation with a tag, newest first, for the list.
    pub(crate) fn tagged_threads(&self, tag_id: i64) -> Result<Vec<ThreadSummary>> {
        let mut out = Vec::new();
        for (account_id, thread_id) in self.store.tagged(tag_id)? {
            // Mail that's gone from this device keeps its tag, in case it comes back.
            if let Some(found) = self.store.summary_of(account_id, &thread_id)? {
                out.push(found);
            }
        }
        out.sort_by_key(|t| std::cmp::Reverse(t.last_date));
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn packed_tags_are_read_back_with_and_without_amounts() {
        let tags = unpack(Some("3:,1:250.5,3:".into()));
        assert_eq!(
            tags,
            vec![
                ThreadTag {
                    tag_id: 1,
                    amount: Some(250.5)
                },
                ThreadTag {
                    tag_id: 3,
                    amount: None
                },
            ]
        );
        assert!(unpack(None).is_empty());
    }

    #[test]
    fn names_are_tidied_and_colours_checked() {
        assert_eq!(
            clean_name("  Waiting   on client ").unwrap(),
            "Waiting on client"
        );
        assert!(clean_name("   ").is_err());
        assert!(clean_name(&"x".repeat(41)).is_err());
        assert!(clean_colour("green").is_ok());
        assert!(clean_colour("#ff0000").is_err());
    }

    #[test]
    fn a_tag_goes_on_and_off_and_two_tags_cannot_share_a_name() {
        let store = Store::open_in_memory().unwrap();
        let account = store
            .insert_account(
                "a@x.test",
                "A",
                "x.test",
                993,
                crate::model::Security::Tls,
                "a",
            )
            .unwrap();
        let paid = store.save_tag(None, "Paid", "green").unwrap();
        assert!(store.save_tag(None, "paid", "red").is_err());
        // Renaming to its own name, in other letters, is fine.
        store.save_tag(Some(paid.id), "PAID", "teal").unwrap();

        store
            .set_thread_tag(account.id, "t1", paid.id, true, Some(120.0))
            .unwrap();
        // Tagging again changes the amount.
        store
            .set_thread_tag(account.id, "t1", paid.id, true, Some(90.0))
            .unwrap();
        assert_eq!(
            store.thread_tags(account.id, "t1").unwrap(),
            vec![ThreadTag {
                tag_id: paid.id,
                amount: Some(90.0)
            }]
        );
        assert_eq!(store.list_tags().unwrap()[0].count, 1);
        assert!(store
            .set_thread_tag(account.id, "t1", 999, true, None)
            .is_err());

        store
            .set_thread_tag(account.id, "t1", paid.id, false, None)
            .unwrap();
        assert!(store.thread_tags(account.id, "t1").unwrap().is_empty());

        store
            .set_thread_tag(account.id, "t1", paid.id, true, None)
            .unwrap();
        store.delete_tag(paid.id).unwrap();
        assert!(store.thread_tags(account.id, "t1").unwrap().is_empty());
    }
}
