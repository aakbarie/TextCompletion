use crate::expansion::{Expansion, ExpansionMatcher, SharedSnippetIndex};
use enigo::{Direction, Enigo, Key as EnigoKey, Keyboard, Settings};
use rdev::{grab, Event, EventType, Key};
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc, Mutex,
};
use std::thread;

/// Starts Scriblet's cross-application keyboard binding loop.
///
/// The loop keeps only the current trigger candidate in memory. It does not
/// persist or log keystrokes. When a delimiter completes a known trigger, the
/// delimiter is suppressed, the trigger is erased from the focused app, and
/// the replacement is injected.
pub fn spawn_global_binding(index: SharedSnippetIndex) -> thread::JoinHandle<()> {
    let matcher = Arc::new(Mutex::new(ExpansionMatcher::new(index)));
    let injecting = Arc::new(AtomicBool::new(false));

    thread::spawn(move || {
        let callback = move |event: Event| -> Option<Event> {
            if injecting.load(Ordering::Acquire) {
                return Some(event);
            }

            match event.event_type {
                EventType::KeyPress(Key::Backspace) => {
                    if let Ok(mut matcher) = matcher.lock() {
                        matcher.backspace();
                    }
                    Some(event)
                }
                EventType::KeyPress(Key::Space) => {
                    handle_delimiter(event, ' ', &matcher, &injecting)
                }
                EventType::KeyPress(Key::Tab) => {
                    handle_delimiter(event, '\t', &matcher, &injecting)
                }
                EventType::KeyPress(Key::Return) => {
                    handle_delimiter(event, '\n', &matcher, &injecting)
                }
                EventType::KeyPress(_) => {
                    if let Some(name) = event.name.as_deref() {
                        let mut chars = name.chars();
                        if let (Some(ch), None) = (chars.next(), chars.next()) {
                            if !ch.is_control() {
                                if let Ok(mut matcher) = matcher.lock() {
                                    let _ = matcher.feed_char(ch);
                                }
                            }
                        }
                    }
                    Some(event)
                }
                _ => Some(event),
            }
        };

        // macOS requires Accessibility permission for this event tap. Windows
        // uses a system-wide low-level keyboard hook through rdev.
        let _ = grab(callback);
    })
}

fn handle_delimiter(
    event: Event,
    delimiter: char,
    matcher: &Arc<Mutex<ExpansionMatcher>>,
    injecting: &Arc<AtomicBool>,
) -> Option<Event> {
    let expansion = matcher
        .lock()
        .ok()
        .and_then(|mut matcher| matcher.feed_char(delimiter));

    let Some(expansion) = expansion else {
        return Some(event);
    };

    // Suppress only the delimiter that activated a valid Scriblet binding.
    // The trigger itself has already been typed into the focused application.
    injecting.store(true, Ordering::Release);
    let injecting = Arc::clone(injecting);
    thread::spawn(move || {
        let _ = inject_expansion(&expansion);
        injecting.store(false, Ordering::Release);
    });

    None
}

fn inject_expansion(expansion: &Expansion) -> Result<(), String> {
    let mut enigo = Enigo::new(&Settings::default()).map_err(|error| error.to_string())?;

    for _ in 0..expansion.backspaces {
        enigo
            .key(EnigoKey::Backspace, Direction::Click)
            .map_err(|error| error.to_string())?;
    }

    enigo
        .text(&expansion.replacement)
        .map_err(|error| error.to_string())?;

    match expansion.trailing {
        ' ' => enigo.key(EnigoKey::Space, Direction::Click),
        '\t' => enigo.key(EnigoKey::Tab, Direction::Click),
        '\n' => enigo.key(EnigoKey::Return, Direction::Click),
        other => enigo.text(&other.to_string()),
    }
    .map_err(|error| error.to_string())?;

    Ok(())
}
