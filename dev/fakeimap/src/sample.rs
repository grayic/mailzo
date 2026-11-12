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
}
