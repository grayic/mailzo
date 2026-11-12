# Mailzo

An email app for the Mac that keeps your mail on your own devices.

[getmailzo.com](https://getmailzo.com) · [Download for Mac](https://updates.getmailzo.com/Mailzo.dmg) · [Help](https://getmailzo.com/help)

- **Your mail stays with you.** It's stored on your Mac, encrypted, and goes straight to your own mail server. It never passes through ours. No account needed: connect an inbox and read.
- **Works with your email.** Use Gmail, Outlook, iCloud or any address that supports IMAP.
- **Private by design.** Tracking pictures are blocked and named, tracking codes come off links, encrypted email (OpenPGP) works with Thunderbird, Proton and GPG, and Touch ID can lock the app.
- **A calmer inbox.** What needs you comes first, new senders can be screened, and filters can be written as a plain sentence. Snooze, send later, follow-ups, templates, contacts and a calendar are built in.
- **Bring your own AI.** Summaries, explaining an email, translating, and writing and polishing replies use your own API key or a plan you already pay for. It's all optional, nothing is sent unless you turn it on, and encrypted email never goes to AI. Outside AI apps like Claude can be connected, with a switch for each thing they may do.
- **Made for teams too.** Share an inbox, assign emails and leave private notes.
- **In your language.** English, Deutsch, Español, Français, Português, 日本語, 한국어 and 中文.

Mailzo Cloud, the paid service, adds a web version, mail and scheduled sending that keep going with your devices closed, AI included with no key, a daily brief, asking your inbox questions, and AI sorting. Its server side isn't part of this repository.

Mailzo is early and still changing quickly.

## Install

Download the `.dmg` from [getmailzo.com](https://getmailzo.com) or [Releases](../../releases), open it and drag Mailzo into Applications. After that, Mailzo updates itself.

Mailzo is signed and checked by Apple, so it opens like any other app. It needs a Mac with Apple silicon (M1 or later).

## How it's built

- `core/` is the mail engine in Rust: accounts, sync and the local database.
- `app/ui/` is the screens (React). `app/src-tauri/` is the Mac app around them (Tauri).
- Everything here runs on your own Mac. The app talks to Mailzo's account server only for sign-in codes, the device list and an encrypted vault, which can't read mail or email passwords. Paid Mailzo Cloud features run on our servers and switch on after signing in to a paid account; their server side isn't part of this repository.
- `dev/` has small fake mail and sign-in servers, so everything can be tested without real accounts.
- `docs/DESIGN.md` is the design system. Raw colours and sizes belong only in `app/ui/tokens.css`.

## Build from source

You need macOS, [Rust](https://rustup.rs), Node.js and [pnpm](https://pnpm.io).

```bash
cd app && pnpm install && cd ..
scripts/check.sh
scripts/dev-web.sh
```

`scripts/check.sh` runs every check. `scripts/dev-web.sh` runs the screens in a browser with a demo mailbox at http://127.0.0.1:5177. The demo's address and password are in `dev/fakeimap/src/lib.rs`, on server `127.0.0.1`, port `1143`.

Other useful commands:

| Task | Command |
|---|---|
| Build the Mac app for your own Mac | `scripts/build-mac.sh` |
| Build a `.dmg` to share | `scripts/build-dmg.sh` |
| Test the real Mac app | `scripts/mac-selftest.sh` (also needs a development account server; without one, run the engine tests and `scripts/dev-web.sh`) |
| Build for the iPhone simulator | `scripts/ios-sim.sh` |

To use your own account server, set `INBOX_ACCOUNT_SERVER` when building. Browser sign-in for Google and Microsoft needs your own client IDs in `INBOX_GOOGLE_CLIENT_ID`, `INBOX_GOOGLE_CLIENT_SECRET` and `INBOX_MICROSOFT_CLIENT_ID`. Copy `.env.example` to `.env`, which is never committed, and fill them in.

## Licence

[AGPL-3.0](LICENSE). Copyright (C) 2026 Grayic LLC. The name and logo aren't covered by the licence.
