# Product Guidelines

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
- Config file: `~/.config/fm/config.toml`
- Project references: "FileManagerTUI" in docs, `file-manager-tui` in Cargo package name

## Visual Identity
- Default theme: dark background, high-contrast text
- Tree icons: Nerd Font glyphs with ASCII fallback (`--no-icons`)
- Color palette: terminal ANSI colors for maximum compatibility; no truecolor requirement

## Documentation Standards
- README: Install → Quick Start → Keybindings → Configuration → Building
- Inline help (`?` key): grouped by category, single screen, no scrolling required
- Code comments: only for non-obvious logic; prefer self-documenting names
