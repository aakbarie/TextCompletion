# TextCompletion

A small, fast Windows text-expansion application built in Rust.

## Goal

Type a trigger such as `;addr`, `;sig`, or `;brb` anywhere in Windows and replace it immediately with configured text.

The first release is intentionally simple: no AI, no cloud dependency, and no server requirement.

## Architecture

- **Rust** for the application and Windows integration
- **Slint** for a modern lightweight desktop UI
- **SQLite** for local persistence and offline execution
- a platform-independent expansion matcher isolated from Windows input plumbing
- eventual **Microsoft SQL Server** synchronization for shared/team/enterprise libraries

SQL Server will not sit in the typing path. The intended enterprise architecture is:

```text
SQL Server shared libraries
          |
          | sync
          v
   Local SQLite cache
          |
          v
   Expansion engine
          |
          v
Focused Windows application
```

This keeps expansion instantaneous and available when the network or VPN is unavailable.

## Current branch

`rust-mvp` establishes:

- Rust application manifest
- Slint snippet editor
- SQLite repository abstraction
- sync-ready snippet model with stable IDs, scope, version, and timestamps
- testable expansion matcher
- Windows GitHub Actions build producing `textcompletion.exe`

## V1

1. Create, edit, enable, and delete snippets.
2. Detect triggers globally while TextCompletion is running.
3. Replace a trigger in the currently focused Windows application.
4. Support multiline and Unicode replacements.
5. Search snippets quickly.
6. Import/export snippets.
7. Pause/resume expansion.
8. Run primarily from the Windows system tray.
9. Optionally start with Windows.

## Data scopes

The schema supports three scopes from the start:

- `Personal`: private/local snippets
- `Shared`: team content, synchronized later
- `Enterprise`: centrally governed content, synchronized later

Only personal/local behavior is required for v1.

## Design principles

- Local-first and offline-first.
- Expansion must feel instantaneous.
- No server round trip during typing.
- Keep matching independent of UI and Windows APIs.
- Keep persistence behind a repository interface so SQL Server synchronization can be added without replacing the core engine.
- Prefer a small native binary over a browser-shell desktop application.

## Product references

Breevy/aBreevy8, PhraseExpress, and Key2Scribe are being used as competitive/product references. See `docs/product-reference.md`.
