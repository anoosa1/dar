//! Helpers and fixtures shared by the agent-git suites (ported from `test/shared/`, `test/support/` and
//! `test/fixtures/`). Only that area's owner edits this file.

#![allow(dead_code)]

use std::process::ExitCode;

/// The Codex CLI stand-in (`test/fixtures/codex-fixture.mjs`).
pub fn codex_fixture(_args: &[String]) -> ExitCode {
    eprintln!("the Codex fixture is not ported yet");
    ExitCode::FAILURE
}

/// The Claude Code CLI stand-in (`test/fixtures/claude-fixture.mjs`).
pub fn claude_fixture(_args: &[String]) -> ExitCode {
    eprintln!("the Claude fixture is not ported yet");
    ExitCode::FAILURE
}
