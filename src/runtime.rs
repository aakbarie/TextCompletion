//! Global keyboard binding: watches typed text in every application, detects
//! a trigger followed by a delimiter, and replaces it with the rendered phrase.
//!
//! Both desktop platforms hook the keyboard natively. Windows uses a
//! `WH_KEYBOARD_LL` hook and decodes the current vkCode directly; macOS uses a
//! CoreGraphics event tap and reads the typed character from the event. Both
//! ignore their own injected events explicitly.

use crate::expansion::{Expansion, SharedSnippetIndex};
use crate::template::{cursor_left_presses, render_now, RenderedTemplate};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
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

/// Tracks one expansion from the moment the hook accepts it until the worker
/// has typed it (or given up). Keystrokes and clicks that arrive in between
/// are counted so the worker can abort instead of interleaving with them.
#[derive(Debug, Default)]
pub struct PendingExpansion {
    in_flight: AtomicBool,
    interruptions: AtomicUsize,
}

impl PendingExpansion {
    pub fn begin(&self) {
        self.interruptions.store(0, Ordering::SeqCst);
        self.in_flight.store(true, Ordering::SeqCst);
    }

    pub fn in_flight(&self) -> bool {
        self.in_flight.load(Ordering::SeqCst)
    }

    /// Records user input that arrived while an expansion was pending.
    /// Returns `true` if an expansion was in flight.
    pub fn note_interruption(&self) -> bool {
        if self.in_flight() {
            self.interruptions.fetch_add(1, Ordering::SeqCst);
            true
        } else {
            false
        }
    }

    pub fn interruptions(&self) -> usize {
        self.interruptions.load(Ordering::SeqCst)
    }

    pub fn finish(&self) {
        self.in_flight.store(false, Ordering::SeqCst);
    }
}

/// Why a queued expansion was dropped rather than typed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AbortReason {
    /// The user typed or clicked before the replacement could be sent.
    Interrupted(usize),
    /// The foreground window or focused control changed since the trigger.
    TargetChanged,
}

impl std::fmt::Display for AbortReason {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Interrupted(n) => write!(f, "{n} keystroke(s) arrived before the replacement"),
            Self::TargetChanged => write!(f, "the target window changed"),
        }
    }
}

