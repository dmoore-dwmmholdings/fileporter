# First Docwalk trial

Use the native Fileporter app. Its React frontend calls Tauri directly; a Vite
browser preview does not provide the native backend, file pickers, or transfers.

## Verified on 2026-09-07

Docwalk builds locally, including its optional `agent` feature. The workspace
test suite passed: 773 tests, zero failures, with the cached Chromium selected
through `DOCWALK_BROWSER`; real web recording and replay were exercised.
Outside the execution sandbox, macOS Screen Recording
and Accessibility checks pass. Attaching to the already running Fileporter app
captured a settled 900×680 screenshot. The session was closed afterward and
Fileporter was left running. No settings were changed or transfers initiated.

The initial accessibility tree returned `role: unavailable` and
`the application exposes no accessibility windows`. That message did not
establish that Fileporter lacks accessibility support: Docwalk collapses AX
read failures and empty window arrays into the same result. See the follow-up
diagnosis below. Keep the configured replay thresholds and locator requirements.

## Start a session

From this repository, with Fileporter already open:

```sh
DOCWALK="$(cd ../docwalk && pwd)/target/debug/docwalk"
"$DOCWALK" doctor
"$DOCWALK" attach --title-regex '^Fileporter$'
# Copy the returned session ID into SESSION:
SESSION='<returned session ID>'
"$DOCWALK" shot --session "$SESSION" --tree
"$DOCWALK" close --session "$SESSION"
```

Keep Fileporter visible and frontmost throughout capture. Run these commands
from a terminal with native permissions; sandbox checks can falsely report
missing permissions. In an agent execution environment that cleans up child
processes after each command, run attach, subsequent actions, and close in one
execution. Otherwise the capture daemon can disappear between commands.

The configuration's launch path points to the existing release binary and
assumes this working directory. If needed, rebuild with `pnpm exec tauri build`.
Prefer attaching for the first trial: launching another instance may reuse the
same application data. Fileporter currently uses Tauri's app data directory;
Docwalk's `--profile` does not establish isolation for this app.

## Bounded first walkthrough

Start with the existing main window and record a short orientation tour:
Transport → Pads → Log → Config → Transport. Inspect each fresh screenshot
and tree before acting. Find and activate controls by role and name only:

```sh
"$DOCWALK" find --session "$SESSION" --role button --name Pads
"$DOCWALK" click --session "$SESSION" --locator '{"ax":{"role":"button","name":"Pads"}}'
"$DOCWALK" shot --session "$SESSION" --tree
```

An unavailable tree is a diagnostic failure, not permission to substitute
coordinates, OCR, or image matching. Do not change settings, pair devices,
or send files as part of this orientation trial.

Follow `../docwalk/SKILL.md` for `tour new`, `step`, `tour finalize`, and `export`.
Use a named tour such as `explore-fileporter`; export HTML and Markdown with:

```sh
"$DOCWALK" export --tour explore-fileporter
```

Generated sessions and raw captures live in ignored `.docwalk/`. Review device
names, transfer history, paths, and screenshots before committing tour packages
or publishing exports. Use Docwalk's source-redaction workflow when necessary.
No tutorial package has been recorded by this setup.

Attached tours are suitable for this first exploration but cannot replay
unattended: they do not record how to launch the app. Before a replayable tour,
resolve accessibility targeting and establish a repeatable app state. A real
send/receive tutorial additionally needs an explicit test file and second peer.

## Accessibility follow-up — 2026-09-07 (America/Chicago)

### Diagnosis and evidence

Fileporter **already exposes native accessibility**. No Tauri flag, WebKit
activation flag, private API, or custom app accessibility bridge was added.
The old trial's exact cause cannot be recovered because its AX error was lost.
The following observations distinguish failures that its message conflates:

