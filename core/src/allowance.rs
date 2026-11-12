//! What each AI request costs, and the plug-in point for AI that a paid plan includes.
//!
//! Every request's real cost is written down, whoever's key it used, so the person can see
//! what their own key spends. Mailzo itself never provides AI: a paid service can plug its
//! own in with `Engine::with_included_ai`, and decide before each request whether it goes
//! ahead. With nothing plugged in, none of this changes anything. A person's own key or plan
//! always comes first and is never counted against what's included.

use crate::engine::Engine;
use crate::error::Result;
use crate::store::Store;
use rusqlite::params;

/// What one request used, as the AI company reports it.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct Usage {
    pub input: i64,
    pub output: i64,
    /// Instructions read back from the AI company's store, at a tenth of the price.
    pub cache_read: i64,
    /// Instructions put in that store, at a quarter more than usual.
    pub cache_write: i64,
}

/// What included AI has cost so far, in dollars.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct Spent {
    pub month: f64,
    pub today: f64,
}

/// AI that comes with a paid plan. Plugged in by the service that provides it.
pub trait IncludedAi: Send + Sync {
    /// The key requests use when the person has no key or plan of their own.
    fn key(&self) -> String;
    /// Asked before each request. `Ok(true)` lets writing use the usual writing model,
    /// `Ok(false)` moves it to the quick model, and an error stops the request, with its
    /// message shown to the person.
    fn allow(&self, spent: Spent, writing: bool) -> Result<bool>;
}

/// Dollars per million tokens in and out, from the AI company's price list. Other
/// companies' prices aren't known here, and a plan has no bill per question.
pub(crate) fn claude_price(model: &str) -> (f64, f64) {
    if model.contains("opus") {
        (4.0, 20.0)
    } else if model.contains("sonnet") {
        (2.0, 10.0)
    } else if model.contains("haiku") {
        (0.10, 0.50)
    } else {
        (0.0, 0.0)
    }
}

pub fn cost(model: &str, u: Usage) -> f64 {
    let (per_in, per_out) = claude_price(model);
    let million = 1_000_000.0;
    (u.input as f64 * per_in
        + u.cache_read as f64 * per_in * 0.1
        + u.cache_write as f64 * per_in * 1.25
        + u.output as f64 * per_out)
        / million
}

impl Store {
    pub fn note_ai_request(
        &self,
        at: i64,
        day: &str,
        model: &str,
        u: Usage,
        dollars: f64,
        included: bool,
    ) -> Result<()> {
        self.db().execute(
            "INSERT INTO ai_requests (at, day, model, input, output, cache_read, cache_write, dollars, included)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
            params![
                at,
                day,
                model,
                u.input,
                u.output,
                u.cache_read,
                u.cache_write,
                dollars,
                included
            ],
        )?;
        // A year is plenty for any question about what something cost.
        self.db()
            .execute("DELETE FROM ai_requests WHERE at < ?1", [at - 400 * 86_400])?;
        Ok(())
    }

    /// What included AI cost on days starting with this, such as a month or one day.
    pub fn included_dollars(&self, starting: &str) -> Result<f64> {
        Ok(self.db().query_row(
            "SELECT COALESCE(SUM(dollars), 0) FROM ai_requests WHERE included = 1 AND day LIKE ?1 || '%'",
            [starting],
            |r| r.get(0),
        )?)
    }
}

impl Engine {
    /// Plugs in AI that a paid plan includes. The person's own key or plan still comes first.
    pub fn with_included_ai(mut self, included: std::sync::Arc<dyn IncludedAi>) -> Self {
        self.included_ai = Some(included);
        self
    }

    /// AI is running on what's included, not on the person's own key or plan.
    pub(crate) fn on_included_ai(&self) -> bool {
        self.included_ai.is_some()
            && self.ai_plan().ok().flatten().is_none()
            && self.secrets.get(crate::ai::AI_KEY).ok().flatten().is_none()
    }

    pub(crate) fn included_key(&self) -> Option<String> {
        self.included_ai.as_ref().map(|i| i.key())
    }

    /// What included AI has cost this month and today.
    pub fn included_ai_spent(&self) -> Result<Spent> {
        let today = crate::assist::day_text(crate::identity::unix_now());
        let month: String = today.chars().take(7).collect();
        Ok(Spent {
            month: self.store.included_dollars(&month)?,
            today: self.store.included_dollars(&today)?,
        })
    }

    /// Asked before every request on included AI: the plug-in decides whether writing may
    /// use the writing model, or stops the request. It sees what's been spent plus what
    /// requests still on their way have set aside, and this one's rough cost is set aside
    /// in the same step, so requests side by side can't each squeeze in under the limit.
    /// What's set aside is given back when the returned hold is dropped, by which time the
    /// real cost has been written down.
    pub(crate) fn included_gate(
        &self,
        writing: bool,
        rough: impl Fn(bool) -> f64,
    ) -> Result<(bool, Held<'_>)> {
        let Some(included) = &self.included_ai else {
            return Ok((writing, Held::nothing(self)));
        };
        let mut held = self.included_held.lock().unwrap_or_else(|e| e.into_inner());
        let mut spent = self.included_ai_spent()?;
        spent.month += *held;
        spent.today += *held;
        let writing = included.allow(spent, writing)?;
        let dollars = rough(writing).max(0.0);
        *held += dollars;
        Ok((
            writing,
            Held {
                engine: self,
                dollars,
            },
        ))
    }

    /// Writes down what a request cost. Whether it was included AI was decided when it set
    /// off, so a key added or removed in the meantime can't move the cost to the wrong side.
    pub(crate) fn note_ai_usage(&self, u: Usage, model: &str, included: bool) {
        let now = crate::identity::unix_now();
        let day = crate::assist::day_text(now);
        let dollars = cost(model, u);
        let _ = self.store.add_ai_usage(
            &day,
            u.input + u.cache_read + u.cache_write,
            u.output,
            dollars,
        );
        let _ = self
            .store
            .note_ai_request(now, &day, model, u, dollars, included);
    }
}

/// What an included AI request has set aside until its real cost is known.
pub(crate) struct Held<'a> {
    engine: &'a Engine,
    dollars: f64,
}

impl<'a> Held<'a> {
    fn nothing(engine: &'a Engine) -> Self {
        Held {
            engine,
            dollars: 0.0,
        }
    }
}

impl Drop for Held<'_> {
    fn drop(&mut self) {
        if self.dollars == 0.0 {
            return;
        }
        let mut held = self
            .engine
            .included_held
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        *held = (*held - self.dollars).max(0.0);
    }
}

/// About what a request can cost at most: the words sent, at roughly three letters a
/// token, and the longest answer it may give.
pub(crate) fn rough_cost(model: &str, letters: i64) -> f64 {
    cost(
        model,
        Usage {
            input: letters / 3 + 1,
            output: crate::ai::MOST_ANSWER_TOKENS,
            ..Default::default()
        },
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reused_instructions_cost_a_tenth_and_storing_them_a_quarter_more() {
        let plain = cost(
            "claude-sonnet-5-5",
            Usage {
                input: 1_000_000,
                ..Default::default()
            },
        );
        assert!((plain - 2.0).abs() < 1e-9);
        let read = cost(
            "claude-sonnet-5-5",
            Usage {
                cache_read: 1_000_000,
                ..Default::default()
            },
        );
        assert!((read - 0.2).abs() < 1e-9);
        let write = cost(
            "claude-haiku-5-5",
            Usage {
                cache_write: 1_000_000,
                output: 1_000_000,
                ..Default::default()
            },
        );
        assert!((write - (0.125 + 0.5)).abs() < 1e-9);
    }
}