/// Decides whether a queued expansion may still be typed safely.
pub fn may_inject(interruptions: usize, target_unchanged: bool) -> Result<(), AbortReason> {
    if !target_unchanged {
        return Err(AbortReason::TargetChanged);
    }
    if interruptions > 0 {
        return Err(AbortReason::Interrupted(interruptions));
    }
    Ok(())
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
        CallNextHookEx, GetForegroundWindow, GetGUIThreadInfo, GetMessageW,
        GetWindowThreadProcessId, SetWindowsHookExW, GUITHREADINFO, KBDLLHOOKSTRUCT,
        LLKHF_INJECTED, MSG, WH_KEYBOARD_LL, WH_MOUSE_LL, WM_KEYDOWN, WM_KEYUP, WM_LBUTTONDOWN,
        WM_MBUTTONDOWN, WM_RBUTTONDOWN, WM_SYSKEYDOWN, WM_SYSKEYUP,
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

    /// Where the trigger was typed: foreground window and focused control.
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

    struct HookState {
        matcher: ExpansionMatcher,
        expansion_tx: mpsc::Sender<QueuedExpansion>,
        modifiers: Modifiers,
        suppress_keyup: Option<u32>,
        paused: PauseFlag,
        pending: Arc<PendingExpansion>,
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

    pub fn spawn(
        index: SharedSnippetIndex,
        paused: PauseFlag,
        status: StatusCallback,
    ) -> thread::JoinHandle<()> {
        let (expansion_tx, expansion_rx) = mpsc::channel::<QueuedExpansion>();
        let pending = Arc::new(PendingExpansion::default());

        let worker_pending = Arc::clone(&pending);
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
                suppress_keyup: None,
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
            log::info!("low-level keyboard hook installed");
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

        if down {
            // Typing into a different window than the last key went to means
            // the partial trigger no longer describes what is on screen.
            let window = GetForegroundWindow() as isize;
            if window != state.last_window {
                state.last_window = window;
                state.matcher.reset();
            }

            // Anything typed while a replacement is still in flight makes
            // that replacement unsafe; the worker drops it.
            if state.pending.note_interruption() {
                state.matcher.reset();
                return CallNextHookEx(null_mut(), code, wparam, lparam);
            }
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
                let queued = QueuedExpansion {
                    expansion,
                    target: current_target(),
                };
                state.pending.begin();
                if state.expansion_tx.send(queued).is_ok() {
                    state.suppress_keyup = Some(vk);
                    return 1;
                }
                state.pending.finish();
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

    /// Mouse clicks move the caret, so a partially typed trigger is stale and a
    /// pending replacement would land in the wrong place.
    unsafe extern "system" fn mouse_proc(code: i32, wparam: usize, lparam: isize) -> isize {
        if code >= 0 {
            let message = wparam as u32;
            if message == WM_LBUTTONDOWN || message == WM_RBUTTONDOWN || message == WM_MBUTTONDOWN {
                with_state(|state| {
                    state.matcher.reset();
                    state.pending.note_interruption();
                });
            }
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
    //! macOS uses a CoreGraphics event tap directly. Earlier versions went
    //! through rdev, which decodes every key inside the tap callback with the
    //! Text Input Source API. Since macOS 14 that API asserts it is on the main
    //! thread, so the first keystroke killed the process with SIGILL. The tap
    //! reads the typed character from the event itself instead, which is safe
    //! on the hook thread.

    use super::*;
    use crate::expansion::{Expansion, ExpansionMatcher};
    use core_foundation::base::TCFType;
    use core_foundation::mach_port::CFMachPortRef;
    use core_foundation::runloop::{kCFRunLoopCommonModes, CFRunLoop};
    use core_graphics::event::{
        CGEvent, CGEventFlags, CGEventTap, CGEventTapLocation, CGEventTapOptions,
        CGEventTapPlacement, CGEventTapProxy, CGEventType, CallbackResult, EventField,
    };
    use core_graphics::sys::CGEventRef;
    use enigo::{Direction, Enigo, Key as EnigoKey, Keyboard, Settings};
    use foreign_types::ForeignType;
    use macos_accessibility_client::accessibility::{
        application_is_trusted, application_is_trusted_with_prompt,
    };
    use std::ffi::{c_ulong, c_void};
    use std::sync::atomic::{AtomicPtr, Ordering};
    use std::sync::Mutex;
    use std::time::Duration;

    /// Stamped on every synthetic event so the tap passes its own input through.
    const SCRIBLET_EVENT_MARKER: i64 = 0x5343_5242; // "SCRB"

    // Virtual key codes from HIToolbox Events.h (kVK_*). Layout independent.
    const KEY_RETURN: i64 = 0x24;
    const KEY_TAB: i64 = 0x30;
    const KEY_SPACE: i64 = 0x31;
    const KEY_DELETE: i64 = 0x33;
    const KEY_ESCAPE: i64 = 0x35;
    const KEY_KEYPAD_ENTER: i64 = 0x4C;
    const KEY_HOME: i64 = 0x73;
    const KEY_PAGE_UP: i64 = 0x74;
    const KEY_FORWARD_DELETE: i64 = 0x75;
    const KEY_END: i64 = 0x77;
    const KEY_PAGE_DOWN: i64 = 0x79;
    const KEY_LEFT: i64 = 0x7B;
    const KEY_RIGHT: i64 = 0x7C;
    const KEY_DOWN: i64 = 0x7D;
    const KEY_UP: i64 = 0x7E;

    #[link(name = "CoreGraphics", kind = "framework")]
    extern "C" {
        fn CGEventKeyboardGetUnicodeString(
            event: CGEventRef,
            max_length: c_ulong,
            actual_length: *mut c_ulong,
            unicode_string: *mut u16,
        );
        fn CGEventTapEnable(tap: CFMachPortRef, enable: bool);
    }

    /// Everything the tap callback mutates, behind one lock.
    struct State {
        matcher: ExpansionMatcher,
        /// A delimiter whose key-down was swallowed. Its auto-repeats are
        /// swallowed too until the physical key comes back up.
        suppressed_key: Option<i64>,
    }

    pub fn spawn(
        index: SharedSnippetIndex,
        paused: PauseFlag,
        status: StatusCallback,
    ) -> thread::JoinHandle<()> {
        thread::spawn(move || {
            if !wait_for_accessibility(&status) {
                status(RuntimeStatus::Failed(
                    "Accessibility permission was not granted".to_string(),
                ));
                return;
            }

            let state = Arc::new(Mutex::new(State {
                matcher: ExpansionMatcher::new(index),
                suppressed_key: None,
            }));
            let pending = Arc::new(PendingExpansion::default());

            if let Err(reason) = run_tap(state, pending, paused, &status) {
                log::error!("keyboard event tap failed: {reason}");
                status(RuntimeStatus::Failed(reason));
            }
        })
    }

    /// Blocks until the process is trusted for Accessibility, showing the
    /// system prompt once. Returns `false` if Scriblet should give up.
    fn wait_for_accessibility(status: &StatusCallback) -> bool {
        if application_is_trusted_with_prompt() {
            return true;
        }
        log::warn!("accessibility permission not granted; waiting for the user");
        status(RuntimeStatus::PermissionRequired);

        // The window already tells the user what to do, so keep polling until
        // they flip the toggle. Expansion then starts without a relaunch.
        loop {
            thread::sleep(Duration::from_secs(1));
            if application_is_trusted() {
                log::info!("accessibility permission granted");
                return true;
            }
        }
    }

    /// Installs the tap on this thread's run loop and services it forever.
    fn run_tap(
        state: Arc<Mutex<State>>,
        pending: Arc<PendingExpansion>,
        paused: PauseFlag,
        status: &StatusCallback,
    ) -> Result<(), String> {
        // The tap re-enables itself if macOS disables it for being slow, so
        // the callback needs the port. It is filled in once the tap exists.
        let port = Arc::new(AtomicPtr::<c_void>::new(std::ptr::null_mut()));
        let port_for_callback = Arc::clone(&port);

        let callback = move |_proxy: CGEventTapProxy,
                             event_type: CGEventType,
                             event: &CGEvent|
              -> CallbackResult {
            handle_event(
                event_type,
                event,
                &state,
                &pending,
                &paused,
                &port_for_callback,
            )
        };

        let tap = CGEventTap::new(
            CGEventTapLocation::HID,
            CGEventTapPlacement::HeadInsertEventTap,
            CGEventTapOptions::Default,
            vec![
                CGEventType::KeyDown,
                CGEventType::KeyUp,
                CGEventType::FlagsChanged,
                CGEventType::LeftMouseDown,
                CGEventType::RightMouseDown,
                CGEventType::OtherMouseDown,
            ],
            callback,
        )
        .map_err(|()| "CGEventTapCreate returned NULL".to_string())?;

        port.store(
            tap.mach_port().as_concrete_TypeRef() as *mut c_void,
            Ordering::Release,
        );
        let source = tap
            .mach_port()
            .create_runloop_source(0)
            .map_err(|()| "could not create run loop source for the event tap".to_string())?;
        CFRunLoop::get_current().add_source(&source, unsafe { kCFRunLoopCommonModes });
        tap.enable();

        log::info!("macOS keyboard event tap installed");
        status(RuntimeStatus::Active);
        CFRunLoop::run_current();
        drop(tap);
        Err("the event tap run loop exited".to_string())
    }

    fn handle_event(
        event_type: CGEventType,
        event: &CGEvent,
        state: &Arc<Mutex<State>>,
        pending: &Arc<PendingExpansion>,
        paused: &PauseFlag,
        port: &AtomicPtr<c_void>,
    ) -> CallbackResult {
        match event_type {
            CGEventType::TapDisabledByTimeout | CGEventType::TapDisabledByUserInput => {
                log::warn!("macOS disabled the event tap ({event_type:?}); re-enabling");
                let port = port.load(Ordering::Acquire);
                if !port.is_null() {
                    unsafe { CGEventTapEnable(port as CFMachPortRef, true) };
                }
                return CallbackResult::Keep;
            }
            _ => {}
        }

        if event.get_integer_value_field(EventField::EVENT_SOURCE_USER_DATA)
            == SCRIBLET_EVENT_MARKER
        {
            return CallbackResult::Keep;
        }

        let Ok(mut state) = state.lock() else {
            return CallbackResult::Keep;
        };
        let keycode = event.get_integer_value_field(EventField::KEYBOARD_EVENT_KEYCODE);
        let flags = event.get_flags();

        match event_type {
            CGEventType::LeftMouseDown
            | CGEventType::RightMouseDown
            | CGEventType::OtherMouseDown => {
                // A click moves the caret: the partial trigger is stale and a
                // pending replacement would land in the wrong place.
                pending.note_interruption();
                state.matcher.reset();
                CallbackResult::Keep
            }
            CGEventType::FlagsChanged => {
                if is_shortcut(flags) {
                    state.matcher.reset();
                }
                CallbackResult::Keep
            }
            CGEventType::KeyUp => {
                if state.suppressed_key == Some(keycode) {
                    state.suppressed_key = None;
                }
                CallbackResult::Keep
            }
            CGEventType::KeyDown => {
                let autorepeat =
                    event.get_integer_value_field(EventField::KEYBOARD_EVENT_AUTOREPEAT) != 0;
                if autorepeat && state.suppressed_key == Some(keycode) {
                    return CallbackResult::Drop;
                }
                handle_key_down(keycode, flags, event, &mut state, pending, paused)
            }
            _ => CallbackResult::Keep,
        }
    }

    fn handle_key_down(
        keycode: i64,
        flags: CGEventFlags,
        event: &CGEvent,
        state: &mut State,
        pending: &Arc<PendingExpansion>,
        paused: &PauseFlag,
    ) -> CallbackResult {
        if pending.note_interruption() || paused.load(Ordering::Relaxed) || is_shortcut(flags) {
            state.matcher.reset();
            return CallbackResult::Keep;
        }

        let delimiter = match keycode {
            KEY_SPACE => Some(' '),
            KEY_TAB => Some('\t'),
            KEY_RETURN | KEY_KEYPAD_ENTER => Some('\n'),
            _ => None,
        };
        if let Some(delimiter) = delimiter {
            // Shift+Enter, Shift+Tab, and Shift+Space mean something to the
            // target application; leave them alone.
            if flags.contains(CGEventFlags::CGEventFlagShift) {
                state.matcher.reset();
                return CallbackResult::Keep;
            }
            return match state.matcher.feed_char(delimiter) {
                Some(expansion) => {
                    state.suppressed_key = Some(keycode);
                    start_injection(expansion, pending);
                    CallbackResult::Drop
                }
                None => CallbackResult::Keep,
            };
        }

        match keycode {
            KEY_DELETE => state.matcher.backspace(),
            KEY_ESCAPE | KEY_FORWARD_DELETE | KEY_HOME | KEY_END | KEY_PAGE_UP | KEY_PAGE_DOWN
            | KEY_LEFT | KEY_RIGHT | KEY_UP | KEY_DOWN => state.matcher.reset(),
            _ => match typed_char(event) {
                Some(ch) => {
                    let _ = state.matcher.feed_char(ch);
                }
                None => state.matcher.reset(),
            },
        }
        CallbackResult::Keep
    }

    /// Command, Control, or Option turn a key into a shortcut rather than text.
    fn is_shortcut(flags: CGEventFlags) -> bool {
        flags.intersects(
            CGEventFlags::CGEventFlagCommand
                | CGEventFlags::CGEventFlagControl
                | CGEventFlags::CGEventFlagAlternate,
        )
    }

    /// The character this key event types under the current layout and
    /// modifiers, read from the event itself. `None` for dead keys, function
    /// keys, and anything that is not exactly one printable character.
    fn typed_char(event: &CGEvent) -> Option<char> {
        let mut buffer = [0u16; 4];
        let mut length: c_ulong = 0;
        unsafe {
            CGEventKeyboardGetUnicodeString(
                event.as_ptr(),
                buffer.len() as c_ulong,
                &mut length,
                buffer.as_mut_ptr(),
            );
        }
        let units = buffer.get(..length as usize)?;
        let mut chars = char::decode_utf16(units.iter().copied());
        match (chars.next(), chars.next()) {
            (Some(Ok(ch)), None) if !ch.is_control() => Some(ch),
            _ => None,
        }
    }

    fn start_injection(expansion: Expansion, pending: &Arc<PendingExpansion>) {
        pending.begin();
        let pending = Arc::clone(pending);
        thread::spawn(move || {
            let plan = plan_injection(&expansion);
            // No frontmost-app check on macOS yet; interruptions still abort.
            match may_inject(pending.interruptions(), true) {
                Ok(()) => {
                    if let Err(error) = inject(&plan) {
                        log::warn!("expansion injection failed: {error}");
                    }
                }
                Err(reason) => log::warn!("expansion skipped: {reason}"),
            }
            pending.finish();
        });
    }

    fn inject(plan: &InjectionPlan) -> Result<(), String> {
        let settings = Settings {
            event_source_user_data: Some(SCRIBLET_EVENT_MARKER),
            ..Settings::default()
        };
        let mut enigo = Enigo::new(&settings).map_err(|error| error.to_string())?;

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
    fn pending_expansion_counts_interruptions_only_while_in_flight() {
        let pending = PendingExpansion::default();
        assert!(!pending.note_interruption(), "nothing in flight yet");
        assert_eq!(pending.interruptions(), 0);

        pending.begin();
        assert!(pending.in_flight());
        assert!(pending.note_interruption());
        assert!(pending.note_interruption());
        assert_eq!(pending.interruptions(), 2);

        pending.finish();
        assert!(!pending.in_flight());
        assert!(!pending.note_interruption());

        pending.begin();
        assert_eq!(pending.interruptions(), 0, "begin resets the count");
    }

    #[test]
    fn injection_is_refused_after_interruption_or_target_change() {
        assert_eq!(may_inject(0, true), Ok(()));
        assert_eq!(may_inject(1, true), Err(AbortReason::Interrupted(1)));
        assert_eq!(may_inject(0, false), Err(AbortReason::TargetChanged));
        assert_eq!(may_inject(3, false), Err(AbortReason::TargetChanged));
        assert!(AbortReason::Interrupted(2)
            .to_string()
            .contains("2 keystroke"));
    }

    #[test]
    fn status_labels_are_user_facing() {
        assert_eq!(RuntimeStatus::Active.label(), "Expansion active");
        assert!(RuntimeStatus::Failed("boom".into())
            .label()
            .contains("boom"));
    }
}
