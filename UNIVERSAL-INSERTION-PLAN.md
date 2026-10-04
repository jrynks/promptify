# Universal text insertion plan

Status: shared delivery foundation implemented; complete universal feature and
cross-platform certification remain open.

## Implementation evidence and remaining gates

- Synchronized with origin/main at
  `41911122d4e8267d7b4974c0e2d716fb7bd95cb0`, preserving both research documents.
- Added metadata-only destination contracts and exact-field revalidation.
  Confirmed writable inputs can authorize adaptive insertion independent of
  app/site classification; protected inputs block Prompt and Dictation.
- Added bounded native inspection adapters. Platform-specific acceptance is
  tracked separately; source presence does not certify operating behavior.
- Removed app-specific insertion and physical-keyboard settings from the UI.
  Obsolete fields remain readable but are omitted on save; obsolete setters
  report retirement. Raw evdev monitoring and its dependency are removed.
- Implemented permissioned Wayland GlobalShortcuts activation and bounded
  AT-SPI active-window discovery. Added macOS Accessibility permission requests.
  These adapters require native runtime validation before certification.
- One common persisted integration control enables or disables insertion.
  On Wayland, input and shortcut sessions are acquired together from the
  explicit action, with rollback on failure, rather than restoring paste alone
  at startup. Permission changes invalidate in-flight jobs, including after
  disable/re-enable; onboarding respects disabled integration.
- Reports distinguish native dispatch using `sent_unverified`; the overlay
  says Paste sent rather than implying a field receipt. Native clipboard
  transactions are serialized and restoration errors are surfaced.
- Windows browser textarea acceptance: 20 each single-line, multiline and
  Unicode selected-text replacements passed. Real adaptive local-model output
  also reached a native browser textarea exactly. This is not certification of VS Code chat,
  every field, other platforms, or the complete voice-to-prompt flow.
- Open engineering gates: controller-only activation, full-format clipboard
  preservation/acknowledgement, optional adapters, packaging compatibility,
  and a sanctioned Gamescope activation/focus/delivery route. Wayland portal
  and accessibility paths are implemented but not runtime-certified.
- Open environment gate: no authorized interactive macOS/Linux/Steam Deck
  device has been validated in this session. A Docker Desktop WSL instance
  cannot substitute for those environments.
- Latest shared validation: 187 core tests, 44 desktop tests, 46 browser tests,
  frontend build, native caller checks and strict core/desktop-library Clippy
  pass. Supplementary Windows-host harnesses pass 11 Linux destination tests,
  two X11 modifier tests and four portal schema tests; they do not exercise a
  Linux compositor or portal at runtime.
- Clipboard snapshot failures now block staging rather than being mistaken for
  empty content. Staged text is verified before dispatch; restoration-read
  failures are surfaced. A shared delivery guard is checked by native backends
  after staging/focus lookup so cancellation and integration-generation changes
  cannot silently proceed through that preparation interval. Empty/whitespace
  destination identities cannot establish a confirmed field.
- Clipboard snapshots remain in memory after staging or verification errors.
  Explicit restoration retains the snapshot on read/write errors for retry;
  changed clipboard contents discard the stale snapshot without restoration.
  Production transaction tests cover these paths without dispatching keys.
- Attended Windows testing exposed arboard's generic error for absent HTML.
  Windows now queries HTML format availability before reading, without treating
  genuine generic read failures as empty content. Clipboard change counters
  protect snapshot consistency and prevent restoring over newer identical text.
  Windows restoration writes previous text, HTML/text alternatives, copied-file
  lists, images, and empty state through a persistent clipboard owner window.
  Native HTML, file-list, and DIBV5 serialization tests pass.
- Test-only Windows focus setup uses a temporary input-thread attachment with
  guaranteed detach and rechecks the unique test window after CLI launch.
  Native paste events now establish that the expected payload can reach the
  field; runs contaminated by typing are failures, not acceptance passes.
