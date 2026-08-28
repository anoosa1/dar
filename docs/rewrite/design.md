# Sanctuary design direction

The maintainer selected direction B, **Sanctuary**, on 2026-08-27.

## Character

- Calm, warm, focused, and comfortable during long work sessions.
- Generous spacing and readable content density without oversized empty areas.
- Soft rounded navigation and controls, with restrained elevation and borders.
- Natural green is the primary action/selection color; warm clay is reserved for
  attention and exceptional states.
- Each app owns the main workspace. Codex remains one gesture away instead of
  permanently competing with the active task.

## Shell

- A stable left app rail on desktop and five-item bottom navigation on mobile.
- Page identity and local primary action stay in the top bar.
- Global search and notifications remain available without dominating the page.
- User identity and administrator status live at the bottom of the app rail.

## States and accessibility

- Loading uses structural skeletons rather than indefinite spinners.
- Empty states explain the next useful action.
- Conflicts preserve the user's draft and lead with comparison.
- Unavailable/offline states retain last-known data where safe and offer retry.
- Destructive operations use product dialogs, never browser-native prompts.
- Keyboard order follows visible order, focus is restored after route/dialog
  changes, and all status changes use appropriate live regions.

The design tokens and reusable SPA components must preserve this character on
desktop and mobile across every built-in and third-party application.
