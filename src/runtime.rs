use crate::expansion::{Expansion, ExpansionMatcher, SharedSnippetIndex};
use std::sync::{Arc, Mutex};
use std::thread;

#[cfg(target_os = "macos")]
use enigo::{Direction, Enigo, Key as EnigoKey, Keyboard, Settings};
#[cfg(target_os = "macos")]
use rdev::{grab, Event, EventType, Key};
#[cfg(target_os = "macos")]
use std::sync::atomic::{AtomicBool, Ordering};

#[cfg(target_os = "windows")]
use std::{
    mem::size_of,
    ptr::null_mut,
    sync::{mpsc, OnceLock},
};
#[cfg(target_os = "windows")]
use windows_sys::Win32::UI::Input::KeyboardAndMouse::{
    SendInput, INPUT, INPUT_0, INPUT_KEYBOARD, KEYBDINPUT, KEYEVENTF_KEYUP,
    KEYEVENTF_UNICODE, VK_BACK, VK_RETURN, VK_SHIFT, VK_SPACE, VK_TAB,
};
#[cfg(target_os = "windows")]
use windows_sys::Win32::UI::WindowsAndMessaging::{
    CallNextHookEx, GetMessageW, KBDLLHOOKSTRUCT, LLKHF_INJECTED, MSG,
    SetWindowsHookExW, WH_KEYBOARD_LL, WM_KEYDOWN, WM_KEYUP, WM_SYSKEYDOWN,
    WM_SYSKEYUP,
};

#[cfg(target_os = "windows")]
const SCRIBLET_INPUT_MARKER: usize = 0x5343_5242; // "SCRB"

#[cfg(target_os = "windows")]
struct WindowsHookState {
    matcher: ExpansionMatcher,
    expansion_tx: mpsc::Sender<Expansion>,
    shift_down: bool,
    suppress_keyup: Option<u32>,
}

#[cfg(target_os = "windows")]
static WINDOWS_HOOK_STATE: OnceLock<Mutex<WindowsHookState>> = OnceLock::new();

/// Starts Scriblet's cross-application keyboard binding loop.
///
/// Windows uses a native WH_KEYBOARD_LL hook rather than rdev. This is
/// intentional: rdev derives `Event.name` inside the low-level hook using the
/// ambient Windows keyboard state, which can lag the current low-level event.
/// Scriblet only needs a small ASCII trigger alphabet, so Windows decodes the
/// current vkCode directly and ignores injected events explicitly.
pub fn spawn_global_binding(index: SharedSnippetIndex) -> thread::JoinHandle<()> {
    #[cfg(target_os = "windows")]
    {
        return spawn_windows_binding(index);
    }

    #[cfg(target_os = "macos")]
    {
        return spawn_macos_binding(index);
    }
}

#[cfg(target_os = "windows")]
fn spawn_windows_binding(index: SharedSnippetIndex) -> thread::JoinHandle<()> {
    let (expansion_tx, expansion_rx) = mpsc::channel::<Expansion>();

    thread::spawn(move || {
        while let Ok(expansion) = expansion_rx.recv() {
            let _ = inject_expansion_windows(&expansion);
        }
    });

    let _ = WINDOWS_HOOK_STATE.set(Mutex::new(WindowsHookState {
        matcher: ExpansionMatcher::new(index),
        expansion_tx,
        shift_down: false,
        suppress_keyup: None,
    }));

    thread::spawn(move || unsafe {
        let hook = SetWindowsHookExW(WH_KEYBOARD_LL, Some(windows_keyboard_proc), null_mut(), 0);
        if hook.is_null() {
            return;
        }

        let mut message: MSG = std::mem::zeroed();
        while GetMessageW(&mut message, null_mut(), 0, 0) > 0 {}
    })
}

