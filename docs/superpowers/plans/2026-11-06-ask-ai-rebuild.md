# Ask AI Rebuild Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Search and Ask AI return only emails that truly match, code does the finding, and AI only writes answers that a code check has verified.

**Architecture:** A pure question reader (`core/src/question.rs`) turns text into kind, people, dates and words. Precise finding in the store returns exact matches only, with "Maybe related" kept apart. Ask AI then answers list and fact questions without AI, and other questions with AI over short passages. A checker removes any fact not found in the cited sources.

**Tech Stack:** Rust (`mailcore`, rusqlite with FTS5, chrono + chrono-tz, existing `fakeai`/`fakeimap` test servers), React/TypeScript (`app/ui`), Tauri self-test JS.

**Spec:** `docs/superpowers/specs/2026-11-06-ask-ai-design.md`. Read it first; this plan argues from it.

## Global Constraints

- Work on `faridul/dev`. `git fetch --all` before starting each task, and pull `main` before touching `core/src/store.rs` migrations, `core/src/engine.rs`'s command list, `app/ui/App.tsx` or `app/ui/api.ts` (AGENTS.md hot spots).
- New migrations are only ever appended at the end of `MIGRATIONS` in `core/src/store.rs`.
- Every new on-screen string goes through `tx()` and gets an entry in all 7 files in `app/ui/locales/` (keys sorted, `JSON.stringify(obj, null, 1)` format).
- No new crates unless a task says so; `chrono`, `chrono-tz`, `ammonia` and `html5ever` (via ammonia) are already there.
- Email text given to any AI is wrapped by `as_material`/`neutral`-style escaping, sender names with `"` replaced, and preceded by `NOT_INSTRUCTIONS`.
- List questions never call AI. Fact questions answered from the facts table never call AI.
- No answer shown on screen contains a date, time, amount, booking code, email address or phone number that isn't in a cited source.
- Apple Intelligence budget: at most 10 passages; Claude/ChatGPT: at most 20. A passage is at most 1,200 characters (about 300 tokens).
- "Maybe related": only in Ask AI, only when exact matches are fewer than 3, only above the closeness cutoff, at most 5, never mixed into results.
- Out of scope: drafting, forwarding, archiving, calendar slots, streaming, and a Reader "ask about this" button.
- The search box never calls AI or meaning search. Ask AI in the sidebar is the only place to ask (Faridul's decisions).
- Done for each task: `scripts/check.sh` passes. Done for on-screen tasks: `DAKLY_CLOUD=... scripts/mac-selftest.sh` passes too.
- Commit messages in plain words, no AI attribution (the user's standing rule).
- Farhan owns `AskChat.tsx`, `ai_ask_sources`, `ai_chat`; the claim is in `docs/FEATURES.md` "In progress". Don't start Task 9 until Faridul confirms Farhan knows.

## Review Focus

1. **Languages without spaces** (Japanese, Chinese): a search for 請求書 must still find the email containing it; the exact-match check must not drop it. Test in Task 1.
2. **Emails whose body isn't downloaded yet** (only a snippet), and server-found results (`extra_ids` from `search_servers`): they must stay results when the snippet or the server says they match. Test in Task 1.
3. **"me", unknown names and two people with one name**: "emails from me" means the person's own addresses; a name nobody matches stays a word, not an empty result; two Priyas uses the one emailed more and says so. Tests in Task 7.
4. **Local time and week start**: "last week" asked at 01:00 on a Monday in Dhaka (UTC+6) is the previous Monday–Sunday in Dhaka, not in UTC. Test in Task 6.
5. **Empty, punctuation-only or very long questions, and very long emails**: no crash, no AI call for an empty question, passages stay within 1,200 characters. Tests in Tasks 6 and 9.

---

## Phase A: precise search (ships on its own, fixes the search box)

### Task 1: Exact matches only, in saved-mail search

**Files:**
- Modify: `core/src/search.rs` (new `matches_exactly`), `core/src/store.rs` (`search_threads_where`, around line 2653)
- Test: `core/tests/search.rs`

**Interfaces:**
- Produces: `pub(crate) fn matches_exactly(words: &[String], subject: &str, people: &str, new_text: &str) -> bool` in `core/src/search.rs`. True when every word appears at the start of a word in subject, people or new text, ignoring case and accents (é = e). Phrases in quotes must appear as a phrase.
- Produces: `search_threads_where` drops a word match whose words appear only in quoted old replies. `new_text` is `crate::priority::without_quotes(body)`, falling back to `messages.snippet` when there's no body. Ids passed in `extra` (from mail servers) skip the check.

- [ ] **Step 1: Write the failing tests** in `core/tests/search.rs`:
  - `words_only_in_a_quoted_reply_dont_count`: email A's body is "Thanks!\n\nOn Mon, Sam wrote:\n> the Wyoming invoice"; email B's body has "Wyoming invoice" in new text. Search `{"words":["wyoming","invoice"]}` → `subjects(...) == [B]`.
  - `word_forms_and_accents_still_match`: "Invoices for October" is found by `invoice`; "Café receipt" is found by `cafe`.
  - `languages_without_spaces_are_found`: body "請求書を送ります" is found by `請求書` (Review Focus 1).
  - `mail_without_a_body_yet_matches_on_its_preview`: a message whose body was never downloaded is found by a word in its snippet (Review Focus 2).
- [ ] **Step 2:** `cargo test -p mailcore --test search -- quoted word_forms without_spaces without_a_body`. Expect the quoted-reply test to FAIL; record whether the others already pass.
- [ ] **Step 3:** Implement `matches_exactly` and apply it in the `search_threads_where` loop, before `keep`. Load subject, people and new text only for candidates that reach the loop, and stop at `limit`. Fold text for comparison in one helper in `search.rs`: lowercase, then map Latin letters with accents (À–ÿ and Latin Extended-A) to their base letter, the same set FTS5's `remove_diacritics 2` folds; leave other scripts unchanged. A word with no spaces in a language without spaces (CJK) matches as a substring.
- [ ] **Step 4:** Run the tests again. All PASS, plus the whole `search` test file.
- [ ] **Step 5:** Commit: "Search: a result has every word in its own text, not only in a quoted old reply".

### Task 2: Meaning search only returns close matches

**Files:**
- Modify: `core/src/meaning.rs` (`search_meaning`, `MOST_FOUND`)
- Test: `core/tests/assistants.rs` (it has `fake_embedder`)

**Interfaces:**
- Produces: `search_meaning(text, readable) -> {"available": bool, "ids": [i64], "scores": [f32]}`. Ids are only those with a score ≥ `MEANING_CLOSE_ENOUGH` (new `pub(crate) const`, start at `0.6`, to be tuned in Task 15), at most `MEANING_MOST_RELATED = 5`, best first.
- `ai_ask_sources`'s current use of `search_meaning` still compiles. Task 9 replaces it.

- [ ] **Step 1: Write the failing test** `meaning_search_leaves_out_what_isnt_close`. With `fake_embedder`, "flight to Rome" finds the flight email, and "garden party" (no shared meaning) returns `ids == []`.
- [ ] **Step 2:** Run it; expect FAIL (today it returns up to 60 ids for anything).
- [ ] **Step 3:** Filter `closest_meanings` results by the cutoff, cap at 5, and return the scores.
- [ ] **Step 4:** Run the test and the `assistants` file. PASS.
- [ ] **Step 5:** Commit: "Meaning search returns only emails that are really close, at most 5".

### Task 3: The search box: exact matches, no AI

**Files:**
- Modify:
  - `app/ui/App.tsx`: `searchDeeper` (about lines 1129–1185) and the `SearchBox` `aiOn` prop (about line 2657)
  - `app/ui/components/SearchBox.tsx`: `SearchFoot` loses `byMeaning`; the placeholder is always `tx("Search all email")`
  - `app/ui/components/settings/AiPage.tsx`: remove the `ai_search` toggle
  - `core/src/settings.rs`: remove `ai_search`
  - `core/src/ai.rs`: `search_understand` no longer asks AI; Task 8 replaces its reading
  - `app/ui/locales/*.json`: remove the strings that are no longer used
- Test: `scripts/selftest/mail-flow.js` (the search part of `mailFlow`), `core/tests/search.rs`

**Interfaces:**
- Produces: the search box makes no AI call and no meaning search. `search_understand` returns `used_ai: false` with `parse_plain` filters until Task 8. When nothing matches, `SearchNote` shows `tx("No emails match")` and its summary of what was looked for.

- [ ] **Step 1: Write the failing tests:**
  - `core/tests/search.rs` `the_search_box_never_asks_ai`: a key is set and `FakeAi` answers anything; `search_understand` for "emails from Ana about the launch" leaves `ai.requests()` empty.
  - `mail-flow.js`: `check("a search shows only emails with every word", rows().every((r) => r.innerText.toLowerCase().includes("<word>")))`, using a two-word phrase whose words are both in exactly one demo email.
  - `mail-flow.js`: `check("nothing similar is added to a search", !pageHas("about the same thing"))`.
- [ ] **Step 2:** Run both; expect the Rust test and the first self-test check to FAIL.
- [ ] **Step 3:** In `searchDeeper`, remove the AI understanding step and the meaning-search merge (`searchExtra` keeps only the server's `extra_ids`). Then remove `ai_search` and its toggle, and remove `byMeaning`.
- [ ] **Step 4:** `scripts/check.sh`, then the self-test. PASS.
- [ ] **Step 5:** Commit: "Search box: exact matches only, with no AI and nothing similar added".

### Task 4: One Ask: the search bar only searches

**Files:**
- Modify:
  - `app/ui/App.tsx`: remove the `asking` state, the `<AskAnswer>` render, and the `onAsk`/`askLocked` props on `SearchNote` and `CommandBar`; `looksLikeASentence` too if it's no longer used
  - `app/ui/components/SearchBox.tsx` (`SearchNote`) and `app/ui/components/CommandBar.tsx`: remove `onAsk`/`askLocked` and the "Ask AI: …" item
  - `app/ui/components/Later.tsx`: delete `AskAnswer`
  - `app/ui/api.ts`: remove `aiAsk`
  - `core/src/engine.rs` and `core/src/assist.rs`: remove the `ai_ask` command and `Engine::ai_ask`
  - `core/src/mcp.rs`: drop `"ai_ask"` from the dev script's command list
  - `app/ui/locales/*.json`: remove strings no longer used
- Test: `scripts/selftest/mail-flow.js`

**Interfaces:**
- Produces: nothing in search offers to ask. ⌘K's "Ask your email" command (it goes to the Ask AI screen) stays. `ai_ask_sources` and `ai_chat` stay for the sidebar chat.

- [ ] **Step 1: Failing self-test check:** after typing "when is the launch" in the search box, `check("search never offers to ask", !buttonWith("Ask") && !pageHas("Ask AI:"))`.
- [ ] **Step 2:** Run; FAIL where a paid account or ⌘K shows Ask.
- [ ] **Step 3:** Remove the pieces listed above. `cargo build` and `tsc` show nothing else used them.
- [ ] **Step 4:** `scripts/check.sh`, then the self-test. PASS.
- [ ] **Step 5:** Commit: "One place to ask: Ask AI in the sidebar; the search bar only searches". Tell Farhan that the client no longer calls `/paid/ask`.

### Task 5: Results show why they matched

**Files:**
- Modify: `app/ui/components/ThreadList.tsx` (`ThreadRowBody` subject and snippet; new `highlight?: string[]` prop passed down from `ThreadList`), `app/ui/App.tsx` (pass the search words when `view.kind === "search"`), `app/ui/styles.css` (`.thread mark` using existing tokens)
- Test: `scripts/selftest/mail-flow.js`

**Interfaces:**
- Produces: `function marked(text: string, words: string[]): ReactNode` in `ThreadList.tsx`. It wraps each word start matching a search word, ignoring case and accents, in `<mark>`.

- [ ] **Step 1: Failing self-test check:** after a search, `check("the matching words are marked", !!document.querySelector(".thread mark"))`.
- [ ] **Step 2:** Run; FAIL.
- [ ] **Step 3:** Implement `marked` and the prop. Rows with `related: true` get no marks.
- [ ] **Step 4:** `scripts/check.sh` and the self-test. PASS.
- [ ] **Step 5:** Commit: "Search results mark the words that matched".

## Phase B: reading the question

### Task 6: The question reader

**Files:**
- Create: `core/src/question.rs` (add `pub mod question;` in `core/src/lib.rs`)
- Test: unit tests at the bottom of `core/src/question.rs`

**Interfaces:**
- Produces:
```rust
pub enum Kind { List, Fact, CatchUp, Plan }
pub struct Question {
    pub kind: Kind,
    pub names: Vec<String>,          // people or companies as typed ("Priya", "Amazon", "me")
    pub after: Option<i64>,          // unix seconds, inclusive
    pub before: Option<i64>,         // exclusive
    pub exact_dates: bool,           // "3 November", "in October" (true) vs "last week" (false)
    pub upcoming: bool,              // "next", "upcoming", "when is my"
    pub latest: bool,                // "latest", "last", "recent"
    pub words: Vec<String>,          // what's left, stop words removed
    pub sure: bool,                  // rules recognised the kind and every date phrase
}
pub fn read_question(text: &str, now: chrono::DateTime<chrono_tz::Tz>, week_starts_monday: bool) -> Question
```
- Kind rules (from the spec): "find/show/list/emails from/who's waiting" → List; "when/what time/how much/which/where" with a booking or bill word → Fact; "catch me up/latest on/summarize/what's blocking/action items/brief me" → CatchUp; "priorities/prepare/plan/today's" → Plan; otherwise Fact.
- Date phrases at minimum: today, yesterday, this/last/next week, this/last month, since <weekday>, in <month> [year], on <day> <month>, past N days.

- [ ] **Step 1: Write failing table tests** for the spec's example questions. Each asserts kind, names, dates and words, for example:
  - "Find emails from Priya sent last week" → List, names ["Priya"], after = previous Monday 00:00 local, before = this Monday 00:00 local, `exact_dates == false`, words [].
  - "When is my flight to Rome?" → Fact, upcoming, words ["flight", "rome"].
  - "What are the action items from our monthly all-hands?" → CatchUp, words ["monthly", "all-hands"].
  - "Show my priorities for today" → Plan, after/before = today local.
  - Review Focus 4: "last week" at Monday 01:00 Asia/Dhaka → the Dhaka Monday–Sunday before.
  - Review Focus 5: "", "???", and a 2,000-character question → no panic; empty gives List with no names and no words, and `sure == false`.
- [ ] **Step 2:** `cargo test -p mailcore question::` → FAIL (module missing).
- [ ] **Step 3:** Implement `read_question` with plain rules and chrono; no AI, no store.
- [ ] **Step 4:** Tests PASS.
- [ ] **Step 5:** Commit: "A question reader: kind, people, dates and words, worked out by code".

### Task 7: Names become addresses

**Files:**
- Modify: `core/src/people.rs` (new store query plus engine method)
- Test: `core/tests/people.rs`

**Interfaces:**
- Produces:
```rust
pub struct Resolved { pub name: String, pub addresses: Vec<String>, pub domains: Vec<String>, pub other_match: Option<String> }
impl Engine { pub(crate) fn resolve_names(&self, names: &[String]) -> Result<Vec<Resolved>> }
```
- "me", "my", "I" → the person's own addresses (`own_addresses`).
- A person: saved contacts and people emailed, ranked by email count, then most recent. When the runner-up is within half the leader's count, `other_match` names it.
- A company: sender domains whose domain or sender name contains the name ("Amazon" → `amazon.com`, `amazon.co.uk`, …) → `domains`.
- No match → empty `addresses` and `domains`. The caller keeps the name as a word.

- [ ] **Step 1: Failing tests:**
  - `a_name_becomes_their_addresses`: Priya at two addresses → both.
  - `two_people_with_one_name_picks_the_closer_and_says_so`
  - `me_means_my_own_addresses`
  - `a_company_becomes_its_sender_domains`
  - `a_name_nobody_has_stays_a_word` (Review Focus 3).
- [ ] **Step 2:** Run; FAIL.
- [ ] **Step 3:** Implement with one SQL over the `EXCHANGED` CTE grouped by address, plus a domain query on `messages.from_addr`.
- [ ] **Step 4:** PASS.
- [ ] **Step 5:** Commit: "Names in a question become the right addresses, and companies their domains".

### Task 8: The search box reads questions with the same code

**Files:**
- Modify: `core/src/ai.rs` (`search_understand`), `core/src/question.rs` (`understand`)
- Test: `core/tests/search.rs`

**Interfaces:**
- Consumes: Task 6 `read_question`, Task 7 `resolve_names`.
- Produces: `pub(crate) async fn understand(&self, text: &str, now: chrono::DateTime<chrono_tz::Tz>, ai_allowed: bool) -> Result<(Question, Vec<Resolved>, bool /* used_ai */)>` on `Engine`, in `core/src/question.rs`. AI is asked only when `sure == false` and `ai_allowed`, and only for the same fields; the AI names a date phrase and code turns it into dates. Ask AI (Task 9) uses this same function.
- Produces: `search_understand` builds `SearchFilters` from `understand(text, now, false)`, so the search box never asks AI. Ask AI calls `understand(text, now, true)`.

- [ ] **Step 1: Failing tests:**
  - `a_sentence_is_understood_without_ai`: "emails from Priya last week", with `FakeAi` set to fail → filters have Priya's address and last week's dates, and `ai.requests().is_empty()`.
  - `ask_ai_asks_ai_only_when_unsure`: `understand("stuff about the thing from a while back", now, true)`, where the rules aren't sure, makes one `FakeAi` request. Its date phrase "a while back" isn't one code knows, so no dates are set; a known phrase it returns ("last month") becomes last month's dates in code.
- [ ] **Step 2:** Run; FAIL.
- [ ] **Step 3:** Implement.
- [ ] **Step 4:** PASS, plus the whole `search` file.
- [ ] **Step 5:** Commit: "The search box reads names and dates by code, the same way Ask AI does".

## Phase C: Ask AI on the new finding

### Task 9: Finding for Ask AI, with passages

**Files:**
- Create: `core/src/ask.rs` (add `mod ask;` in `core/src/lib.rs`). It holds finding, passages and (Task 11) checking, so `assist.rs` doesn't grow.
- Modify: `core/src/assist.rs` (`ai_ask_sources` delegates to `ask_find`)
- Test: create `core/tests/ask.rs`, reusing the rig pattern from `core/tests/assistants.rs`

**Interfaces:**
- Consumes: Tasks 1, 2, 6, 7, and Task 8's `understand`.
- Produces:
```rust
pub(crate) struct Passage { pub id: i64, pub account_id: i64, pub thread_id: String, pub folder_id: i64,
                            pub from: String, pub date: i64, pub subject: String, pub text: String, pub related: bool }
impl Engine { pub(crate) fn ask_find(&self, q: &Question, people: &[Resolved], scope: Option<i64>) -> Result<Vec<Passage>> }
pub(crate) fn follow_up(previous: &Question, next: Question) -> Question
pub(crate) fn passage_around(text: &str, words: &[String], most_chars: usize) -> String
```
- Filters versus preferences are as written in the spec's "Decisions" section. Order: subject match over body, the named person first, newer first when `latest`, future dates first when `upcoming`, newsletters and promotions last.
- Passages are cut from the email's new text (`without_quotes` of the visible text), never from quoted old replies.
- `follow_up`: a follow-up keeps the previous question's names, dates and kind unless the new one gives its own, and adds its words. `ai_ask_sources` gains an optional `previous: String` argument (the last user question), and `AskChat.tsx` sends it instead of gluing the two questions together.
- `ai_ask_sources` returns its sources from `ask_find`, plus `"related"` per source. Its `AskSource` JSON shape stays as it is otherwise.

- [ ] **Step 1: Failing tests** in `core/tests/ask.rs`:
  - `the_wyoming_question_finds_the_wyoming_email`: inbox has "Payment Successful" from `billing@registeredagent.test` with body "Wyoming Registered Agent annual fee $125" and "LumozAI Invoice for October 2026". Asking "What did the Wyoming Registered Agent payment cover?" puts the Wyoming email first, and the LumozAI one isn't among the exact passages.
  - `a_passage_stays_short_and_around_the_words`: a 50,000-character email gives a passage of at most 1,200 characters containing the matched word (Review Focus 5).
  - `nothing_exact_gives_maybe_related_only`: with no exact match, at most 5 passages, all `related == true`.
  - `suspicious_and_unreadable_mail_stays_out` (an existing rule, kept).
  - `a_follow_up_keeps_the_person_and_dates`: "emails from Priya last week" then "any about the budget?" → Priya's last-week emails with "budget".
- [ ] **Step 2:** `cargo test -p mailcore --test ask` → FAIL.
- [ ] **Step 3:** Implement `ask_find` on `search_threads_where` (now exact) plus `search_meaning` for the "Maybe related" fallback, then `passage_around`. Remove the old word-by-word and "newest mail fills in" padding from `ai_ask_sources`.
- [ ] **Step 4:** PASS.
- [ ] **Step 5:** Commit: "Ask AI finds exact matches and gives AI short passages, never padding".

### Task 10: List questions answered with no AI

**Files:**
- Modify: `core/src/ask.rs`, `core/src/assist.rs` (`ai_ask_sources` without an AI key), `app/ui/components/AskChat.tsx` (`ask`: skip `aiChat` when `kind === "list"`, render the found rows), `app/ui/api.ts` (`aiAskSources` return type gains `kind`), `app/ui/locales/*.json`
- Test: `core/tests/ask.rs`; `scripts/selftest/mail-flow.js` (an Ask AI check, if the self-test reaches Ask AI; otherwise none)

**Interfaces:**
- Consumes: Task 9.
- Produces: `ai_ask_sources` works with no AI connected, and returns `{ kind: "list" | "fact" | "catch_up" | "plan", sources }`. "Who's waiting on a reply from me?" lists threads matching `NEEDS_YOU_SQL`. The chat shows a list answer as rows, with a one-line count like `tx("{n} emails")`.

- [ ] **Step 1: Failing tests:**
  - `a_list_question_needs_no_ai`: no key set; "Find emails from Priya sent last week" → only Priya's emails from last week, and `ai.requests().is_empty()`.
  - `whos_waiting_on_me_lists_needs_you`.
- [ ] **Step 2:** Run; FAIL.
- [ ] **Step 3:** Implement the engine side, then `AskChat.tsx`.
- [ ] **Step 4:** `scripts/check.sh`; PASS.
- [ ] **Step 5:** Commit: "Ask AI answers list questions straight away, with no AI".

### Task 11: Answers are checked before they show

**Files:**
- Modify: `core/src/ask.rs` (`check_answer`), `core/src/assist.rs` (`ai_chat`: new AI schema, budgets, partial note), `app/ui/components/AskChat.tsx` (show "needs a check" and "couldn't find"), `app/ui/locales/*.json`
- Test: `core/tests/ask.rs`

**Interfaces:**
- Consumes: Task 9 `Passage`.
- Produces: the AI's schema is `{ "answer": string, "facts": [{ "text": string, "source": int }], "not_found": bool }`. `facts[].text` is an exact copy from the source.
```rust
pub(crate) struct Checked { pub answer: String, pub cited: Vec<i64>, pub removed: Vec<String>, pub not_found: bool }
pub(crate) fn check_answer(answer: &Value, sent: &[Passage]) -> Checked
```
- A fact whose text isn't found (case and whitespace folded) in passage `source` is removed, and its sentence is dropped from `answer`. A `source` that wasn't sent is dropped. If nothing is left, or the AI said `not_found`, `ai_chat` returns `"not_found": true` with the closest passages, and the UI shows `tx("I couldn't find that in your mail.")` above them.
- The answer is shown as plain text. Only cited sources are links; nothing in the answer text is ever turned into a link or an image.
- Budgets come from Global Constraints. When there's more to read than Apple Intelligence's 10 passages, `ai_chat` returns `"partial": true` and the UI adds `tx("Answered from the closest few emails. Connect Claude or ChatGPT in Settings › AI for more.")`.

- [ ] **Step 1: Failing tests:**
  - `a_made_up_amount_is_removed`: `FakeAi` answers with a fact "$900" whose source passage says "$125" → `answer` lacks "$900", and `removed == ["$900"]`.
  - `a_source_that_wasnt_sent_is_dropped`
  - `not_found_says_so_with_the_closest_emails`
  - `answers_in_other_languages_keep_exact_copies`: the answer is in Japanese, the fact "Nov 3" is copied from an English source → kept.
- [ ] **Step 2:** Run; FAIL.
- [ ] **Step 3:** Implement `check_answer` and wire it into `ai_chat`. Strongest passages go first and last in the prompt.
- [ ] **Step 4:** `scripts/check.sh`; PASS.
- [ ] **Step 5:** Commit: "Ask AI answers are checked: any date, amount or name not in the emails is taken out".

### Task 12: Hidden text never reaches AI

**Files:**
- Create: `core/src/visible.rs` (add `mod visible;` in `lib.rs`)
- Modify: `core/src/assist.rs` (`plain_of`: use visible text when there's HTML), `core/src/ask.rs` (passages from visible text)
- Test: unit tests in `visible.rs`; one engine test in `core/tests/ask.rs`

**Interfaces:**
- Produces: `pub(crate) fn visible_text(html: &str) -> String`. It drops elements styled `display:none`, `visibility:hidden`, `font-size:0`, `opacity:0`, or with text the same colour as their background, plus `<script>`, `<style>` and HTML comments. How to parse is left to the builder (spec); prefer what `ammonia`/`html5ever` already bring in.

- [ ] **Step 1: Failing tests:** one per hiding trick, each asserting the hidden sentence "IGNORE PREVIOUS INSTRUCTIONS" is absent and the visible sentence is present. Engine test `hidden_instructions_never_reach_the_ai`: `FakeAi`'s recorded request lacks the hidden sentence.
- [ ] **Step 2:** Run; FAIL.
- [ ] **Step 3:** Implement; use it in `plain_of` and `passage_around`'s input.
- [ ] **Step 4:** PASS, plus the full `assistants` file (replies use `plain_of` too).
- [ ] **Step 5:** Commit: "Hidden text in an email never reaches the AI".

### Task 13: Facts saved as mail arrives, and fact questions with no AI

**Files:**
- Modify: `core/src/store.rs` (append migration: `CREATE TABLE facts (id INTEGER PRIMARY KEY, message_id INTEGER NOT NULL REFERENCES messages(id) ON DELETE CASCADE, kind TEXT NOT NULL, starts_at INTEGER, place TEXT, value_json TEXT NOT NULL); CREATE INDEX facts_kind_time ON facts(kind, starts_at);`), the place bodies are saved (save the facts there), `core/src/ask.rs` (answer Fact questions from facts)
- Create: `core/src/facts.rs` (`pub(crate) fn facts_in(html: Option<&str>, text: &str, attachments: &[Attachment]) -> Vec<Fact>`)
- Test: unit tests in `facts.rs`; `core/tests/ask.rs`

**Interfaces:**
- Produces: `Fact { kind: "flight" | "lodging" | "event" | "order" | "invoice", starts_at: Option<i64>, place: Option<String>, value: Value }`. The sources are schema.org JSON-LD and microdata (`FlightReservation`, `LodgingReservation`, `Order`, `Invoice`, `ParcelDelivery`) and `.ics` attachments. A fact question whose words match a fact's kind or place answers from a fixed template per kind, with the email as its source and no AI.

- [ ] **Step 1: Failing tests:**
  - `a_flight_in_json_ld_becomes_a_fact`: airline HTML with `FlightReservation` to FCO → `kind == "flight"`, `place` contains "Rome" or "FCO", `starts_at` set.
  - `when_is_my_flight_to_rome_needs_no_ai`: `ai.requests().is_empty()`, and the answer names the departure time from the email.
- [ ] **Step 2:** Run; FAIL.
- [ ] **Step 3:** Implement the extraction and the migration (appended after pulling `main`), and the template answers.
- [ ] **Step 4:** `scripts/check.sh`; PASS.
- [ ] **Step 5:** Commit: "Flights, stays, orders and invoices are read from the email itself, so their questions need no AI".

### Task 14: Catch-ups, plans and briefs

**Files:**
- Modify: `core/src/ask.rs` (Plan and CatchUp gathering; event sources), `app/ui/api.ts` (`AskSource.source_type?: "email" | "event"`), `app/ui/components/AskChat.tsx` (an event source opens Calendar on that day)
- Test: `core/tests/ask.rs`

**Interfaces:**
- Consumes: `calendar_events(from, to)` (`core/src/calendar.rs:1073`), `NEEDS_YOU_SQL`, `upcoming_due_messages`.
- Produces: Plan for today = needs-you threads plus mail due today plus today's events, then AI writes at most 5 lines. "Brief me before my next call" = the next event, its attendees' last 3 threads and your last sent message to each. Event passages count as sources in `check_answer`.
- CatchUp: the top conversations from `ask_find` (at most 3 on Apple Intelligence, 6 on Claude/ChatGPT), each giving its newest 2 messages as passages. For a single conversation ("summarize this thread", "latest on the contract thread"), also its last sent message from you ("what did I commit to"). A saved summary from `ai_summaries` may go along as background, marked as not a source; facts from it are removed by `check_answer`.

- [ ] **Step 1: Failing tests:** `priorities_today_include_todays_event_and_waiting_mail`, `a_brief_cites_the_next_event`, `a_catch_up_reads_the_newest_of_each_conversation`, `what_did_i_commit_to_includes_my_last_sent_message`. Each uses `FakeAi` and asserts which passages were sent.
- [ ] **Step 2:** Run; FAIL.
- [ ] **Step 3:** Implement.
- [ ] **Step 4:** `scripts/check.sh`; PASS.
- [ ] **Step 5:** Commit: "Ask AI catches you up, plans your day and briefs you before a call, from mail and calendar".

### Task 15: Measuring on real questions

**Files:**
- Create: `core/tests/ask_real.rs` (`#[ignore]`, the same pattern as `core/tests/priority_real.rs`)
- Modify: `docs/FEATURES.md` (move the "In progress" line into the AI table with the key commits)

**Interfaces:**
- Consumes: everything above.
- Produces: `INBOX_REAL_COPY=<plain copy of the mail db> ASK_QUESTIONS=<json file> cargo test -p mailcore --test ask_real -- --ignored --nocapture`. The JSON is `[{ "question": "...", "ids": [..], "answer": "..." }]`. It prints, per question: right email in top 5, kind, AI used, time, and any removed facts. Counts only, unless `INBOX_REAL_SHOW` is set. The question file stays on Faridul's Mac and is never committed.

- [ ] **Step 1:** Write the test reading both env vars, returning early without them, as `priority_real.rs` does.
- [ ] **Step 2:** `cargo test -p mailcore --test ask_real` → passes trivially (ignored).
- [ ] **Step 3:** Faridul writes about 20 questions; run it by hand, and tune `MEANING_CLOSE_ENOUGH` (Task 2) and the order rules (Task 9) from the results.
- [ ] **Step 4:** `scripts/check.sh` and the self-test PASS.
- [ ] **Step 5:** Commit: "Ask AI: a by-hand measure on real questions; features list updated".
