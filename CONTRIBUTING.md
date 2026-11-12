# Contributing

Bug reports and ideas are welcome as issues. Please open an issue before starting on a large change.

Before sending a pull request:

- Run `scripts/check.sh` and make sure it passes.
- Add a test for what you changed.
- Use the shared design tokens and components. See `docs/DESIGN.md`.

## Branches

- `main` always works, and every release is made from it. Nobody commits to it directly.
- Work happens on your own branch and comes in by pull request once the checks pass.
- Saved data must survive every update: database changes only add new migrations in `core/src/store.rs`, and never remove or rename what is there.

## What lives here

This repository holds everything that runs on the person's own device: the app, the mail engine and every feature that works with the person's own keys. A few optional online features need our servers, and that server code lives elsewhere. The app only has small, clearly marked points that call those servers, and they do nothing unless they're switched on.

By contributing, you agree your work is released under this project's [licence](LICENSE).
