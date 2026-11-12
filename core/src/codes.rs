//! Sign-in and verification codes in an email, found by rules alone, so one click copies them.

/// Words that start or end a word saying "here's a code": code, Bestätigungscode, OTP,
/// verification, one-time, 2FA.
const CODE_WORDS: &[&str] = &[
    "code",
    "otp",
    "passcode",
    "verif",
    "one-time",
    "2fa",
    "two-factor",
    "authenticat",
    "código",
    "codice",
];
/// The same in scripts that join words.
const CODE_WORDS_JOINED: &[&str] = &["验证码", "認証", "인증", "কোড"];
/// "Code" after these is something else: a promo code, a zip code, a booking reference.
const OTHER_CODES: &[&str] = &[
    "promo",
    "discount",
    "coupon",
    "voucher",
    "gift",
    "referral",
    "tracking",
    "zip",
    "postal",
    "area",
    "country",
    "booking",
    "confirmation",
    "error",
    "status",
    "source",
    "bar",
    "qr",
];
/// A number right after these is a reference, not a code.
const REFERENCES: &[&str] = &[
    "order",
    "invoice",
    "ticket",
    "account",
    "reference",
    "ref",
    "no",
    "number",
    "id",
    "tracking",
    "phone",
    "call",
    "fax",
    "case",
    "booking",
];
/// How far apart, in words, a code and the word saying so can be.
const NEAR: usize = 8;
/// "8188 is your One Time Password": how far after "is" the word can be.
const IS_THEN_WORD: usize = 4;
/// "Your OTP for the payment of BDT 1,250.00 at Daraz is 4821": how far before "is".
const WORD_THEN_IS: usize = 16;

fn says_code(word: &str, before: Option<&str>) -> bool {
    let w = word.to_lowercase();
    let one_time =
        || before.is_some_and(|b| ["time", "one-time"].contains(&b.to_lowercase().as_str()));
    let is = w == "pin"
        || (w == "password" && one_time())
        || CODE_WORDS
            .iter()
            .any(|k| w.starts_with(k) || w.ends_with(k))
        || CODE_WORDS_JOINED.iter().any(|k| w.contains(k));
    is && !before.is_some_and(|b| OTHER_CODES.contains(&b.to_lowercase().as_str()))
}

fn digits(w: &str, sizes: std::ops::RangeInclusive<usize>) -> bool {
    sizes.contains(&w.len()) && w.chars().all(|c| c.is_ascii_digit())
}

