# Development Workflow

<!-- Refreshed: 2026-10-04 -->

## Test-Driven Development
- Write tests before or alongside implementation
- Coverage: meet the floor the active track's spec/plan requires; the
  terminal-workspace track measured 94.6% lines / 94.8% functions via
  `cargo llvm-cov` — treat repo-total 80% as a minimum, never report
  whole-repo totals as new-code coverage
- Run `cargo test` before each commit
- Run `cargo clippy -- -D warnings` and `cargo fmt --check` before each commit

## Test Tiers
- **Rust unit gates**: `cargo test`, `cargo clippy [--all-targets] -- -D warnings`,
  `cargo fmt --check`, `cargo build --release`
- **PTY acceptance matrix**: `scripts/test-terminal-workspace.py` — stdlib-only
  runner driving the release binary over a real PTY (resize, TERM variants,
  nested tmux, LSP/recovery/git fixtures)
- **Browser-terminal suite**: `tools/terminal-tests/` — node-pty + xterm.js +
  Playwright for browser-reserved shortcuts, paste paths, and copy fallback
- **Quality runner**: `scripts/check-terminal-workspace.sh` runs all gates with
  a binary-size check and nonzero exit on mandatory failures — use it as the
  definitive "all gates" command

## Commit Strategy
- **Commit after each task** completion
- Commit message convention: `Phase <n> Task <m>: <short description>`
  (older commits used `conductor(<track_id>): <task description>`)
- Each commit should be atomic — one logical change per commit
- Never commit broken code (tests must pass)
- Commits stay local; the user decides when to push

## Task Summaries
- Use **Git Notes** for task completion summaries
- Format: `git notes add -m "<summary>" <commit_hash>`
- Include: what changed, files modified, tests added

## Phase Verification
- Verify at the end of each phase according to the track's spec: manual
  verification when the spec asks for it, automated acceptance evidence
  when the spec requires automation (e.g. terminal-workspace's
  `acceptance-evidence.md` gates)
- Verify all tasks in the phase are complete and tests pass
- Review the phase deliverable matches the spec

## Branch Strategy
- Work on `master` branch (single developer workflow)
- Create feature branches for experimental work if needed

## Code Review Checklist (Self-Review)
- [ ] Tests pass (`cargo test`)
- [ ] No clippy warnings (`cargo clippy -- -D warnings`, plus `--all-targets`)
- [ ] Formatted (`cargo fmt --check`)
- [ ] Quality runner green when the task touches shipped behavior
      (`scripts/check-terminal-workspace.sh`; `--quick` for Rust-only)
- [ ] No `.unwrap()` in non-test code
- [ ] Public APIs have doc comments
- [ ] Error messages are actionable
