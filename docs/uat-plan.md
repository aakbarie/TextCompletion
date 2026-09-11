# Scriblet UAT plan

Two layers:

1. **Automated scenarios** run in CI on every platform: `tests/uat_scenarios.rs` plays out user
   stories through the same service layer the window uses, and `tests/uat_synthetic.rs` runs 200
   MD/RN trigger cases. They cover storage, sync, import/export, templates, and the matcher.
2. **Manual desktop checks** below, for the parts no test can reach: the keyboard hook, the
   window, the tray, dialogs, and start-at-login. Run them on a real Windows and macOS machine
   before each release and record the results in the table at the end.

Build under test: `Scriblet.exe` or `Scriblet.app` from the release workflow, not a dev build.

## Setup

- Fresh user profile, or delete the data directory first
  (Windows `%LOCALAPPDATA%\aakbarie\Scriblet\data`,
  macOS `~/Library/Application Support/com.aakbarie.Scriblet`).
- Open Notepad (Windows) or TextEdit in plain-text mode (macOS), a browser text box, and Word or
  Outlook if available. Expansion must behave the same in all three.
- For enterprise checks, set `SCRIBLET_SQL_SERVER` and `SCRIBLET_SQL_DATABASE` for one account
  and leave them unset for another.

## A. First launch

| # | Step | Expected |
|---|------|----------|
| A1 | Launch Scriblet | Window opens. Header shows "Expansion active" with a green dot within a second. Status badge says "Ready · enterprise sync not configured" (Windows, no env) or "Ready · enterprise sync configured". |
| A2 | macOS only: first launch without Accessibility | Header shows "Grant Accessibility permission to enable expansion" and the system prompt appears. After granting and relaunching, A1 applies. |
| A3 | Launch Scriblet a second time while it is running | A dialog "Scriblet is already running" appears and the second process exits. No second tray icon. |
| A4 | Look at the data directory | `scriblet.db`, `scriblet.lock`, and `scriblet.log` exist. The log records the version and "low-level keyboard hook installed" (Windows). |

## B. Snippets and expansion

| # | Step | Expected |
|---|------|----------|
| B1 | New snippet: title "Peer to peer", category Clinical, binding `;p2p`, phrase "Peer-to-peer review completed." Create | Status "Saved". Card appears in the list with the `;p2p` badge under Clinical. |
| B2 | In Notepad type `;p2p` then Space | Trigger is replaced by the phrase followed by one space. No leftover characters, no double space. |
| B3 | Repeat B2 with Tab and with Enter | Same replacement; Tab inserts a tab, Enter a newline after the phrase. |
| B4 | Type `;p2px` then Space | Nothing expands. Text stays as typed. |
| B5 | Type `;p2`, press Backspace twice, type `2p`, Space | Expands. Backspace edits the candidate correctly. |
| B6 | Binding with a capital and a shifted symbol: `;Sig!` phrase "Signed" | Typing `;Sig!` then Space expands. Typing `;sig!` does not. (Windows: this was broken before 0.5.0.) |
| B7 | Turn Caps Lock on, type `;SIG!` | Does not expand (Caps Lock inverts letters). Turn Caps Lock off. |
| B8 | Type `;p2` then press Ctrl+S (or Cmd+S), then `p` and Space | Nothing expands; the shortcut cleared the candidate. |
| B9 | Phrase with `{{date}}`, `{{clipboard}}`, `{{cursor}}`: "On {{date}} re: {{clipboard}} — {{cursor}} done". Copy "Jane Doe" to the clipboard, expand | Today's date as MM/DD/YYYY, "Jane Doe" inserted, no marker text, and the caret sits right after "— " with " done " after it. |
| B10 | Copy text containing `{{cursor}}` to the clipboard and expand a phrase using `{{clipboard}}` | The literal characters `{{cursor}}` from the clipboard are typed; the caret is not moved by them. |
| B11 | Multi-line Unicode phrase ("Reviewed:\n• Criteria met\n• Médico notified") | All lines and characters appear correctly in Notepad, the browser, and Word. |
| B12 | Uncheck Enabled on a snippet, save, type its binding | No expansion. Re-enable and it expands again. |
| B13 | Edit a snippet's binding from `;p2p` to `;peer` | Old binding no longer expands; new one does. |
| B14 | Try to give a second snippet the binding `;peer` | Status "Binding ;peer is already in use"; nothing saved. |
| B15 | Delete a snippet | First click shows "Confirm delete" and "Keep"; only "Confirm delete" removes it. Its binding stops expanding. |