#[cfg(target_os = "windows")]
unsafe extern "system" fn windows_keyboard_proc(code: i32, wparam: usize, lparam: isize) -> isize {
    if code < 0 {
        return CallNextHookEx(null_mut(), code, wparam, lparam);
    }

    let event = &*(lparam as *const KBDLLHOOKSTRUCT);

    // Never feed Scriblet-generated SendInput events back into the matcher.
    if (event.flags & LLKHF_INJECTED as u32) != 0 || event.dwExtraInfo == SCRIBLET_INPUT_MARKER {
        return CallNextHookEx(null_mut(), code, wparam, lparam);
    }

    let Some(state_mutex) = WINDOWS_HOOK_STATE.get() else {
        return CallNextHookEx(null_mut(), code, wparam, lparam);
    };
    let Ok(mut state) = state_mutex.lock() else {
        return CallNextHookEx(null_mut(), code, wparam, lparam);
    };

    let message = wparam as u32;
    let vk = event.vkCode;

    if message == WM_KEYUP || message == WM_SYSKEYUP {
        if vk == VK_SHIFT as u32 {
            state.shift_down = false;
        }

        if state.suppress_keyup == Some(vk) {
            state.suppress_keyup = None;
            return 1;
        }

        return CallNextHookEx(null_mut(), code, wparam, lparam);
    }

    if message != WM_KEYDOWN && message != WM_SYSKEYDOWN {
        return CallNextHookEx(null_mut(), code, wparam, lparam);
    }

    if vk == VK_SHIFT as u32 {
        state.shift_down = true;
        return CallNextHookEx(null_mut(), code, wparam, lparam);
    }

    if vk == VK_BACK as u32 {
        state.matcher.backspace();
        return CallNextHookEx(null_mut(), code, wparam, lparam);
    }

    let delimiter = if vk == VK_SPACE as u32 {
        Some(' ')
    } else if vk == VK_TAB as u32 {
        Some('\t')
    } else if vk == VK_RETURN as u32 {
        Some('\n')
    } else {
        None
    };

    if let Some(delimiter) = delimiter {
        if let Some(expansion) = state.matcher.feed_char(delimiter) {
            if state.expansion_tx.send(expansion).is_ok() {
                state.suppress_keyup = Some(vk);
                return 1;
            }
        }
        return CallNextHookEx(null_mut(), code, wparam, lparam);
    }

    if let Some(ch) = decode_ascii_vk(vk, state.shift_down) {
        let _ = state.matcher.feed_char(ch);
    } else {
        // Unknown keys break the trigger candidate rather than risking a
        // false match against stale text.
        state.matcher.reset();
    }

    CallNextHookEx(null_mut(), code, wparam, lparam)
}

#[cfg(target_os = "windows")]
fn decode_ascii_vk(vk: u32, shift: bool) -> Option<char> {
    match vk {
        0x41..=0x5A => {
            let ch = char::from_u32(vk)?;
            Some(if shift { ch } else { ch.to_ascii_lowercase() })
        }
        0x30..=0x39 => {
            let plain = char::from_u32(vk)?;
            if !shift {
                return Some(plain);
            }
            Some(match plain {
                '0' => ')',
                '1' => '!',
                '2' => '@',
                '3' => '#',
                '4' => '$',
                '5' => '%',
                '6' => '^',
                '7' => '&',
                '8' => '*',
                '9' => '(',
                _ => return None,
            })
        }
        0xBA => Some(if shift { ':' } else { ';' }),
        0xBB => Some(if shift { '+' } else { '=' }),
        0xBC => Some(if shift { '<' } else { ',' }),
        0xBD => Some(if shift { '_' } else { '-' }),
        0xBE => Some(if shift { '>' } else { '.' }),
        0xBF => Some(if shift { '?' } else { '/' }),
        0xC0 => Some(if shift { '~' } else { '`' }),
        0xDB => Some(if shift { '{' } else { '[' }),
        0xDC => Some(if shift { '|' } else { '\\' }),
        0xDD => Some(if shift { '}' } else { ']' }),
        0xDE => Some(if shift { '"' } else { '\'' }),
        _ => None,
    }
}

#[cfg(target_os = "windows")]
fn inject_expansion_windows(expansion: &Expansion) -> Result<(), String> {
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
        push_virtual_key_click(&mut inputs, VK_BACK as u16);
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
        ' ' => push_virtual_key_click(&mut inputs, VK_SPACE as u16),
        '\t' => push_virtual_key_click(&mut inputs, VK_TAB as u16),
        '\n' => push_virtual_key_click(&mut inputs, VK_RETURN as u16),
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

#[cfg(target_os = "macos")]
fn spawn_macos_binding(index: SharedSnippetIndex) -> thread::JoinHandle<()> {
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
                    handle_macos_delimiter(event, ' ', &matcher, &injecting)
                }
                EventType::KeyPress(Key::Tab) => {
                    handle_macos_delimiter(event, '\t', &matcher, &injecting)
                }
                EventType::KeyPress(Key::Return) => {
                    handle_macos_delimiter(event, '\n', &matcher, &injecting)
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

        let _ = grab(callback);
    })
}

#[cfg(target_os = "macos")]
fn handle_macos_delimiter(
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

    injecting.store(true, Ordering::Release);
    let injecting = Arc::clone(injecting);
    thread::spawn(move || {
        let _ = inject_expansion_macos(&expansion);
        injecting.store(false, Ordering::Release);
    });

    None
}

#[cfg(target_os = "macos")]
fn inject_expansion_macos(expansion: &Expansion) -> Result<(), String> {
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
    fn ascii_decoder_tracks_current_key_not_previous_key() {
        assert_eq!(decode_ascii_vk(0x42, false), Some('b'));
        assert_eq!(decode_ascii_vk(0x35, false), Some('5'));
        assert_eq!(decode_ascii_vk(0xBA, false), Some(';'));
        assert_eq!(decode_ascii_vk(0xBF, false), Some('/'));
    }

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
