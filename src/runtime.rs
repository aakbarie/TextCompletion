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
use std::mem::size_of;
#[cfg(target_os = "windows")]
use windows_sys::Win32::UI::Input::KeyboardAndMouse::{
    SendInput, INPUT, INPUT_0, INPUT_KEYBOARD, KEYBDINPUT, KEYEVENTF_KEYUP,
    KEYEVENTF_UNICODE, VK_BACK, VK_RETURN, VK_SPACE, VK_TAB,
};

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

    // The delimiter that activated the binding is suppressed. On Windows the
    // complete edit is injected synchronously as one SendInput batch so the
    // user's next physical keystroke cannot be interleaved between trigger
    // deletion, replacement text, and the trailing delimiter.
    injecting.store(true, Ordering::Release);

    #[cfg(target_os = "windows")]
    {
        let _ = inject_expansion(&expansion);
        injecting.store(false, Ordering::Release);
    }

    #[cfg(not(target_os = "windows"))]
    {
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
            for unit in other.encode_utf16(&mut [0; 2]).iter().copied() {
                inputs.push(keyboard_input(0, unit, KEYEVENTF_UNICODE));
                inputs.push(keyboard_input(
                    0,
                    unit,
                    KEYEVENTF_UNICODE | KEYEVENTF_KEYUP,
                ));
            }
        }
    }

    if inputs.is_empty() {
        return Ok(());
    }

    let expected = inputs.len() as u32;
    let sent = unsafe {
        SendInput(
            expected,
            inputs.as_ptr(),
            size_of::<INPUT>() as i32,
        )
    };

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
                dwExtraInfo: 0,
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
