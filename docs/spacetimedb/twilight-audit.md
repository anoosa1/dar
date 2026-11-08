# Twilight UI audit — November 8, 2026

The maintainer selected the [Twilight mocks](twilight-mocks.html): Afterglow
(light) and Nightfall (dark). Existing `latte`/`mocha` account preferences retain
their stored identifiers, with new visible names and palette values.

## Changes

- Shared plum/violet surfaces, orchid/rose actions, neutral writing text and
  distinct error, success and warning treatments across the six apps and settings.
- Visible selection and hover states for navigation, files, contacts and menus;
  stronger form boundaries, readable placeholders and calendar dates.
- Consistent filled creation controls and explicit destructive confirmation
  labels. Completing tasks remains a primary action; deletion uses danger styling.
- Files now bounds long lists to an independent scrolling content pane, keeping
  the header, navigation and desktop details pane in place.

## Reproducible checks

Run `nix develop`, `npm ci --prefix next`, then `npm --prefix next run verify`.
The full verification creates temporary roots, accounts and browser contexts.
Its `test/twilight-browser.mjs` sweep captures 44 states in each light/dark and
desktop/phone combination (176 images), with a JSON manifest under the temporary
fixture's `twilight/` directory. It checks appearance persistence, sampled visible
text contrast, viewport overflow and long Files list scrolling. It waits for the
Files details pane and filename before capture, checks the pane fits the viewport,
and verifies the middle list really scrolls without moving desktop details.
255-character unbroken notebook/task-list names exercise confirmation wrapping. Existing workflow
tests exercise saved data, keyboard/focus behavior, reduced motion, sharing,
unsaved/conflict recovery, recurrence and restart persistence.

The themed sweep includes Files, Contacts, Calendar, Notes, Tasks, Prayer Times,
sign-in, personal settings and administration, including their shared menus,
creation/editing panes, settings dialogs and confirmations. The visible-text scan
is targeted verification, not a complete accessibility certification.

## Council status

The maintainer requested Codex only. Round 1 scored **9/10**, identifying early
phone Files details captures, overflowing confirmation labels with long collection
names, and missing scroll assertions. The fixes wait for actual details, use short
Delete notebook/Delete task list actions with wrapping target text, and verify
scroll movement plus stationary details. Paging collections also keeps the phone
navigation open; the test waits for drawer sizing before using pagination.
Typecheck and the shared UI, Files and Notes/Tasks workflow checks passed. The
updated 176-capture sweep passed contrast, viewport, long-name confirmation and
independent-scroll assertions. Round 2 scored **10/10 for the agreed Twilight
scope**, with no actionable findings.

| Delegate | Round 1 | Round 2 |
| --- | --- | --- |
| Codex (GPT-6.1 Sol, high effort) | 9/10 | 10/10 |

The second review inspected code, representative screenshots and the passing
176-capture manifest/logs without restarting the fixture or rerunning acceptance.
Coverage is Chromium at 390/1440 widths and sampled text contrast; this is not
exhaustive browser or assistive-technology certification.
Screenshots and generated test data remain outside the repository.
