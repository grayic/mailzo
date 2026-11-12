# Ask AI, rebuilt: code finds, AI answers

Faridul, 2026-11-06. Replaces what happens behind Ask AI (Farhan's `AskChat.tsx`, `ai_ask_sources`, `ai_chat` in `core/src/assist.rs`). The chat screen stays.

## Why

Ask AI answers questions about your mail. Today every question goes the same way: a search by its words and meaning, then the closest few emails go to the AI, which answers from them. That is slow, uses a lot of AI, and goes wrong in ways ordinary code would not. On 2026-11-06 the question "What did the Wyoming Registered Agent payment cover?" was answered with a LumozAI invoice, on Apple Intelligence.

Research (two passes, 2026-11-06) says:

- Most questions people put to their mail are lookups of one email they already know something about: who sent it, roughly when (Microsoft's email search logs, Ai et al. 2017). Code finds those better than AI.
- Small AI models get relative dates ("last week", "my next call") almost always wrong (arXiv 2507.06450). Code should work out dates.
- The failures users report most: wrong facts in answers, the wrong person, past mixed up with upcoming, "latest" not being the latest.
- Hidden text in an email can take over the AI and leak other mail (shown on Gemini and Superhuman in 2025).
- The best-documented product (Shortwave) does exactly this split: understand the question, search with code, one AI call to answer, then check.

## Goals

1. Fast: list questions show results with no AI wait. For other questions the emails found show at once, and the checked answer follows in about 3–5 seconds on Apple Intelligence. Answers don't stream in this version (Faridul's call), so nothing unchecked is ever on screen.
2. Accurate: every fact in an answer is in an email the answer cites. When it can't find the answer, it says so.
3. Cheap: the AI reads short passages, never whole mailboxes. List questions use no AI at all.
4. Safe: nothing an email says can make the AI do something, and nothing leaves the Mac except to the AI the person chose.
5. Precise: a result matches what was asked, or it isn't a result. Gmail's search fills its results with emails nowhere near the words typed (Faridul's finding); Mailzo never pads.
6. One place to ask: Ask AI in the sidebar. The search bar only searches, with no AI at all (Faridul's decision).

## Not in this version

- Doing things: drafting, forwarding, archiving, offering calendar slots. That comes later, behind a preview the person confirms (decided by Faridul).
- A general chatbot. Questions are about the person's mail, calendar and contacts.

## Must never happen (anti-goals)

- An answer that states a date, amount, name or code that isn't in the emails it cites.
- An email's content changing what the AI does, or where anything is sent.
- Waiting on AI for a question that is really a search ("Priya's emails last week").
- Making the person choose a mode. They type a question; the app works out what kind it is.

## The four kinds of question

| Kind | Examples | Who does the work | AI used |
|---|---|---|---|
| Find a list | "Find emails from Priya sent last week", "Who's waiting on a reply from me?" | Code: filters, then the list | None |
| Look up a fact | "When is my flight to Rome?" | Code: facts saved as mail arrives; AI only if nothing is saved | None, or small |
| Catch up / summarize | "Catch me up on the launch emails", "What's the latest on the contract thread?", "What's blocking the Smith contract?", "Action items from our monthly all-hands", "Summarize this thread", "Brief me before my next call" | Code picks the conversations; AI reads saved summaries plus the newest messages | Small for one conversation, more for many |
| Make a plan | "Show my priorities for today", "What do I need to prepare for my trip?" | Code gathers mail waiting on you, things due, calendar, trip emails in the trip dates; AI writes a short list | Small |

## How it works

### 0. As mail arrives (code)

- **Clean text.** Hidden text is dropped before anything else: `display:none`, `visibility:hidden`, zero font size, zero opacity, text the same colour as its background. Quoted replies and signatures are cut (`priority::without_quotes` already does the quotes). Only the new text is indexed for meaning and shown to AI.
- **Facts table.** A new table of facts read from the email itself, not guessed:
  - schema.org markup that airlines, hotels and shops put in their emails (`FlightReservation`, `LodgingReservation`, `ParcelDelivery`, `Order`, `Invoice`), from JSON-LD and microdata;
  - `.ics` attachments (events);
  - with plain patterns: amounts with a currency, flight numbers, booking codes, due dates (`upcoming_due_messages` already finds due dates).
  Each fact keeps the message id it came from. The migration goes at the end of the list, after pulling `main` (see AGENTS.md).
- **Waiting on me.** Already worked out: `NEEDS_YOU_SQL` and the "Waiting N days" / "No reply yet" marks. Reused, not rebuilt.
- **Summaries.** `ai_summaries` already keeps a summary per conversation, keyed by the last message. Catch-up questions use these, made on demand, never for every email.

### 1. Understanding the question (code first)

> **Changed on 2026-11-07 (Faridul's call).** Ask AI no longer reads questions with word lists: they broke on every new wording and only knew English. The AI now reads every Ask question, with the questions before it in the chat, into a search plan: names, topics with the other words for each, real dates, whether it wants a list, an answer, a summary or a plan, and whether it's waiting on a reply (`core/src/plan.rs`). Code checks the plan (dates, names against real people and domains) and searches it, ranking emails by how many of the topics they have and by meaning. With no AI, the question is searched as typed. A question asked beside search results uses the search, with its own words only ranking. The search box below still reads with the code parser and no AI.

One parser, shared by the search box and Ask AI, so the two never disagree (Faridul's call). It turns the question into:

- **Kind**: one of the four above, from the wording ("find", "show", "from", "who's waiting" → list; "when", "how much", "what time" → fact; "catch me up", "latest", "summarize", "what's blocking", "action items" → catch-up; "priorities", "prepare", "plan" → plan).
- **People**: names matched against contacts and people emailed, ranked by how often and how recently. Two close matches (two Priyas) → the one emailed more is used, and the answer says which.
- **Companies**: "Amazon" → sender domains.
- **Dates**: "last week", "since Monday", "before my next call", "upcoming" → real dates, in the person's time zone and week start. Code always does the arithmetic.
- **Words**: what's left, for the text search.
- **Scope**: this conversation (when asked from inside one), one inbox, or all.

When the rules aren't sure, a small AI call fills the same fields (about 300–500 tokens, Apple Intelligence is fine). The AI only names the date phrase; code turns it into dates. `parse_plain` and `SearchFilters` in `core/src/search.rs` are the starting point.

Follow-up questions ("and the one before?") keep the previous turn's filters and change only what the new question says.

### 2. Finding (code, precise)

**Exact matches are the results.** An email is a result only if it matches everything that was asked:
- the person (any of their addresses), the dates, the folder and the scope;
- every word, in any of its forms ("invoice" finds "invoices", accents don't matter), in the subject, the people or the new text of the email (not the quoted old replies).

This is plain SQLite with `search_index` (FTS5), no AI. Suspicious mail and folders AI may not read are left out of what AI sees, as now.

**Similar-meaning emails never mix in.** Today the search box adds up to 60 "closest meaning" emails to every phrase, with no cutoff, and AI widens the words with synonyms; Ask AI fills in with similar and newest mail. All of that goes:
- The search box never uses meaning search or AI.
- Ask AI uses meaning search only when exact matches find fewer than 3, and then only emails above a closeness cutoff, at most 5, kept apart as "Maybe related" and never among the exact ones.
- AI-suggested synonyms aren't used to widen anything. A synonym the person typed with "or" is theirs and counts.

**Each result says why it's there.** The matching words are highlighted in the subject and preview, and a person match shows on the sender.

**Order.** For a list, newest first, as people expect from mail search. For Ask AI picking passages, best match first: subject over body, the named person over others, newer mail when "latest" is asked, future dates when "upcoming" or "next" is asked, newsletters and promotions last.

**Nothing found is an answer.** "No emails match 'Wyoming payment'", with what was looked for (person, dates, words) so the person can drop one. In Ask AI, the "Maybe related" emails show under it if there are any.

For "Find a list" the app stops here and shows the list. For the other kinds, each conversation found gives a short passage around the matches (about 150–300 tokens), not the whole email, to the AI. If the passages come only from "Maybe related", the answer says so.

### 3. Answering (AI)

- **Given**: today's date, the person's addresses, the question, and numbered passages, each with sender, date and subject. The strongest passages go first and last, as models miss what's in the middle.
- **Asked for**: a short answer, the numbers of the emails it used, and a "not found" flag. No free text outside that shape.
- **Apple Intelligence** (about 4,000 tokens in total): up to about 8–10 passages. A question needing more (a catch-up over many conversations) is answered from the top few, says it's partial, and suggests connecting Claude or ChatGPT (Faridul's call).
- **Claude or ChatGPT**: up to about 15–20 passages, plus saved summaries for catch-ups.
- Email text is marked as material to read, never instructions (as `NOT_INSTRUCTIONS` and `neutral` already do).

### 4. Checking before showing (code)

- Every cited number must be one of the passages sent.
- Every date, time, amount, booking code, email address and phone number in the answer must appear in a cited passage (after normalising: "Nov 3" = "3 November", "$1,800" = "1800 USD"). Any that doesn't is removed, and the answer is marked as needing a check.
- If nothing survives, "I couldn't find that in your mail" with the closest emails listed.
- No link or image the AI wrote is ever loaded. Only the cited emails are linked.

## Decisions made while writing this (Faridul's calls, open to change)

- **No streaming.** The answer appears once it has passed the check. The emails found show straight away, as they do now.
- **Filters versus preferences.**
  - For a list question, people and dates the person named are hard filters: "Priya, last week" shows only that.
  - For other questions, a person is a hard filter only when the name matched one contact exactly. Dates are a hard filter only when written as real dates ("3 November", "in October"); "recently" or "last week" only rank.
  - A company or organisation name ("Wyoming Registered Agent") never filters by sender, only ranks, because receipts often come from a payment company.
- **Calendar events can be sources.** They get their own source type, open from the chat like an email does, and their times count in the check. Plan and briefing questions need this.
- **Saved summaries are never evidence.** They help choose which conversations matter and give the overview, but every fact in an answer must come from a passage of an email or an event. Summaries are made during a question for one conversation only; a catch-up over many conversations uses the newest passages instead, so it doesn't wait on several AI calls.
- **List questions need no AI.** They work with no AI connected, and show as rows in the chat, like the emails found do now. Fact questions answered from the facts table also need no AI. Ask AI turned off in Settings hides the whole screen as before.
- **No new "ask about this conversation" button.** "Summarize this thread" is handled by finding the conversation from the question; the Reader's Summarize stays as it is. A button in the Reader can come later.
- **The fact check works in any language.** The AI returns each date, amount, code and name it relies on as an exact copy from the source. The check looks for that exact text in the cited passage. The answer itself is written in the app's language around those copies, so no translation of dates is needed.
- **Search box: no AI at all** (Faridul's decision). It reads what's typed with the same code parser (people, dates, words) and shows exact matches only. No AI reading of the text, no AI synonyms, and no "Maybe related" group; that group exists only in Ask AI. When nothing matches, it says what it looked for ("From Priya · last week · 'budget'") so a part can be dropped. The "Search in your own words" switch in Settings › AI goes, and the placeholder is always "Search all email".
- **One Ask, in the sidebar** (Faridul's decision). The search bar's Ask button and ⌘K's "Ask AI: …" are removed, and nothing in search opens the chat. The separate answer box that only Mailzo Cloud answered (`AskAnswer` in `Later.tsx`, `api.aiAsk`, the engine's `ai_ask` command) is removed from the app. Farhan's server side in `dakly-cloud` is untouched. ⌘K's "Ask your email", which just goes to the Ask AI screen, stays.

## Measuring it

A test set before any tuning: 50–100 real questions with the email ids that answer them.

- **Finding**: questions on the demo mailbox (`dev/fakeimap`), each with the ids that answer it, run in `cargo test` and gate `scripts/check.sh`. They check the right email is in the top 5 and that list questions make no AI call.
- **Answering**: about 20 questions from Faridul's own mailbox, each with the expected answer written by hand. A script run by hand on his Mac (like the ignored real-mail priority test) asks each one on Apple Intelligence and on a Claude plan, and reports: right email found, answer matches, any fact the check removed, time and tokens. These stay on his Mac and are never committed. They don't gate `check.sh`, as they need real AI.

## Done when

- The four kinds work on every example above, on Apple Intelligence and on a Claude plan ("Summarize this thread" by finding the conversation from the question).
- "Find a list" questions show results with no AI call.
- No answer in the test set contains an unsupported fact.
- The Wyoming question finds the Wyoming email, or says it can't.
- Every result in the search box and every list answer contains all the words, the person and the dates asked for; "Maybe related" appears only in Ask AI, only when fewer than 3 do, in its own group (tested in `cargo test`).
- The search box makes no AI call, and nothing in the app offers to ask from the search bar or ⌘K.
- `scripts/check.sh` and `scripts/mac-selftest.sh` pass.

## Left to whoever builds it

- How the facts table is laid out, and which schema.org types come first (flights and invoices are the examples that matter most).
- The exact wording rules for the four kinds.
- How hidden text is detected inside the HTML-to-text step.
- Whether a reranker model is ever needed. Only if the test set shows ranking misses.
