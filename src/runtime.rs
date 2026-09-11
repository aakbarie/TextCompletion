//! Global keyboard binding: watches typed text in every application, detects
//! a trigger followed by a delimiter, and replaces it with the rendered phrase.
//!
//! Windows uses a native `WH_KEYBOARD_LL` hook rather than rdev. rdev derives
//! `Event.name` inside the low-level hook using the ambient keyboard state,
//! which can lag the current event. Scriblet only needs a small ASCII trigger
//! alphabet, so Windows decodes the current vkCode directly and ignores
//! injected events explicitly.

use crate::expansion::{Expansion, SharedSnippetIndex};
use crate::template::{cursor_left_presses, render_now, RenderedTemplate};
use std::sync::atomic::AtomicBool;
use std::sync::Arc;
use std::thread;

/// Reported once by the platform hook when it starts, and again if it stops.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RuntimeStatus {
    /// The hook is installed and expansions will fire.
    Active,
    /// macOS Accessibility permission has not been granted yet.
    PermissionRequired,
    /// The hook could not be installed or was lost.
    Failed(String),
    /// Global expansion is not implemented for this operating system.
    Unsupported,
}

impl RuntimeStatus {
    pub fn label(&self) -> String {
        match self {
            Self::Active => "Expansion active".to_string(),
            Self::PermissionRequired => {
                "Grant Accessibility permission to enable expansion".to_string()
            }
            Self::Failed(reason) => format!("Expansion unavailable: {reason}"),
            Self::Unsupported => "Expansion is not available on this platform".to_string(),
        }
    }
}

pub type StatusCallback = Arc<dyn Fn(RuntimeStatus) + Send + Sync>;

/// Shared pause switch. When set, keystrokes pass through untouched.
pub type PauseFlag = Arc<AtomicBool>;

/// Starts Scriblet's cross-application keyboard binding loop on a background
/// thread. `status` is invoked from that thread.
pub fn spawn_global_binding(
    index: SharedSnippetIndex,
    paused: PauseFlag,
    status: StatusCallback,
) -> thread::JoinHandle<()> {
    imp::spawn(index, paused, status)
}

/// Everything the platform layer needs to type one expansion.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InjectionPlan {
    pub backspaces: usize,
    pub text: String,
    pub trailing: char,
    pub left_presses: usize,
}

/// Renders the template right before typing, so `{{date}}` and
/// `{{clipboard}}` reflect the moment of expansion.
pub fn plan_injection(expansion: &Expansion) -> InjectionPlan {
    plan_from_rendered(expansion, render_now(&expansion.replacement))
}

fn plan_from_rendered(expansion: &Expansion, rendered: RenderedTemplate) -> InjectionPlan {
    InjectionPlan {
        backspaces: expansion.backspaces,
        left_presses: cursor_left_presses(&rendered, true),
        text: rendered.text,
        trailing: expansion.trailing,
    }
}