| Observation | Verified result |
| --- | --- |
| Permission checks outside the sandbox | Screen Recording and Accessibility both passed. |
| Original running app | CG window 19915 belonged to PID 26760; 900×680 at (306,86). Docwalk's daemon log explicitly identified PID 26760 as the owner it passed to `Drive::attach`. The attach response's `app_pid: null` means Docwalk did not launch it; it is not the AX query PID. |
| Before app activation | A direct `AXUIElementCopyAttributeValue(AXWindows)` succeeded with an empty array. This was a genuine empty list, not an AX error. |
| After normal `NSRunningApplication.activate` | At 1 s and 4 s: one AXWindow, groups, AXScrollArea, AXWebArea, named navigation, headings and controls. No explicit AX activation attribute was set. Focus/visibility affected this reproduction; it does not identify the old trial's unrecorded error. |
| Original app navigation | Two complete Transport → Pads → Log → Config → Transport sequences, with close/fresh attach between them. Every `find` returned one button with nonzero bounds; every click resolved at tier 1 with `drift: false`. Native screenshots showed the expected screens. |
| Updated trial, initial launch | PID 42439, window 20385: one native window and web-content descendants, including the corrected frontend. Native Tab/Space navigation and menu arrow/Escape focus assertions passed. |
| PID-based attach edge case | An attach specifying both PID 42439 and `^Fileporter$` selected a **66×20 window named “Window”** (titlebar overlay), so Transport was absent. Main-window raw AX inspection remained available. This is Docwalk window selection, not missing frontend semantics. |
| Stale capture session | Following an overlapping capture attempt, the retained session's screenshots stayed on Transport while its AX tree correctly changed to Pads/Log/Config. `settle: unchanged`, `settled: false`, and ages exceeding 290,000 ms exposed the stale frames. Those screenshots are **not** visual verification of the keyboard actions. |
| Later hidden window | The exact unavailable-tree message recurred with the main CG window still enumerated but not on screen; direct AXWindows succeeded with count 0. A CG window record alone does not prove an accessible, visible window. |
| Final rebuilt trial startup | PID 45222 was waiting in Keychain authentication. Direct AXWindows returned **-25204 (`kAXErrorCannotComplete`)**, not an empty array. SecurityAgent displayed “Fileporter wants to use your confidential information stored in ‘io.fileporter.desktop’ in your keychain” and requested the login Keychain password. Startup was not complete; a web-content tree cannot be inferred from this read. |

Representative original navigation bounds, relative to the captured window:
Transport `[136,54,57,20]`, Pads `[212,54,30,20]`, Log `[261,54,21,20]`,
Config `[301,54,38,20]`. The app exposes AXButton names directly. WebKit maps
pressed retention buttons and switches to AXCheckBox with AXValue 0/1; the
original Config tree reported Forever=1, other retention choices=0, and disabled
Apply/Discard. Text fields exposed their labels and actual values. Native
navigation exposes `AXARIACurrent`; Docwalk currently does not serialize it.

After the user authorized Keychain access and closed the app, a fresh launch of
the final trial succeeded as PID **47386**, without another authentication prompt.
The final build passed two full role/name sequences, with a fresh attach between
sequences. All clicks resolved at tier 1 without drift, every screen had its
expected heading, and fresh screenshots were visually inspected. The corrected
Add/Choose names and disabled Apply/Discard states passed the native assertions.
Sessions: `20260908035234-attached-801e4c47` and
`20260908035322-attached-72cce3d4` (snapshots 0000–0004 in each).

Apple Accessibility Inspector independently confirmed the same final process.
After selecting `Fileporter (47386)` in its process menu and targeting Transport,
its inspection pane displayed Title **Transport**, Type **button**, action
**press**, and **ARIA Current = page**. Its hierarchy contained Fileporter
(application, TaoApp) → Fileporter (standard window, TaoWindow) → groups →
scroll area → Fileporter (HTML content) → Main navigation, with Transport,
Pads, Log and Config buttons. The Inspector pane and hierarchy were captured
through Inspector's own AX tree in `resumed-inspector.txt`. Earlier attempts
that left the pane unselected are superseded by this successful comparison.

Final-build keyboard verification also passed in session
`20260908035724-attached-8754b0ea`: public AX focus on Transport, then Docwalk
`key --keys tab` / `key --keys space` reached Pads, Log and Config. AXPress on
“Send files or folders” opened the menu; Down/Up moved between Browse folder
and Browse files; Escape removed the menu and restored trigger focus. Screenshots
0002, 0004, 0006–0010 were fresh and the resulting screens/focus were visually
checked (0009 repeats the verified Browse files focus). One intermediate
pre-activation Config focus shot was stale and is not counted as visual evidence;
the subsequent Config activation shot was fresh. No picker item was activated.
The exact key command form is `"$DOCWALK" key --session "$SESSION" --keys tab`
(substitute `space`, `down`, `up`, or `escape`); inspect with `shot --tree` after
each step and close the session in the same controlling process.

All detailed artifacts are local and ignored under `.docwalk/a11y-evidence/`:

- `raw-ax.txt`, `attach.json`, `shot.json`: original PID/AX reproduction.
- `navigation.txt`, `nav1-*-{find,click,shot}.json`, `nav2-*-{find,click,shot}.json`:
  successful original navigation. Screenshots are at each JSON's `image` path.
- `trial-ax.txt`, `trial-shot.json`, `keyboard-*.json`, `menu-*.json`: updated
  native tree/focus results; heed the stale-frame limitation above.
- `updated-navigation/`: failed PID attach, with its 66×20 window recorded in
  `.docwalk/sessions/20260908033347-pid-42439-d131846f/daemon.log`.