- These checks do not solve clipboard-consumption acknowledgement. Generated text
  now remains on the clipboard until an explicit Restore action in the overlay or
  General settings. The saved snapshot is memory-only and lost on app exit.
  Preservation includes plain text, HTML plus its
  text alternative, copied-file lists, and images; incompatible combinations
  block staging rather than silently losing a component. Custom application
  formats and arbitrary multi-format snapshots remain unsupported.
  Full-format snapshots and a reliable native consumption/receipt path remain
  engineering gates, not certified behavior.
- Latest Windows acceptance passes 60 exact selected-text replacements each in
  bundled Chromium, installed Edge, installed Chrome, and the isolated installed
  VS Code editor. Real adaptive local-model output reaches each target exactly,
  without a software-specific consent toggle. Test-only focus setup is working
  without removing production focus checks. An earlier bundled-Chromium run
  delivered empty paste payloads even though an independent clipboard read saw
  the staged text; the current loopback-origin harness and owned restoration
  path pass, but the original intermittent cause is not established. These
  samples do not certify
  VS Code chat or universal Windows reliability. The test-only
  `PROMPTIFY_TEST_BROWSER` variable permits independent installed-browser checks.
- A cold VS Code accessibility provider initially exposed no writable pattern.
  Bounded metadata-only readiness checks (at most three 50 ms waits) now handle
  provider initialization across Windows applications without assuming writable
  inputs, overriding protection, or retrying paste. Readiness tests execute as
  top-level tests. Clipboard recovery tests exercise the production transaction;
  overlay tests cover failed restoration and stale completion responses.
- The latest embedded desktop executable builds successfully. Rebuilding the
  unchanged inference worker encountered the existing Vulkan shader-generator
  CMake/install failure; native generation checks use the compatible existing
  worker. This is not a clean all-binary release build.
- Do not publish this as a completed universal-insertion feature or remove
  remaining working platform integrations before replacement validation.

Detailed Linux, immutable-distro and Steam Deck evidence:
[Linux insertion research](./LINUX-INSERTION-RESEARCH.md).

Platform requirement: this feature ships across Windows, macOS, Linux X11,
and explicitly supported Linux Wayland desktops together. Windows-only delivery
is not an acceptable completion milestone. Platform-specific transports are
implementations of one shared contract, not separate user-facing features.

User experience requirement: no app-, distro-, compositor-, or OS-specific
insertion settings. Users do not choose a backend, paste chord, keyboard device,
or "allow VS Code/Cursor" exception. Detect capabilities internally and expose
one consistent setup and delivery workflow. Required OS permission dialogs
remain explicit; a universal UI cannot remove platform security boundaries.

## Goal and boundary

Make Promptify insert into the user's intended writable input across browsers,
IDE chat, editors, documents, and other applications without needing a prompt
profile for each destination. Universal means a common delivery contract with
multiple platform transports, not a guarantee that protected or inaccessible
controls can always accept input.

Never bypass OS permissions, write to password/secure controls, execute a terminal
command, submit a chat message, or replace an entire document implicitly.
Prompt mode retains mandatory numbered steps, bounded Loop, and Done when.
Dictation remains literal text and Answer remains display-only.

## Confirmed findings

- Recent runtime receipts show VS Code classified as `code_chat`, with
  `auto_paste=false`; Grok receipts show `auto_paste=true`. The screenshot's
  "AI input not confirmed" result is a policy block, not evidence of a failed
  native keyboard injection.
- `detected_surface` in `crates/promptify-core/src/routing.rs` excludes VS Code,
  Cursor, and terminal profiles from detected automatic paste. Brand/process
  recognition cannot distinguish an AI composer from an editor or terminal.
- Current main contains a remembered `code_chat_paste` consent setting and
  per-request surface overrides. These are useful compatibility mechanisms,
  but neither identifies the focused control nor generalizes to other apps.
  Their presence in source does not establish which build is running.
- `WindowIdentity` contains only window handle and process ID. The pipeline
  checks that identity and, for adaptive prompts, the site/profile again.
  A move from chat to editor within the same window can pass those checks.
