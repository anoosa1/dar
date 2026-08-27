# Dar TODO

Dar is a single-process Go service. The server owns authentication, shared SQLite access,
app route registration, and graceful lifecycle management.

## Platform

- [ ] Add CSRF protection to browser mutations.
- [ ] Add request body and upload limits.
- [ ] Add security headers and structured request logging.
- [ ] Add administration and app enable/disable controls.

## Apps

- [x] Integrate Codex CLI and register typed assistant tools for every app.
- [ ] Expand CalDAV and CardDAV interoperability coverage.
- [ ] Add persistent News refresh and Email sync status reporting.

## Verification

- [ ] Extend authenticated end-to-end coverage across every app.
- [ ] Add shutdown tests with active SSE, feed refresh, and email sync work.
- [ ] Add backup/restore documentation for the database and data root.
