# Changelog

All notable changes to Scriblet are recorded here. Versions follow semantic versioning.

## [0.5.0] - 2026-09-11

### Fixed
- Windows: Shift was never detected by the keyboard hook because low-level hooks report the left and right Shift keys separately. Bindings containing capital letters or shifted symbols such as `:` now match. Caps Lock is honoured too.
- Windows: Ctrl, Alt, and Win chords no longer feed letters into the trigger buffer.
- Template variables `{{date}}`, `{{clipboard}}`, and `{{cursor}}` are now rendered when a snippet expands. Previously they were typed literally.
- Saving a snippet and syncing the enterprise library now run inside a single transaction, so a failure cannot leave a snippet without its binding or a half-replaced cache.
- An enterprise binding that collides with a personal binding is skipped and reported instead of aborting the whole sync.
- Enterprise sync runs in the background after the window appears instead of blocking startup on an unreachable SQL Server.
- Keyboard hook failures and missing macOS Accessibility permission are shown in the window instead of failing silently.
- Fatal startup errors are shown in a message box on Windows and macOS.
- A corrupt row no longer hides the whole library; it is skipped and logged.
- Databases created before v0.3 failed to open because the category index was created before the category column. Found by the new migration test.
- The test suite builds on Linux, so formatting and lint can run in CI.

### Added
- System tray icon with Open, Pause expansion, and Quit. Closing the window keeps Scriblet running.
- Pause and resume expansion from the window or the tray.
- Start at login (Windows Run key, macOS LaunchAgent).
- JSON import and export of the personal library.
- Sync now button for the enterprise library.
- Sidebar categories are built from the library instead of a fixed list, with Personal and Enterprise library filters.
- Live search, resizable window, read-only editor for enterprise snippets, and a two-step delete.
- Numpad digits decode on Windows.
- `SCRIBLET_SQL_LOGIN_TIMEOUT_SECONDS` setting, default 5 seconds.
- A log file, `scriblet.log`, in the data directory.
- Versioned SQLite schema; the legacy trigger migration now runs once instead of on every launch.
- Tag-driven release workflow that builds Windows, Intel macOS, and Apple Silicon macOS, and a CI workflow with rustfmt, clippy, and tests on Linux, Windows, and macOS.
- Committed `Cargo.lock` for reproducible builds.

### Changed
- Enterprise sync has no built-in server defaults. Set `SCRIBLET_SQL_SERVER` and `SCRIBLET_SQL_DATABASE` to enable it.
- Default ODBC driver is now `ODBC Driver 18 for SQL Server`.
- The unimplemented hotkey binding kind was removed from the model; the `kind` column remains reserved.

### Removed
- The dead `binding` module and the per-version release workflows.

## [0.4.2] - 2026-09-10
- Windows expansion submits trigger deletion, replacement text, and the trailing delimiter as one atomic SendInput batch, fixing corrupted output.
- Create snippet uses a category dropdown.
- Unreleased follow-up on main: the Windows hook moved from rdev to a native low-level keyboard hook.

## [0.4.1] - 2026-09-10
- Library-first visual polish.

## [0.4.0] - 2026-09-10
- SQL Server enterprise library sync on Windows with an offline SQLite cache.
- First-class bindings separate from snippets.

## [0.3.0] - 2026-09-10
- Functional library UI backed by SQLite, favorites, and copy to clipboard.

## [0.2.0] - 2026-09-10
- Library-first desktop interface.

## [0.1.0] - 2026-09-10
- Initial Windows and Intel macOS builds.
