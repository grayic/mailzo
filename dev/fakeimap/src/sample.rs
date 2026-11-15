//! Made-up demo mailbox. Every name, address and company here is fictional.

use crate::FakeServer;

pub fn seed(server: &FakeServer) {
    server.add_folder("INBOX", &["\\HasNoChildren"], 1);
    server.add_folder("Sent", &["\\HasNoChildren", "\\Sent"], 1);
    server.add_folder("Drafts", &["\\HasNoChildren", "\\Drafts"], 1);
    server.add_folder("Archive", &["\\HasNoChildren", "\\Archive"], 1);
    server.add_folder("Trash", &["\\HasNoChildren", "\\Trash"], 1);
    server.add_folder("Junk", &["\\HasNoChildren", "\\Junk"], 1);

    let msg = |from: &str, id: &str, refs: &str, subject: &str, date: &str, body: &str| {
        let refs = if refs.is_empty() {
            String::new()
        } else {
            format!("In-Reply-To: <{refs}>\nReferences: <{refs}>\n")
        };
        format!(
            "From: {from}\nTo: Demo User <demo@inbox.test>\nSubject: {subject}\nDate: {date}\n\
             Message-ID: <{id}>\n{refs}MIME-Version: 1.0\nContent-Type: text/plain; charset=utf-8\n\n{body}\n"
        )
    };

    server.add_message_dated(
        "INBOX",
        &msg(
            "Maya Chen <maya@northwind.test>",
            "q4-1@northwind.test",
            "",
            "Q4 launch plan",
            "Thu, 29 Oct 2026 09:12:00 +0000",
            "Hi,\n\nHere is the draft plan for the Q4 launch. Can you review the timeline by Friday?\n\nMaya",
        ),
        true,
        "29-Oct-2026 09:12:00 +0000",
    );
    server.add_message_dated(
        "Sent",
        &msg(
            "Demo User <demo@inbox.test>",
            "q4-2@inbox.test",
            "q4-1@northwind.test",
            "Re: Q4 launch plan",
            "Thu, 29 Oct 2026 11:40:00 +0000",
            "Looks good. I would move the press date back a week.",
        ),
        true,
        "29-Oct-2026 11:40:00 +0000",
    );
    server.add_message_dated(
        "INBOX",
        &msg(
            "PayPal <service@paypa1.com>",
            "scam-1@paypa1.com",
            "",
            "Your account has been suspended",
            "Sat, 31 Oct 2026 07:00:00 +0000",
            "Please verify your account now.",
        ),
        true,
        "31-Oct-2026 07:00:00 +0000",
    );
    server.add_message_dated(
        "INBOX",
        &msg(
            "Maya Chen <maya@northwind.test>",
            "q4-3@northwind.test",
            "q4-2@inbox.test",
            "Re: Q4 launch plan",
            "Sun, 01 Nov 2026 08:05:00 +0000",
            "Agreed, moved it. Final version goes out on Tuesday.",
        ),
        false,
        "01-Nov-2026 08:05:00 +0000",
    );
    server.add_message_dated(
        "INBOX",
        &msg(
            "Sam Okafor <sam@fieldwork.test>",
            "invoice-1@fieldwork.test",
            "",
            "Invoice 1042 for October",
            "Sat, 31 Oct 2026 16:30:00 +0000",
            "Hello,\n\nInvoice 1042 is attached to our portal. Payment is due in 14 days.\n\nThanks,\nSam",
        ),
        false,
        "31-Oct-2026 16:30:00 +0000",
    );

    // Hostile on purpose: the reader must not run the script or load the pixel.
    server.add_message_dated(
        "INBOX",
        "From: Weekly Digest <news@digest.test>\n\
         To: Demo User <demo@inbox.test>\n\
         Subject: This week in design\n\
         Date: Sun, 01 Nov 2026 12:00:00 +0000\n\
         Message-ID: <digest-44@digest.test>\n\
         List-Unsubscribe: <https://digest.test/unsubscribe>\n\
         MIME-Version: 1.0\n\
         Content-Type: multipart/alternative; boundary=\"b1\"\n\n\
         --b1\nContent-Type: text/plain; charset=utf-8\n\n\
         Five ideas worth stealing this week. Read more at https://digest.test/44\n\n\
         --b1\nContent-Type: text/html; charset=utf-8\n\n\
         <html><body style=\"font-family:Georgia,serif\">\
         <h2 style=\"color:#b3261e\">This week in design</h2>\
         <p>Five ideas worth stealing this week.</p>\
         <p><a id=\"web\" href=\"https://digest.test/44\">Read the full issue</a></p>\
         <p><a id=\"self\" href=\"https://digest.test/self\" target=\"_self\">Open here</a></p>\
         <p><a id=\"mail\" href=\"mailto:editor@digest.test\">Write to us</a></p>\
         <p><a id=\"bad\" href=\"javascript:alert(1)\" onclick=\"alert(2)\">Bad link</a></p>\
         <form action=\"https://digest.test/steal\"><input name=\"pw\"></form>\
         <meta http-equiv=\"refresh\" content=\"0;url=https://digest.test/redirect\">\
         <img src=\"http://127.0.0.1:4790/tracking-pixel.gif\" width=\"1\" height=\"1\" alt=\"\">\
         <img src=\"http://127.0.0.1:4790/tracking-pixel.gif?picture\" width=\"60\" height=\"60\" alt=\"\">\
         <script>document.title='SCRIPT RAN';window.parent.document.title='SCRIPT RAN';</script>\
         </body></html>\n\
         --b1--\n",
        true,
        "01-Nov-2026 12:00:00 +0000",
    );

    // Receipts and renewal emails for Purchases › Subscriptions, filed in Archive so the
    // inbox stays as it was.
    let paid = |from: &str, id: &str, subject: &str, day: &str, body: &str| {
        let header = format!("{} 08:00:00 +0000", day.replace('-', " "));
        let internal = format!("{day} 08:00:00 +0000");
        server.add_message_dated(
            "Archive",
            &msg(from, id, "", subject, &header, body),
            true,
            &internal,
        );
    };
    for (i, day) in ["05-Jul-2026", "05-Aug-2026", "05-Sep-2026", "05-Oct-2026"]
        .iter()
        .enumerate()
    {
        paid(
            "Pixelboard <billing@pixelboard.test>",
            &format!("pixelboard-{i}@pixelboard.test"),
            "Your Pixelboard receipt",
            day,
            "Thanks for your payment.\n\nProfessional plan, billed monthly.\nTotal: $15.00",
        );
        paid(
            "Tasklane <invoice+statements@stripe.com>",
            &format!("tasklane-{i}@stripe.test"),
            &format!("Your receipt from Tasklane #20{i}"),
            &day.replace("05-", "12-"),
            "Tasklane Standard, per month.\nAmount paid $8.00",
        );
        paid(
            "Streamly <no-reply@streamly.test>",
            &format!("streamly-{i}@streamly.test"),
            "Your Streamly Premium receipt",
            &day.replace("05-", "20-"),
            "Premium Individual, monthly. Total $11.99",
        );
    }
    paid(
        "Pixelboard <billing@pixelboard.test>",
        "pixelboard-5@pixelboard.test",
        "Your Pixelboard receipt",
        "05-Nov-2026",
        "Thanks for your payment.\n\nProfessional plan, billed monthly.\nTotal: $15.00",
    );
    paid(
        "Tasklane <invoice+statements@stripe.com>",
        "tasklane-5@stripe.test",
        "Your receipt from Tasklane #205",
        "12-Nov-2026",
        "Tasklane Standard, per month.\nAmount paid $8.00",
    );
    paid(
        "Streamly <no-reply@streamly.test>",
        "streamly-cancel@streamly.test",
        "Your subscription has been cancelled",
        "25-Oct-2026",
        "We're sorry to see you go. You'll keep Premium until November 20, 2026.",
    );
    paid(
        "Notewell Team <team@notewell.test>",
        "notewell-1@notewell.test",
        "Your Notewell receipt",
        "01-Mar-2026",
        "Plus plan, billed annually.\nAmount paid $96.00",
    );
    paid(
        "Domainly <support@domainly.test>",
        "domainly-1@domainly.test",
        "Renewal reminder: northwind.test",
        "10-Nov-2026",
        "Your domain will renew on November 19, 2026 for $14.58 a year.",
    );
    for (i, day) in ["01-Aug-2026", "01-Sep-2026", "01-Oct-2026"]
        .iter()
        .enumerate()
    {
        paid(
            "Canvaso <billing@canvaso.test>",
            &format!("canvaso-{i}@canvaso.test"),
            "Your Canvaso Pro receipt",
            day,
            "Canvaso Pro, monthly. Total $14.99",
        );
    }
    paid(
        "Cloudvault <billing@cloudvault.test>",
        "cloudvault-1@cloudvault.test",
        "Your Cloudvault receipt",
        "15-Oct-2026",
        "Cloudvault 2 TB, monthly. Total $9.99",
    );
    paid(
        "Cloudvault <billing@cloudvault.test>",
        "cloudvault-2@cloudvault.test",
        "Your payment failed",
        "15-Nov-2026",
        "We couldn't charge your card. Update your card to keep your 2 TB.",
    );
}
