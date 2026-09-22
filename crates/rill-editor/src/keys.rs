//! Preserve native navigation, adding only Rill submission and menu actions.
use reedline::{
    EditCommand, Emacs, KeyCode, KeyModifiers, ReedlineEvent, default_emacs_keybindings,
};

pub fn bindings() -> Emacs {
    let mut keys = default_emacs_keybindings();
    keys.add_binding(
        KeyModifiers::NONE,
        KeyCode::Tab,
        ReedlineEvent::UntilFound(vec![
            ReedlineEvent::Menu("completion".into()),
            ReedlineEvent::MenuNext,
        ]),
    );
    keys.add_binding(
        KeyModifiers::SHIFT,
        KeyCode::BackTab,
        ReedlineEvent::MenuPrevious,
    );
    keys.add_binding(
        KeyModifiers::CONTROL,
        KeyCode::Char('j'),
        ReedlineEvent::Edit(vec![EditCommand::InsertNewline]),
    );
    keys.add_binding(
        KeyModifiers::ALT,
        KeyCode::Enter,
        ReedlineEvent::Edit(vec![EditCommand::InsertNewline]),
    );
    keys.add_binding(
        KeyModifiers::CONTROL,
        KeyCode::Char('_'),
        ReedlineEvent::Edit(vec![EditCommand::Undo]),
    );
    keys.add_binding(
        KeyModifiers::ALT,
        KeyCode::Char('r'),
        ReedlineEvent::Edit(vec![EditCommand::Redo]),
    );
    keys.add_binding(
        KeyModifiers::CONTROL,
        KeyCode::Char('z'),
        ReedlineEvent::ExecuteHostCommand("suspend".into()),
    );
    keys.add_binding(
        KeyModifiers::NONE,
        KeyCode::F(1),
        ReedlineEvent::ExecuteHostCommand("help".into()),
    );
    keys.add_binding(
        KeyModifiers::CONTROL,
        KeyCode::Char('o'),
        ReedlineEvent::ExecuteHostCommand("edit".into()),
    );
    Emacs::new(keys)
}