- `ClipboardPaste` sets text, sends a paste chord, waits 350 ms, and restores
  previous text/image if the clipboard still contains the generated output.
  Restoration errors are ignored and original non-text/image formats are not
  preserved. The fixed delay is not a delivery acknowledgement.
- `Inserter::insert` returns `Result<(), BackendError>`. `Outcome::Inserted`
  therefore means the backend accepted the attempt, not that the target field
  demonstrably received it.
- Windows UI Automation currently reads optional surrounding text only for
  opted-in apps. It is not a focused-element delivery/identity interface.
- Wayland already has permission-based portal paste support. Keep and extend
  it rather than proposing unrestricted synthetic input.
- `scripts/test-native-paste.mjs` provides a real clipboard/OS-paste test into
  a browser textarea. Onboarding also requires a matching practice receipt.
  Reuse these patterns; browser-only mocked IPC tests cannot prove universal
  native delivery.

## Architecture

Separate three decisions:

1. **Content contract:** what to generate for the target/site/task.
2. **Destination permission:** whether this user-directed text may be inserted
   into this particular field.
3. **Transport and receipt:** how to deliver and what proves delivery.

An unknown prompt category must not itself prohibit delivery. An unknown or
unsafe destination must not silently become safe because its process is known.
Generator-only surfaces keep their existing graph-execution warning/review
behavior; extending delivery must not imply the generator can execute a graph.

### Proposed data structures

- `DestinationSnapshot`: window/process identity, optional platform element
  token, optional tab/frame/document token, optional origin, capabilities,
  editability/security status, and evidence source. Tokens are job-scoped.
  Unknown capability values are explicit, not false defaults.
- `DeliveryPolicy`: `review_only`, `focused_editable`, or `confirmed_target`;
  permitted verification and operation-scoped destination confirmation.
  Transport/chord selection is internal capability negotiation, not a setting.
- `InsertionRequest`: job ID, snapshot, mode, validated text, selection-aware
  insertion intent, deadline, and cancellation token.
- `InsertionReceipt`: attempted transport, status (`verified`, `sent_unverified`,
  `blocked`, `failed`, or `partial`), verification method, error code, clipboard
  restoration status, and job-correlated operation ID.

Use opaque platform tokens rather than raw pointers in core contracts. Do not
persist control contents, clipboard values, or browser document snapshots in
diagnostic logs. Native receipts remain separate from generation validation.

### Destination acquisition

Capture the focused destination when recording starts, before showing the
overlay. Prefer platform accessibility metadata without reading text:
focused element identity, writable/readonly, enabled, password/secure, and
supported patterns. Bound calls and report unavailable/timed-out inspection.

Immediately before insertion, revalidate window, process, element, and
document/site identity wherever available. Cancellation and focus changes
block before the side effect. Without element identity, explicitly report
reduced assurance; use user-confirmed delivery rather than pretending exact
control safety.

Do not infer field identity from arbitrary window titles or generic labels.
Account for Electron renderer process IDs: a control may belong to a related
renderer, not the top-level window process. Prove association with the target
window rather than accepting any renderer or demanding equal IDs.

## Delivery policy and UX

Recommended user-facing choices:

- **Automatic into confirmed writable inputs:** app-independent, focused-field
  insertion when metadata/adapter provides adequate evidence.
- **Confirmed target:** user deliberately authorizes an otherwise opaque input
  for this operation. Requires the original destination to remain focused.
- **Review only:** retain Copy/Dismiss.

Use a single app-independent delivery preference and common setup. Remove
software-specific insertion toggles when the replacement is wired and tested.
Do not reinterpret previous VS Code/Cursor consent as authorization for every
app: explain the universal behavior once and obtain appropriate consent.
Existing per-request surface overrides still control content semantics; they
must not override secure/readonly/focus restrictions. Destination identity is
operation-scoped evidence, not a user-maintained app/site allowlist.

