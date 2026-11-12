//! Apple Intelligence: the Mac's own AI, handed to the mail engine for when the person
//! hasn't connected a plan or key. The Swift part is in swift/AppleAI.swift; build.rs only
//! builds it in when Xcode has Apple's framework, and otherwise this says "not this Mac".

use mailcore::on_device::{OnDeviceAi, OnDeviceError, OnDeviceState};

pub struct AppleAi;

#[cfg(apple_ai)]
mod native {
    use std::ffi::{c_char, CStr, CString};

    extern "C" {
        fn mailzo_ai_state() -> i32;
        fn mailzo_ai_ask(
            system: *const c_char,
            prompt: *const c_char,
            schema: *const c_char,
            answer: *mut *mut c_char,
        ) -> i32;
        fn mailzo_ai_free(text: *mut c_char);
    }

    pub fn state() -> i32 {
        unsafe { mailzo_ai_state() }
    }

    /// The answer, or the Swift side's reason number.
    pub fn ask(system: &str, prompt: &str, schema: &str) -> Result<String, i32> {
        // Text with a zero byte in it can't cross; emails rarely have one, so drop them.
        let c = |s: &str| CString::new(s.replace('\0', "")).unwrap_or_default();
        let (system, prompt, schema) = (c(system), c(prompt), c(schema));
        let mut answer: *mut c_char = std::ptr::null_mut();
        let code = unsafe {
            mailzo_ai_ask(
                system.as_ptr(),
                prompt.as_ptr(),
                schema.as_ptr(),
                &mut answer,
            )
        };
        if code != 0 || answer.is_null() {
            return Err(if code == 0 { 4 } else { code });
        }
        let text = unsafe { CStr::from_ptr(answer) }
            .to_string_lossy()
            .into_owned();
        unsafe { mailzo_ai_free(answer) };
        Ok(text)
    }
}

#[cfg(apple_ai)]
impl OnDeviceAi for AppleAi {
    fn state(&self) -> OnDeviceState {
        match native::state() {
            0 => OnDeviceState::Ready,
            2 => OnDeviceState::TurnedOff,
            3 => OnDeviceState::Downloading,
            _ => OnDeviceState::NotThisMac,
        }
    }

    fn ask(&self, system: &str, prompt: &str, schema: &str) -> Result<String, OnDeviceError> {
        native::ask(system, prompt, schema).map_err(|code| match code {
            1 => OnDeviceError::TooLong,
            2 => OnDeviceError::Refused,
            3 => OnDeviceError::NotReady,
            _ => OnDeviceError::Failed,
        })
    }
}

#[cfg(not(apple_ai))]
impl OnDeviceAi for AppleAi {
    fn state(&self) -> OnDeviceState {
        OnDeviceState::NotThisMac
    }

    fn ask(&self, _: &str, _: &str, _: &str) -> Result<String, OnDeviceError> {
        Err(OnDeviceError::NotReady)
    }
}

#[cfg(all(test, apple_ai))]
mod tests {
    use super::*;

    /// Asks this Mac's real model, so only run by hand: `cargo test -p inbox-app apple_ai -- --ignored`.
    #[test]
    #[ignore]
    fn answers_in_the_shape_asked() {
        let ai = AppleAi;
        assert_eq!(
            ai.state(),
            OnDeviceState::Ready,
            "Apple Intelligence isn't ready here"
        );
        let schema = r#"{"type":"object","required":["headline","items"],"properties":{"headline":{"type":"string"},"items":{"type":"array","items":{"type":"string"}},"folder":{"anyOf":[{"type":"string","enum":["inbox","archive"]},{"type":"null"}]}}}"#;
        let started = std::time::Instant::now();
        let answer = ai
            .ask(
                "You help someone with their email. Answer from the emails given.",
                "<email id=\"1\">From: Hostinger\nSubject: Your hosting renews on 12 Nov\nYour plan for example.com renews on 12 November for $59.88.</email>\nQuestion: is anything expiring soon?",
                schema,
            )
            .expect("an answer");
        eprintln!("{:?}: {answer}", started.elapsed());
        let value: serde_json::Value = serde_json::from_str(&answer).expect("JSON");
        assert!(value["headline"].is_string());
        assert!(value["items"].is_array());
    }

    /// Ask AI's own job, with six emails like the app sends.
    #[test]
    #[ignore]
    fn answers_a_question_about_mail() {
        let system = "You help someone find things in their own email. Today is 6 Nov 2026. Answer only from the emails given, and say plainly when they don't hold the answer. Write in English, in plain friendly words. `answer` is one or two short sentences that answer the question directly, with the key fact first. `points` are up to 6 short lines with the details worth seeing, each naming the ids of the emails it came from. `key_ids` are the ids of the 1 to 3 emails the person most needs to open. `suggestions` are up to 3 short follow-up questions.";
        let schema = r#"{"type":"object","additionalProperties":false,"required":["answer","points","key_ids","suggestions"],"properties":{"answer":{"type":"string"},"points":{"type":"array","items":{"type":"object","additionalProperties":false,"required":["text","ids"],"properties":{"text":{"type":"string"},"ids":{"type":"array","items":{"type":"integer"}}}}},"key_ids":{"type":"array","items":{"type":"integer"}},"suggestions":{"type":"array","items":{"type":"string"}}}}"#;
        let emails = [
            (11, "Hostinger", "Your hosting plan expires in 7 days", "Your Premium Web Hosting for thefarhan.com expires on 13 Nov 2026. Renew now for $35.88 to keep your site online."),
            (12, "Namecheap", "Domain renewal reminder", "grayic.com will auto-renew on 2 Dec 2026 for $13.98 using your saved card."),
            (13, "Faridul", "Lunch?", "Are you free for lunch on Friday?"),
            (14, "Figma", "Your invoice", "Thanks for your payment of $15.00 for Figma Professional."),
            (15, "Apple", "Your receipt", "iCloud+ 200 GB, $2.99, renews monthly."),
            (16, "Stripe", "Payout sent", "A payout of $412.10 is on its way to your bank."),
        ];
        let material: String = emails
            .iter()
            .map(|(id, from, subject, text)| {
                format!("<email id=\"{id}\">\nFrom: {from}\nSubject: {subject}\n{text}\n</email>\n")
            })
            .collect();
        let started = std::time::Instant::now();
        let answer = AppleAi
            .ask(
                system,
                &format!("Question: is anything expiring soon?\n\nThe emails:\n{material}"),
                schema,
            )
            .expect("an answer");
        eprintln!("{:?}: {answer}", started.elapsed());
        let value: serde_json::Value = serde_json::from_str(&answer).expect("JSON");
        assert!(
            value["answer"]
                .as_str()
                .unwrap()
                .to_lowercase()
                .contains("hosting")
                || value["answer"].as_str().unwrap().contains("13")
        );
    }
}
