use crate::expansion::{Expansion, ExpansionMatcher, SharedSnippetIndex};
#[cfg(not(target_os = "windows"))]
use enigo::{Direction, Enigo, Key as EnigoKey, Keyboard, Settings};
use rdev::{grab, Event, EventType, Key};
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc, Mutex,
};
use std::thread;

#[cfg(target_os = "windows")]
use std::{
    mem::size_of,
    sync::mpsc,
    time::Duration,
};
#[cfg(target_os = "windows")]
use windows_sys::Win32::UI::Input::KeyboardAndMouse::{
    SendInput, INPUT, INPUT_0, INPUT_KEYBOARD, KEYBDINPUT, KEYEVENTF_KEYUP,
    KEYEVENTF_UNICODE, VK_BACK, VK_RETURN, VK_SPACE, VK_TAB,
};

#[cfg(target_os = "windows")]
type ExpansionDispatcher = mpsc::Sender<Expansion>;
#[cfg(not(target_os = "windows"))]
type ExpansionDispatcher = ();

#[cfg(target_os = "windows")]
const SCRIBLET_INPUT_MARKER: usize = 0x5343_5242; // "SCRB"

/// Starts Scriblet's cross-application keyboard binding loop.
///
/// The loop keeps only the current trigger candidate in memory. It does not
/// persist or log keystrokes. When a delimiter completes a known trigger, the
/// delimiter is suppressed, the trigger is erased from the focused app, and
/// the replacement is injected.
///
/// On Windows, injection happens on a dedicated worker thread. Calling
/// SendInput from inside a WH_KEYBOARD_LL callback can cause Scriblet's own
/// synthetic events to be observed after the callback has already cleared its
/// guard. Keeping injection off the hook thread lets the hook continue to see
/// `injecting = true` while Windows dispatches the synthetic events.
pub fn spawn_global_binding(index: SharedSnippetIndex) -> thread::JoinHandle<()> {
    let matcher = Arc::new(Mutex::new(ExpansionMatcher::new(index)));
    let injecting = Arc::new(AtomicBool::new(false));

    #[cfg(target_os = "windows")]
    let dispatcher: ExpansionDispatcher = {
        let (tx, rx) = mpsc::channel::<Expansion>();
        let worker_injecting = Arc::clone(&injecting);

        thread::spawn(move || {
            while let Ok(expansion) = rx.recv() {
                let _ = inject_expansion(&expansion);

                // SendInput returns after inserting the batch, but the low-level
                // hook can still be draining those events. Keep the guard raised
                // briefly so Scriblet never treats its own replacement text as
                // fresh physical typing.
                thread::sleep(Duration::from_millis(25));
                worker_injecting.store(false, Ordering::Release);
            }
        });

        tx
    };

    #[cfg(not(target_os = "windows"))]
    let dispatcher: ExpansionDispatcher = ();

    thread::spawn(move || {
        let callback = move |event: Event| -> Option<Event> {
            if injecting.load(Ordering::Acquire) {
                // Synthetic Scriblet input and any extremely fast physical
                // typing during the replacement are passed through untouched,
                // but are deliberately excluded from the rolling matcher.
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
                    handle_delimiter(event, ' ', &matcher, &injecting, &dispatcher)
                }
                EventType::KeyPress(Key::Tab) => {
                    handle_delimiter(event, '\t', &matcher, &injecting, &dispatcher)
                }
                EventType::KeyPress(Key::Return) => {
                    handle_delimiter(event, '\n', &matcher, &injecting, &dispatcher)
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
    dispatcher: &ExpansionDispatcher,
) -> Option<Event> {
    let expansion = matcher
        .lock()
        .ok()
        .and_then(|mut matcher| matcher.feed_char(delimiter));

    let Some(expansion) = expansion else {
        return Some(event);
    };

    // Suppress only the delimiter that activated a valid binding. The trigger
    // itself has already been typed into the focused application.
    injecting.store(true, Ordering::Release);

    #[cfg(target_os = "windows")]
    {
        if dispatcher.send(expansion).is_err() {
            injecting.store(false, Ordering::Release);
            return Some(event);
        }
    }

    #[cfg(not(target_os = "windows"))]
    {
        let _ = dispatcher;
        let injecting = Arc::clone(injecting);
        thread::spawn(move || {
            let _ = inject_expansion(&expansion);
            injecting.store(false, Ordering::Release);
        });
    }

    None
}

#[cfg(target_os = "windows")]
fn inject_expansion(expansion: &Expansion) -> Result<(), String> {
    let inputs = build_expansion_inputs(expansion);

    if inputs.is_empty() {
        return Ok(());
    }

    let expected = inputs.len() as u32;
    let sent = unsafe { SendInput(expected, inputs.as_ptr(), size_of::<INPUT>() as i32) };

    if sent == expected {
        Ok(())
    } else {
        Err(format!(
            "SendInput submitted {sent} of {expected} events: {}",
            std::io::Error::last_os_error()
        ))
    }
}

#[cfg(target_os = "windows")]
fn build_expansion_inputs(expansion: &Expansion) -> Vec<INPUT> {
    let mut inputs = Vec::with_capacity(
        expansion.backspaces * 2 + expansion.replacement.encode_utf16().count() * 2 + 2,
    );

    for _ in 0..expansion.backspaces {
        push_virtual_key_click(&mut inputs, VK_BACK);
    }

    for unit in expansion.replacement.encode_utf16() {
        inputs.push(keyboard_input(0, unit, KEYEVENTF_UNICODE));
        inputs.push(keyboard_input(
            0,
            unit,
            KEYEVENTF_UNICODE | KEYEVENTF_KEYUP,
        ));
    }

    match expansion.trailing {
        ' ' => push_virtual_key_click(&mut inputs, VK_SPACE),
        '\t' => push_virtual_key_click(&mut inputs, VK_TAB),
        '\n' => push_virtual_key_click(&mut inputs, VK_RETURN),
        other => {
            let mut buffer = [0u16; 2];
            for unit in other.encode_utf16(&mut buffer).iter().copied() {
                inputs.push(keyboard_input(0, unit, KEYEVENTF_UNICODE));
                inputs.push(keyboard_input(
                    0,
                    unit,
                    KEYEVENTF_UNICODE | KEYEVENTF_KEYUP,
                ));
            }
        }
    }

    inputs
}

#[cfg(target_os = "windows")]
fn push_virtual_key_click(inputs: &mut Vec<INPUT>, key: u16) {
    inputs.push(keyboard_input(key, 0, 0));
    inputs.push(keyboard_input(key, 0, KEYEVENTF_KEYUP));
}

#[cfg(target_os = "windows")]
fn keyboard_input(vk: u16, scan: u16, flags: u32) -> INPUT {
    INPUT {
        r#type: INPUT_KEYBOARD,
        Anonymous: INPUT_0 {
            ki: KEYBDINPUT {
                wVk: vk,
                wScan: scan,
                dwFlags: flags,
                time: 0,
                dwExtraInfo: SCRIBLET_INPUT_MARKER,
            },
        },
    }
}

#[cfg(not(target_os = "windows"))]
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

#[cfg(all(test, target_os = "windows"))]
mod windows_tests {
    use super::*;

    #[test]
    fn expansion_batch_contains_complete_edit() {
        let expansion = Expansion {
            backspaces: 4,
            replacement: "testing scriblet".into(),
            trailing: ' ',
        };

        let inputs = build_expansion_inputs(&expansion);
        let replacement_units = expansion.replacement.encode_utf16().count();

        assert_eq!(
            inputs.len(),
            expansion.backspaces * 2 + replacement_units * 2 + 2
        );
    }
}
