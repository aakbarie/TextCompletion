# Scriblet

Scriblet is a small, fast text-expansion app for Windows and macOS, written in Rust.
Type a short binding such as `;sig` or `;p2p` in any application, press space, Tab, or Enter,
and Scriblet replaces it with the phrase you saved.

It is local-first: snippets live in a SQLite database on your machine, expansion never
touches the network, and there is no telemetry.

## Features

- Global expansion in every application, with one atomic keystroke batch per expansion
- Snippet library with titles, categories, favorites, search, and enable/disable
- Optional text binding per snippet, with collision detection
- Template variables: `{{date}}`, `{{clipboard}}`, and `{{cursor}}`
- Pause and resume expansion from the window or the tray menu
- Runs from the system tray; closing the window keeps expansion alive
- Start at login (Windows Run key, macOS LaunchAgent)
- JSON import and export of the personal library
- Enterprise library sync from Microsoft SQL Server on Windows, cached for offline use
- Multiline and Unicode phrases

## Install

Download the latest release from the GitHub Releases page:

- `Scriblet.exe`: portable Windows x64 build. It is unsigned, so SmartScreen may warn on first run.
- `Scriblet-macOS-Intel.zip` and `Scriblet-macOS-AppleSilicon.zip`: app bundles. They are not
  notarized, so use "Open" from the context menu on first launch, then grant Accessibility
  permission when prompted. Scriblet needs it to watch and replace keystrokes.

## Using Scriblet

1. Click **New snippet**, write the phrase, optionally set a binding such as `;addr`, and save.
2. In any app, type the binding followed by space, Tab, or Enter.
3. Use the sidebar to filter by favorites, personal or enterprise library, or category.
   Categories are built from the snippets you have, plus a few defaults.

Template variables inside a phrase are resolved at the moment of expansion:

| Variable        | Result                                             |
|-----------------|----------------------------------------------------|
| `{{date}}`      | Today's date, `MM/DD/YYYY`                         |
| `{{clipboard}}` | Current text clipboard                             |
| `{{cursor}}`    | Where the caret lands after the phrase is inserted |

The header shows two badges: the expansion status (active, paused, or why it is unavailable)
and the result of the last action. Errors, sync results, and hook problems are also written to
`scriblet.log` in the data directory.

Data directory:

- Windows: `%LOCALAPPDATA%\aakbarie\Scriblet\data`
- macOS: `~/Library/Application Support/com.aakbarie.Scriblet`

## Enterprise library (Windows)

Scriblet can pull a centrally governed snippet library from SQL Server using Windows
Integrated Authentication. Sync runs in the background at startup and on demand from the
sidebar, and the result is cached locally so expansion works offline. Enterprise snippets are
read-only in the editor; duplicate one to make a personal copy.

Sync is enabled by setting two environment variables:

```text
SCRIBLET_SQL_SERVER=<server host>
SCRIBLET_SQL_DATABASE=<database>
```

See `docs/enterprise-sqlserver.md` for all settings, the schema, and the privacy boundary.

## Architecture

```text
ui/main.slint          Slint window: library, editor, controls
src/main.rs            Window glue, tray, background sync, logging
src/app.rs             Save/delete/duplicate rules, filters, index rebuild
src/storage.rs         SQLite repository with transactions and versioned migrations
src/expansion.rs       In-memory trigger index and matcher (platform independent)
src/template.rs        {{date}}, {{clipboard}}, {{cursor}} rendering
src/runtime.rs         Keyboard hook and injection: Windows (WH_KEYBOARD_LL + SendInput),
                       macOS (rdev + enigo), stub elsewhere
src/enterprise.rs      Enterprise sync and the SQL Server source
src/transfer.rs        JSON import/export
src/autostart.rs       Start-at-login registration
src/tray.rs            Tray icon and menu
src/platform.rs        File dialogs and fatal-error message box
```

SQL Server never sits in the typing path:

```text
SQL Server shared library ──sync──▶ local SQLite cache ──▶ in-memory index ──▶ keyboard hook
```

## Building

```sh
cargo build --release --bin scriblet
```

The core library, tests, and a non-expanding window build on Linux too, which is what CI
uses for formatting and lint. Global expansion and the tray are implemented for Windows and
macOS only.

## Testing

```sh
cargo fmt --all -- --check
cargo clippy --all-targets -- -D warnings
cargo test --all-targets
```

Tests cover storage and migrations, transactions, the matcher, templates, the save rules,
import/export, enterprise sync against a fake source, the Windows key decoder, and a 200-case
synthetic MD/RN expansion suite.

## Releasing

1. Bump `version` in `Cargo.toml` and both version keys in `packaging/macos/Info.plist`.
2. Add a `## [x.y.z]` section to `CHANGELOG.md`.
3. Tag and push: `git tag vX.Y.Z && git push origin vX.Y.Z`.

The release workflow verifies the three versions match, runs the tests on every platform,
builds Windows, Intel macOS, and Apple Silicon macOS binaries, and publishes a GitHub release
with the changelog section as its notes.

## Product references

Breevy/aBreevy8, PhraseExpress, and Key2Scribe are used as product references.
See `docs/product-reference.md`.
