# Changelog

All notable changes to Scriblet are recorded here. Versions follow semantic versioning.

## [0.5.2] - 2026-09-11

Windows upgrade hotfix. This release also includes the v0.5.1 correctness fixes, which were merged to main but did not receive a standalone GitHub release.

### Fixed
- **Legacy v0.5.0 tray instances are detected before a second hook is installed.** v0.5.0 could remain hidden in the system tray while a newer Scriblet was launched, leaving two `WH_KEYBOARD_LL` hook chains active. The first process could consume the first Space/Tab/Enter while the older process remained armed and expanded on the next delimiter, producing delayed duplicate or interleaved text. Newer Scriblet builds now detect the legacy top-level window and refuse to install a second global hook.
- Includes the v0.5.1 target/focus verification, input-interruption cancellation, mouse/caret reset behavior, and single-instance file lock for current versions.

## [0.5.1] - 2026-09-11

Correctness fixes from the v0.5.0 code review. No new features.

### Fixed
- **Transactions are now isolated.** The transaction wrapper released the connection while its work ran, so a snippet saved during a background enterprise sync could be silently rolled back with the sync. The connection is now held for the whole transaction and other threads wait. Covered by a two-thread regression test.
- **Expansion targets are verified.** On Windows the hook records the foreground window and focused control when a trigger fires, and the injection is dropped if either changed. Mouse clicks and window switches reset the pending trigger. On both platforms, any keystroke typed while an expansion is in flight aborts the expansion instead of interleaving with it.
- **Enterprise sync is order-independent.** Old enterprise bindings are cleared before the new library is applied, so two enterprise snippets can swap triggers. Collisions are checked only against personal bindings and duplicates within the incoming library.
- **Single instance.** A second launch shows a message and exits instead of installing a second keyboard hook and expanding every trigger twice.
- **Template rendering** tokenizes the template once. Clipboard text containing `{{cursor}}` or `{{date}}` is typed literally, and only the first `{{cursor}}` positions the caret.
- **Window layout.** Hidden action buttons still reserved space, so the editor pane overflowed the window; actions now sit on two rows and are only created when applicable. Minimum window size is 1100×620, verified in the widest editor state.
- **Category dropdown** did not follow the selected snippet's category (Slint ComboBox ignores `current-value`); it is now driven by index.

### Added
- `tests/uat_scenarios.rs`: twelve end-to-end user stories (create, edit, disable, restart, upgrade from v0.4, enterprise arrive/offline/retire, collision, save during failing sync, export/import between machines, templates, search and filters) run in CI on every platform.
- `docs/uat-plan.md`: manual desktop checklist for Windows and macOS with expected results and a results table.

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