#[cfg(target_os = "windows")]
mod imp {
    use super::*;
    use crate::expansion::ExpansionMatcher;
    use std::mem::size_of;
    use std::ptr::null_mut;
    use std::sync::atomic::Ordering;
    use std::sync::{mpsc, Mutex, OnceLock};
    use windows_sys::Win32::UI::Input::KeyboardAndMouse::{
        GetKeyState, SendInput, INPUT, INPUT_0, INPUT_KEYBOARD, KEYBDINPUT, KEYEVENTF_KEYUP,
        KEYEVENTF_UNICODE, VK_BACK, VK_CAPITAL, VK_CONTROL, VK_LCONTROL, VK_LEFT, VK_LMENU,
        VK_LSHIFT, VK_LWIN, VK_MENU, VK_RCONTROL, VK_RETURN, VK_RMENU, VK_RSHIFT, VK_RWIN,
        VK_SHIFT, VK_SPACE, VK_TAB,
    };
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        CallNextHookEx, GetMessageW, SetWindowsHookExW, KBDLLHOOKSTRUCT, LLKHF_INJECTED, MSG,
        WH_KEYBOARD_LL, WM_KEYDOWN, WM_KEYUP, WM_SYSKEYDOWN, WM_SYSKEYUP,
    };

    const SCRIBLET_INPUT_MARKER: usize = 0x5343_5242; // "SCRB"

    /// Keyboard state tracked from the raw event stream. Low-level hooks
    /// report left/right specific virtual keys for modifiers, so both sides
    /// are tracked and the generic codes are accepted defensively.
    #[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
    pub(super) struct Modifiers {
        pub left_shift: bool,
        pub right_shift: bool,
        pub control: bool,
        pub alt: bool,
        pub win: bool,
        pub caps_lock: bool,
        caps_key_held: bool,
    }

    impl Modifiers {
        pub fn shift(&self) -> bool {
            self.left_shift || self.right_shift
        }

        /// Any modifier that turns a letter into a shortcut rather than text.
        pub fn shortcut(&self) -> bool {
            self.control || self.alt || self.win
        }

        /// Applies a key event. Returns `true` if the key was a modifier and
        /// needs no further processing.
        pub fn apply(&mut self, vk: u32, down: bool) -> bool {
            let vk = vk as u16;
            match vk {
                VK_LSHIFT | VK_SHIFT => self.left_shift = down,
                VK_RSHIFT => self.right_shift = down,
                VK_LCONTROL | VK_RCONTROL | VK_CONTROL => self.control = down,
                VK_LMENU | VK_RMENU | VK_MENU => self.alt = down,
                VK_LWIN | VK_RWIN => self.win = down,
                VK_CAPITAL => {
                    if down && !self.caps_key_held {
                        self.caps_lock = !self.caps_lock;
                    }
                    self.caps_key_held = down;
                }
                _ => return false,
            }
            true
        }
    }

    struct HookState {
        matcher: ExpansionMatcher,
        expansion_tx: mpsc::Sender<Expansion>,
        modifiers: Modifiers,
        suppress_keyup: Option<u32>,
        paused: PauseFlag,
    }

    static HOOK_STATE: OnceLock<Mutex<HookState>> = OnceLock::new();

    pub fn spawn(
        index: SharedSnippetIndex,
        paused: PauseFlag,
        status: StatusCallback,
    ) -> thread::JoinHandle<()> {
        let (expansion_tx, expansion_rx) = mpsc::channel::<Expansion>();

        thread::spawn(move || {
            while let Ok(expansion) = expansion_rx.recv() {
                let plan = plan_injection(&expansion);
                if let Err(error) = inject(&plan) {
                    log::warn!("expansion injection failed: {error}");
                }
            }
        });

        let modifiers = Modifiers {
            caps_lock: unsafe { GetKeyState(VK_CAPITAL as i32) } & 1 != 0,
            ..Modifiers::default()
        };

        if HOOK_STATE
            .set(Mutex::new(HookState {
                matcher: ExpansionMatcher::new(index),
                expansion_tx,
                modifiers,
                suppress_keyup: None,
                paused,
            }))
            .is_err()
        {
            log::warn!("keyboard hook state was already initialised");
        }

        thread::spawn(move || unsafe {
            let hook = SetWindowsHookExW(WH_KEYBOARD_LL, Some(keyboard_proc), null_mut(), 0);
            if hook.is_null() {
                let error = std::io::Error::last_os_error();
                log::error!("SetWindowsHookExW failed: {error}");
                status(RuntimeStatus::Failed(error.to_string()));
                return;
            }
            log::info!("low-level keyboard hook installed");
            status(RuntimeStatus::Active);

            let mut message: MSG = std::mem::zeroed();
            while GetMessageW(&mut message, null_mut(), 0, 0) > 0 {}
            status(RuntimeStatus::Failed(
                "keyboard hook loop ended".to_string(),
            ));
        })
    }

    unsafe extern "system" fn keyboard_proc(code: i32, wparam: usize, lparam: isize) -> isize {
        if code < 0 {
            return CallNextHookEx(null_mut(), code, wparam, lparam);
        }

        let event = &*(lparam as *const KBDLLHOOKSTRUCT);

        // Never feed Scriblet-generated SendInput events back into the matcher.
        if (event.flags & LLKHF_INJECTED) != 0 || event.dwExtraInfo == SCRIBLET_INPUT_MARKER {
            return CallNextHookEx(null_mut(), code, wparam, lparam);
        }

        let Some(state_mutex) = HOOK_STATE.get() else {
            return CallNextHookEx(null_mut(), code, wparam, lparam);
        };
        let Ok(mut state) = state_mutex.lock() else {
            return CallNextHookEx(null_mut(), code, wparam, lparam);
        };

        let message = wparam as u32;
        let vk = event.vkCode;
        let down = message == WM_KEYDOWN || message == WM_SYSKEYDOWN;
        let up = message == WM_KEYUP || message == WM_SYSKEYUP;

        if state.modifiers.apply(vk, down) {
            if down && state.modifiers.shortcut() {
                state.matcher.reset();
            }
            return CallNextHookEx(null_mut(), code, wparam, lparam);
        }

        if state.paused.load(Ordering::Relaxed) {
            state.matcher.reset();
            return CallNextHookEx(null_mut(), code, wparam, lparam);
        }

        if up {
            if state.suppress_keyup == Some(vk) {
                state.suppress_keyup = None;
                return 1;
            }
            return CallNextHookEx(null_mut(), code, wparam, lparam);
        }

        if !down {
            return CallNextHookEx(null_mut(), code, wparam, lparam);
        }

        if state.modifiers.shortcut() {
            // Ctrl/Alt/Win chords are commands, not text.
            state.matcher.reset();
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

        let modifiers = state.modifiers;
        if let Some(ch) = decode_vk(vk, &modifiers) {
            let _ = state.matcher.feed_char(ch);
        } else {
            // Unknown keys break the trigger candidate rather than risking a
            // false match against stale text.
            state.matcher.reset();
        }

        CallNextHookEx(null_mut(), code, wparam, lparam)
    }

    /// Decodes a virtual key on a US layout. Letters honour Shift and Caps
    /// Lock; digits and punctuation honour Shift only.
    pub(super) fn decode_vk(vk: u32, modifiers: &Modifiers) -> Option<char> {
        let shift = modifiers.shift();
        match vk {
            0x41..=0x5A => {
                let ch = char::from_u32(vk)?;
                let upper = shift ^ modifiers.caps_lock;
                Some(if upper { ch } else { ch.to_ascii_lowercase() })
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
            0x60..=0x69 => char::from_u32('0' as u32 + (vk - 0x60)),
            0x6A => Some('*'),
            0x6B => Some('+'),
            0x6D => Some('-'),
            0x6E => Some('.'),
            0x6F => Some('/'),
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

    fn inject(plan: &InjectionPlan) -> Result<(), String> {
        let inputs = build_inputs(plan);
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

    /// One SendInput batch: erase the trigger, type the phrase, re-insert the
    /// delimiter, then walk the caret back to `{{cursor}}`.
    pub(super) fn build_inputs(plan: &InjectionPlan) -> Vec<INPUT> {
        let mut inputs = Vec::with_capacity(
            plan.backspaces * 2 + plan.text.encode_utf16().count() * 2 + 2 + plan.left_presses * 2,
        );

        for _ in 0..plan.backspaces {
            push_virtual_key_click(&mut inputs, VK_BACK);
        }

        for unit in plan.text.encode_utf16() {
            push_unicode_unit(&mut inputs, unit);
        }

        match plan.trailing {
            ' ' => push_virtual_key_click(&mut inputs, VK_SPACE),
            '\t' => push_virtual_key_click(&mut inputs, VK_TAB),
            '\n' => push_virtual_key_click(&mut inputs, VK_RETURN),
            other => {
                let mut buffer = [0u16; 2];
                for unit in other.encode_utf16(&mut buffer).iter().copied() {
                    push_unicode_unit(&mut inputs, unit);
                }
            }
        }

        for _ in 0..plan.left_presses {
            push_virtual_key_click(&mut inputs, VK_LEFT);
        }

        inputs
    }

    fn push_unicode_unit(inputs: &mut Vec<INPUT>, unit: u16) {
        inputs.push(keyboard_input(0, unit, KEYEVENTF_UNICODE));
        inputs.push(keyboard_input(0, unit, KEYEVENTF_UNICODE | KEYEVENTF_KEYUP));
    }

    fn push_virtual_key_click(inputs: &mut Vec<INPUT>, key: u16) {
        inputs.push(keyboard_input(key, 0, 0));
        inputs.push(keyboard_input(key, 0, KEYEVENTF_KEYUP));
    }

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

    #[cfg(test)]
    mod tests {
        use super::*;
        use crate::template::render_template;

        #[test]
        fn decoder_tracks_current_key_not_previous_key() {
            let plain = Modifiers::default();
            assert_eq!(decode_vk(0x42, &plain), Some('b'));
            assert_eq!(decode_vk(0x35, &plain), Some('5'));
            assert_eq!(decode_vk(0xBA, &plain), Some(';'));
            assert_eq!(decode_vk(0xBF, &plain), Some('/'));
        }

        #[test]
        fn left_and_right_shift_both_uppercase_letters_and_shift_symbols() {
            let mut left = Modifiers::default();
            assert!(left.apply(VK_LSHIFT as u32, true));
            let mut right = Modifiers::default();
            assert!(right.apply(VK_RSHIFT as u32, true));
            for modifiers in [left, right] {
                assert_eq!(decode_vk(0x53, &modifiers), Some('S'));
                assert_eq!(decode_vk(0xBA, &modifiers), Some(':'));
                assert_eq!(decode_vk(0x31, &modifiers), Some('!'));
            }
            assert!(left.apply(VK_LSHIFT as u32, false));
            assert_eq!(decode_vk(0x53, &left), Some('s'));
        }

        #[test]
        fn caps_lock_toggles_once_per_press_and_combines_with_shift() {
            let mut modifiers = Modifiers::default();
            modifiers.apply(VK_CAPITAL as u32, true);
            modifiers.apply(VK_CAPITAL as u32, true); // auto-repeat while held
            assert!(modifiers.caps_lock);
            assert_eq!(decode_vk(0x41, &modifiers), Some('A'));
            assert_eq!(decode_vk(0xBA, &modifiers), Some(';'));

            modifiers.apply(VK_LSHIFT as u32, true);
            assert_eq!(decode_vk(0x41, &modifiers), Some('a'));
            assert_eq!(decode_vk(0xBA, &modifiers), Some(':'));

            modifiers.apply(VK_CAPITAL as u32, false);
            modifiers.apply(VK_CAPITAL as u32, true);
            assert!(!modifiers.caps_lock);
        }

        #[test]
        fn control_alt_and_win_count_as_shortcuts() {
            for vk in [
                VK_LCONTROL,
                VK_RCONTROL,
                VK_LMENU,
                VK_RMENU,
                VK_LWIN,
                VK_RWIN,
            ] {
                let mut modifiers = Modifiers::default();
                assert!(modifiers.apply(vk as u32, true));
                assert!(modifiers.shortcut());
                modifiers.apply(vk as u32, false);
                assert!(!modifiers.shortcut());
            }
        }

        #[test]
        fn numpad_digits_decode() {
            let plain = Modifiers::default();
            assert_eq!(decode_vk(0x60, &plain), Some('0'));
            assert_eq!(decode_vk(0x69, &plain), Some('9'));
            assert_eq!(decode_vk(0x6E, &plain), Some('.'));
        }

        #[test]
        fn input_batch_contains_complete_edit_and_cursor_moves() {
            let expansion = Expansion {
                backspaces: 4,
                replacement: "Decision: {{cursor}} because".into(),
                trailing: ' ',
            };
            let plan = plan_from_rendered(
                &expansion,
                render_template(&expansion.replacement, None, None),
            );
            assert_eq!(plan.text, "Decision:  because");
            assert_eq!(plan.left_presses, " because".len() + 1);

            let inputs = build_inputs(&plan);
            let units = plan.text.encode_utf16().count();
            assert_eq!(inputs.len(), 4 * 2 + units * 2 + 2 + plan.left_presses * 2);
        }
    }
}

#[cfg(target_os = "macos")]
mod imp {
    use super::*;
    use crate::expansion::ExpansionMatcher;
    use enigo::{Direction, Enigo, Key as EnigoKey, Keyboard, Settings};
    use rdev::{grab, Event, EventType, Key};
    use std::sync::atomic::Ordering;
    use std::sync::Mutex;

    #[derive(Default)]
    struct Modifiers {
        control: bool,
        alt: bool,
        meta: bool,
    }

    impl Modifiers {
        fn shortcut(&self) -> bool {
            self.control || self.alt || self.meta
        }

        fn apply(&mut self, key: Key, down: bool) -> bool {
            match key {
                Key::ControlLeft | Key::ControlRight => self.control = down,
                Key::Alt | Key::AltGr => self.alt = down,
                Key::MetaLeft | Key::MetaRight => self.meta = down,
                _ => return false,
            }
            true
        }
    }

    pub fn spawn(
        index: SharedSnippetIndex,
        paused: PauseFlag,
        status: StatusCallback,
    ) -> thread::JoinHandle<()> {
        let matcher = Arc::new(Mutex::new(ExpansionMatcher::new(index)));
        let injecting = Arc::new(AtomicBool::new(false));
        let modifiers = Arc::new(Mutex::new(Modifiers::default()));

        thread::spawn(move || {
            if !macos_accessibility_client::accessibility::application_is_trusted_with_prompt() {
                log::warn!("accessibility permission not granted; keyboard grab may fail");
                status(RuntimeStatus::PermissionRequired);
            }

            let status_for_callback = status.clone();
            let reported_active = Arc::new(AtomicBool::new(false));
            let callback = move |event: Event| -> Option<Event> {
                if !reported_active.swap(true, Ordering::AcqRel) {
                    status_for_callback(RuntimeStatus::Active);
                }

                if injecting.load(Ordering::Acquire) {
                    return Some(event);
                }

                let (key, down) = match event.event_type {
                    EventType::KeyPress(key) => (key, true),
                    EventType::KeyRelease(key) => (key, false),
                    _ => return Some(event),
                };

                let is_shortcut = {
                    let mut modifiers = modifiers.lock().ok()?;
                    if modifiers.apply(key, down) {
                        if down {
                            if let Ok(mut matcher) = matcher.lock() {
                                matcher.reset();
                            }
                        }
                        return Some(event);
                    }
                    modifiers.shortcut()
                };

                if !down {
                    return Some(event);
                }

                if paused.load(Ordering::Relaxed) || is_shortcut {
                    if let Ok(mut matcher) = matcher.lock() {
                        matcher.reset();
                    }
                    return Some(event);
                }

                match key {
                    Key::Backspace => {
                        if let Ok(mut matcher) = matcher.lock() {
                            matcher.backspace();
                        }
                        Some(event)
                    }
                    Key::Space => handle_delimiter(event, ' ', &matcher, &injecting),
                    Key::Tab => handle_delimiter(event, '\t', &matcher, &injecting),
                    Key::Return => handle_delimiter(event, '\n', &matcher, &injecting),
                    _ => {
                        let mut handled = false;
                        if let Some(name) = event.name.as_deref() {
                            let mut chars = name.chars();
                            if let (Some(ch), None) = (chars.next(), chars.next()) {
                                if !ch.is_control() {
                                    if let Ok(mut matcher) = matcher.lock() {
                                        let _ = matcher.feed_char(ch);
                                    }
                                    handled = true;
                                }
                            }
                        }
                        if !handled {
                            if let Ok(mut matcher) = matcher.lock() {
                                matcher.reset();
                            }
                        }
                        Some(event)
                    }
                }
            };

            if let Err(error) = grab(callback) {
                log::error!("keyboard grab failed: {error:?}");
                status(RuntimeStatus::Failed(format!("{error:?}")));
            }
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

        injecting.store(true, Ordering::Release);
        let injecting = Arc::clone(injecting);
        thread::spawn(move || {
            let plan = plan_injection(&expansion);
            if let Err(error) = inject(&plan) {
                log::warn!("expansion injection failed: {error}");
            }
            injecting.store(false, Ordering::Release);
        });

        None
    }

    fn inject(plan: &InjectionPlan) -> Result<(), String> {
        let mut enigo = Enigo::new(&Settings::default()).map_err(|error| error.to_string())?;

        for _ in 0..plan.backspaces {
            enigo
                .key(EnigoKey::Backspace, Direction::Click)
                .map_err(|error| error.to_string())?;
        }

        enigo.text(&plan.text).map_err(|error| error.to_string())?;

        match plan.trailing {
            ' ' => enigo.key(EnigoKey::Space, Direction::Click),
            '\t' => enigo.key(EnigoKey::Tab, Direction::Click),
            '\n' => enigo.key(EnigoKey::Return, Direction::Click),
            other => enigo.text(&other.to_string()),
        }
        .map_err(|error| error.to_string())?;

        for _ in 0..plan.left_presses {
            enigo
                .key(EnigoKey::LeftArrow, Direction::Click)
                .map_err(|error| error.to_string())?;
        }

        Ok(())
    }
}

#[cfg(not(any(target_os = "windows", target_os = "macos")))]
mod imp {
    use super::*;

    pub fn spawn(
        _index: SharedSnippetIndex,
        _paused: PauseFlag,
        status: StatusCallback,
    ) -> thread::JoinHandle<()> {
        thread::spawn(move || {
            log::warn!("global text expansion is not implemented for this platform");
            status(RuntimeStatus::Unsupported);
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::template::render_template;

    #[test]
    fn plan_keeps_trigger_length_and_moves_cursor_over_trailing_delimiter() {
        let expansion = Expansion {
            backspaces: 5,
            replacement: "Hello {{cursor}}world".into(),
            trailing: '\n',
        };
        let plan = plan_from_rendered(
            &expansion,
            render_template(&expansion.replacement, None, None),
        );
        assert_eq!(plan.backspaces, 5);
        assert_eq!(plan.text, "Hello world");
        assert_eq!(plan.trailing, '\n');
        assert_eq!(plan.left_presses, 6);
    }

    #[test]
    fn plan_without_cursor_marker_moves_nothing() {
        let expansion = Expansion {
            backspaces: 2,
            replacement: "plain".into(),
            trailing: ' ',
        };
        let plan = plan_injection(&expansion);
        assert_eq!(plan.text, "plain");
        assert_eq!(plan.left_presses, 0);
    }

    #[test]
    fn status_labels_are_user_facing() {
        assert_eq!(RuntimeStatus::Active.label(), "Expansion active");
        assert!(RuntimeStatus::Failed("boom".into())
            .label()
            .contains("boom"));
    }
}
