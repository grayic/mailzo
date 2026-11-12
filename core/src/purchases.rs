//! Purchases: receipts, orders, deliveries, bills, payments and bank alerts, gathered on their
//! own list in the sidebar. Worked out from the sender and subject alone, so it's known the moment an
//! email arrives, before its body is downloaded. Mail stays where it is as well.

use crate::error::Result;
use crate::model::ThreadSummary;
use crate::store::Store;
use rusqlite::params;

/// Words that on their own say an email is about something bought or paid.
const SURE: &[&str] = &[
    "receipt",
    "invoice",
    "e-statement",
    "estatement",
    "card statement",
    "account statement",
    "monthly statement",
    "transaction alert",
    "transaction notification",
    "transaction successful",
    "payment received",
    "payment confirmation",
    "payment confirmed",
    "payment successful",
    "payment complete",
    "payment failed",
    "payment declined",
    "payment due",
    "payment reminder",
    "payment of",
    "payment to",
    "you paid",
    "you sent",
    "you've paid",
    "you have paid",
    "money sent",
    "money received",
    "order confirmation",
    "order confirmed",
    "order received",
    "order placed",
    "order has been placed",
    "order is confirmed",
    "your order",
    "your amazon.com order",
    "order #",
    "order no",
    "order number",
    "has shipped",
    "have shipped",
    "is on its way",
    "out for delivery",
    "has been delivered",
    "was delivered",
    "delivery update",
    // A refund done, not one asked for: "Refund please" waits on a reply.
    "your refund",
    "refund processed",
    "refund issued",
    "refund initiated",
    "refund completed",
    "refund confirmation",
    "refund has been",
    "been refunded",
    "subscription renewed",
    "subscription renewal",
    "renewal notice",
    "your subscription",
    "billing",
    "bill payment",
    "bill pay",
    "bill is ready",
    "bill is due",
    "bill due",
    "bill for",
    "e-bill",
    "ebill",
    "booking confirmation",
    "booking confirmed",
    "your booking",
    "your trip",
    "e-ticket",
    "your purchase",
    "purchase confirmation",
    "purchase confirmed",
    "purchase receipt",
    "purchase of",
    "you purchased",
    "debited",
    "has been credited",
    "was credited",
    "credited to your",
    "wallet transfer",
    "fund transfer",
    "funds transfer",
    "transfer successful",
    "you transferred",
    "you have transferred",
    "top-up",
    "top up successful",
    "recharge successful",
    "been charged",
    "was charged",
    "charged to your",
];

/// Addresses that only send receipts and the like.
const SENDERS: &[&str] = &[
    "receipt",
    "receipts",
    "invoice",
    "invoices",
    "billing",
    "payments",
    "payment",
    "orders",
    "order",
    "order-update",
    "shipment-tracking",
    "auto-confirm",
    "transactions",
];

/// Selling, not selling-to-you: an offer that happens to mention cards or orders.
const SELLING: &[&str] = &[
    "% off",
    "discount",
    "cashback",
    "cash back",
    "offer",
    "deal",
    "sale",
    "save up to",
    "coupon",
    "promo",
    "free shipping",
    "get up to",
    "enjoy up to",
    "win ",
    "chance to",
    "limited time",
    "shop now",
    "buy now",
    "buy 1",
];

/// Words inside other words don't count: "bill" isn't in "billion" and "order" isn't in "border".
fn has_phrase(text: &str, phrase: &str) -> bool {
    let wordy = |c: char| c.is_alphanumeric();
    let mut from = 0;
    while let Some(at) = text[from..].find(phrase) {
        let start = from + at;
        let end = start + phrase.len();
        let before_ok = !text[..start].chars().next_back().is_some_and(wordy)
            || !phrase.chars().next().is_some_and(wordy);
        let after_ok = !text[end..].chars().next().is_some_and(wordy)
            || !phrase.chars().next_back().is_some_and(wordy);
        if before_ok && after_ok {
            return true;
        }
        from = start + phrase.len().max(1);
    }
    false
}

/// "bill" written small, as in "Your Netflix bill", rather than someone called Bill.
fn has_small_bill(subject: &str) -> bool {
    has_phrase(subject, "bill")
}

/// Whether an email is about something bought or paid.
pub fn is_purchase(subject: &str, from_address: &str) -> bool {
    let small_bill = has_small_bill(subject);
    let subject = subject.to_lowercase();
    let local = from_address
        .split('@')
        .next()
        .unwrap_or_default()
        .to_lowercase();
    let from_sender = SENDERS
        .iter()
        .any(|s| local == *s || local.starts_with(&format!("{s}+")));
    let said = small_bill || SURE.iter().any(|p| has_phrase(&subject, p));
    if !said && !from_sender {
        return false;
    }
    // A receipt that also offers a discount on the next order is still a receipt.
    let record = [
        "receipt",
        "invoice",
        "statement",
        "transaction",
        "refund",
        "your order",
    ]
    .iter()
    .any(|p| has_phrase(&subject, p));
    record || !SELLING.iter().any(|p| has_phrase(&subject, p))
}

/// How many emails one round looks at, for mail that came in before this existed.
const SORT_ROUND: i64 = 20_000;
/// How many are saved at once, so a save never holds anything else up for long.
const SORT_STEP: i64 = 2_000;

/// How many conversations the Purchases list shows, newest first.
const LISTED: i64 = 200;

