# Development Workflow

## Test-Driven Development
- Write tests before or alongside implementation
- Target **80% test coverage** minimum
- Run `cargo test` before each commit
- Run `cargo clippy -- -D warnings` and `cargo fmt --check` before each commit

## Commit Strategy
- **Commit after each task** completion
- Commit message format: `conductor(<track_id>): <task description>`
- Each commit should be atomic — one logical change per commit
- Never commit broken code (tests must pass)

## Task Summaries
- Use **Git Notes** for task completion summaries
- Format: `git notes add -m "<summary>" <commit_hash>`
- Include: what changed, files modified, tests added

## Phase Verification
- At the end of each phase, perform manual verification
- Verify all tasks in the phase are complete and tests pass
- Review the phase deliverable matches the spec

## Branch Strategy
- Work on `master` branch (single developer workflow)
- Create feature branches for experimental work if needed

## Code Review Checklist (Self-Review)
- [ ] Tests pass (`cargo test`)
- [ ] No clippy warnings (`cargo clippy -- -D warnings`)
- [ ] Formatted (`cargo fmt --check`)
- [ ] No `.unwrap()` in non-test code
- [ ] Public APIs have doc comments
- [ ] Error messages are actionable