Remove compositor/backend selection and mandatory physical-keyboard selection
from the normal workflow. Use a supported permissioned activation mechanism
and expose the same configurable activation action across platforms. A
controller-only device must not require a physical Ctrl+Shift keyboard. Do not
replace device selection with unrestricted evdev monitoring or root access.
If a session lacks a safe activation backend, report that capability gap rather
than sending users through undocumented system modifications.

Unify permission readiness behind a common "Enable desktop integration"
action; invoke the required platform consent flow internally and show specific
denial/revocation recovery. Automatic detection must not suppress errors.
Privacy consent for capturing surrounding text remains distinct from insertion
permission; removing source-specific paste settings must not enable broad
screen-text capture or silently erase existing privacy choices.

For uncertain results, offer **Insert into focused field** as a deliberate
action, not a blind background retry. Clicking the overlay must not retarget
insertion to the overlay: arm the action, let the user refocus the destination,
then confirm through a non-focus-stealing hotkey. Show the destination and
assurance level. Offer recovery guidance for missing permission and privilege
mismatch, plus Copy as the final fallback.

Terminals require explicit confirmation and insertion without synthetic Enter.
Multiline pastes can execute commands even without Enter in some terminals;
block them unless a tested terminal adapter establishes safe bracketed-paste
behavior or the user explicitly accepts that risk. Do not treat terminal
compatibility as an ordinary editable-field case.

## Transport strategy

| Transport | Use | Restrictions |
|---|---|---|
| Application adapter | Highest assurance when a supported API can address the exact field and acknowledge insertion | Optional integration; verify supported API, do not rely on undocumented internal commands |
| Clipboard + native paste | Broad default for browsers, Electron, editors, and document apps | Permission, exact focus checks, modifier-state checks, clipboard transaction |
| Accessibility insertion | Controls exposing tested, selection-aware editing | `ValuePattern.SetValue` replaces a value; never use as a universal append/insert fallback |
| Native Unicode typing | Explicit compatibility fallback where clipboard is unavailable | Small, tested inputs only; chunked cancellation/focus checks; Unicode/IME/layout and partial-delivery handling |
| Copy/manual paste | Unsupported, protected, or ambiguous cases | Explicit recovery, not a claim of automatic delivery |

Choose transport before side effects. After any potentially delivered input,
do not automatically retry with another transport unless non-delivery is
proven. Exactly-once delivery cannot be guaranteed with generic keystrokes.

### Platform boundaries

- **Windows:** UIA for destination metadata/optional verification, clipboard +
  SendInput for common delivery. Check input-event counts and modifier state.
  Detect elevation mismatch and explain UIPI; do not automatically elevate.
  Never use forced window activation or WM_SETTEXT as universal workarounds.
- **macOS:** Accessibility permission and focused AX element where available;
  Cmd+V transport. Report permission denial explicitly. Test secure input,
  Electron controls, and whether additional window-metadata permissions are
  needed rather than requesting broad permissions preemptively.
- **Linux X11:** current native paste with optional AT-SPI metadata. Validate
  focus and document/control identity where available.
- **Linux Wayland:** reuse authorized remote-input portal sessions and existing
  supported compositor integration; AT-SPI may improve field evidence.
  Unsupported compositor/portal capabilities remain an explicit limitation.
- **Browser extension, later:** opt-in host permissions and authenticated native
  messaging can identify the exact tab/frame/editable element and acknowledge
  application-compatible edits. DOM assignment and synthetic events are not
  equivalent to trusted user input; frameworks, rich editors, cross-origin
  frames, and closed shadow roots need tested handling.
- **VS Code extension, later:** verify stable APIs separately for editor,
  Copilot chat, and third-party chat webviews. `TextEditor.edit` is not an API
  for arbitrary chat fields. Do not promise extension-based universal chat
  insertion without an API feasibility spike.

## Clipboard transaction and verification

Serialize clipboard transactions. Preserve original formats where supported;
otherwise explain the preservation limitation instead of silently discarding
them. Detect user clipboard changes with platform sequence/change information
where available, not only text equality.

