//! Configurable, context-scoped workspace key routing.
use crate::commands::CommandId;
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use serde::Deserialize;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize, clap::ValueEnum)]
#[serde(rename_all = "snake_case")]
pub enum KeymapProfile {
    #[default]
    Standard,
    Web,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FocusContext {
    Tree,
    Preview,
    Editor,
    Terminal,
    EditorFind,
    Modal,
    Menu,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct KeymapConfig {
    pub profile: Option<KeymapProfile>,
    pub bindings: Option<Vec<BindingOverride>>,
    pub timeout_ms: Option<u64>,
}
impl KeymapConfig {
    /// Replace each explicitly supplied command/context pair; empty `keys`
    /// unbinds that pair. An absent list preserves lower-priority overrides.
    pub fn merge(self, other: &Self) -> Self {
        let bindings = match &other.bindings {
            None => self.bindings,
            Some(over) => {
                let mut bindings = self.bindings.unwrap_or_default();
                bindings.retain(|base| {
                    !over
                        .iter()
                        .any(|b| b.command == base.command && b.context == base.context)
                });
                bindings.extend(over.iter().cloned());
                Some(bindings)
            }
        };
        Self {
            profile: other.profile.or(self.profile),
            timeout_ms: other.timeout_ms.or(self.timeout_ms),
            bindings,
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BindingOverride {
    pub command: String,
    pub context: FocusContext,
    pub keys: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Resolution {
    Forward,
    Consumed,
    Command(CommandId),
}

/// A parsed portable chord. Ctrl+Shift+letters are deliberately not distinct.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Chord {
    code: KeyCode,
    modifiers: KeyModifiers,
}
impl Chord {
    fn event(key: KeyEvent) -> Self {
        let mut code = key.code;
        let mut modifiers = key.modifiers;
        if let KeyCode::Char(ch) = code {
            if modifiers.intersects(KeyModifiers::CONTROL | KeyModifiers::ALT)
                && ch.is_ascii_alphabetic()
            {
                code = KeyCode::Char(ch.to_ascii_lowercase());
                modifiers.remove(KeyModifiers::SHIFT);
            } else if ch.is_ascii_alphabetic() && modifiers == KeyModifiers::SHIFT {
                code = KeyCode::Char(ch.to_ascii_uppercase());
                modifiers = KeyModifiers::NONE;
            }
        }
        if code == KeyCode::BackTab {
            code = KeyCode::Tab;
            modifiers.insert(KeyModifiers::SHIFT);
        }
        Self { code, modifiers }
    }
    fn parse(text: &str) -> Result<Self, String> {
        if text.len() > 32 || text.is_empty() {
            return Err("Invalid or oversized key chord".into());
        }
        let parts: Vec<_> = text.split('+').collect();
        let mut modifiers = KeyModifiers::NONE;
        for part in &parts[..parts.len() - 1] {
            let modifier = match part.to_ascii_lowercase().as_str() {
                "ctrl" => KeyModifiers::CONTROL,
                "alt" => KeyModifiers::ALT,
                "shift" => KeyModifiers::SHIFT,
                _ => return Err(format!("Unsupported key modifier: {part}")),
            };
            if modifiers.contains(modifier) {
                return Err("Duplicate key modifier".into());
            }
            modifiers.insert(modifier);
        }
        if modifiers.contains(KeyModifiers::CONTROL | KeyModifiers::ALT) {
            return Err("Ctrl+Alt is not a portable terminal encoding".into());
        }
        let name = parts[parts.len() - 1];
        let code = match name.to_ascii_lowercase().as_str() {
            "tab" => KeyCode::Tab,
            "enter" => KeyCode::Enter,
            "esc" => KeyCode::Esc,
            "space" => KeyCode::Char(' '),
            "backspace" => KeyCode::Backspace,
            "delete" => KeyCode::Delete,
            "insert" => KeyCode::Insert,
            "left" => KeyCode::Left,
            "right" => KeyCode::Right,
            "up" => KeyCode::Up,
            "down" => KeyCode::Down,
            "home" => KeyCode::Home,
            "end" => KeyCode::End,
            "pageup" => KeyCode::PageUp,
            "pagedown" => KeyCode::PageDown,
            _ if name.starts_with(['f', 'F']) && name.len() > 1 => {
                let n = name[1..]
                    .parse::<u8>()
                    .map_err(|_| "Invalid function key")?;
                if !(1..=12).contains(&n) {
                    return Err("Only F1..F12 are portable".into());
                }
                KeyCode::F(n)
            }
            _ if name.len() == 1 && name.is_ascii() && !name.as_bytes()[0].is_ascii_control() => {
                KeyCode::Char(name.as_bytes()[0] as char)
            }
            _ => return Err(format!("Unknown key: {name}")),
        };
        if let KeyCode::Char(ch) = code {
            if modifiers.contains(KeyModifiers::CONTROL) {
                if modifiers.contains(KeyModifiers::SHIFT)
                    || !ch.is_ascii_alphabetic()
                    || matches!(ch.to_ascii_lowercase(), 'h' | 'i' | 'j' | 'm')
                {
                    return Err("Control chord is ambiguous on byte-only terminals".into());
                }
            } else if modifiers.contains(KeyModifiers::SHIFT) {
                return Err("Use a literal uppercase letter, not Shift+letter".into());
            }
        } else if matches!(
            code,
            KeyCode::Tab | KeyCode::Enter | KeyCode::Esc | KeyCode::Backspace
        ) && !modifiers.is_empty()
            && !(code == KeyCode::Tab && modifiers == KeyModifiers::SHIFT)
        {
            return Err("Modified control key is not a portable terminal encoding".into());
        }
        Ok(Self::event(KeyEvent::new(code, modifiers)))
    }
    fn label(self) -> String {
        let name = match self.code {
            KeyCode::Char(' ') => "Space".into(),
            KeyCode::Char(ch) if !self.modifiers.is_empty() => ch.to_ascii_uppercase().to_string(),
            KeyCode::Char(ch) => ch.to_string(),
            KeyCode::F(n) => format!("F{n}"),
            KeyCode::Tab => "Tab".into(),
            KeyCode::Enter => "Enter".into(),
            KeyCode::Esc => "Esc".into(),
            KeyCode::Backspace => "Backspace".into(),
            KeyCode::Delete => "Delete".into(),
            KeyCode::Insert => "Insert".into(),
            KeyCode::Left => "Left".into(),
            KeyCode::Right => "Right".into(),
            KeyCode::Up => "Up".into(),
            KeyCode::Down => "Down".into(),
            KeyCode::Home => "Home".into(),
            KeyCode::End => "End".into(),
            KeyCode::PageUp => "PageUp".into(),
            KeyCode::PageDown => "PageDown".into(),
            _ => unreachable!("only parsed chords have labels"),
        };
        let mut label = String::new();
        for (modifier, text) in [
            (KeyModifiers::CONTROL, "Ctrl+"),
            (KeyModifiers::ALT, "Alt+"),
            (KeyModifiers::SHIFT, "Shift+"),
        ] {
            if self.modifiers.contains(modifier) {
                label.push_str(text);
            }
        }
        label + &name
    }
}
#[derive(Debug)]
struct Binding {
    command: CommandId,
    context: FocusContext,
    sequence: Vec<Chord>,
}
#[derive(Debug)]
struct Pending {
    context: FocusContext,
    sequence: Vec<Chord>,
    started_ms: u64,
}
/// Bounded resolution state; no I/O, clocks, processes or app references.
#[derive(Debug)]
pub struct Keymap {
    bindings: Vec<Binding>,
    pending: Option<Pending>,
    cancelled_suffix: bool,
    timeout_ms: u64,
}
impl Keymap {
    pub fn compile(config: &KeymapConfig) -> Result<Self, String> {
        let timeout_ms = config.timeout_ms.unwrap_or(1200);
        if !(100..=10000).contains(&timeout_ms) {
            return Err("keymap timeout_ms must be 100..10000".into());
        }
        let mut map = Self {
            bindings: vec![],
            pending: None,
            cancelled_suffix: false,
            timeout_ms,
        };
        map.defaults(config.profile.unwrap_or_default())?;
        let overrides = config.bindings.as_deref().unwrap_or_default();
        if overrides.len() > 256 {
            return Err("At most 256 keymap overrides are allowed".into());
        }
        let mut seen = Vec::new();
        for over in overrides {
            let command = crate::commands::REGISTRY
                .iter()
                .find(|m| m.id.as_str() == over.command)
                .map(|m| m.id)
                .ok_or_else(|| format!("Unknown keymap command: {}", over.command))?;
            if seen.contains(&(command, over.context)) {
                return Err("Duplicate command/context override".into());
            }
            seen.push((command, over.context));
            if over.keys.len() > 8 {
                return Err("At most eight shortcuts per command/context".into());
            }
            if matches!(over.context, FocusContext::Modal)
                || (matches!(over.context, FocusContext::EditorFind | FocusContext::Menu)
                    && command != CommandId::Commands)
            {
                return Err(
                    "Raw overlays only admit explicit menu entry in EditorFind/Menu".into(),
                );
            }
            map.bindings
                .retain(|b| b.command != command || b.context != over.context);
            for keys in &over.keys {
                map.add(command, over.context, keys)?;
            }
        }
        if map.bindings.len() > 1024 {
            return Err("Too many resolved key bindings".into());
        }
        for (index, a) in map.bindings.iter().enumerate() {
            for b in &map.bindings[index + 1..] {
                if a.context == b.context
                    && (a.sequence.starts_with(&b.sequence) || b.sequence.starts_with(&a.sequence))
                {
                    return Err(format!(
                        "Duplicate or conflicting sequence prefix in {:?}: {} / {}",
                        a.context,
                        a.command.as_str(),
                        b.command.as_str()
                    ));
                }
            }
        }
        Ok(map)
    }
    fn add(&mut self, command: CommandId, context: FocusContext, keys: &str) -> Result<(), String> {
        if keys.len() > 128 {
            return Err("Key sequence exceeds 128 bytes".into());
        }
        let sequence: Vec<_> = keys
            .split_whitespace()
            .map(Chord::parse)
            .collect::<Result<_, _>>()?;
        if sequence.is_empty() || sequence.len() > 4 {
            return Err("Key sequences require one to four chords".into());
        }
        if sequence
            .iter()
            .skip(1)
            .any(|chord| chord.code == KeyCode::Esc)
        {
            return Err("Esc cancels a pending sequence and cannot be a suffix".into());
        }
        if sequence.len() > 1
            && matches!(sequence[0].code, KeyCode::Char(_))
            && sequence[0].modifiers.is_empty()
        {
            return Err("Textual workspace prefixes steal ordinary input".into());
        }
        self.bindings.push(Binding {
            command,
            context,
            sequence,
        });
        Ok(())
    }
    fn defaults(&mut self, profile: KeymapProfile) -> Result<(), String> {
        use CommandId::*;
        let workspace = [
            FocusContext::Tree,
            FocusContext::Preview,
            FocusContext::Editor,
            FocusContext::Terminal,
        ];
        // Alt+G is an explicit non-textual prefix, not a browser-reserved Ctrl shortcut.
        for context in workspace {
            self.add(Commands, context, "F8")?;
            for (command, suffix) in [
                (Commands, "m"),
                (Save, "s"),
                (SaveAs, "a"),
                (Close, "q"),
                (QuickOpen, "o"),
                (Documents, "d"),
                (Next, "n"),
                (Previous, "b"),
                (FocusTree, "1"),
                (FocusEditor, "2"),
                (FocusTerminal, "3"),
                (FocusPreview, "4"),
                (ToggleTerminal, "t"),
                (Wrap, "w"),
                (ToggleExplorer, "e"),
                (MaximizeEditor, "x"),
                (MaximizeTerminal, "z"),
                (RestoreLayout, "r"),
                (GrowExplorer, "l"),
                (ShrinkExplorer, "h"),
                (GrowTerminal, "k"),
                (ShrinkTerminal, "j"),
            ] {
                self.add(command, context, &format!("Alt+G {suffix}"))?;
            }
            if profile == KeymapProfile::Standard {
                for (command, keys) in [
                    (ToggleTerminal, "Ctrl+T"),
                    (FocusLeft, "Ctrl+Left"),
                    (FocusRight, "Ctrl+Right"),
                    (FocusUp, "Ctrl+Up"),
                    (FocusDown, "Ctrl+Down"),
                    (GrowTerminal, "Ctrl+Shift+Up"),
                    (ShrinkTerminal, "Ctrl+Shift+Down"),
                ] {
                    self.add(command, context, keys)?;
                }
                if context != FocusContext::Terminal {
                    for (command, keys) in [
                        (Documents, "Alt+O"),
                        (Pin, "Alt+P"),
                        (Next, "Alt+N"),
                        (Previous, "Alt+B"),
                        (Reveal, "Alt+R"),
                        (Close, "Alt+Q"),
                    ] {
                        self.add(command, context, keys)?;
                    }
                }
                if context == FocusContext::Editor {
                    self.add(Save, context, "Ctrl+S")?;
                    self.add(Wrap, context, "Alt+W")?;
                }
                if context == FocusContext::Preview {
                    self.add(Wrap, context, "Alt+W")?;
                }
                if matches!(context, FocusContext::Tree | FocusContext::Preview) {
                    self.add(QuickOpen, context, "Ctrl+P")?;
                }
            }
            if matches!(context, FocusContext::Tree | FocusContext::Preview) {
                for (command, keys) in [
                    (Quit, "q"),
                    (Quit, "Ctrl+C"),
                    (ToggleTerminal, "t"),
                    (FocusCycle, "Tab"),
                ] {
                    self.add(command, context, keys)?;
                }
            }
        }
        self.add(Commands, FocusContext::EditorFind, "F8")?;
        self.add(Commands, FocusContext::EditorFind, "Alt+G m")?;
        self.add(Commands, FocusContext::Menu, "F8")?;
        Ok(())
    }
    /// `now_ms` is supplied by the event adapter; tests inject a monotonic fake clock.
    /// Timeout, Esc, mismatch and changed context consume the offending suffix.
    pub fn feed(&mut self, context: FocusContext, key: KeyEvent, now_ms: u64) -> Resolution {
        if key.kind == crossterm::event::KeyEventKind::Release {
            return Resolution::Consumed;
        }
        if self.cancelled_suffix {
            self.cancelled_suffix = false;
            return Resolution::Consumed;
        }
        let chord = Chord::event(key);
        let mut sequence = vec![chord];
        let started_ms = if let Some(mut pending) = self.pending.take() {
            if pending.context != context
                || now_ms.saturating_sub(pending.started_ms) >= self.timeout_ms
                || chord.code == KeyCode::Esc
            {
                return Resolution::Consumed;
            }
            if key.kind == crossterm::event::KeyEventKind::Repeat {
                self.pending = Some(pending);
                return Resolution::Consumed;
            }
            pending.sequence.push(chord);
            sequence = pending.sequence;
            pending.started_ms
        } else {
            now_ms
        };
        if let Some(binding) = self
            .bindings
            .iter()
            .find(|b| b.context == context && b.sequence == sequence)
        {
            return Resolution::Command(binding.command);
        }
        if self
            .bindings
            .iter()
            .any(|b| b.context == context && b.sequence.starts_with(&sequence))
        {
            self.pending = Some(Pending {
                context,
                sequence,
                started_ms,
            });
            return Resolution::Consumed;
        }
        if sequence.len() > 1 {
            Resolution::Consumed
        } else {
            Resolution::Forward
        }
    }
    /// Cancel without replay; quarantine one suffix when an external modal/paste/focus
    /// change interrupted a prefix, so it cannot become a destructive tree command.
    pub fn reset(&mut self) {
        self.cancelled_suffix |= self.pending.take().is_some();
    }
    pub fn replace(&mut self, mut replacement: Self) {
        replacement.cancelled_suffix = self.cancelled_suffix || self.pending.is_some();
        *self = replacement;
    }
    #[allow(dead_code)] // Metadata consumer is the following help/settings task.
    pub fn binding_labels(&self, command: CommandId, context: FocusContext) -> Vec<String> {
        self.bindings
            .iter()
            .filter(|b| b.command == command && b.context == context)
            .map(|b| {
                b.sequence
                    .iter()
                    .map(|c| c.label())
                    .collect::<Vec<_>>()
                    .join(" ")
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn adaptive_keymap_both_profiles_offer_keyboard_only_pane_routes() {
        for profile in [KeymapProfile::Standard, KeymapProfile::Web] {
            let mut map = Keymap::compile(&KeymapConfig {
                profile: Some(profile),
                ..Default::default()
            })
            .unwrap();
            for context in [
                FocusContext::Tree,
                FocusContext::Preview,
                FocusContext::Editor,
                FocusContext::Terminal,
            ] {
                for (id, suffix) in [
                    ("pane.explorer.toggle", 'e'),
                    ("pane.editor.maximize", 'x'),
                    ("pane.terminal.maximize", 'z'),
                    ("pane.layout.restore", 'r'),
                    ("pane.explorer.grow", 'l'),
                    ("pane.explorer.shrink", 'h'),
                    ("pane.terminal.grow", 'k'),
                    ("pane.terminal.shrink", 'j'),
                ] {
                    let command = crate::commands::REGISTRY
                        .iter()
                        .find(|m| m.id.as_str() == id)
                        .expect("registered pane command")
                        .id;
                    assert_eq!(
                        map.feed(context, key('g', KeyModifiers::ALT), 0),
                        Resolution::Consumed
                    );
                    assert_eq!(
                        map.feed(context, key(suffix, KeyModifiers::NONE), 1),
                        Resolution::Command(command),
                        "{profile:?} {context:?} {id}"
                    );
                }
                for ch in ['h', 'j', 'k', 'l'] {
                    assert_eq!(
                        map.feed(FocusContext::Terminal, key(ch, KeyModifiers::NONE), 2),
                        Resolution::Forward
                    );
                }
            }
        }
    }
    fn key(ch: char, modifiers: KeyModifiers) -> KeyEvent {
        KeyEvent::new(KeyCode::Char(ch), modifiers)
    }
    #[test]
    fn keymap_web_prefix_and_fake_clock() {
        let mut map = Keymap::compile(&KeymapConfig {
            profile: Some(KeymapProfile::Web),
            ..Default::default()
        })
        .unwrap();
        assert_eq!(
            map.feed(FocusContext::Terminal, key('g', KeyModifiers::ALT), 10),
            Resolution::Consumed
        );
        assert_eq!(
            map.feed(FocusContext::Terminal, key('s', KeyModifiers::NONE), 11),
            Resolution::Command(CommandId::Save)
        );
        assert_eq!(
            map.feed(FocusContext::Editor, key('g', KeyModifiers::ALT), 20),
            Resolution::Consumed
        );
        assert_eq!(
            map.feed(FocusContext::Editor, key('s', KeyModifiers::NONE), 3000),
            Resolution::Consumed
        );
    }
    #[test]
    fn keymap_unknown_commands_are_rejected() {
        let cfg = KeymapConfig {
            bindings: Some(vec![BindingOverride {
                command: "unknown".into(),
                context: FocusContext::Tree,
                keys: vec!["F9".into()],
            }]),
            ..Default::default()
        };
        assert!(Keymap::compile(&cfg).is_err());
    }
    #[test]
    fn keymap_standard_save_label() {
        let map = Keymap::compile(&KeymapConfig::default()).unwrap();
        assert!(map
            .binding_labels(CommandId::Save, FocusContext::Editor)
            .contains(&"Ctrl+S".into()));
    }
    fn override_cfg(command: &str, context: FocusContext, keys: &[&str]) -> KeymapConfig {
        KeymapConfig {
            bindings: Some(vec![BindingOverride {
                command: command.into(),
                context,
                keys: keys.iter().map(|s| s.to_string()).collect(),
            }]),
            ..Default::default()
        }
    }
    #[test]
    fn keymap_override_unbind_and_default_fallback() {
        let mut map = Keymap::compile(&override_cfg(
            "document.save",
            FocusContext::Editor,
            &["F9"],
        ))
        .unwrap();
        assert_eq!(
            map.feed(FocusContext::Editor, key('s', KeyModifiers::CONTROL), 0),
            Resolution::Forward
        );
        assert_eq!(
            map.feed(
                FocusContext::Editor,
                KeyEvent::new(KeyCode::F(9), KeyModifiers::NONE),
                0
            ),
            Resolution::Command(CommandId::Save)
        );
        let map =
            Keymap::compile(&override_cfg("document.save", FocusContext::Editor, &[])).unwrap();
        assert!(map
            .binding_labels(CommandId::Save, FocusContext::Editor)
            .is_empty());
        assert!(!map
            .binding_labels(CommandId::QuickOpen, FocusContext::Tree)
            .is_empty());
    }
    #[test]
    fn keymap_rejects_duplicate_conflicting_and_undeliverable_routes() {
        for keys in [
            vec!["F9", "F9"],
            vec!["Alt+G", "Alt+G s"],
            vec!["g s"],
            vec!["Ctrl+Shift+S"],
            vec!["Super+S"],
            vec!["Ctrl+Alt+S"],
            vec!["F99"],
            vec!["Ctrl+"],
            vec!["Alt+G a b c d"],
        ] {
            assert!(
                Keymap::compile(&override_cfg("document.save", FocusContext::Editor, &keys))
                    .is_err(),
                "{keys:?}"
            );
        }
        let mut cfg = override_cfg("document.save", FocusContext::Editor, &["F9"]);
        cfg.bindings.as_mut().unwrap().push(BindingOverride {
            command: "document.close".into(),
            context: FocusContext::Editor,
            keys: vec!["F9".into()],
        });
        assert!(Keymap::compile(&cfg).is_err());
        let mut cfg = override_cfg("document.save", FocusContext::Editor, &[]);
        let duplicate = cfg.bindings.as_ref().unwrap()[0].clone();
        cfg.bindings.as_mut().unwrap().push(duplicate);
        assert!(Keymap::compile(&cfg).is_err());
    }
    #[test]
    fn keymap_raw_contexts_and_context_reset_do_not_run_suffix() {
        let mut map = Keymap::compile(&KeymapConfig::default()).unwrap();
        for context in [
            FocusContext::Modal,
            FocusContext::EditorFind,
            FocusContext::Menu,
        ] {
            assert_eq!(
                map.feed(context, key('s', KeyModifiers::CONTROL), 0),
                Resolution::Forward
            );
        }
        for key in [
            key('q', KeyModifiers::NONE),
            key('c', KeyModifiers::CONTROL),
            KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE),
            KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE),
        ] {
            assert_eq!(
                map.feed(FocusContext::Terminal, key, 0),
                Resolution::Forward
            );
        }
        assert_eq!(
            map.feed(FocusContext::Editor, key('g', KeyModifiers::ALT), 0),
            Resolution::Consumed
        );
        assert_eq!(
            map.feed(FocusContext::Tree, key('d', KeyModifiers::NONE), 1),
            Resolution::Consumed
        );
        assert_eq!(
            map.feed(FocusContext::Editor, key('g', KeyModifiers::ALT), 2),
            Resolution::Consumed
        );
        assert_eq!(
            map.feed(FocusContext::Editor, key('?', KeyModifiers::NONE), 3),
            Resolution::Consumed
        );
        assert_eq!(
            map.feed(FocusContext::Editor, key('g', KeyModifiers::ALT), 4),
            Resolution::Consumed
        );
        assert_eq!(
            map.feed(
                FocusContext::Editor,
                KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE),
                5
            ),
            Resolution::Consumed
        );
        map.reset();
        assert_eq!(
            map.feed(FocusContext::Editor, key('s', KeyModifiers::NONE), 6),
            Resolution::Forward
        );
    }
    #[test]
    fn keymap_normalizes_legacy_control_uppercase_without_shift_promise() {
        let mut map = Keymap::compile(&KeymapConfig::default()).unwrap();
        assert_eq!(
            map.feed(FocusContext::Editor, key('S', KeyModifiers::CONTROL), 0),
            Resolution::Command(CommandId::Save)
        );
        assert_eq!(
            map.feed(
                FocusContext::Editor,
                key('S', KeyModifiers::CONTROL | KeyModifiers::SHIFT),
                1
            ),
            Resolution::Command(CommandId::Save)
        );
    }
    #[test]
    fn keymap_menu_stable_id_exists() {
        assert!(crate::commands::REGISTRY
            .iter()
            .any(|m| m.id.as_str() == "workspace.commands"));
        for id in [
            "focus.left",
            "focus.right",
            "focus.up",
            "focus.down",
            "focus.cycle",
            "pane.terminal.grow",
            "pane.terminal.shrink",
        ] {
            assert!(crate::commands::REGISTRY
                .iter()
                .any(|m| m.id.as_str() == id));
        }
    }
    #[test]
    fn keymap_repeats_do_not_drop_prefix_and_release_does_not_progress() {
        let mut map = Keymap::compile(&KeymapConfig::default()).unwrap();
        map.feed(FocusContext::Tree, key('g', KeyModifiers::ALT), 0);
        let mut repeat = key('g', KeyModifiers::ALT);
        repeat.kind = crossterm::event::KeyEventKind::Repeat;
        assert_eq!(
            map.feed(FocusContext::Tree, repeat, 1),
            Resolution::Consumed
        );
        let mut release = key('s', KeyModifiers::NONE);
        release.kind = crossterm::event::KeyEventKind::Release;
        assert_eq!(
            map.feed(FocusContext::Tree, release, 2),
            Resolution::Consumed
        );
        assert_eq!(
            map.feed(FocusContext::Tree, key('s', KeyModifiers::NONE), 3),
            Resolution::Command(CommandId::Save)
        );
    }
    #[test]
    fn keymap_shift_lowercase_and_backtab_normalization() {
        assert_eq!(
            Chord::event(key('a', KeyModifiers::SHIFT)),
            Chord::parse("A").unwrap()
        );
        assert_eq!(
            Chord::event(KeyEvent::new(KeyCode::BackTab, KeyModifiers::NONE)),
            Chord::parse("Shift+Tab").unwrap()
        );
    }
    #[test]
    fn keymap_limits_raw_overrides_and_disjoint_contexts() {
        for context in [
            FocusContext::Modal,
            FocusContext::Menu,
            FocusContext::EditorFind,
        ] {
            assert!(Keymap::compile(&override_cfg("document.save", context, &["F9"])).is_err());
        }
        for keys in [
            "",
            "Ctrl+Ctrl+S",
            "Ctrl+H",
            "Ctrl+1",
            "Shift+A",
            "Ctrl+Tab",
            "BadKey",
            "Fbad",
        ] {
            assert!(
                Keymap::compile(&override_cfg(
                    "document.save",
                    FocusContext::Editor,
                    &[keys]
                ))
                .is_err(),
                "{keys}"
            );
        }
        let mut cfg = override_cfg("document.save", FocusContext::Editor, &["F9"]);
        cfg.bindings.as_mut().unwrap().push(BindingOverride {
            command: "document.close".into(),
            context: FocusContext::Tree,
            keys: vec!["F9".into()],
        });
        assert!(Keymap::compile(&cfg).is_ok());
        cfg.bindings = Some(vec![cfg.bindings.unwrap()[0].clone(); 257]);
        assert!(Keymap::compile(&cfg).is_err());
        assert!(Keymap::compile(&override_cfg(
            "document.save",
            FocusContext::Editor,
            &["F9"; 9]
        ))
        .is_err());
        for timeout in [0, 99, 10001, u64::MAX] {
            assert!(Keymap::compile(&KeymapConfig {
                timeout_ms: Some(timeout),
                ..Default::default()
            })
            .is_err());
        }
        assert!(Keymap::compile(&override_cfg(
            "document.save",
            FocusContext::Editor,
            &[&"a".repeat(129)]
        ))
        .is_err());
        assert!(Chord::parse(&"a".repeat(33)).is_err());
    }
    #[test]
    fn keymap_rejects_literal_control_and_unreachable_cancel_suffix() {
        for keys in ["\u{1}", "\u{7}", "F9 Esc"] {
            assert!(
                Keymap::compile(&override_cfg(
                    "document.save",
                    FocusContext::Editor,
                    &[keys]
                ))
                .is_err(),
                "{keys:?}"
            );
        }
    }
    #[test]
    fn keymap_all_portable_named_labels_roundtrip_and_four_chords() {
        for text in [
            "Tab",
            "Shift+Tab",
            "Enter",
            "Esc",
            "Space",
            "Backspace",
            "Delete",
            "Insert",
            "Left",
            "Right",
            "Up",
            "Down",
            "Home",
            "End",
            "PageUp",
            "PageDown",
            "F1",
            "F12",
            "Ctrl+S",
            "Alt+G",
            "A",
        ] {
            let chord = Chord::parse(text).unwrap();
            assert_eq!(Chord::parse(&chord.label()).unwrap(), chord, "{text}");
        }
        let mut map = Keymap::compile(&override_cfg(
            "document.save",
            FocusContext::Editor,
            &["F9 a b c"],
        ))
        .unwrap();
        assert_eq!(
            map.feed(
                FocusContext::Editor,
                KeyEvent::new(KeyCode::F(9), KeyModifiers::NONE),
                0
            ),
            Resolution::Consumed
        );
        assert_eq!(
            map.feed(FocusContext::Editor, key('a', KeyModifiers::NONE), 1),
            Resolution::Consumed
        );
        assert_eq!(
            map.feed(FocusContext::Editor, key('b', KeyModifiers::NONE), 2),
            Resolution::Consumed
        );
        assert_eq!(
            map.feed(FocusContext::Editor, key('c', KeyModifiers::NONE), 3),
            Resolution::Command(CommandId::Save)
        );
    }
    #[test]
    fn keymap_web_essential_workflows_are_browser_safe_sequences() {
        let mut map = Keymap::compile(&KeymapConfig {
            profile: Some(KeymapProfile::Web),
            ..Default::default()
        })
        .unwrap();
        for context in [
            FocusContext::Tree,
            FocusContext::Preview,
            FocusContext::Editor,
            FocusContext::Terminal,
        ] {
            for (command, suffix) in [
                (CommandId::Save, 's'),
                (CommandId::QuickOpen, 'o'),
                (CommandId::Next, 'n'),
                (CommandId::Previous, 'b'),
                (CommandId::Documents, 'd'),
                (CommandId::FocusTree, '1'),
                (CommandId::FocusEditor, '2'),
                (CommandId::FocusTerminal, '3'),
                (CommandId::ToggleTerminal, 't'),
            ] {
                let labels = map.binding_labels(command, context);
                assert!(labels.iter().any(|label| label.starts_with("Alt+G ")));
                for forbidden in ["Ctrl+P", "Ctrl+T", "Ctrl+S", "Ctrl+R"] {
                    assert!(!labels.iter().any(|label| label.contains(forbidden)));
                }
                assert_eq!(
                    map.feed(context, key('g', KeyModifiers::ALT), 0),
                    Resolution::Consumed
                );
                assert_eq!(
                    map.feed(context, key(suffix, KeyModifiers::NONE), 1),
                    Resolution::Command(command)
                );
            }
            for letter in ['p', 't', 's', 'r', 'a', 'e', 'u', 'k', 'w'] {
                assert_eq!(
                    map.feed(context, key(letter, KeyModifiers::CONTROL), 2),
                    Resolution::Forward
                );
            }
        }
    }
}
