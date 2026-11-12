//! Decides which new mail deserves a notification.

use crate::engine::Engine;
use crate::error::Result;
use crate::model::{Account, NewMail};

/// More than this at once becomes a single summary, not a burst of alerts.
const MOST_AT_ONCE: usize = 3;

#[derive(Default)]
pub struct Alert {
    pub title: String,
    /// One line under the title, cut short with "…" by the Mac: the subject.
    pub subtitle: String,
    pub body: String,
    pub sound: bool,
    /// The inbox and conversation a click opens, if it's about just one.
    pub thread: Option<(i64, String)>,
}

impl Alert {
    /// The line under the title: the subject, or else the body.
    pub fn second_line(&self) -> &str {
        if self.subtitle.is_empty() {
            &self.body
        } else {
            &self.subtitle
        }
    }
}

/// Mailing lists and automatic mail set these, and people's own mail never does.
pub(crate) fn looks_like_a_person(raw_header: &[u8], from_address: &str) -> bool {
    let lower = String::from_utf8_lossy(raw_header).to_ascii_lowercase();
    let automatic = [
        "\nlist-unsubscribe:",
        "\nlist-id:",
        "\nprecedence: bulk",
        "\nprecedence: list",
        "\nauto-submitted: auto",
    ]
    .iter()
    .any(|h| lower.contains(h));
    !automatic && !is_robot_address(from_address)
}

/// Parts of an address that mean a machine sends from it, not a person.
pub(crate) const ROBOT_MARKS: &[&str] = &[
    "noreply",
    "no-reply",
    "donotreply",
    "do-not-reply",
    "notifications@",
    "mailer-daemon",
];

pub(crate) fn is_robot_address(address: &str) -> bool {
    let address = address.to_ascii_lowercase();
    ROBOT_MARKS.iter().any(|n| address.contains(n))
}

/// Words in the subject of a sign-in code, a link to confirm an address, a password reset or a
/// warning about a sign-in. They only matter for a few minutes, so they alert even when only
/// what needs the person does.
const RIGHT_NOW: &[&str] = &[
    "verification code",
    "verify code",
    "security code",
    "login code",
    "log-in code",
    "sign-in code",
    "sign in code",
    "confirmation code",
    "one-time",
    "one time code",
    "one time password",
    "passcode",
    "otp",
    "2fa",
    "two-factor",
    "two factor",
    "your code",
    "verify your email",
    "verify your e-mail",
    "verify your account",
    "verify your identity",
    "confirm your email",
    "confirm your e-mail",
    "confirm your account",
    "reset your password",
    "password reset",
    "magic link",
    "sign-in link",
    "login link",
    "log in link",
    "new sign-in",
    "sign-in attempt",
    "unusual sign-in",
    "suspicious sign-in",
];

/// A code or link someone is waiting for, like "Verify your email address" or "482913 is your
/// code".
pub(crate) fn is_right_now(subject: &str) -> bool {
    let s = subject.to_lowercase();
    let has_word = |w: &str| {
        s.match_indices(w).any(|(i, _)| {
            let before = s[..i].chars().next_back();
            let after = s[i + w.len()..].chars().next();
            !before.is_some_and(char::is_alphanumeric) && !after.is_some_and(char::is_alphanumeric)
        })
    };
    if RIGHT_NOW.iter().any(|w| has_word(w)) {
        return true;
    }
    // A number of 4 to 8 digits standing alone, next to the word "code".
    let number = s
        .split(|c: char| !c.is_ascii_digit())
        .any(|n| (4..=8).contains(&n.len()));
    number && has_word("code")
}

pub(crate) struct AlertChoices {
    pub on: bool,
    /// `all`, `people` or `important`.
    pub which: String,
    pub details: bool,
    /// A few lines of the email under its subject.
    pub preview: bool,
    pub sound: bool,
    pub quiet_now: bool,
}

