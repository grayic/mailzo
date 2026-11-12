//! The Mac's own AI (Apple Intelligence). It's built in, free, works offline and nothing
//! leaves the device, so it's what AI uses when the person hasn't connected a plan or key.
//! The app hands it in (`Engine::with_on_device`); the engine itself knows nothing of Apple.
//!
//! Its model is small and reads only a few thousand words at once, so what goes to it is
//! kept short, and a plan or key the person connects always comes first.

use crate::ai::ai_error;
use crate::engine::Engine;
use crate::error::{CoreError, Result};
use serde_json::Value;
use std::sync::Arc;
use std::time::Duration;

/// Stands in for a key when the Mac's own AI answers.
pub(crate) const ON_DEVICE_NOT_A_KEY: &str = "on-device";
/// Roughly what fits in its window with room left for the answer.
pub(crate) const MOST_CHARS: usize = 9_000;
const ANSWER_WAIT: Duration = Duration::from_secs(120);

/// Whether this Mac can use its own AI right now.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OnDeviceState {
    Ready,
    /// An older Mac or macOS that doesn't have it.
    NotThisMac,
    /// The Mac has it, but Apple Intelligence is switched off in System Settings.
    TurnedOff,
    /// Switched on, and the model is still downloading.
    Downloading,
}

impl OnDeviceState {
    pub fn id(self) -> &'static str {
        match self {
            OnDeviceState::Ready => "ready",
            OnDeviceState::NotThisMac => "not_this_mac",
            OnDeviceState::TurnedOff => "turned_off",
            OnDeviceState::Downloading => "downloading",
        }
    }
}

/// Why it didn't answer.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OnDeviceError {
    /// More than its window holds.
    TooLong,
    /// Apple's safety check stopped it.
    Refused,
    NotReady,
    Failed,
}

pub trait OnDeviceAi: Send + Sync {
    fn state(&self) -> OnDeviceState;
    /// One question. `schema` is the JSON schema the answer must follow, as text; the model
    /// is held to it where it can be, so the answer is a JSON object.
    fn ask(
        &self,
        system: &str,
        prompt: &str,
        schema: &str,
    ) -> std::result::Result<String, OnDeviceError>;
}

pub type OnDevice = Arc<dyn OnDeviceAi>;

/// Fits text into `most` characters by taking out the middle. The start often says what to
/// do and the end holds the newest email, so both stay.
fn keep_ends(text: &str, most: usize) -> String {
    let count = text.chars().count();
    if count <= most {
        return text.to_string();
    }
    let head = most * 2 / 5;
    let tail = most.saturating_sub(head + 5);
    let start: String = text.chars().take(head).collect();
    let end: String = text.chars().skip(count - tail).collect();
    format!("{start}\n…\n{end}")
}

impl Engine {
    /// How this Mac's own AI is doing, if the app has one to give.
    pub(crate) fn on_device_state(&self) -> Option<OnDeviceState> {
        self.on_device.as_ref().map(|ai| ai.state())
    }

    /// The Mac's own AI answers when it's ready, the person hasn't switched it off, and
    /// nothing they connected (a plan, a key, or AI included with Cloud) comes first.
    pub(crate) fn on_device_in_use(&self) -> Result<bool> {
        if self.on_device_state() != Some(OnDeviceState::Ready) {
            return Ok(false);
        }
        if self.setting("ai_on_device")? != "on" {
            return Ok(false);
        }
        if self.ai_plan()?.is_some() {
            return Ok(false);
        }
        Ok(self.secrets.get(crate::ai::AI_KEY)?.is_none() && self.included_key().is_none())
    }

    /// One question to the Mac's own AI, made to fit its small window.
    pub(crate) async fn ask_on_device(
        &self,
        system: &str,
        question: &str,
        schema: Value,
    ) -> Result<String> {
        let ai = self
            .on_device
            .clone()
            .ok_or_else(|| ai_error("ai_key", "Apple Intelligence isn't on this Mac."))?;
        let schema = schema.to_string();
        let room = MOST_CHARS.saturating_sub(system.chars().count() + schema.chars().count());
        let mut question = keep_ends(question, room.max(1_000));
        // One at a time: it runs on this Mac, and a second would only wait behind the first.
        let _turn = self
            .on_device_turns
            .acquire()
            .await
            .map_err(|_| ai_error("ai_failed", "The AI couldn't work that one out."))?;
        for attempt in 0..2 {
            let (ai, system, prompt, schema) = (
                ai.clone(),
                system.to_string(),
                question.clone(),
                schema.clone(),
            );
            let started = std::time::Instant::now();
            let (logged_system, logged_prompt) = (system.clone(), prompt.clone());
            let asked = tokio::task::spawn_blocking(move || ai.ask(&system, &prompt, &schema));
            let answer = tokio::time::timeout(ANSWER_WAIT, asked)
                .await
                .map_err(|_| ai_error("ai_failed", "The AI took too long to answer."))?
                .map_err(|_| ai_error("ai_failed", "The AI couldn't work that one out."))?;
            self.note_ai_usage(crate::allowance::Usage::default(), "", false);
            crate::plans::log_for_measuring(
                &logged_system,
                &logged_prompt,
                started.elapsed(),
                &serde_json::json!({ "on_device": true }),
            );
            match answer {
                Ok(text) => return Ok(crate::ai::without_code_fence(&text).to_string()),
                // Words count differently in other languages, so a guess can be too long.
                // Once more with half as much.
                Err(OnDeviceError::TooLong) if attempt == 0 => {
                    let half = question.chars().count() / 2;
                    question = keep_ends(&question, half);
                }
                // It sometimes just fails, and works when asked again.
                Err(OnDeviceError::Failed) if attempt == 0 => {}
                Err(e) => return Err(said(e)),
            }
        }
        Err(said(OnDeviceError::Failed))
    }
}

fn said(e: OnDeviceError) -> CoreError {
    match e {
        OnDeviceError::TooLong => ai_error(
            "ai_failed",
            "That's too much for Apple Intelligence. Connect Claude or ChatGPT in Settings › AI for bigger jobs.",
        ),
        OnDeviceError::Refused => ai_error(
            "ai_failed",
            "Apple's safety check stopped this one. Try other words, or connect Claude or ChatGPT in Settings › AI.",
        ),
        OnDeviceError::NotReady => {
            ai_error("ai_key", "Apple Intelligence isn't ready on this Mac yet.")
        }
        OnDeviceError::Failed => ai_error(
            "ai_failed",
            "Apple Intelligence couldn't do this one. Try again, or connect Claude or ChatGPT in Settings › AI.",
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::keep_ends;

    #[test]
    fn keeps_the_start_and_the_end() {
        let text = format!("START{}END", "x".repeat(100));
        let kept = keep_ends(&text, 40);
        assert!(kept.starts_with("START") && kept.ends_with("END"));
        assert!(kept.chars().count() <= 40);
        assert_eq!(keep_ends("héllo", 10), "héllo");
    }
}
