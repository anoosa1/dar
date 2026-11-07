# Calendar workspace refinement

The maintainer's November 7 instructions refine the selected Material 3 workspace
and retain Catppuccin Latte/Mocha. No dependency or database is added.

- Fresh Calendar navigation defaults to Month. Existing view/date URLs keep their
  meaning. Previous/Next move by the selected view's period; Month navigation
  preserves the selected day where possible and clamps month/year boundaries.
- The single centre control shows Today for today's date and the selected date
  otherwise. Clicking it always returns to today in the user's configured zone.
  There is no duplicate Today action in the sidebar.
- All calendar views use the available workspace height. Day/week/agenda scroll
  their own content; Month keeps six bounded week rows. Crowded cells expose a
  labelled link to the full day instead of silently dropping events.
- Every owned calendar has its own themed Edit/Share/Delete menu. Action targets
  are separate from the currently selected event-creation calendar. Deletion
  waits for its resource count and confirms removal of events, tasks and series.
- Calendar settings owns week start, durable cancelled-event visibility and ICS
  import/export. Transfers select a specific calendar, enforce read/write access,
  retain existing bounds and report per-item import failures.
- Personal → Regional settings owns the existing IANA timezone preference. Its
  revision-checked save preserves other preferences. Calendar display, task dates
  and reminder processing continue using that same stored value. Event/task
  editors retain per-resource timezone fields.
- The calendar sidebar uses a viewport-dependent page of calendars. Its footer
  stays available and the sidebar itself does not scroll. Files storage usage is
  grouped immediately above Files settings.

Browser coverage checks date-label changes and return-to-today, month default,
page/side-panel dimensions at desktop/tablet/phone sizes, multi-page calendars,
action targets when another calendar is selected, settings ICS round trips,
cancelled-event settings and regional timezone inheritance in new events.
Pure navigation tests cover day/week periods, leap years and month/year clamps.

Verification passed: `nix develop ./next -c npm --prefix next run verify` ran
58 Node tests and five real Chromium journeys, independent vdirsyncer round
trips, module/backup upgrades and forced process/database restart recovery.
The Calendar journey exercised a multi-page list, desktop/tablet/phone viewport
bounds, action targeting, cancelled-event settings and an ICS import/export
round trip. The shared journey verified Files footer placement and saved
Personal timezone inheritance in a new event. Captured screenshots and all
test data remain in isolated temporary roots outside the repository.