/// `inbox_choice` is the inbox's own preference: default, all, people or none.
pub(crate) fn choose_alerts(
    choices: &AlertChoices,
    inbox_choice: &str,
    inbox_name: &str,
    mail: &[NewMail],
) -> Vec<Alert> {
    if !choices.on || choices.quiet_now || inbox_choice == "none" {
        return vec![];
    }
    let which = if matches!(inbox_choice, "all" | "people" | "important") {
        inbox_choice
    } else {
        choices.which.as_str()
    };
    let wanted: Vec<&NewMail> = mail
        .iter()
        .filter(|m| match which {
            "all" => true,
            "people" => m.is_person,
            _ => m.needs_you || is_right_now(&m.subject),
        })
        .collect();
    if wanted.len() > MOST_AT_ONCE {
        return vec![Alert {
            title: format!("{} new emails", wanted.len()),
            body: inbox_name.to_string(),
            sound: choices.sound,
            thread: None,
            ..Default::default()
        }];
    }
    wanted
        .into_iter()
        .map(|m| {
            let thread = Some((m.account_id, m.thread_id.clone()));
            if choices.details {
                let from = if m.from_name.is_empty() {
                    &m.from_address
                } else {
                    &m.from_name
                };
                Alert {
                    title: from.clone(),
                    subtitle: if m.subject.is_empty() {
                        "(no subject)".to_string()
                    } else {
                        m.subject.clone()
                    },
                    body: if choices.preview {
                        m.preview.clone()
                    } else {
                        String::new()
                    },
                    sound: choices.sound,
                    thread,
                }
            } else {
                Alert {
                    title: "New email".to_string(),
                    body: inbox_name.to_string(),
                    sound: choices.sound,
                    thread,
                    ..Default::default()
                }
            }
        })
        .collect()
}

impl Engine {
    /// A teammate needs this person's attention.
    pub(crate) fn send_team_alert(&self, title: &str, body: &str) -> Result<()> {
        if let Some(sender) = &self.alert_sender {
            if self.setting("alerts")? == "on" {
                sender(&Alert {
                    subtitle: String::new(),
                    title: title.to_string(),
                    body: body.to_string(),
                    sound: self.setting("alert_sound")? == "on",
                    thread: None,
                });
            }
        }
        Ok(())
    }

    pub(crate) fn send_alerts(
        &self,
        account: &Account,
        mail: &[NewMail],
        quiet_now: bool,
    ) -> Result<()> {
        let Some(sender) = &self.alert_sender else {
            return Ok(());
        };
        // Nothing about anyone's mail shows while signing in is half done. Using Mailzo on
        // this device alone, with no Mailzo account, is fine.
        if self.cloud.is_some() && self.store.identity()?.is_some() && !self.is_ready()? {
            return Ok(());
        }
        let is_mine = |address: &str| {
            address.eq_ignore_ascii_case(&account.email)
                || account
                    .prefs
                    .aliases
                    .iter()
                    .any(|a| address.eq_ignore_ascii_case(&a.address))
        };
        let mail: Vec<NewMail> = mail
            .iter()
            .filter(|m| !is_mine(&m.from_address))
            .cloned()
            .collect();
        if mail.is_empty() {
            return Ok(());
        }
        let choices = AlertChoices {
            on: self.setting("alerts")? == "on",
            which: self.setting("alert_which")?,
            details: self.setting("alert_details")? == "on",
            preview: self.setting("alert_preview")? == "on",
            sound: self.setting("alert_sound")? == "on",
            quiet_now,
        };
        let name = if account.prefs.label.is_empty() {
            &account.email
        } else {
            &account.prefs.label
        };
        for alert in choose_alerts(&choices, &account.prefs.alerts, name, &mail) {
            sender(&alert);
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mail(name: &str, person: bool) -> NewMail {
        NewMail {
            account_id: 1,
            thread_id: format!("{name}-thread"),
            rfc_id: None,
            key: String::new(),
            needs_you: person,
            from_name: name.into(),
            from_address: format!("{name}@x.test"),
            subject: format!("From {name}"),
            preview: format!("Hi, it's {name}. Can we move the call?"),
            is_person: person,
        }
    }

    fn choices() -> AlertChoices {
        AlertChoices {
            on: true,
            which: "people".into(),
            details: true,
            preview: false,
            sound: true,
            quiet_now: false,
        }
    }

    #[test]
    fn newsletters_and_robots_are_recognised() {
        assert!(looks_like_a_person(
            b"From: Ana <ana@x.test>\r\nSubject: Hi\r\n",
            "ana@x.test"
        ));
        assert!(!looks_like_a_person(
            b"From: N\r\nList-Unsubscribe: <x>\r\n",
            "news@x.test"
        ));
        assert!(!looks_like_a_person(
            b"From: N\r\nPrecedence: bulk\r\n",
            "a@x.test"
        ));
        assert!(!looks_like_a_person(b"From: N\r\n", "no-reply@x.test"));
    }

    #[test]
    fn codes_and_verify_links_count_as_needing_me() {
        for subject in [
            "Verify your email address",
            "Your Microsoft account security code",
            "482913 is your Slack code",
            "Reset your password",
            "Your one-time passcode",
            "New sign-in to your account",
        ] {
            assert!(is_right_now(subject), "{subject}");
        }
        for subject in [
            "Weekly digest",
            "Order #12345 has shipped",
            "Code review for the new screen",
            "Bottle of 2024 wine",
            "Hotpot night",
        ] {
            assert!(!is_right_now(subject), "{subject}");
        }
        let only_needed = AlertChoices {
            which: "important".into(),
            ..choices()
        };
        let mut code = mail("microsoft", false);
        code.subject = "Verify your email address".into();
        let news = mail("news", false);
        assert_eq!(
            choose_alerts(&only_needed, "default", "Work", &[code, news]).len(),
            1
        );
    }

    #[test]
    fn people_only_leaves_out_newsletters() {
        let got = choose_alerts(
            &choices(),
            "default",
            "Work",
            &[mail("ana", true), mail("news", false)],
        );
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].title, "ana");
        assert_eq!(got[0].subtitle, "From ana");
        assert_eq!(got[0].body, "");
    }

