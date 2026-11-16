//! Shared production bindings for desktop startup and interaction tests.
use super::*;
use crate::platform::shortcut;

pub fn playback_bindings() -> [KeyBinding; 2] {
    [
        KeyBinding::new("space", TogglePlayback, Some("Sonora")),
        KeyBinding::new("space", NoAction, Some("Input")),
    ]
}

pub fn queue_bindings() -> [KeyBinding; 11] {
    [
        KeyBinding::new(shortcut("cmd-z", "ctrl-z"), UndoQueueEdit, Some("Queue")),
        KeyBinding::new("backspace", RemoveQueueSelection, Some("Queue")),
        KeyBinding::new("delete", RemoveQueueSelection, Some("Queue")),
        KeyBinding::new("alt-up", MoveQueueEarlier, Some("Queue")),
        KeyBinding::new("alt-down", MoveQueueLater, Some("Queue")),
        KeyBinding::new(shortcut("cmd-a", "ctrl-a"), SelectAllQueued, Some("Queue")),
        KeyBinding::new("up", PreviousQueueEntry, Some("Queue")),
        KeyBinding::new("down", NextQueueEntry, Some("Queue")),
        KeyBinding::new("shift-up", ExtendQueuePrevious, Some("Queue")),
        KeyBinding::new("shift-down", ExtendQueueNext, Some("Queue")),
        KeyBinding::new("enter", PlayQueueSelection, Some("Queue")),
    ]
}

/// Shared settings shortcut used by the application and integration inputs.
pub fn settings_bindings() -> [KeyBinding; 1] {
    [KeyBinding::new(
        shortcut("cmd-,", "ctrl-,"),
        OpenSettings,
        Some("Sonora"),
    )]
}