Use a bounded acknowledgement deadline instead of assuming 350 ms is enough.
With adapter acknowledgement or opted-in field verification, confirm the
expected insertion at the selection/caret, allowing documented newline
normalization. Verify only the minimum necessary text locally, separately
from consent to send surrounding context to the model.

When verification is unavailable, report `sent_unverified`, not `verified`.
Never read protected text. Never restore the clipboard over a newer user copy.
Restoration failure is a visible secondary error and must not trigger another
paste. Keep generated output recoverable for partial/uncertain delivery.

## Integration and rollout

1. **Contract and diagnostics:** add destination/receipt abstractions alongside
   legacy contracts; distinguish policy blocks, focus changes, permission
   failures, sent-but-unverified, and verified delivery. Version exposed
   API/MCP contracts or introduce additive negotiated capabilities.
2. **Cross-platform capability spikes:** establish destination identity,
   permission handling, safe paste, clipboard preservation, and receipt
   capabilities on Windows, macOS, X11, and supported Wayland desktops.
   Reproduce IDE chat and browser input with the same known text on each
   platform before involving model generation. Record unsupported capabilities
   explicitly and resolve them before declaring that platform supported.
3. **End-to-end wiring:** core pipeline, desktop controller/settings/IPC,
   overlay, onboarding, history, CLI diagnostics, and API/MCP compatibility.
   Remote transform calls do not gain automatic local desktop insertion.
   History distinguishes generated, attempted, verified, and manual delivery.
   Retire `code_chat_paste` and other insertion-specific workaround controls
   through an explicit settings migration. The current settings schema denies
   unknown fields, so removing serialized keys without a compatibility reader
   would break existing settings. Read old fields for migration, preserve
   unrelated settings and privacy consent, and write the new version only
   after a successful migration. Remove obsolete IPC/UI only with compatible
   handling for clients that still negotiate the previous contract.
4. **Native validation:** extend the existing smoke harness and add target
   app cases. Include spoken recording -> generation -> field insertion;
   frontend simulations alone do not satisfy release acceptance.
5. **Cross-platform release gate:** test Windows, macOS, X11, and supported
   Wayland on real interactive desktops. Publish separate platform coverage
   evidence. Do not release the universal insertion feature based solely on
   Windows results or mark untested platforms as complete. If an interactive
   test environment is unavailable, that platform remains an open release
   blocker, not a presumed pass.
6. **Optional adapters:** prototype browser and VS Code integrations only for
   gaps demonstrated by native tests, then ship supported capabilities rather
   than unsupported universal claims.

No model replacement is required. Keep generation and delivery changes
separate so that a graph-validation failure cannot be mistaken for paste
failure. Preserve legacy rendering, Dictation, Answer, cancellation, and
generator review semantics throughout rollout.

### Required platform matrix

| Platform | Destination inspection | Default delivery | Native release evidence |
|---|---|---|---|
| Windows | UI Automation; exact control identity where exposed | Clipboard + Ctrl+V | IDE chat, browser fields, editor, permission/privilege and focus failures |
| macOS | AX focused element; explicit Accessibility authorization | Pasteboard + Cmd+V | IDE chat, browser fields, editor, permission denial/revocation and focus failures |
| Linux X11 | Window identity + AT-SPI where exposed | Clipboard + native paste chord | IDE chat, browser fields, editor, clipboard ownership and focus failures |
| KDE Wayland | Existing KWin window integration + AT-SPI where exposed | Authorized RemoteDesktop portal keyboard session | Same field matrix plus permission denial, session revocation and compositor focus behavior |
| GNOME Wayland | Existing supported window integration + AT-SPI where exposed | Authorized RemoteDesktop portal keyboard session | Same field matrix plus permission denial, session revocation and integration availability |

