# TextCompletion

A lightweight Windows text-expansion utility.

## Goal

Type a short trigger such as `;addr` or `;sig` anywhere in Windows and have it replaced immediately with the configured expansion.

## Initial architecture

- C# / .NET 10
- WPF desktop UI
- Win32 `WH_KEYBOARD_LL` global keyboard hook
- Win32 `SendInput` for replacement text
- Local snippet store, starting with JSON and moving to SQLite when search/history/sync justify it
- System tray operation

## MVP

1. Create, edit, enable, and delete snippets.
2. Detect triggers globally while the app is running.
3. Replace the trigger in the active application.
4. Support multi-line expansions.
5. Start with Windows and live primarily in the system tray.
6. Provide pause/resume and per-app exclusions.

## Design principles

- Local-first and offline by default.
- Fast enough that expansion feels instantaneous.
- Minimal permissions and no cloud dependency.
- Keep the text-matching engine separate from Windows input plumbing so it can be tested independently.

## Planned project structure

```text
src/
  TextCompletion.App/       WPF application and settings UI
  TextCompletion.Core/      matching, snippets, expansion rules
  TextCompletion.Windows/   keyboard hook and SendInput integration
tests/
  TextCompletion.Core.Tests/
```

The first implementation branch will establish this structure and a minimal end-to-end expansion engine.