/// The code an email gives to sign in or verify, from its subject and opening text: a run of
/// 4 to 8 digits (also written in two halves, "482 913"), or 6 to 8 capitals and digits, close
/// to a word saying it's a code. Amounts, dates, times and references are never codes.
pub(crate) fn one_time_code(subject: &str, text: &str) -> Option<String> {
    // The subject ends a sentence of its own.
    let all = format!("{subject}. {text}");
    let raw: Vec<&str> = all.split_whitespace().collect();
    let words: Vec<&str> = raw
        .iter()
        .map(|w| w.trim_matches(|c: char| "().,:;!?\"'[]{}«»“”".contains(c)))
        .collect();
    // Whether a sentence or clause ends among these words.
    let breaks = |from: usize, to: usize| {
        raw[from..to]
            .iter()
            .any(|w| w.ends_with(['.', ',', ';', '!', '?']))
    };
    let is = |k: usize| words.get(k).is_some_and(|w| w.eq_ignore_ascii_case("is"));
    let saying: Vec<usize> = (0..words.len())
        .filter(|&i| says_code(words[i], i.checked_sub(1).map(|b| words[b])))
        .collect();
    let mut best: Option<(usize, String)> = None;
    let mut joined = None;
    for (i, w) in words.iter().enumerate() {
        // The second half of a code written in two is part of the first.
        if joined == Some(i) {
            continue;
        }
        let halves = w
            .split_once('-')
            .filter(|(a, b)| digits(a, 3..=4) && digits(b, 3..=4));
        let code = if digits(w, 4..=8) {
            match words.get(i + 1) {
                Some(next) if digits(w, 3..=4) && digits(next, 3..=4) => {
                    joined = Some(i + 1);
                    format!("{w}{next}")
                }
                _ => w.to_string(),
            }
        } else if digits(w, 3..=3) && words.get(i + 1).is_some_and(|n| digits(n, 3..=4)) {
            joined = Some(i + 1);
            format!("{w}{}", words[i + 1])
        } else if let Some((a, b)) = halves {
            format!("{a}{b}")
        } else if (6..=8).contains(&w.len())
            && w.chars()
                .all(|c| c.is_ascii_uppercase() || c.is_ascii_digit())
            && w.chars().filter(char::is_ascii_digit).count() >= 2
            && w.chars().any(|c| c.is_ascii_uppercase())
        {
            w.to_string()
        } else {
            continue;
        };
        let reference = (i.saturating_sub(2)..i)
            .any(|b| REFERENCES.contains(&words[b].to_lowercase().trim_start_matches('#')));
        if reference {
            continue;
        }
        let Some(apart) = saying.iter().map(|&s| s.abs_diff(i)).min() else {
            continue;
        };
        // "8188 is your One Time Password", "Your OTP for … is 4821": "is" ties a code to the
        // word in the same clause, even a code of four digits a few words away.
        let last = if joined == Some(i + 1) { i + 1 } else { i };
        let tied = (is(last + 1)
            && saying
                .iter()
                .any(|&s| s > last + 1 && s - (last + 1) <= IS_THEN_WORD && !breaks(last + 1, s)))
            || (i >= 1
                && is(i - 1)
                && saying
                    .iter()
                    .any(|&s| s < i - 1 && i - 1 - s <= WORD_THEN_IS && !breaks(s, i - 1)));
        let apart = if tied { apart.min(1) } else { apart };
        // Four digits alone are often a year or a time, so they have to sit right by the word.
        let near = if code.len() == 4 { 3 } else { NEAR };
        if apart <= near && best.as_ref().is_none_or(|(d, _)| apart < *d) {
            best = Some((apart, code));
        }
    }
    if best.is_some() {
        return best.map(|(_, code)| code);
    }
    // The subject says it's a code ("OTP for … Login") and the email has just one number of
    // 6 to 8 digits that isn't a reference ("order 5551234"): that's it, however far from the
    // word. "Your account: 939486" still counts, the colon handing the code over.
    let clean = |w: &str| {
        w.trim_matches(|c: char| "().,:;!?\"'[]{}«»“”".contains(c))
            .to_string()
    };
    let subject_words: Vec<String> = subject.split_whitespace().map(clean).collect();
    let subject_says = (0..subject_words.len()).any(|i| {
        says_code(
            &subject_words[i],
            i.checked_sub(1).map(|b| subject_words[b].as_str()),
        )
    });
    if !subject_says {
        return None;
    }
    let raw: Vec<&str> = text.split_whitespace().collect();
    let mut found: Vec<String> = (0..raw.len())
        .filter(|&i| digits(&clean(raw[i]), 6..=8))
        .filter(|&i| {
            i.checked_sub(1).is_none_or(|b| {
                let before = clean(raw[b]).to_lowercase();
                raw[b].ends_with(':') || !REFERENCES.contains(&before.trim_start_matches('#'))
            })
        })
        .map(|i| clean(raw[i]))
        .collect();
    found.dedup();
    match found.as_slice() {
        [one] => Some(one.to_string()),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_code_the_subject_announces_is_found_far_from_its_word() {
        // The code sits after "account:" and twelve words from "One-Time".
        let text =
            "Programming Hero Let's code_your Career Your One-Time Password Dear MD. Mirazul \
                    Islam Shourov, Here is your One-Time Password to securely log in to your \
                    Programming Hero account: 939486 Note: This OTP is valid for 5 minutes.";
        assert_eq!(
            one_time_code("OTP for Programming Hero Login", text).as_deref(),
            Some("939486")
        );
        // A statement's account number is still not a code.
        assert_eq!(
            one_time_code("Your statement", "Account: 12345678 balance due"),
            None
        );
        // Nor is one of two numbers the subject can't tell apart.
        assert_eq!(
            one_time_code(
                "Your OTP",
                "Ref 482913 and order 551234 are in your account: 778899 663311"
            ),
            None
        );
    }

    #[test]
    fn a_sign_in_code_is_found() {
        assert_eq!(
            one_time_code(
                "Cloudflare Access login code for staging.orvio.app",
                "Your Cloudflare Access code 147363 This code expires after 10 minutes or if you request a new code."
            )
            .as_deref(),
            Some("147363")
        );
        assert_eq!(
            one_time_code(
                "Verify your email",
                "Your verification code is: 482 913. It expires soon."
            )
            .as_deref(),
            Some("482913"),
            "written in two halves"
        );
        assert_eq!(
            one_time_code("Sign in", "1234 5678 is your code").as_deref(),
            Some("12345678"),
            "two halves before the word"
        );
        assert_eq!(
            one_time_code("Your login code", "Enter K7XQ2P to sign in.").as_deref(),
            Some("K7XQ2P"),
            "letters and digits"
        );
        assert_eq!(
            one_time_code(
                "Security alert",
                "Your one-time passcode: 5521. Don't share it."
            )
            .as_deref(),
            Some("5521")
        );
        assert_eq!(
            one_time_code("Ihr Bestätigungscode", "Ihr Code lautet 839201.").as_deref(),
            Some("839201"),
            "in German"
        );
    }

    #[test]
    fn is_ties_a_code_to_its_word_in_the_same_sentence() {
        assert_eq!(
            one_time_code(
                "One Time Password (OTP) to verify e-commerce transaction with your City Bank Card",
                "Dear Cardholder, Thank you for shopping with your City Bank Card. 8188 is your \
                 One Time Password (OTP) for your City Bank card 376***311 Please insert the \
                 password in the pop-up window on the screen asking for OTP verification."
            )
            .as_deref(),
            Some("8188")
        );
        assert_eq!(
            one_time_code(
                "Payment",
                "Your OTP for the payment of BDT 1,250.00 at Daraz is 4821. Don't share it."
            )
            .as_deref(),
            Some("4821")
        );
        // A sentence end between them breaks the tie.
        assert_eq!(
            one_time_code(
                "Your OTP",
                "The OTP is valid for 10 minutes. Your balance is 4500 now."
            ),
            None
        );
        // "is" with no code word on its other side ties nothing.
        assert_eq!(
            one_time_code(
                "Your OTP",
                "Hello the total is 1250 for the three items you bought from us."
            ),
            None
        );
        assert_eq!(
            one_time_code(
                "Receipt",
                "8188 is your table number tonight, the OTP desk is upstairs."
            ),
            None
        );
    }

    #[test]
    fn numbers_that_arent_codes_are_left_alone() {
        // An order email: no code is asked for.
        assert_eq!(
            one_time_code(
                "Transfer(s) Initiated (order 27298692) (account faridul)",
                "Hi Faridul Haque, Hooray! The transfer of your domain(s) has been initiated. DATE ORDER NUMBER 2026/11/06 27298692"
            ),
            None
        );
        // Amounts, dates, times and an order number next to the word "code".
        assert_eq!(
            one_time_code(
                "Your discount code",
                "Use code SAVE on order 55512345 before 2026/12/01 at 10:30 for $1,250 off."
            ),
            None
        );
        assert_eq!(
            one_time_code("Lunch", "See you at 1230 near the station."),
            None
        );
    }
}
