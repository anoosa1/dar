# Go baseline checkpoint

The ClojureScript and Cordis rewrite uses the complete `go-rewrite` worktree as
its behavioral baseline. The checkpoint was created from parent commit
`f765d65517eae7a3ac2f7ee5e44495b35d769431` on 2026-08-27.

## Verification

`nix develop 'path:/home/anas/dar' --command bash -lc './test/run.sh fast'`
completed successfully before the checkpoint was committed. It built the
commands and passed the Go vet and package test suite.

## Deliberately excluded local artifacts

- `.playwright-mcp/`: generated browser captures and console logs.
- `.grok/`: developer-local assistant settings.
- `test-artifacts/`: generated coverage reports.
- Files already excluded by `.gitignore`, including databases, credentials,
  data roots, logs, build outputs, editor files, and local environment files.

Product source, tests, configuration, plans, documentation, the `ideas` note,
and deliberate visual audit artifacts are included in the baseline. `TODO.md`
and `ideas` are reference material, not parity requirements unless they describe
behavior already implemented elsewhere in the checkpoint.