Use the same policy, cancellation semantics, receipt vocabulary, and recovery
UX across this matrix. Equivalent guarantees do not require identical APIs:
when exact-field inspection or acknowledgement is unavailable, every platform
must expose the same explicit reduced-assurance path. Never silently weaken
safety on Linux or treat permission denial as successful delivery.

Other Wayland compositors require a capability investigation and actual native
tests before joining the supported matrix. A permissioned portal may provide
keyboard delivery without sufficient destination identity; that alone does
not establish safe automatic insertion.

Required expansion targets include major distro desktop variants, Cinnamon
X11/Wayland, Sway/Hyprland, immutable Bazzite, and stock Steam Deck. Steam Deck
Desktop Mode and Gamescope Gaming Mode have separate acceptance gates. Missing
RemoteDesktop/GlobalShortcuts backends and sandbox focus access are unresolved
capability blockers, not reasons to claim these targets already work. If
Gaming Mode is required, its unresolved activation/focus/delivery route remains
open until proven; Desktop Mode results cannot substitute.

## Acceptance criteria

- For each declared supported field, 20 consecutive native attempts deliver
  the exact expected single-line and multiline text, with no duplicates,
  unintended submission, lost selected-text semantics, or wrong-field writes.
  This is a release gate for the tested matrix, not a universal success claim.
- Test VS Code Copilot chat and this user's Copilot SDK chat input separately;
  also Cursor chat, Zen/Grok, Chromium textarea/contenteditable, a plain editor,
  and representative document/rich-text fields.
- Test emoji, non-Latin text, CRLF/newlines, existing selection/content, IME,
  clipboard changes, slow clipboard consumption, held modifiers, and multiple
  windows. Test terminals only under their separate safety policy.
- Zero insertion in secure/readonly controls and after cancellation, window
  switch, same-window field switch, or tab navigation when identity can be
  established. Opaque fields require explicit reduced-assurance confirmation.
- Permission denial, elevation mismatch, partial input, and unavailable
  verification produce honest statuses and actionable recovery.
- Automated native tests observe actual destination contents; they must not
  populate the expected value with Playwright or mocks.
- Run the native acceptance matrix on every supported platform. Compilation,
  cross-compilation, shared Rust unit tests, and headless browser tests do not
  replace interactive platform delivery evidence.
- Unit-test policy and transport state machines; extend browser tests for
  recovery and receipt UI; run relevant Rust tests, frontend type-check/build,
  native smoke tests, and complete platform acceptance before release.

## Feasibility

High: universal delivery contracts, decoupled classification, diagnostics,
clipboard/native paste compatibility, Windows destination metadata.
Medium: exact-field identity and minimal verification across Electron/rich
editors; safe cross-platform clipboard preservation; optional browser adapters.
Unproven until prototype: stable insertion APIs for every VS Code chat surface.
Not achievable as an unconditional guarantee: arbitrary secure fields,
higher-privilege processes, inaccessible custom controls, denied OS permission,
and unsupported compositor integrations.

## Platform references

- [Microsoft SendInput](https://learn.microsoft.com/en-us/windows/win32/api/winuser/nf-winuser-sendinput):
  input counts, UIPI restrictions, and existing modifier-state interference.
- [Microsoft UIA Value pattern](https://learn.microsoft.com/en-us/dotnet/framework/ui-automation/implementing-the-ui-automation-value-control-pattern):
  control-pattern availability is not universal or selection-aware insertion.
- [Apple AX trust API](https://developer.apple.com/documentation/applicationservices/1459186-axisprocesstrustedwithoptions):
  Accessibility permission boundary.
- [RemoteDesktop portal](https://flatpak.github.io/xdg-desktop-portal/docs/doc-org.freedesktop.portal.RemoteDesktop.html):
  permissioned keyboard sessions and session lifecycle on supported desktops.
- [VS Code API](https://code.visualstudio.com/api/references/vscode-api):
  validate each proposed integration against public extension APIs.
- [MDN Event.isTrusted](https://developer.mozilla.org/en-US/docs/Web/API/Event/isTrusted):
  synthetic DOM events must not be assumed equivalent to user input.
