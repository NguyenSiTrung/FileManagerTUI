# Rust Code Style Guide

## Formatting
- Use `cargo fmt` (rustfmt) — enforced in CI
- Max line width: 100 characters
- Use trailing commas in multi-line constructs

## Linting
- `cargo clippy -- -D warnings` — all warnings are errors in CI
- No `#[allow(clippy::...)]` without a comment explaining why

## Naming
- `snake_case` for functions, variables, modules, file names
- `PascalCase` for types, traits, enums
- `SCREAMING_SNAKE_CASE` for constants and statics
- Prefix unused variables with `_` only when required by the compiler

## Error Handling
- Use `Result<T, E>` for fallible operations
- No `.unwrap()` or `.expect()` in library/application code — use `?` propagation
- `.unwrap()` is acceptable only in tests and `main()` setup
- Define domain-specific error types with `thiserror` or manual `impl`

## Imports
- Group imports in order: `std` → external crates → `crate`/`super`
- One `use` per line (no nested glob imports like `use std::{a, b, c}` for unrelated items)
- Prefer explicit imports over glob (`use module::*`)

## Documentation
- Public API items must have `///` doc comments
- Module-level docs with `//!` at the top of each file
- Include code examples in doc comments for complex APIs

## Types & Patterns
- Prefer `&str` over `String` in function parameters when ownership isn't needed
- Use `impl Into<T>` for flexible public API parameters
- Prefer iterators over index-based loops
- Use `enum` over boolean flags for clarity (e.g., `Expanded::Yes` vs `true`)

## Testing
- Test files live in `tests/` for integration tests
- Unit tests use `#[cfg(test)] mod tests` at bottom of source file
- Test function names: `test_<function>_<scenario>_<expected>`
- Use `assert_eq!` with descriptive messages