## C. Expansion safety (0.5.1)

| # | Step | Expected |
|---|------|----------|
| C1 | Type `;p2p`, click into a different application window, then press Space | No expansion anywhere. The click cleared the candidate. |
| C2 | Type `;p2p` in Notepad, Alt+Tab to another window, press Space | No expansion. |
| C3 | Type `;p2p` and press Space, then immediately type `xyz` as fast as possible | Either the full phrase appears followed by `xyz`, or the replacement is skipped and the log says "expansion skipped: … keystroke(s) arrived". Never a phrase with `xyz` inside it or missing characters. |
| C4 | Repeat B2 twenty times quickly in a row | Twenty clean expansions, no corruption. |
| C5 | Windows: type `;p2p` in Notepad, then click into Word in the same second as pressing Space | Word receives nothing from Scriblet; log shows "the target window changed" or the click reset. |

## D. Pause, tray, lifecycle

| # | Step | Expected |
|---|------|----------|
| D1 | Click "Pause expansion" | Header shows "Expansion paused" with an amber dot. Triggers do not expand. Tray menu shows "Pause expansion" checked. |
| D2 | Resume from the tray menu | Header returns to "Expansion active"; the window button reads "Pause expansion" again. |
| D3 | Close the window with the X | Window disappears; tray icon remains; typing a binding still expands. |
| D4 | Tray: Open Scriblet (or left-click / double-click the icon) | Window returns with the library intact. |
| D5 | Tray: Quit Scriblet | Process exits, tray icon disappears, bindings stop expanding, `scriblet.lock` can be deleted. |
| D6 | Check "Start at login", sign out and in (or reboot) | Scriblet starts, tray icon present, expansion active. Uncheck and repeat: it does not start. |

## E. Library, search, import/export

| # | Step | Expected |
|---|------|----------|
| E1 | Create snippets in Clinical, RN, and a new category "Medical Director" | Sidebar lists Medical Director and RN alongside the defaults; clicking a category filters the list. |
| E2 | Type in the search box | The list filters as you type, matching title, category, phrase, and binding. Clearing shows everything. |
| E3 | Mark a snippet Favorite | It sorts to the top with a star; the Favorites filter shows only it. |
| E4 | Export… | Save dialog defaults to Documents with a dated file name; status shows the count and path. File is readable JSON. |
| E5 | Import… the same file | Status "Imported N snippets"; no duplicates created. Import on a machine where a binding is taken reports "binding skipped (in use: …)". |
| E6 | Resize the window down to 900×600 and up to full screen | Layout stays usable; nothing overlaps; the list scrolls. |

## F. Enterprise sync (Windows)

| # | Step | Expected |
|---|------|----------|
| F1 | Launch with the env vars set and the server reachable | Window opens immediately; status changes to "Syncing enterprise library…" then "Enterprise synced · N snippets · M bindings". Enterprise cards show "Enterprise · category". |
| F2 | Select an enterprise snippet | Title "View snippet", fields disabled, "Enterprise · read-only" label, Delete hidden, Duplicate available. Duplicate creates an editable personal copy without a binding. |
| F3 | Create a personal snippet whose binding equals an enterprise one, then Sync now | Status lists the trigger as skipped; typing the trigger produces the personal phrase. |
| F4 | Disconnect from the network, launch again | Window opens without delay; status "Offline · using cached enterprise library (…)"; enterprise triggers still expand. |
| F5 | Retire a row on the server, reconnect, Sync now | The snippet disappears from the library and stops expanding; personal snippets untouched. |
| F6 | Swap two triggers between two enterprise rows on the server, Sync now | Both new assignments apply; nothing reported as skipped. |
| F7 | Save a personal snippet while a sync is in progress, then make the sync fail (pull the network mid-sync) | The personal snippet is still there after the failure. |

## Results

| Build | Platform | Tester | Date | Pass | Fail | Notes |
|-------|----------|--------|------|------|------|-------|
| 0.5.1 | Windows 11 | | | | | |
| 0.5.1 | macOS Intel | | | | | |
| 0.5.1 | macOS Apple Silicon | | | | | |

A release is UAT-complete when every row in sections A through E passes on Windows and macOS,
and section F passes on Windows. Log any failure with the step number and the relevant lines
from `scriblet.log`.
