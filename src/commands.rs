#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum CommandId {
    NewNote,
    QuickOpen,
    Complete,
    Rewrite,
    Summarize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommandSpec {
    pub id: CommandId,
    pub label: &'static str,
    pub shortcut: &'static str,
    pub chord: Option<char>,
    pub enabled_in_phase_2a: bool,
}

pub fn command_registry() -> &'static [CommandSpec] {
    &[
        CommandSpec {
            id: CommandId::NewNote,
            label: "New note",
            shortcut: "Cmd/Ctrl+N",
            chord: None,
            enabled_in_phase_2a: true,
        },
        CommandSpec {
            id: CommandId::QuickOpen,
            label: "Quick open / command palette",
            shortcut: "Cmd/Ctrl+P",
            chord: None,
            enabled_in_phase_2a: true,
        },
        CommandSpec {
            id: CommandId::Complete,
            label: "Complete sentence",
            shortcut: "Cmd/Ctrl+K, C",
            chord: Some('c'),
            enabled_in_phase_2a: true,
        },
        CommandSpec {
            id: CommandId::Rewrite,
            label: "Rewrite selection",
            shortcut: "Cmd/Ctrl+K, R",
            chord: Some('r'),
            enabled_in_phase_2a: false,
        },
        CommandSpec {
            id: CommandId::Summarize,
            label: "Summarize selection",
            shortcut: "Cmd/Ctrl+K, S",
            chord: Some('s'),
            enabled_in_phase_2a: false,
        },
    ]
}

pub fn command_for_chord(key: char) -> Option<CommandId> {
    let normalized = key.to_ascii_lowercase();
    command_registry()
        .iter()
        .find(|spec| spec.chord == Some(normalized) && spec.enabled_in_phase_2a)
        .map(|spec| spec.id)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn completion_chord_is_discoverable_and_enabled() {
        assert_eq!(command_for_chord('C'), Some(CommandId::Complete));
        assert_eq!(command_for_chord('r'), None);
        assert!(command_registry().iter().any(|c| c.shortcut == "Cmd/Ctrl+K, C"));
    }
}