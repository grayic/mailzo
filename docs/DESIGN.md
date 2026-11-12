# Design system

One set of decisions, used everywhere. If a screen needs something new, add it here first, then use it.

## Where things live

| What | Where |
|---|---|
| Every raw value: colour, type, spacing, size, corners, shadow, motion, layers | `app/ui/tokens.css` |
| The look of each component, using token names only | `app/ui/styles.css` |
| Shared parts: icon, avatar, buttons, menu, dialog, confirm, tooltip, problem | `app/ui/components/ui.tsx` |
| The icons the app is allowed to use, from the free Hugeicons set | `app/ui/icons.ts` |

`scripts/check-design.mjs` runs inside `scripts/check.sh` and fails if a raw colour, raw size, unknown token, one-off styling class or stray icon import appears outside those files.

## Tokens

**Colour.** Named by job, not by shade, and each has a light and a dark value, so screens never think about the theme.
- Surfaces: `canvas`, `sidebar`, `raised`, `sunken`, plus `hover` and `press` overlays.
- Lines: `line` for dividers, `line-strong` for edges of controls.
- Text: `ink` (main), `ink-2` (secondary), `ink-3` (quiet), `ink-4` (placeholder).
- Meaning: `accent` for the one main action and selection, `danger` for anything destructive, `star` for stars.

**Type.** System font. `text-2xs` 10.5 to `text-number` 30. Interface text is `text-base` 13. Text people read or write is `text-read` 14. Weights are regular, medium, semibold and bold only.

**Spacing.** `s-1` to `s-13`: 2, 4, 6, 8, 10, 12, 14, 16, 20, 24, 28, 32, 48.

**Sizes.** Controls use the `h-*` scale. Buttons are `h-l` (`h-2xl` for the main action on a screen, `h-s` inside tight rows). Icons are `icon-xs` 13, `icon-s` 15, `icon-m` 17, `icon-l` 20. Avatars are `avatar-xs`, `avatar-s`, `avatar-m`.

**Corners.** `r-s` for small controls, `r-m` for buttons and inputs, `r-l` for cards and menus, `r-xl` for the composer, `r-2xl` for welcome cards and dialogs, `r-full` for pills.

**Elevation.** `shadow-1` for controls, `shadow-2` for cards that float, `shadow-3` for anything over other content.

**Motion.** `dur-fast` for hover and press, `dur-base` for things appearing. Reduced-motion settings are respected.

## Rules

- One primary button per view. Everything else is secondary or ghost.
- Icon-only buttons always have a tooltip and a label for screen readers. Tooltips show the keyboard shortcut when there is one.
- Anything that can't be undone asks first with `Confirm`. Its safe choice is labelled in words, never just "Cancel".
- Problems appear with `Problem`, next to what caused them. Notices that aren't tied to a place use the toast.
- People's avatars get a soft colour from their name, so the same person always looks the same.
- Plain-text mail follows the theme. HTML mail always sits on white paper, because senders design for white.

## The exception

Text colours and sizes in the rich editor (`RichEditor.tsx`) are written as real values, because they become part of the email and other mail apps never see these tokens. The email frame's own styles in `prepareEmail.ts` are the same.