- `updated-navigation-main-window/`, `updated-navigation-sequential/`: failed
  capture/visibility runs retained rather than counted as passing tests.
- `inspector-*.txt`, `inspector-transport.png`: Inspector automation attempt.
- `final-launch-ax.txt`, `keychain-prompt.txt`, `final-native-run.txt`: final
  startup blocker, including preserved AX error codes.
- `resumed-navigation-prepared/`: passing final-build navigation, owner logs,
  exact role/name locators, fresh screenshots and trees for both sequences.
- `resumed-inspector.txt`: successful Inspector pane and full hierarchy evidence.
- `resumed-keyboard/`: final native key actions, focus trees and screenshots.
- `trial-build.log`, `standard-build.log`: native build output.

### Small frontend fixes and sequential audit

The accessibility audit used the installed claude-design-system workflow, with
contrast, semantics, keyboard/focus, and motion/forms reviewed in that order.

1. **Contrast:** ordinary dim text is 7.11:1 on `--bg`; dimmer text 4.86:1,
   navigation 6.06:1, and accent focus 12.42:1. The `.act` container reduced enabled
   row-action text to approximately **2.09:1** at opacity 0.4. It now retains full
   opacity. Existing colors, layout, decorative art and animations are retained.
2. **Semantics:** navigation was already correctly named native buttons in a
   labeled landmark with `aria-current`. Decorative SVGs/glows were already
   hidden without hiding interactive descendants. Native WebKit exposed two
   genuine label problems: the Add field was named “Add a pad by address Add”,
   and its button inherited the field label; Choose inherited its surrounding
   label and help text. Labels now bind explicitly to their input, outside the
   adjacent action button. Receive-directory and listen-address help is linked
   with `aria-describedby`.
3. **Keyboard/focus:** the Transport menu now supports Up/Down/Home/End, closes
   on Tab, and restores trigger focus on Escape or item activation. Rename
   restores focus after editing ends. Pending pairing dialogs cycle Tab only
   through enabled actions and restore prior focus when removed. Pairing remains
   an explicit Confirm/Reject action; Escape does not implicitly reject a peer.
   Main navigation keeps focus on its button, preserving sequential Tab order;
   tray navigation already focuses Config.
4. **Motion/forms:** reduced-motion overrides already disable the animations;
   inputs, pressed/checked states, disabled actions, headings and progress values
   already use semantic attributes. No transfer, trust, pairing-protocol,
   persistence, or operating-system settings logic changed.

Tests cover menu navigation/cancellation, field/action label separation, rename
cancellation focus, and the disabled-confirm dialog Tab boundary. Dialog and
rename fixes have DOM regression coverage; their conditional native flows were
not exercised because this empty trial contains no peers or pending requests. No real pairing, file
picker selection, setting application, or file send was used to test them.

### Repeatable commands

The diagnostic reader preserves errors and compares activation timing:

```sh
swiftc -module-cache-path /tmp/fileporter-swift-cache \
  scripts/inspect-macos-accessibility.swift -o /tmp/fileporter-ax
/tmp/fileporter-ax <actual-window-owning-pid> --activate \
  > .docwalk/a11y-evidence/reproduction-ax.txt
```

Run outside sandbox restrictions. `AX_ERROR … code=-25204` is a failed read;
`windows_count=0` is a successful empty array. A window with no AXWebArea children
is a third result, requiring a different investigation. The reader uses public
ApplicationServices APIs and is not included in Fileporter.

With exactly one visible, onboarded Fileporter main window, use the strict
navigation smoke test (replace the PID with the actual owner):

```sh
# Actual final-build run (PID changes after relaunch):
open -n .docwalk/a11y-trial-final/Fileporter.app
/tmp/fileporter-ax 47386 --activate
python3 scripts/check-native-accessibility.py --pid 47386 \
  --out .docwalk/a11y-evidence/resumed-navigation-prepared
```

It selects by exact title, checks the daemon's resolved owner PID **before any
input**, and performs two complete attach/find/click/shot/close sequences in one
process. It first activates Config by role/name so the initial Transport action changes
the screen; an idempotent click can legitimately produce no new frame. It checks
tier-1 resolution, bounds, screen headings, corrected names,
and disabled Apply/Discard. It rejects stale/unsettled captures. Inspect the
saved screenshots too; AX assertions alone do not establish visual rendering.
It deliberately fails against the old frontend's incorrect Add/Choose labels.
Do not run two capture sessions concurrently. Do not treat a failed run as a
passing replay by replacing its locator or relaxing freshness requirements.

For a manual sequence, keep attach, all actions, and close within the same shell
or Python process if the execution host cleans up children between calls.
Use only `{"ax":{"role":"button","name":"Transport"}}` (and the other
three names) as navigation locators. Never rely on `app_pid: null` to infer that
Docwalk queried PID zero; inspect the daemon's owner log and CGWindowOwnerPID.

