# Scriblet Windows input strategy, v0.5.3

## Problem reproduced on a single process

Live Windows testing showed corrupted expansions even after duplicate-process protection was in place. Example output from an ASCII replacement such as `testing scriblet 4.2` included repeated characters and partial replacement text.

## v0.5.3 change

Windows now uses a dedicated runtime facade with two changes:

1. A matched Space, Tab, or Enter is suppressed on key-down, but the replacement is queued only after the matching physical key-up.
2. Ordinary ASCII replacement text is emitted using normal virtual-key input. `KEYEVENTF_UNICODE` is retained only as a fallback for characters that do not have a direct key mapping.

The existing target/focus validation, injected-event filtering, pause state, mouse/caret cancellation, and single-instance protection remain in place.

## Privacy

Diagnostics log only event counts and errors. Scriblet does not log the replacement phrase or raw typed keystrokes.

## Manual acceptance test

With exactly one Scriblet process running, create a snippet with binding `;brb` and phrase `testing scriblet 4.2`. In Notepad, type `;brb` followed by Space once. Expected output is exactly `testing scriblet 4.2 `, with no repeated characters and no second expansion on a later delimiter.
