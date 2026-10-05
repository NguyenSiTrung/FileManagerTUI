# Product Guidelines

> Last refreshed: 2026-10-05 — Cargo package name corrected to `file_manager_tui`

## Tone & Voice
- **Technical & concise** — Developer-focused, no fluff
- Status messages are short and actionable (e.g., "3 files copied", "Permission denied: /root")
- Error messages include the cause and path, not generic "something went wrong"

## UI Principles
- **Keyboard-first** — Every action reachable via keyboard; mouse is supplementary
- **Minimal chrome** — Maximize content area; status bar for context, not decoration
- **Responsive feedback** — Every action produces visible feedback (status message, cursor move, highlight)
- **Non-destructive defaults** — Destructive operations (delete, overwrite) always require confirmation

## Naming Conventions
- Binary name: `fm`
- Config file: `~/.config/fm-tui/config.toml`
- Project references: "FileManagerTUI" in docs, `file_manager_tui` in Cargo package name

## Visual Identity
- Default theme: dark background, high-contrast text
- Tree icons: Nerd Font glyphs with ASCII fallback (`--no-icons`)
- Color palette: terminal ANSI colors for maximum compatibility; no truecolor requirement

## Safety Standards
- **Git is read-only** — the only allowed query is `git status --porcelain=v2 -z --branch` under `--no-optional-locks`; no git write commands ever.
- **LSP executes only installed servers** — never download a server; project-local argv needs an interactive session-scoped trust grant; durable grants live in the global config only.
- **Recovery is private and non-destructive** — snapshots stay in a user-private state dir (0700/0600), keyed to the on-disk revision; restoring produces a dirty buffer and never writes the original file.

## Documentation Standards
- README: Install → Quick Start → Keybindings → Configuration → Building → Testing
- Inline help (`?` key): grouped by category, single screen, no scrolling required
- Code comments: only for non-obvious logic; prefer self-documenting names
