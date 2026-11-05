# Sanctuary design direction

The maintainer selected direction B, **Sanctuary**, on 2026-08-27.

On 2026-10-08 the maintainer selected polish treatment C, **Reading comfort**,
after reviewing three static treatments across Email, Notes, Settings and
Calendar on desktop and phone in both themes. Keep the approved structures and
toolbar placements, with warm cream surfaces, serif page/reading headings,
rounded controls, comfortable spacing and theme-aware states. Mobile controls
retain comfortable touch targets and editor actions stay above navigation.

## Character

- Calm, warm, focused, and comfortable during long work sessions.
- Generous spacing and readable content density without oversized empty areas.
- Soft rounded navigation and controls, with restrained elevation and borders.
- Natural green is the primary action/selection color; warm clay is reserved for
  attention and exceptional states.
- Each app owns the main workspace. Chat remains one gesture away instead of
  permanently competing with the active task.

The 2026-09-10 [Chat plan](chat-plan.md) replaces the proposed Codex app with a
conversation-first app and selectable harnesses (Codex initially). Keep the
selected Workspace A, collapsible sidebar, and resizable optional right panel.
The maintainer selected Chat composition A after reviewing the revised mockups:
centered conversation, composer-level harness/model selectors, collapsible history,
and optional resizable details closed by default.

## Shell

- A stable left app rail on desktop and bottom navigation on mobile. The
  2026-10-08 v1 composition B selection shows all apps in saved order, with Home
  fixed first and only items that do not fit moved into an anchored More menu.
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

On 2026-09-06 the maintainer selected Workspace A for the working apps:
a compact rail and persistent desktop detail panes. Place New after the
Files search field, New contact after Import, and Calendar New after
previous / Today / next. See [workspace usability](workspace-usability.md)
for the implemented workflows and verification entry points.

On 2026-09-09 the maintainer selected composition A for the remaining apps,
Home, and Settings: contextual sidebar, list, and persistent right details.
The contextual sidebar must collapse and reopen with a keyboard-accessible
button. The right detail panel must resize using a pointer or keyboard,
within bounds that preserve a usable main area. Mobile retains focused
single-column views. This selection authorizes implementation of the remaining
app restoration plan; it does not change the existing toolbar placements.
The maintainer also selected toolbar treatment C, **Softly tinted**, after
reviewing bordered, grouped minimal and softly tinted static icon comparisons.
Use locally served Font Awesome Free 7.3.1 SVGs for toolbar and editor actions;
keep navigation, menus, forms, guidance and confirmation dialogs visibly named.
Icon controls retain native semantics, accessible names, keyboard tooltips and
44px touch targets in both themes.

The maintainer selected v1 workspace composition B, **Grouped sidebar /
next-prayer card**, on 2026-10-08 after reviewing three static compositions of
Settings, navigation, Prayer and narrow Files rows on desktop and phones in both
themes. Settings has six URL-addressable pages and a compact phone selector;
Personal and Location retain independent drafts. Prayer emphasizes the selected
day and next time, keeps Sunrise informational, and places analytics below the
schedule. Narrow Files rows retain the filename, Details and More together.
This extends Sanctuary C and the softly tinted icon treatment.