impl Store {
    /// Conversations with a receipt, order, bill or payment in them, newest first.
    pub(crate) fn purchase_threads(&self) -> Result<Vec<ThreadSummary>> {
        let found: Vec<(i64, String)> = {
            let db = self.reading_db();
            let mut st = db.prepare_cached(
                "SELECT m.account_id, m.thread_id FROM messages m
                 JOIN folders f ON f.id = m.folder_id
                 WHERE m.purchase = 1 AND f.role NOT IN ('trash', 'junk', 'drafts', 'sent')
                 GROUP BY m.account_id, m.thread_id
                 ORDER BY MAX(m.date) DESC LIMIT ?1",
            )?;
            let rows = st.query_map([LISTED], |r| Ok((r.get(0)?, r.get(1)?)))?;
            rows.collect::<rusqlite::Result<_>>()?
        };
        self.summaries_of(&found)
    }

    /// Works out purchases for mail that came before this existed, a round at a time,
    /// in small saves so nothing else waits long. New mail is worked out as it arrives.
    /// Returns how many.
    pub(crate) fn sort_purchases(&self) -> Result<usize> {
        let mut done = 0;
        while done < SORT_ROUND {
            let step = self.sort_purchases_step()?;
            if step == 0 {
                break;
            }
            done += step as i64;
        }
        Ok(done as usize)
    }

    fn sort_purchases_step(&self) -> Result<usize> {
        let waiting: Vec<(i64, String, String)> = {
            let db = self.reading_db();
            let mut st = db.prepare(
                "SELECT id, subject, from_addr FROM messages WHERE purchase IS NULL LIMIT ?1",
            )?;
            let rows = st.query_map([SORT_STEP], |r| {
                Ok((
                    r.get(0)?,
                    r.get::<_, Option<String>>(1)?.unwrap_or_default(),
                    r.get::<_, Option<String>>(2)?.unwrap_or_default(),
                ))
            })?;
            rows.collect::<rusqlite::Result<_>>()?
        };
        if waiting.is_empty() {
            return Ok(0);
        }
        let mut db = self.db();
        let tx = db.transaction()?;
        {
            let mut st = tx.prepare("UPDATE messages SET purchase = ?2 WHERE id = ?1")?;
            for (id, subject, from) in &waiting {
                st.execute(params![id, is_purchase(subject, from)])?;
            }
        }
        tx.commit()?;
        Ok(waiting.len())
    }
}

#[cfg(test)]
mod tests {
    use super::is_purchase;

    #[test]
    fn receipts_orders_bills_and_bank_alerts_are_purchases() {
        for (subject, from) in [
            ("Your receipt from Apple", "no_reply@email.apple.com"),
            (
                "Transaction Alert for UCB Credit Card",
                "ucb.retail@ucb.com.bd",
            ),
            (
                "E-statement (1781XXXXX756) for the month of October 2026",
                "estatement@citybank.com.bd",
            ),
            ("Your order has been placed.", "info@foodpanda.com.bd"),
            (
                "Your Amazon.com order #112-334 has shipped",
                "shipment-tracking@amazon.com",
            ),
            ("Invoice INV-2041 from Figma", "billing@figma.com"),
            ("Payment received – thank you", "noreply@stripe.com"),
            ("Refund processed", "help@uber.com"),
            ("Your refund is on its way", "orders@shop.test"),
            ("Your payment has been refunded", "help@app.test"),
            ("Your Netflix bill", "info@account.netflix.com"),
            ("Bill Payment Successful", "alerts@bkash.test"),
            ("You made a Wallet Transfer", "citytouch@thecitybank.com"),
            ("৳500 has been credited to your account", "alerts@bank.test"),
            ("Thank you for your purchase", "hello@gumroad.test"),
            ("Thanks for your business", "receipts@stripe.com"),
            (
                "Your receipt – plus 10% off your next order",
                "hello@shop.test",
            ),
        ] {
            assert!(is_purchase(subject, from), "{subject}");
        }
    }

    #[test]
    fn offers_and_everyday_mail_are_not() {
        for (subject, from) in [
            (
                "Get twice the feast exclusively with City Bank Cards!",
                "offers@citybank.com.bd",
            ),
            (
                "Get the latest iPhone 18 series with Flexibuy",
                "offers@citybank.com.bd",
            ),
            (
                "Enjoy up to 12% discount on total bill at Ritz using EBL Visa",
                "ebl@ebl.com.bd",
            ),
            ("These Prices End at Midnight !!!", "deals@stacksocial.com"),
            ("Project Discussion with Framerbite", "nazim@koinju.io"),
            ("Security alert", "no-reply@accounts.google.com"),
            ("The border is open", "news@paper.test"),
            ("A billion reasons", "news@paper.test"),
            ("Purchase 2 get 1 free – shop now", "promo@shop.test"),
            ("Lunch with Bill on Friday", "ana@company.test"),
            ("Bill Smith shared a file", "drive@google.test"),
            ("Credited to the design team", "ana@company.test"),
            ("Purchase order for the new office", "ana@company.test"),
            // Someone asking for their money back is waiting on a reply, not a record.
            ("Refund please", "ana@people.test"),
            ("Can I get a refund?", "bo@customer.test"),
        ] {
            assert!(!is_purchase(subject, from), "{subject}");
        }
    }
}