    #[test]
    fn the_subject_is_one_line_and_a_preview_comes_only_when_chosen() {
        let one = [mail("ana", true)];
        let mut with_preview = choices();
        with_preview.preview = true;
        let got = choose_alerts(&with_preview, "default", "Work", &one);
        assert_eq!(got[0].subtitle, "From ana");
        assert_eq!(got[0].body, "Hi, it's ana. Can we move the call?");
        // Hiding the sender and subject hides the preview too.
        with_preview.details = false;
        let got = choose_alerts(&with_preview, "default", "Work", &one);
        assert_eq!(
            (got[0].subtitle.as_str(), got[0].body.as_str()),
            ("", "Work")
        );
    }

    #[test]
    fn an_inbox_can_override_the_app_choice() {
        let mixed = [mail("ana", true), mail("news", false)];
        assert_eq!(choose_alerts(&choices(), "all", "Work", &mixed).len(), 2);
        assert!(choose_alerts(&choices(), "none", "Work", &mixed).is_empty());
        let mut everyone = choices();
        everyone.which = "all".into();
        assert_eq!(choose_alerts(&everyone, "default", "Work", &mixed).len(), 2);
        assert_eq!(choose_alerts(&everyone, "people", "Work", &mixed).len(), 1);
    }

    #[test]
    fn switched_off_or_quiet_hours_send_nothing() {
        let one = [mail("ana", true)];
        let mut off = choices();
        off.on = false;
        assert!(choose_alerts(&off, "all", "Work", &one).is_empty());
        let mut quiet = choices();
        quiet.quiet_now = true;
        assert!(choose_alerts(&quiet, "all", "Work", &one).is_empty());
    }

    #[test]
    fn private_alerts_hide_the_sender_and_subject() {
        let mut private = choices();
        private.details = false;
        let got = choose_alerts(&private, "default", "Work", &[mail("ana", true)]);
        assert_eq!(got[0].title, "New email");
        assert_eq!(got[0].body, "Work");
        assert_eq!(got[0].thread, Some((1, "ana-thread".to_string())));
    }

    #[test]
    fn a_burst_becomes_one_summary() {
        let many: Vec<NewMail> = (0..5).map(|i| mail(&format!("p{i}"), true)).collect();
        let got = choose_alerts(&choices(), "default", "Work", &many);
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].title, "5 new emails");
        assert_eq!(got[0].thread, None, "it isn't about any one of them");
    }
}
