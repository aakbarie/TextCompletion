//! System tray icon and menu. Scriblet keeps running from the tray when the
//! window is closed so the keyboard hook stays alive.

use std::rc::Rc;

/// What the tray asks the application to do.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(not(any(target_os = "windows", target_os = "macos")), allow(dead_code))]
pub enum TrayAction {
    ShowWindow,
    TogglePause,
    Quit,
}

pub type TrayHandler = Rc<dyn Fn(TrayAction)>;

#[cfg(any(target_os = "windows", target_os = "macos"))]
mod imp {
    use super::*;
    use std::cell::RefCell;
    use std::time::Duration;
    use tray_icon::menu::{CheckMenuItem, Menu, MenuEvent, MenuItem, PredefinedMenuItem};
    use tray_icon::{
        Icon, MouseButton, MouseButtonState, TrayIcon, TrayIconBuilder, TrayIconEvent,
    };

    const SHOW_ID: &str = "scriblet.show";
    const PAUSE_ID: &str = "scriblet.pause";
    const QUIT_ID: &str = "scriblet.quit";
    const POLL_INTERVAL: Duration = Duration::from_millis(150);

    struct TrayState {
        _icon: TrayIcon,
        pause_item: CheckMenuItem,
        _poll: slint::Timer,
    }

    thread_local! {
        static TRAY: RefCell<Option<TrayState>> = const { RefCell::new(None) };
    }

    /// Creates the tray once the event loop is running (required on macOS).
    pub fn install(handler: TrayHandler, paused: bool) {
        slint::Timer::single_shot(Duration::from_millis(50), move || {
            match build(handler, paused) {
                Ok(state) => TRAY.with(|slot| *slot.borrow_mut() = Some(state)),
                Err(error) => log::error!("tray icon unavailable: {error}"),
            }
        });
    }

    pub fn set_paused(paused: bool) {
        TRAY.with(|slot| {
            if let Some(state) = slot.borrow().as_ref() {
                state.pause_item.set_checked(paused);
            }
        });
    }

    pub fn remove() {
        TRAY.with(|slot| slot.borrow_mut().take());
    }

    fn build(handler: TrayHandler, paused: bool) -> anyhow::Result<TrayState> {
        let menu = Menu::new();
        let show = MenuItem::with_id(SHOW_ID, "Open Scriblet", true, None);
        let pause_item = CheckMenuItem::with_id(PAUSE_ID, "Pause expansion", true, paused, None);
        let quit = MenuItem::with_id(QUIT_ID, "Quit Scriblet", true, None);
        menu.append_items(&[&show, &pause_item, &PredefinedMenuItem::separator(), &quit])?;

        let icon = TrayIconBuilder::new()
            .with_menu(Box::new(menu))
            .with_tooltip(format!("Scriblet {}", textcompletion::APP_VERSION))
            .with_icon(icon()?)
            .with_menu_on_left_click(false)
            .build()?;

        let poll = slint::Timer::default();
        poll.start(slint::TimerMode::Repeated, POLL_INTERVAL, move || {
            while let Ok(event) = MenuEvent::receiver().try_recv() {
                match event.id.0.as_str() {
                    SHOW_ID => handler(TrayAction::ShowWindow),
                    PAUSE_ID => handler(TrayAction::TogglePause),
                    QUIT_ID => handler(TrayAction::Quit),
                    _ => {}
                }
            }
            while let Ok(event) = TrayIconEvent::receiver().try_recv() {
                let open = matches!(
                    event,
                    TrayIconEvent::DoubleClick {
                        button: MouseButton::Left,
                        ..
                    } | TrayIconEvent::Click {
                        button: MouseButton::Left,
                        button_state: MouseButtonState::Up,
                        ..
                    }
                );
                if open {
                    handler(TrayAction::ShowWindow);
                }
            }
        });

        Ok(TrayState {
            _icon: icon,
            pause_item,
            _poll: poll,
        })
    }

    /// A 32x32 rounded blue tile with three white "text lines". Generated in
    /// code so the binary needs no asset files.
    fn icon() -> anyhow::Result<Icon> {
        const SIZE: u32 = 32;
        let mut rgba = Vec::with_capacity((SIZE * SIZE * 4) as usize);
        for y in 0..SIZE {
            for x in 0..SIZE {
                let inside = rounded_rect(x, y, SIZE, 7);
                let line = matches!(y, 9..=11 | 15..=17 | 21..=23)
                    && (7..=24).contains(&x)
                    && !(y >= 21 && x > 18);
                let (r, g, b, a) = if !inside {
                    (0, 0, 0, 0)
                } else if line {
                    (255, 255, 255, 255)
                } else {
                    (0x17, 0x68, 0xB2, 255)
                };
                rgba.extend_from_slice(&[r, g, b, a]);
            }
        }
        Ok(Icon::from_rgba(rgba, SIZE, SIZE)?)
    }

    fn rounded_rect(x: u32, y: u32, size: u32, radius: u32) -> bool {
        let (x, y, size, r) = (x as i64, y as i64, size as i64, radius as i64);
        let inner = |v: i64| v.clamp(r, size - 1 - r);
        let dx = x - inner(x);
        let dy = y - inner(y);
        dx * dx + dy * dy <= r * r
    }
}

#[cfg(not(any(target_os = "windows", target_os = "macos")))]
mod imp {
    use super::*;

    pub fn install(_handler: TrayHandler, _paused: bool) {
        log::info!("system tray is not available on this platform");
    }

    pub fn set_paused(_paused: bool) {}

    pub fn remove() {}
}

pub use imp::{install, remove, set_paused};
