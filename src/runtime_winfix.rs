//! Platform runtime facade.
//!
//! macOS and other non-Windows platforms continue to use the established
//! runtime implementation. Windows uses a dedicated native implementation that
//! waits for the physical delimiter key-up before injecting and prefers normal
//! virtual-key events for ASCII text. This avoids relying on a long stream of
//! VK_PACKET / KEYEVENTF_UNICODE events for ordinary clinical text.

use crate::expansion::SharedSnippetIndex;
use std::thread;

pub use crate::runtime_legacy::{
    may_inject, plan_injection, AbortReason, InjectionPlan, PauseFlag, PendingExpansion,
    RuntimeStatus, StatusCallback,
};

#[cfg(not(target_os = "windows"))]
pub use crate::runtime_legacy::spawn_global_binding;

#[cfg(target_os = "windows")]
pub fn spawn_global_binding(
    index: SharedSnippetIndex,
    paused: PauseFlag,
    status: StatusCallback,
) -> thread::JoinHandle<()> {
    windows::spawn(index, paused, status)
}

#[cfg(target_os = "windows")]
mod windows {
    use super::*;
    use crate::expansion::{Expansion, ExpansionMatcher};
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
        CallNextHookEx, GetForegroundWindow, GetGUIThreadInfo, GetMessageW,
        GetWindowThreadProcessId, SetWindowsHookExW, GUITHREADINFO, KBDLLHOOKSTRUCT,
        LLKHF_INJECTED, MSG, WH_KEYBOARD_LL, WH_MOUSE_LL, WM_KEYDOWN, WM_KEYUP, WM_LBUTTONDOWN,
        WM_MBUTTONDOWN, WM_RBUTTONDOWN, WM_SYSKEYDOWN, WM_SYSKEYUP,
    };

    const SCRIBLET_INPUT_MARKER: usize = 0x5343_5242; // "SCRB"

    #[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
    struct Modifiers {
        left_shift: bool,
        right_shift: bool,
        control: bool,
        alt: bool,
        win: bool,
        caps_lock: bool,
        caps_key_held: bool,
    }

    impl Modifiers {
        fn shift(&self) -> bool {
            self.left_shift || self.right_shift
        }

        fn shortcut(&self) -> bool {
            self.control || self.alt || self.win
        }

        fn apply(&mut self, vk: u32, down: bool) -> bool {
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

    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    struct Target {
        window: isize,
        focus: isize,
    }

    fn current_target() -> Target {
        unsafe {
            let window = GetForegroundWindow();
            let thread = GetWindowThreadProcessId(window, null_mut());
            let mut info: GUITHREADINFO = std::mem::zeroed();
            info.cbSize = size_of::<GUITHREADINFO>() as u32;
            let focus = if GetGUIThreadInfo(thread, &mut info) != 0 {
                info.hwndFocus as isize
            } else {
                0
            };
            Target {
                window: window as isize,
                focus,
            }
        }
    }

    struct QueuedExpansion {
        expansion: Expansion,
        target: Target,
    }

    struct ArmedExpansion {
        delimiter_vk: u32,
        queued: QueuedExpansion,
    }

    struct HookState {
        matcher: ExpansionMatcher,
        expansion_tx: mpsc::Sender<QueuedExpansion>,
        modifiers: Modifiers,
        armed: Option<ArmedExpansion>,
        paused: PauseFlag,
        pending: std::sync::Arc<PendingExpansion>,
        last_window: isize,
    }

    static HOOK_STATE: OnceLock<Mutex<HookState>> = OnceLock::new();

    fn with_state(f: impl FnOnce(&mut HookState)) {
        if let Some(state) = HOOK_STATE.get() {
            if let Ok(mut state) = state.lock() {
                f(&mut state);
            }
        }
    }

    pub(super) fn spawn(
        index: SharedSnippetIndex,
        paused: PauseFlag,
        status: StatusCallback,
    ) -> thread::JoinHandle<()> {
        let (expansion_tx, expansion_rx) = mpsc::channel::<QueuedExpansion>();
        let pending = std::sync::Arc::new(PendingExpansion::default());

        let worker_pending = std::sync::Arc::clone(&pending);
        thread::spawn(move || {
            while let Ok(queued) = expansion_rx.recv() {
                let plan = plan_injection(&queued.expansion);
                let same_target = current_target() == queued.target;
                match may_inject(worker_pending.interruptions(), same_target) {
                    Ok(()) => {
                        if let Err(error) = inject(&plan) {
                            log::warn!("expansion injection failed: {error}");
                        }
                    }
                    Err(reason) => log::warn!("expansion skipped: {reason}"),
                }
                worker_pending.finish();
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
                armed: None,
                paused,
                pending,
                last_window: 0,
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
            log::info!("low-level keyboard hook installed (vkey emitter)");

            let mouse_hook = SetWindowsHookExW(WH_MOUSE_LL, Some(mouse_proc), null_mut(), 0);
            if mouse_hook.is_null() {
                log::warn!(
                    "mouse hook unavailable, clicks will not reset triggers: {}",
                    std::io::Error::last_os_error()
                );
            }
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

        // A matched delimiter is suppressed on key-down, but the replacement is
        // not queued until the corresponding physical key-up. This guarantees
        // that no synthetic typing starts while Space/Tab/Enter is still held.
        if up {
            if state
                .armed
                .as_ref()
                .is_some_and(|armed| armed.delimiter_vk == vk)
            {
                let armed = state.armed.take().expect("checked above");
                if state.expansion_tx.send(armed.queued).is_err() {
                    state.pending.finish();
                }
                return 1;
            }
            return CallNextHookEx(null_mut(), code, wparam, lparam);
        }

        if !down {
            return CallNextHookEx(null_mut(), code, wparam, lparam);
        }

        // Auto-repeat for a held delimiter must remain suppressed while armed.
        if state
            .armed
            .as_ref()
            .is_some_and(|armed| armed.delimiter_vk == vk)
        {
            return 1;
        }

        let window = GetForegroundWindow() as isize;
        if window != state.last_window {
            state.last_window = window;
            state.matcher.reset();
        }

        if state.pending.note_interruption() {
            state.matcher.reset();
            state.armed = None;
            return CallNextHookEx(null_mut(), code, wparam, lparam);
        }

        if state.paused.load(Ordering::Relaxed) {
            state.matcher.reset();
            return CallNextHookEx(null_mut(), code, wparam, lparam);
        }

        if state.modifiers.shortcut() {
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
            // Shift+Enter/Tab/Space has application-specific meaning. Do not
            // convert it into a text expansion.
            if state.modifiers.shift() {
                state.matcher.reset();
                return CallNextHookEx(null_mut(), code, wparam, lparam);
            }

            if let Some(expansion) = state.matcher.feed_char(delimiter) {
                state.pending.begin();
                state.armed = Some(ArmedExpansion {
                    delimiter_vk: vk,
                    queued: QueuedExpansion {
                        expansion,
                        target: current_target(),
                    },
                });
                return 1;
            }
            return CallNextHookEx(null_mut(), code, wparam, lparam);
        }

        let modifiers = state.modifiers;
        if let Some(ch) = decode_vk(vk, &modifiers) {
            let _ = state.matcher.feed_char(ch);
        } else {
            state.matcher.reset();
        }

        CallNextHookEx(null_mut(), code, wparam, lparam)
    }

    unsafe extern "system" fn mouse_proc(code: i32, wparam: usize, lparam: isize) -> isize {
        if code >= 0 {
            let message = wparam as u32;
            if message == WM_LBUTTONDOWN || message == WM_RBUTTONDOWN || message == WM_MBUTTONDOWN {
                with_state(|state| {
                    state.matcher.reset();
                    state.armed = None;
                    state.pending.note_interruption();
                });
            }
        }
        CallNextHookEx(null_mut(), code, wparam, lparam)
    }

    fn decode_vk(vk: u32, modifiers: &Modifiers) -> Option<char> {
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
        let caps_lock = unsafe { GetKeyState(VK_CAPITAL as i32) } & 1 != 0;
        let inputs = build_inputs(plan, caps_lock);
        if inputs.is_empty() {
            return Ok(());
        }

        let expected = inputs.len() as u32;
        let sent = unsafe { SendInput(expected, inputs.as_ptr(), size_of::<INPUT>() as i32) };
        log::debug!(
            "Windows expansion submitted {sent}/{expected} input events ({} backspaces, {} chars)",
            plan.backspaces,
            plan.text.chars().count()
        );

        if sent == expected {
            Ok(())
        } else {
            Err(format!(
                "SendInput submitted {sent} of {expected} events: {}",
                std::io::Error::last_os_error()
            ))
        }
    }

    /// Build one uninterrupted input batch. Ordinary ASCII text uses normal
    /// virtual keys instead of VK_PACKET. Unicode is retained only as a
    /// fallback for characters that do not have a direct US-keyboard mapping.
    fn build_inputs(plan: &InjectionPlan, caps_lock: bool) -> Vec<INPUT> {
        let mut inputs = Vec::new();

        for _ in 0..plan.backspaces {
            push_virtual_key_click(&mut inputs, VK_BACK);
        }

        for ch in plan.text.chars() {
            push_text_char(&mut inputs, ch, caps_lock);
        }

        push_text_char(&mut inputs, plan.trailing, caps_lock);

        for _ in 0..plan.left_presses {
            push_virtual_key_click(&mut inputs, VK_LEFT);
        }

        inputs
    }

    fn push_text_char(inputs: &mut Vec<INPUT>, ch: char, caps_lock: bool) {
        if let Some((vk, shifted)) = ascii_key(ch, caps_lock) {
            if shifted {
                push_key_down(inputs, VK_SHIFT);
            }
            push_virtual_key_click(inputs, vk);
            if shifted {
                push_key_up(inputs, VK_SHIFT);
            }
            return;
        }

        let mut buffer = [0u16; 2];
        for unit in ch.encode_utf16(&mut buffer).iter().copied() {
            push_unicode_unit(inputs, unit);
        }
    }

    /// Maps common text to real keyboard keys. Letter case accounts for Caps
    /// Lock so the emitted character remains deterministic.
    fn ascii_key(ch: char, caps_lock: bool) -> Option<(u16, bool)> {
        if ch.is_ascii_alphabetic() {
            let upper = ch.is_ascii_uppercase();
            let vk = ch.to_ascii_uppercase() as u16;
            return Some((vk, upper ^ caps_lock));
        }
        if ch.is_ascii_digit() {
            return Some((ch as u16, false));
        }

        Some(match ch {
            ' ' => (VK_SPACE, false),
            '\t' => (VK_TAB, false),
            '\n' | '\r' => (VK_RETURN, false),
            ')' => ('0' as u16, true),
            '!' => ('1' as u16, true),
            '@' => ('2' as u16, true),
            '#' => ('3' as u16, true),
            '$' => ('4' as u16, true),
            '%' => ('5' as u16, true),
            '^' => ('6' as u16, true),
            '&' => ('7' as u16, true),
            '*' => ('8' as u16, true),
            '(' => ('9' as u16, true),
            ';' => (0xBA, false),
            ':' => (0xBA, true),
            '=' => (0xBB, false),
            '+' => (0xBB, true),
            ',' => (0xBC, false),
            '<' => (0xBC, true),
            '-' => (0xBD, false),
            '_' => (0xBD, true),
            '.' => (0xBE, false),
            '>' => (0xBE, true),
            '/' => (0xBF, false),
            '?' => (0xBF, true),
            '`' => (0xC0, false),
            '~' => (0xC0, true),
            '[' => (0xDB, false),
            '{' => (0xDB, true),
            '\\' => (0xDC, false),
            '|' => (0xDC, true),
            ']' => (0xDD, false),
            '}' => (0xDD, true),
            '\'' => (0xDE, false),
            '"' => (0xDE, true),
            _ => return None,
        })
    }

    fn push_unicode_unit(inputs: &mut Vec<INPUT>, unit: u16) {
        inputs.push(keyboard_input(0, unit, KEYEVENTF_UNICODE));
        inputs.push(keyboard_input(0, unit, KEYEVENTF_UNICODE | KEYEVENTF_KEYUP));
    }

    fn push_virtual_key_click(inputs: &mut Vec<INPUT>, key: u16) {
        push_key_down(inputs, key);
        push_key_up(inputs, key);
    }

    fn push_key_down(inputs: &mut Vec<INPUT>, key: u16) {
        inputs.push(keyboard_input(key, 0, 0));
    }

    fn push_key_up(inputs: &mut Vec<INPUT>, key: u16) {
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

        #[test]
        fn ascii_emitter_maps_problem_phrase_to_physical_keys() {
            for ch in "testing scriblet 4.2".chars() {
                assert!(ascii_key(ch, false).is_some(), "missing mapping for {ch:?}");
            }
        }

        #[test]
        fn caps_lock_inverts_only_letter_shift_requirement() {
            assert_eq!(ascii_key('a', false), Some(('A' as u16, false)));
            assert_eq!(ascii_key('a', true), Some(('A' as u16, true)));
            assert_eq!(ascii_key('A', false), Some(('A' as u16, true)));
            assert_eq!(ascii_key('A', true), Some(('A' as u16, false)));
            assert_eq!(ascii_key('.', true), Some((0xBE, false)));
        }

        #[test]
        fn unicode_is_only_a_fallback() {
            let plan = InjectionPlan {
                backspaces: 0,
                text: "abc é".into(),
                trailing: ' ',
                left_presses: 0,
            };
            let inputs = build_inputs(&plan, false);
            assert!(!inputs.is_empty());
            assert!(ascii_key('a', false).is_some());
            assert!(ascii_key('é', false).is_none());
        }
    }
}
