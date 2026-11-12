# Features

Who built what, where it lives, and its key commits, so nobody builds the same thing twice. Check it before starting, claim new work under "In progress", and move your line into the list when it's done. Keep each line short.

## In progress

- Faridul: the search bar with exact matches only, no AI, and no Ask (Tasks 3–5 of `docs/superpowers/plans/2026-11-06-ask-ai-rebuild.md`), branch `faridul/dev`.

## Mail and reading

| Feature | Where | Who | Key commits |
|---|---|---|---|
| New mail as soon as the server says so, and the banner for it | `core/src/sync.rs`, `core/src/engine.rs`, `app/src-tauri/src/banners.rs` | Faridul | 7148664, c3f2fb3, ebcc29a |
| Notifications: sender as the title, subject on one line, a preview of the message if chosen | `core/src/alerts.rs`, `app/src-tauri/src/banners.rs`, Settings › Notifications | Faridul | 8978754 |
| Gmail being busy or a token needing renewal never asks to sign in again; an inbox is flagged as not refreshing only after a day | `core/src/imap.rs`, `core/src/engine.rs`, `App.tsx` | Faridul | 0528929, c1ebe20 |
| Clicking a banner opens its email | `app/src-tauri/src/lib.rs`, `App.tsx` | Faridul | 17c9764 |
| Emails open at once, remembered and read ahead | `App.tsx`, `Reader.tsx` | Faridul | 30aaeab |
| Actions show at once and save in the background, with Undo | `App.tsx`, `UndoMove.tsx` | Faridul | 1cab531, abd175b |
| Show original | `ShowOriginal.tsx` | Faridul | d2f4629 |
| Full screen compose | `Composer.tsx` | Faridul | 2e07d71 |
| The earlier message in a reply folds under …, including replies sent before the quote mark was kept | `core/src/compose.rs` (`clean_html`), `app/ui/prepareEmail.ts` | Faridul | 30afa7c |
| Quick-reply taps write a whole reply; an About you note per inbox that AI replies and suggestions use | `core/src/assist.rs` (`ai_write_reply`, `about_line`), `Priority.tsx`, `InboxPage.tsx` | Faridul | 6ac719f |
| Unread emails stand out: bold sender and subject on a faint tint | `styles.css`, `tokens.css` (`--accent-faint`) | Faridul | 4cd2a26 |
| AI replies type out where you write, closing the reply stops them; Quick reply shows only replies written for that email; opening a reply scrolls fully down | `Composer.tsx`, `RichEditor.tsx`, `Priority.tsx`, `styles.css` | Faridul | ea093de, 8066763, 793bbcf |
| Paste and Match Style on ⌘⇧V: pasted text takes the message's own style | `app/src-tauri/src/lib.rs` (Edit menu) | Faridul | a286b01 |
| Search looks in the inbox it starts from, or in all of them from All Inbox, with a picker to switch | `App.tsx`, `core/src/store.rs` (`account` filter) | Faridul | 67a261e, d864077 |
| Search shows the newest 300 at first and more as you scroll | `core/src/search.rs` (`most`), `App.tsx` (`growList`) | Faridul | 50ca6c9 |
| Files and links shared in a conversation | `Reader.tsx`, `core/src/store.rs` | Farhan | f463592 |
| Your own tags | `core/src/tags.rs`, `app/ui/tags.tsx` | Farhan | e1d1994 |
| Ask: chat with one inbox or all of them, answers with numbered sources, on your own AI | `AskChat.tsx`, `core/src/assist.rs` (`ai_chat`) | Farhan | 8e983d9 |
| Ask AI rebuilt: code reads the question and finds exact matches; lists, waiting-on-me and booking facts need no AI; answers come from short passages and every date, amount and name is checked against the emails; hidden text never reaches AI; catch-ups, today's plan and briefs before a call use mail and calendar | `core/src/question.rs`, `core/src/ask.rs`, `core/src/facts.rs`, `core/src/visible.rs`, `AskChat.tsx` | Faridul, on Farhan's Ask | a0cc50f, 8bdf894, bcde0aa, 683b926, 47f08d3, 49c5487, 04f7787, 00161da, bff44c3 |
| Ask about a conversation from its AI menu: everything with its people (their address, or their company's domain), that conversation first | `core/src/ask.rs` (`around`), `AskChat.tsx`, `Reader.tsx` | Faridul | fe02949 |
| Didn't find it? Ask AI beside the search results: a question of its own, in the inbox searched or another from its dropdown, never told the search words | `AskChat.tsx`, `App.tsx` | Faridul | 3bb82bb, fe14776, c505031 |
| Ask AI never says how it or the search works, or what it was told: such questions get one fixed line, with no AI | `core/src/question.rs` (`about_itself`), `core/src/assist.rs`, `AskChat.tsx` | Faridul | c505031 |
| Ask AI reads every question with the AI, in any wording or language: a search plan of names, topics with their other words, real dates and the kind of answer, checked by code; no word lists; with no AI, searched as typed. The AI names the time ("last month") and code works out the days; "how many" is counted by the app and the AI only says it | `core/src/plan.rs`, `core/src/ask.rs`, `core/src/assist.rs` | Faridul | 2574421, 527a8d5 |
| Ask AI's header picks Apple Intelligence or the AI from Settings › AI, for Ask AI alone (replies and the rest keep that AI), and opens Settings › AI; Apple Intelligence reads up to 6 emails one by one and answers in about half the time; when Apple's safety check stops it, it says so, and "Settings › AI" in any problem opens that page | `AskChat.tsx`, `ui.tsx` (`Problem`), `core/src/settings.rs` (`ask_ai_apple`), `core/src/ai.rs` (`ask_ai_credential`), `swift/AppleAI.swift` | Faridul | 2f3b13a |
| Ask AI remembers the open chat: a follow-up starts from the last question's plan and changes only what it says ("and in September?" after "how many asked for a refund last month?" keeps the count and the topic) | `core/src/plan.rs` (`carried`), `core/src/assist.rs`, `AskChat.tsx` | Faridul | 551861e, 9278e7f |
| Read and unread hold against a refresh that read the server before it heard, so read all never flickers back; read all changes this Mac first and every window shows it at once, every round of it before any server hears | `core/src/store.rs` (`set_seen_held`), `core/src/actions.rs` (`bulk_seen_step`), `UndoMove.tsx` (`markAllInRounds`), `MenuBarPanel.tsx`, `App.tsx` | Faridul | 3b106c0, a0c48f9 |
| New mail shows in the menu bar and the main window as its banner does, banner or not | `core/src/engine.rs` (`with_new_mail_listener`), `app/src-tauri/src/lib.rs`, `MenuBarPanel.tsx`, `App.tsx` | Faridul | 983dea6 |
| Mailzo stays quiet on a big mailbox: each email carries a "body saved" mark the database keeps itself, and what the refresh, sidebar, health note, spam check, learning, bulk mail and rules look for comes from small indexes of the few still to do; a refresh that changed nothing leaves the windows alone (about 1.6% CPU between refreshes, like Spark) | `core/src/store.rs` (migrations, `BODIES_WANTED`, `mail_to_check_sql`, `learn_sql`), `core/src/rules.rs`, `App.tsx` | Faridul | |
| "Waiting on my reply", "haven't replied", "unanswered", and only the ones about the topic asked ("refund requests waiting on me"), listed with no AI | `core/src/question.rs` (`WAITING`), `core/src/ask.rs` (`waiting_on_me`) | Faridul | 20029a4, d36fb74 |
| Better Apple Intelligence answers: it reads each email on its own and answers from its notes; who, which and how many show the emails themselves | `core/src/assist.rs` (`notes_on_device`, `lists_better`) | Faridul | 9caf8ab |
| Ask AI measured on a copy of real mail: a test that times each step, with tokens and cost logged privately to a file the app never sets (`INBOX_AI_LOG`) | `core/tests/ask_live.rs`, `core/src/plans.rs` | Faridul | 0110b8b, 15b7f0a |
| Copy an OTP in one click from the list, the menu bar and the open email, found by rules alone ("8188 is your One Time Password" and "Your OTP … is 4821" too, when "is" joins them in one sentence); the email then goes to the bin, or as chosen in Settings › Reading | `core/src/codes.rs`, `ThreadList.tsx` (`CodeChip`), `app/src-tauri/src/pasteboard.rs` | Faridul | 0f6f541, 61b0792 |
| Follow-ups that send until someone replies, set up from the conversation (1–10, days or dates, AI writes) | `core/src/sequences.rs`, `FollowUps.tsx` | Farhan | 0108835, 383d643 |

## Sorting and cleaning up

| Feature | Where | Who | Key commits |
|---|---|---|---|
| Tabs (Needs you, Good to know, The rest), the brief; cold sales emails (pitch words, follow-up chains, tool-made HTML) and bank transfer alerts stay out of Needs you; sign-in codes and links, mailings from people written to and SEO pitches too; "may need action" notices leave after 3 days; Done button (D key, several at once); "Doesn't need me" / "Needs me" on a row's menu and an open email's ⋯ menu; "Doesn't need me" covers only the emails there were, so a newer reply that needs you brings it back | `core/src/priority.rs`, `Priority.tsx` | Farhan | d9b88c6 |
| Purchases in the sidebar (receipts, orders, bills, payments, bank alerts; also stay in the inbox) | `core/src/purchases.rs`, `App.tsx` | Farhan | this branch |
| Spam and Not spam at once | `core/src/spam.rs` | Farhan | a7691e0, d3325ba |
| "A link shows X, but really goes to Y" leaves out a confirmed sender's social links sent through its mail service (Customer.io, Mailchimp, SendGrid and the like), and warnings saved before are taken back once; login-style brands still warn | `core/src/spam.rs` (`MAIL_SERVICE_LINKS`, `recheck_link_warnings`) | Faridul | |
| Newsletters: unsubscribe one or many, archive; only senders with an unsubscribe link (the List-Unsubscribe header, or for a checked sender an "Unsubscribe" link in the email to an https page on its own site, read as mail arrives and newest first for mail already saved); unfinished ones stay flagged, new mail from left senders goes to Archive, let back in (Farhan, 2026-11-06); the Unsubscribe beside a sender in an email follows the same rule (`can_unsubscribe!`) | `Newsletters.tsx`, `core/src/newsletters.rs`, `core/src/spam.rs` (`unsubscribe_in_body`), `core/src/store.rs` (`note_way_out`, `fill_some_ways_out`) | Faridul (from Farhan's clean-up dialog) | 99d6f0f, b30f702, 607a0f7, cf2e026 |
| Always put a sender on a tab | `ThreadMenu.tsx`, `core/src/spam.rs` | Farhan, tidied by Faridul | 5833464 |
| Filters | `settings/RulesPage.tsx`, `core/src/spam.rs` | (before this list) | |
| Filter by AI: a filter written in plain words; AI reads each new email against every AI filter in one quick question (several batches side by side) before it shows, and mail shows anyway if AI can't answer within 20 seconds; Try it on the latest 30 emails | `settings/RulesPage.tsx`, `core/src/rules.rs` (`ai_filter_hits`, `run_rules`, `rule_try_ai`) | Farhan | |

## People

| Feature | Where | Who | Key commits |
|---|---|---|---|
| Contacts: saved contacts, vCard import and export, how many emails with each | `ContactsView.tsx`, `core/src/people.rs` | Farhan, Emails column Faridul | b0c4e53, f98eecf |
| Emails with someone, the same in Contacts and Newsletters | `core/src/newsletters.rs` (`mail_with`) | Faridul | d25d18f |
| Teams and shared inboxes | `core/src/team.rs`, `Team.tsx` | Farhan | ba80126 |

## The app on the Mac

| Feature | Where | Who | Key commits |
|---|---|---|---|
| Menu bar icon, panel and open at login; Mark all as read asks first when there's more than one, tabs switch at once | `app/src-tauri/src/panel.rs`, `MenuBarPanel.tsx` | Faridul | ead5714, 4def18d, 6eebc9b, 65e5fce, bfb7ff2, 1a1a594 |
| Download progress: in the middle while a new inbox's list is blank, in the sidebar after, one count and one bar | `App.tsx`, `ThreadList.tsx` (`FirstLoad`), `Sidebar.tsx`, `ui.tsx` (`DownloadBar`) | Farhan, merged by Faridul | 38d47ad, e551f02 |
| Full-width (Gmail-style) layout | `styles.css`, `ThreadList.tsx`, `BulkPanel.tsx` | Farhan, then Faridul | 48db12e, 3e86ccc, 4b7fd15, d4fe4a7 |
| Sidebar rail, reordering and hiding | `Sidebar.tsx`, `app/ui/dragOrder.ts` | Faridul | 72a4c93, cf4095d, 920dadf |
| Back and forward, also with a mouse's side buttons (Logi Options+ too) and ⌘← ⌘→ | `App.tsx`, `Sidebar.tsx`, `app/src-tauri/src/mouse_buttons.rs` | Faridul | 736bdd0, d8d013c, 311132a, a7aaf4b |
| Settings search, inbox order, Settings in the app menu | `settings/SettingsWindow.tsx` | Faridul | 6024c5b, 9d0d86c, 57cc10f |
| Calendar | `CalendarView.tsx`, `core/src/calendar.rs` | (before this list) | |

## AI

| Feature | Where | Who | Key commits |
|---|---|---|---|
| Apple Intelligence: the Mac's own AI by default when no plan or key is connected (free, on-device; smaller Ask AI bundle, answers held to the JSON shape) | `core/src/on_device.rs`, `app/src-tauri/swift/AppleAI.swift`, `app/src-tauri/src/apple_ai.rs`, `app/src-tauri/build.rs`, Settings › AI | Farhan | |
| How you write, AI replies, the AI allowance | `core/src/assist.rs` | Farhan | 0f3571c, a0341e5, 5f5e4a2 |
| One-tap short replies: three written for the email, a tap puts one in the reply; written once when Needs-you mail arrives (3 per refresh, last 3 days) and kept in `quick_replies`, so opening it never asks the AI again | `Priority.tsx` (`ReplyHelp`), `core/src/assist.rs` (`ai_short_replies`, `write_quick_replies_ahead`), `engine.rs` (`act_on_new_mail`) | Faridul, on Farhan's quick replies; kept + ahead by Farhan | e5f998c |
| Connected AI apps (MCP) and what they did | `core/src/mcp.rs`, `core/src/connections.rs`, `settings/ActivityPage.tsx` | Faridul | 504dfba, 8072b6d |
| Each person allows AI on shared inboxes | `core/src/assist.rs` | Farhan | 780e229 |

## Security and accounts

| Feature | Where | Who | Key commits |
|---|---|---|---|
| Encrypted mail on the Mac (SQLCipher) | `core/src/store.rs` (`open_protected`) | Farhan | 60a3d83 |
| Encrypted email (OpenPGP) | `core/src/pgp.rs` | Farhan | 17db524, 05b6f2d, b9955fa |
| App lock | `AppLock.tsx`, `app/src-tauri/src/lock.rs` | Farhan | 847b47c |
| Email pictures and attachments kept safe | `core/src/images.rs`, `core/src/net.rs`, `core/src/actions.rs` | Farhan | e47784f, bfa2946, 44d42cd |
| Sign-in, devices and deleting the account | `core/src/identity.rs` | Farhan | 5094d56, 7975d6b |
| Pre-release security fixes (AI-app replies, team inboxes, invites, PGP size, file names) | `core/src/mcp.rs`, `core/src/team.rs`, `core/src/calendar.rs`, `core/src/pgp.rs` | Farhan | see git log |
| Mailzo Cloud (server code in the private `grayic/dakly-cloud`) | `core/src/paid.rs` | Farhan | ac3cccd, afb06a4 |

## Checks

| Feature | Where | Who | Key commits |
|---|---|---|---|
| Two-device self-test on the Mac | `scripts/mac-selftest.sh`, `scripts/selftest/` | Faridul | c20d250, 900e90a, bfdc3bc |
