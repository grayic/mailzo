# Working on Mailzo

Two people build this app, Faridul and Farhan (Rizvi), each with their own AI agent, often at the same time. These rules are for both of you and your agents. How the code is laid out is in `README.md`, how changes come in is in `CONTRIBUTING.md`, and how things look is in `docs/DESIGN.md`.

## Before building anything

1. `git fetch --all`, so you see the other person's latest work.
2. Read `docs/FEATURES.md`. If the feature, or one close to it, is already there or in progress, build on that code and don't start a second version.
3. Search every branch for the same work: `git log --all --oneline -i --grep "<a few words>"`, and search the code for the screen, command or setting you'd add.
4. Check open issues and pull requests: `gh issue list` and `gh pr list`.
5. If anything overlaps, stop and tell your person before writing code.

## While building

- Claim the work in your first commit by adding a line under "In progress" in `docs/FEATURES.md`: who, what, which branch. For anything big, also open a draft pull request.
- Each branch is someone's own: Faridul works on `faridul/dev`, Farhan on `farhan/dev`, and both come into `main`. Don't commit to the other person's branch.
- One finished piece per commit, described in plain words.
- Some files are shared hot spots. Pull before touching them and keep the change small:
  - `core/src/store.rs` migrations only ever get added at the end of the list, after pulling `main`. A database whose migrations were numbered differently on two branches can't be opened by the merged app.
  - The command list in `core/src/engine.rs`, and `app/ui/App.tsx` and `app/ui/api.ts`.
- Use the existing design tokens and components, and the words other apps use for the same thing.

## Before calling it done

Test only the part you changed, not everything. A full run takes many minutes and mostly proves again what nobody touched.

- Rust: `cargo fmt --all --check`, `cargo clippy -p <crate> --all-targets -- -D warnings`, and the tests for what you changed (`cargo test -p <crate> <test name>`).
- Screens: `pnpm -s exec tsc --noEmit` in `app`, then try the changed part in the app.
- Merging another branch in brings many changes at once, so then run `scripts/check.sh` in full. `scripts/mac-selftest.sh` runs every screen and only when asked. It needs the private `dakly-cloud` project next to this one, or `DAKLY_CLOUD` pointing at it.
- In `docs/FEATURES.md`, move your line from "In progress" into the feature list, with where it lives and its key commits.