Validation run:

```sh
pnpm typecheck
pnpm lint
pnpm test
pnpm exec tauri build --bundles app --config '{"identifier":"io.fileporter.accessibility-trial"}'
pnpm exec tauri build --bundles app
```

Typecheck and lint passed; **59 tests passed**. Both the isolated and standard
native bundles built successfully; the standard output has identifier
`io.fileporter.desktop`. Native Rust source was unchanged; the Tauri build compiles the real
Rust host and bundled frontend. Final native navigation was independently
verified as recorded above.

### Precise Docwalk follow-up (read-only review)

1. `../docwalk/crates/docwalk-drive/src/macos/ax.rs`:
   `copy_attribute` returns `None` for **every** non-success AXError;
   `copy_children` and `target_window` propagate that `Option`; `snapshot`
   consequently labels all such failures “the application exposes no
   accessibility windows”. Preserve a typed result through structural reads,
   including the attribute, numeric/named AXError and target PID. Distinguish
   success with an empty array, unsupported/no-value attributes, malformed types,
   dead/unresponsive processes and permission/messaging failures. Surface child
   read failures as an incomplete tree rather than silently treating them as
   leaves. Bounded retries may be appropriate for cannotComplete; do not invent
   an accessibility tree or suppress the final error. Test these cases separately.
2. `../docwalk/crates/docwalk-cli/src/session/daemon.rs`:
   `find_window` tries `frontmost_capturable_window(pid)` before its title match;
   the helper accepts any titled layer-0 capturable window without a size or
   semantic check. When both selectors are supplied, honor their intersection
   and reject transient titlebar overlays. Reuse the dialog plausibility checks
   from `window_to_follow` where appropriate. Add a regression with a 66×20
   “Window” above a 900×680 “Fileporter”, plus an actual modal-dialog case.
3. Treat screenshot freshness independently from AX success. The existing
   `settle`, `settled` and `age_ms` fields already identify stale captures; any
   automation consuming `shot --tree` must reject mismatched stale-image/live-tree
   observations. Investigate capture restarts separately if overlapping sessions
   or visibility changes reproduce this. No claim is made that the capture
   backend cause was fully diagnosed here.
4. `Walk::node` currently serializes value/enabled/focused, but not current,
   selected or expanded state. Extend the shared tree model and macOS reader if
   replay needs those assertions; do not add duplicate frontend state just to
   compensate for fields the reader omits.

No file in `../docwalk` was changed.

### Remaining native and state-isolation limits

The final rebuilt trial completed both role/name sequences and the independent
Inspector comparison. The startup authentication blocker was resolved by the
user's direct approval; no password was handled by automation. This does not
guarantee that future rebuilds will avoid Keychain prompts. Conditional native
rename and pending-pairing dialog flows remain untested; their DOM regressions
passed. No custom accessibility bridge or native product-code change was needed.

The trial used `io.fileporter.accessibility-trial` and an empty database seeded
from the repository migrations, with onboarding complete, a test receive folder,
receiving off, automatic trust off, notifications off and launch-at-login off.
No production database, peers, history or transfer payloads were copied. The
trial data lives at `~/Library/Application Support/io.fileporter.accessibility-trial/`;
the test receive folder and bundle copies are inside ignored `.docwalk/`.
However, `src-tauri/src/secret_store.rs` deliberately fixes the Keychain service
and account scope to `io.fileporter.desktop` / `local-device-v1`, independent of
bundle identifier. Both existing entries were confirmed by metadata-only checks;
the app's reader reuses them. A different identifier or Docwalk `--profile` does
**not** isolate identity. Freshly signed/rebuilt binaries may require Keychain
approval and can block startup before an accessible main window exists.

Attached tours still lack a replayable launch recipe. Network discovery, history
timestamps, animations, window visibility and user state can affect replay;
no deterministic unattended replay or file-transfer tour is claimed. Existing
`threshold = 0.02`, `fail_on_drift = false`, and `locator_min_tier = 2` were not
changed. The smoke test itself insists on tier 1.

References: [Apple AXUIElementCopyAttributeValue](https://developer.apple.com/documentation/applicationservices/1462085-axuielementcopyattributevalue),
[Apple Accessibility Inspector](https://developer.apple.com/documentation/accessibility/accessibility-inspector),
[W3C menu-button keyboard pattern](https://www.w3.org/WAI/ARIA/apg/patterns/menu-button/).
The installed macOS SDK's `HIServices.framework/Headers/AXUIElement.h` was also
checked: it documents messaging failures separately from no-value attributes
and recommends retry/timeout consideration for cannotComplete.
