# Spec: Focus Navigation Keybinding Remap

## Overview

Replace the tab-only focus cycling with directional `Ctrl+Arrow` focus navigation and reassign terminal resize from `Ctrl+↑/↓` to `Ctrl+Shift+↑/↓`. This provides a more intuitive spatial navigation model where arrow direction corresponds to panel position.

## Functional Requirements

1. **Ctrl+← / Ctrl+→**: Navigate focus horizontally between Tree and Preview panels
2. **Ctrl+↑ / Ctrl+↓**: Navigate focus vertically to/from Terminal panel (when visible)
3. **Ctrl+Shift+↑ / Ctrl+Shift+↓**: Resize the terminal panel height (previously `Ctrl+↑/↓`)
4. **Tab**: Retained as alternative forward-cycle through panels (backward compatibility)
5. When terminal is focused, `Ctrl+Arrow` keys are intercepted for focus navigation (NOT forwarded to PTY)
6. When terminal is focused, `Tab` continues to be forwarded to PTY for shell autocompletion

### Focus Direction Mapping

| Key | From Tree | From Preview | From Terminal |
|-----|-----------|-------------|---------------|
| `Ctrl+←` | no-op | → Tree | → Tree |
| `Ctrl+→` | → Preview | no-op | → Preview |
| `Ctrl+↑` | no-op | no-op | → Tree (or last horizontal) |
| `Ctrl+↓` | → Terminal* | → Terminal* | no-op |

*Only when terminal is visible

## Non-Functional Requirements

- No performance impact (pure input routing change)
- Help overlay (`?`) must be updated to reflect new keybindings

## Acceptance Criteria

- [ ] `Ctrl+←` moves focus left (Preview→Tree, Terminal→Tree)
- [ ] `Ctrl+→` moves focus right (Tree→Preview, Terminal→Preview)
- [ ] `Ctrl+↑` moves focus up from Terminal to previously focused panel (Tree or Preview)
- [ ] `Ctrl+↓` moves focus down to Terminal (when visible)
- [ ] `Ctrl+Shift+↑` shrinks terminal panel
- [ ] `Ctrl+Shift+↓` grows terminal panel
- [ ] `Tab` still cycles focus forward (Tree→Preview→Terminal→Tree)
- [ ] `Tab` in terminal still forwards to PTY for autocompletion
- [ ] Help overlay updated with new keybindings
- [ ] All existing tests pass; new tests for directional focus

## Out of Scope

- Configurable keybindings (already handled by config system, not part of this track)
- Mouse-based focus switching (already works, unchanged)
