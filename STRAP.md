# STRAP.md — Project Rules

Rules for this project. The agent reads this file automatically.

## Principles

- **Act, don't narrate.** Use tools immediately. Never describe what you would do.
- **Token efficiency.** Minimize output. No filler, no preamble, no summaries unless asked. One sentence status updates between tool calls at most.
- **Test everything.** Every change must include tests. No exceptions. Aim for edge cases, not just the happy path.
- **Verify your work.** After edits, read the file back or run tests. Don't assume success.

## Code style

- No comments unless the WHY is non-obvious.
- No docstrings on obvious functions.
- Use the language's idioms — don't write Java in Python or C in Rust.
- Prefer editing over rewriting. Targeted diffs, not full-file replacements.
- No dead code, no unused imports, no TODO comments left behind.

## Testing rules

- Every new function gets a test.
- Every bug fix gets a regression test.
- Test edge cases: empty inputs, large inputs, unicode, missing files, permission errors.
- Tests must be fast. Mock expensive operations. Use temp directories for filesystem tests.
- Run the test suite after changes: `cargo test` (Rust), `pytest` (Python), `npm test` (JS/TS).
- If tests fail, fix them before moving on.

## File operations

- Use `edit_file` with empty `old_string` to create new files.
- Use `edit_file` with exact string matches for surgical edits. Include enough surrounding context for uniqueness.
- Create parent directories automatically — don't worry about `mkdir -p`.
- Read before editing unfamiliar files.

## Workflow

1. Explore first: `list_dir`, `read_file`, `grep`.
2. Make changes: `edit_file`, `bash`.
3. Verify: `read_file` the changed file, or `bash` to run tests/build.
4. Report: one line saying what changed.
