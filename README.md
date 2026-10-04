# Promptify

Speak a rough request and get a clear, well-structured prompt pasted into the AI app you are using. Everything runs on your
own computer: Whisper for speech, a local Qwen model for writing. No cloud AI services are used.

- **Prompt mode** (`Ctrl+Alt+Space`): turns what you said into a prompt that fits the target app (ChatGPT, Claude,
  Gemini, Grok, Copilot, Perplexity, Cursor, VS Code, Claude Code, Codex CLI, terminals, image generators...).
  AI requests become a **task graph**, sized to their complexity: numbered steps, explicit dependencies (`after 1, 2`),
  bounded check-and-revise loops (`max N rounds`) and `Done when:` completion checks. **Every final prompt retains this
  structure**, including media, writing, tutoring, and role-play requests. The finished output is validated and,
  if needed, repaired once; an invalid draft is never returned as a usable prompt.
- **Task-aware adaptation** (experimental, off by default): Settings -> **Prompt types** adapts the content to the
  identified tool/site, confirmed input surface, and requested task without removing the graph/loop mandate.
- **Dictation mode** (`Ctrl+Alt+Shift+Space`): pastes what you said with filler words removed. Say "new line",
  "new paragraph" or "scratch that" as their own sentence to edit as you speak.
- **Answer mode** (optional hotkey, set it in Settings): ask a question; the local model's answer is shown with a Copy
  button and never pasted.
- **Copy results**: the final prompt or answer dialog expands to show the full text. It scrolls only
  when the content exceeds the current monitor's available height; Copy and Dismiss stay visible.
- Hold the hotkey while speaking, or tap it once to start and again to stop. `Esc` cancels. All are rebindable.
  Optionally, holding **Ctrl+Shift on their own** also starts a prompt on Windows, macOS, and Linux
  (requires input-monitoring permission on macOS and physical-keyboard access on Linux Wayland).
  Shortcuts like Ctrl+Shift+T and quick layout-switch taps are ignored.
- **Live transcription**: speech is transcribed at natural pauses while you talk, so the result is ready moments after
  you stop. The overlay shows what was heard so far.
- **Model compatibility**: Models compares detected total system RAM with each model's manifest
  minimum and grays out models below it, with an explanation. GPU/VRAM and CPU core count are not
  minimum requirements because CPU inference is supported. This is an advisory check, not a
  guarantee that a model will load with current memory usage; downloads and saved selections are
  preserved. Hardware detection failures are shown as unknown, not unsupported.
- **Automatic mode** (optional): outside AI apps the prompt hotkey types plain dictation. Start with "prompt:" or
  "dictate:" to choose yourself.
- **Your words**: names and terms Whisper should expect, corrections (`heard => meant`), and the apps whose focused
  text box may be used as context (never password fields, never saved).
- **Phone networking is paused** until the native iOS and Android apps are ready. Normal desktop builds do not
  accept phone connections, connect to relays, or use mDNS. Desktop MCP and model downloads are unaffected.
- **MCP**: Promptify is an MCP server (other AI tools can ask it to write prompts) and an MCP client (your MCP tools can
  add reference facts before a prompt is written).

## Download and install

Download the x64 installer for your platform from the
[v1.1.1 release](https://github.com/jrynks/promptify/releases/tag/v1.1.1).
Quit any running Promptify instance before upgrading. Models are downloaded separately on first launch.
Verify your download against the release's **SHA256SUMS.txt**.

### Windows

Promptify supports Windows 10 and 11 on x64 computers with AVX2. Run **Promptify_1.1.1_x64-setup.exe**. The installer is currently
unsigned, so Windows SmartScreen may show a warning: choose **More info**, verify that the app is **Promptify**, and then
choose **Run anyway**.

The installer:

- installs Promptify for the current user;
- includes the local llama.cpp worker and `promptify-cli.exe`;
- installs the required Visual C++ and OpenMP runtime files alongside the app; and
- downloads Microsoft WebView2 if it is not already installed.

Install an up-to-date graphics driver that provides the Vulkan runtime. Even when loading models
on the CPU, the Windows binaries require the Vulkan loader; the installer does not redistribute
or replace your graphics driver. A Vulkan SDK is not needed to run the app.

### Linux

Linux packages are built on Ubuntu 22.04 (glibc 2.35) for x86-64 PCs with AVX2, without requiring a source
build, Node, Rust, or a Vulkan SDK. They include the local language-model worker and developer CLI.
Use the package for your distribution:

| Distribution | Package | Install |
| --- | --- | --- |
| Ubuntu 22.04+, Debian 12+, Linux Mint 21+ | `.deb` | `sudo apt install ./Promptify_1.1.1_amd64.deb` |
| Fedora and compatible RPM desktops with WebKitGTK 4.1 | `.rpm` | `sudo dnf install ./Promptify-1.1.1-1.x86_64.rpm` |
| Bazzite / Fedora Atomic, Arch, and other modern glibc desktops | `.AppImage` | Make executable, then run as your normal user |

For the AppImage:

```bash
chmod +x Promptify_1.1.1_amd64.AppImage
./Promptify_1.1.1_amd64.AppImage
```

If FUSE 2 is unavailable (common on immutable desktops), use
`./Promptify_1.1.1_amd64.AppImage --appimage-extract-and-run` instead. The AppImage avoids
layering application packages on Bazzite; it is not a Flatpak. Your graphics driver's Vulkan
loader and desktop audio services must be available. CPU inference is supported, but the binary
still needs the Vulkan loader. Native `.deb`/`.rpm` installers declare their runtime dependencies.
AppImage portability does not imply that every distribution or compositor has been tested.

The published v1.1.1 packages support X11 or KDE Plasma 6 Wayland with its RemoteDesktop
portal backend; GNOME Wayland additionally needs the `x-win` window-tracking extension.
The in-development integration below removes manual extension dependence when accessibility
metadata is available, but has not been certified on Linux or Steam Deck.
GNOME users may need an AppIndicator extension to see the tray. The packages do not grant
keyboard-device access, install input permission rules, or run the app as root.
See [Wayland integration and keyboard permissions](#fedora--bazzite) below.

Linux settings and models live in `${XDG_DATA_HOME:-~/.local/share}/dev.promptify.app`.
Installed and development builds share that location; do not run both at once.

On first launch, Promptify opens a required guided tour in Settings:

1. **Models:** download and select a speech model and a prompt-writing model. The recommended balanced pair is
   Whisper small English and Qwen3.5 4B (about 3.50 GB combined, at least 8 GB RAM). Smaller and multilingual
   alternatives remain available. Downloads can be cancelled and resumed; both selected models must load before
   you continue. Loading failures offer a retry and, when appropriate, a CPU option.
2. **General:** check the system-default microphone and Prompt shortcut. Refresh after connecting a microphone,
   allow microphone access in your operating system, and change the shortcut if another app has claimed it.
3. **Practice:** prepare the practice field, use the Prompt shortcut, speak a request, and use the shortcut again
   to finish. Keep the field focused until the generated prompt is pasted into it. Press Esc to cancel. Typing,
   a preview, or an unsuccessful paste does not complete this check.
4. **Finish setup:** unlock normal use after the real voice-to-prompt exercise succeeds and setup is saved.

No account or API key is needed. Internet access is needed to download models; speech and prompt generation run
locally afterward. The practice exercise does not call connected tools or save its text to history. A current
graphics driver with Vulkan support is recommended; CPU loading is also supported.

Closing Settings or quitting does not skip setup: the tour resumes when you return. Interrupted downloads can be
resumed from Models. Existing configured users keep their usual tray-first startup and are not forced through the
tour. Optional features such as Answer mode, Desktop API/MCP, autostart, and vocabulary customization stay outside
required setup.

GitHub also provides ZIP and tar.gz source archives on the release page. Those archives are for building Promptify
yourself and are not portable application packages.

## Use Promptify

Promptify runs in the system tray. In any text box:

1. Press `Ctrl+Alt+Space`, say a rough request, and press the shortcut again. Promptify rewrites it as a structured
   prompt and pastes it into the active app.
2. Press `Ctrl+Alt+Shift+Space` for plain dictation with filler words removed.
3. Press `Esc` while recording to cancel.

For example, saying:

> Make a launch plan. Review the docs first, then package the app and make the repository public. Retry failed uploads.

produces a numbered prompt with dependencies, a bounded retry loop, and a clear completion condition. Hotkeys, models,
accuracy, app context, and optional Answer mode can all be changed in Settings.

App data lives in `%APPDATA%\dev.promptify.app` (settings, models, history, `remote\`, and optional `mcp.json`); logs are
in `%LOCALAPPDATA%\dev.promptify.app\logs`. Installed and development builds use the same data folder, so do not run both
at the same time.

## Platform support

| Platform | Status |
| --- | --- |
| Windows 10/11 x64 | NSIS installer |
| Linux x86-64, glibc 2.35+ | Debian, RPM, and AppImage packages; KDE Plasma 6 Wayland or X11 |
| macOS | Source support is experimental; packaged installer and some hotkeys are not yet available |

## Build from source

### Build installers

After installing the native build prerequisites below and running `npm ci`, run `npm run installer`.
Windows produces an NSIS installer; Linux produces `.deb`, `.rpm`, and `.AppImage` files.
Output is under `$CARGO_TARGET_DIR/release/bundle` (by default `~/.cache/promptify-target` on Linux
and `C:\ptb` on Windows). Both the worker and CLI are staged as sidecars next to the desktop
executable. Linux release packages must be built on Ubuntu 22.04 or an equally old compatible
build environment, not a newer Fedora host, to retain the glibc 2.35 baseline.

The **Installers** GitHub Actions workflow builds and verifies Windows and Linux packages
from the selected ref, and uploads artifacts for inspection. It does not publish a release
automatically. Publish only artifacts from the same source commit as the release tag, with a
combined SHA-256 checksum file.

### Windows

Requirements: Rust 1.85+ (tested 1.97), Node 22+, the Vulkan SDK (`VULKAN_SDK` set), and libclang
(`LIBCLANG_PATH`, e.g. from `pip install libclang`). A Vulkan-capable GPU is recommended; models fall back to CPU.

```powershell
npm ci
npm run desktop        # builds and runs the embedded desktop UI (no Vite or source watcher)
npm run app            # development mode: Vite and automatic restarts when source changes
```

`PROMPTIFY_DATA_DIR` and `PROMPTIFY_MODELS_DIR` override the normal data locations for isolated test runs.
For normal testing, prefer `npm run desktop`: it uses the built frontend instead of depending on a live Vite
server. Leave its launcher terminal running and quit from the tray when finished. Stopping the development
command also stops its app; source edits can restart it. Startup, requested exits, and Rust panic locations
are recorded in `%LOCALAPPDATA%\dev.promptify.app\logs` so an application failure can be distinguished from
a stopped launcher. A forced process termination may not produce an exit record.

### Fedora / Bazzite

Node 22+ must be installed natively. On Bazzite, install the native Rust, Tauri, and Vulkan build dependencies:

```bash
sudo rpm-ostree install --idempotent --allow-inactive \
  rust cargo rustfmt gcc-c++ make cmake ninja-build clang clang-libs \
  vulkan-headers vulkan-loader-devel glslc glslang-devel spirv-headers-devel \
  gtk3-devel webkit2gtk4.1-devel libappindicator-gtk3-devel librsvg2-devel \
  openssl-devel alsa-lib-devel dbus-devel libxdo-devel wayland-devel libXtst-devel patchelf
```

Reboot to activate the staged deployment, or explicitly apply it live after reviewing any other pending system changes.
On regular Fedora, use `sudo dnf install` with the same package list instead.

Run from a terminal in your desktop session:

```bash
export PATH="/home/linuxbrew/.linuxbrew/bin:$PATH"
export VULKAN_SDK=/usr
export LIBCLANG_PATH=/usr/lib64
npm ci
npm run app
```

The PATH entry supports Node installed through Linuxbrew; it is unnecessary when Node is already on PATH.
The Vulkan and libclang paths above use Fedora's system packages, not a separately downloaded SDK.
In VS Code, **Terminal > Run Task > Promptify: Run desktop app** supplies this environment automatically.

Wayland desktop integration:

- **One control**: choose **Enable desktop integration** in setup or General Settings.
  On Wayland this requests keyboard control through RemoteDesktop and activation through
  GlobalShortcuts. Follow the desktop's permission dialogs; no screen capture is requested.
  Actual system-assigned shortcuts appear in Settings. A backend that lacks these capabilities
  reports an explicit error instead of requesting raw keyboard-device access.
- **Focus evidence**: bounded AT-SPI metadata inspection can establish the active accessible
  window and writable field without reading its contents or requiring a manual extension.
  Providers do not expose every application. Existing native window tracking is a reduced-assurance
  fallback, not proof of a writable input. KDE's KWin snapshot uses a short-lived script,
  unloaded after each query, accepting replies only from KWin's authenticated D-Bus connection.
- **Permission lifecycle**: a private `remote-desktop-token` can reduce repeated permission
  prompts. Sessions are acquired together by the explicit Enable action, not restored partially
  at startup. Re-enable after a new launch or permission loss. **Disable desktop integration**
  persists across launches, blocks insertion, invalidates old jobs even after re-enabling,
  and closes active Wayland input/shortcut sessions. Desktop-wide stored permission can also
  be revoked through the system's permission controls.
- Paste never opens a permission dialog or retries an ambiguous keystroke delivery. It checks
  focus immediately before injection, releases held modifiers on failure, and retains the
  generated result in the overlay for copying if insertion is blocked.
- Clipboard access/conversion failures are not treated as an empty clipboard. Staged text is
  checked before dispatch, and cancellation/integration changes are checked again after
  staging and native focus inspection. Generated text stays on the clipboard until an explicit
  **Restore previous clipboard** action in the overlay or General settings. The previous supported
  snapshot is memory-only and is lost when Promptify exits. Restoration-check failures are reported explicitly.
  Text equality is not an OS clipboard ownership token or proof the destination consumed it.
  On Windows, clipboard change counters also prevent restoring over a newer copy with identical
  text and reject snapshots that changed while being read.
- Windows restores saved content through an owned clipboard window, so plain text,
  HTML/text alternatives, file lists, images, and an empty clipboard retain their
  native formats without relying on an ownerless clipboard handle.
- This implementation is not certification of GNOME, KDE, other Wayland compositors, or
  Steam Deck Gaming Mode. Native desktop, clipboard, controller and suspend/resume tests remain
  required. Clipboard restoration supports plain text, HTML with its text alternative,
  copied-file lists, or images. Unsupported combinations of these types block staging instead
  of knowingly losing a component; arbitrary application formats are not preserved.
  keystroke dispatch is reported as unverified rather than claiming destination receipt.

On Wayland, the taskbar and titlebar icons are resolved through a desktop entry, not the window's embedded PNG.
For an unbundled development build, a user-local `promptify.desktop` entry must match `StartupWMClass=promptify`
and point `Icon` to this checkout's `src-tauri/icons/icon.png`. Keep that development entry hidden with
`NoDisplay=true` and continue launching through the task above; Linux bundles install their own desktop entry and icons.

### Optional Ctrl+Shift hold gesture

Enable **Also start a prompt by holding Ctrl+Shift on their own** in General Settings after setup.
Hold both modifiers, without other keys, for at least 350 ms; releasing either modifier ends the hold.
The ordinary Prompt shortcut remains available if the monitor is disabled or permission is denied.

- **Windows:** a listen-only low-level keyboard hook, unchanged from previous releases.
- **macOS:** a listen-only Core Graphics event tap. Allow Promptify (or the development launcher)
  in **System Settings > Privacy & Security > Input Monitoring**, then retry or restart the app.
  Synthetic paste keystrokes are ignored.
- **Linux X11:** XInput2 raw keyboard events. No root permission or device-access changes are needed;
  XTest-generated paste events are ignored.
- **Linux Wayland:** modifier-only monitoring through raw physical-keyboard access is retired.
  Use the configured Prompt shortcut through desktop integration instead. The portal's
  keyboard-control grant is not permission to monitor every keystroke. Promptify no longer
  exposes a physical keyboard selector or instructs users to change device ACLs.
  Enabling an unsupported gesture reports an explicit error rather than opening raw devices.

The monitor does not grab, block, or consume keys. Permission or runtime failures remain
visible and the optional gesture can be disabled without changing the main shortcut.

### Verify native automatic paste on Windows and macOS

Automatic insertion uses the clipboard plus native Ctrl+V on Windows and Cmd+V on macOS.
Copy-only results are a fallback when focus changes, insertion fails, or an experimental routing
policy requires review; they are not the normal Windows/macOS insertion path. Task-aware adaptation deliberately requires surface confirmation in
mixed-purpose apps because they also contain editors. Native accessibility inspection can
confirm a writable, non-protected field independently of its app or site. The exact confirmed
field is checked again before insertion; changing fields inside the same window blocks it.
This reads metadata, not surrounding text. Unconfirmed inputs still require review or an
explicit per-request input-surface selection. Generator-only fields retain review semantics.
The software-specific paste checkbox is retired. Old settings remain readable, are omitted
on the next settings save, and do not authorize insertion. Deprecated IPC reports retirement
instead of silently enabling a brand exception.
Keep the destination text field focused while generating. macOS requires **Accessibility** permission for input
simulation (separate from **Input Monitoring** for Ctrl+Shift). Windows UIPI can block a
non-elevated app from injecting into an elevated destination.

Job reports include additive `delivery: "sent_unverified"` metadata for dispatched native
pastes. The historical `inserted` outcome and history flag indicate dispatch, not proof of
receipt. The overlay says **Paste sent** and preserves a Copy recovery action. Onboarding
still requires the actual matching field receipt before completing setup. Clipboard
transactions are serialized, and restoration failures are surfaced explicitly. Original
clipboard preservation supports plain text, HTML with its text alternative, copied files, or images.
Combinations that cannot be restored together block staging; custom application formats are not
preserved. There is no timed automatic restoration or automatic retry of an ambiguous paste.
Windows staging requests exclusion from clipboard history, cloud synchronization, and monitoring;
this does not change system-wide clipboard settings or establish proof of delivery.

For repeated native selection-replacement checks (single-line, multiline and Unicode):

```bash
npm run test:native-paste -- /path/to/promptify-cli --acceptance
node scripts/test-generated-paste.mjs /path/to/promptify-cli
```

The second command uses the configured local model and the real adaptive pipeline, then
checks exact generated field contents. Neither test records history or submits the field.
It is not a microphone/global-shortcut test.
The harness retains the generated clipboard until it observes exact field contents, then explicitly
acknowledges restoration to the CLI. Set `PROMPTIFY_TEST_BROWSER` to an installed Chromium-based
browser executable to test a real installed browser instead of bundled test Chromium. This is a
test-only choice, not a source-specific application setting. The test field is served from a
temporary loopback HTTP origin (not an opaque data URL), and the test server is closed afterwards.

On Windows, test the installed VS Code editor in a temporary isolated profile:

```powershell
node scripts\test-native-vscode.mjs C:\ptb\release\promptify-cli.exe 'C:\Users\you\AppData\Local\Programs\Microsoft VS Code\Code.exe' --acceptance
node scripts\test-native-vscode.mjs C:\ptb\release\promptify-cli.exe 'C:\Users\you\AppData\Local\Programs\Microsoft VS Code\Code.exe' --generated
```

The first command checks 60 native selected-text replacements; the second checks real adaptive
local-model output. Neither uses your workspace, enables extensions, submits prompts, or certifies
authenticated chat fields. Unknown Windows accessibility patterns receive up to three 50 ms
metadata rechecks while a provider initializes. This never infers writability or retries delivery.

Cross-platform destination adapters and native test results are tracked in
[the universal insertion plan](./UNIVERSAL-INSERTION-PLAN.md) and
[Linux/Steam Deck research](./LINUX-INSERTION-RESEARCH.md). This development work is not
certification of every distro or Steam Deck Gaming Mode.

Run the native smoke test in an interactive Windows or macOS desktop session:

```bash
cargo build -p promptify --bin promptify-cli
npx playwright install chromium
npm run test:native-paste -- target/debug/promptify-cli
```

On Windows use `target/debug/promptify-cli.exe`; if `CARGO_TARGET_DIR` is set, use that directory
instead of `target`. On macOS grant Accessibility to the CLI/launcher when requested and rerun.
If macOS hides the test window title, its focused-window backend may also need Screen Recording
permission for the CLI/launcher to read other applications' window metadata.
The harness opens its own browser textarea and uses the **real desktop ClipboardPaste backend**,
not browser typing, to inject single-line and multiline text. It passes only when the exact text
appears in the field. Do not switch windows during the test. A unique test-window title is checked
before injection so an unrelated window cannot intentionally receive the test text. No models,
user settings, or history are changed. Passing this test verifies the native paste backend, not
every target application or the complete speech/model/hotkey flow. Shared tests run on Linux are
not a substitute for these native Windows and macOS runs.
On Windows the test harness attempts native activation of its own uniquely identified browser
window and fails explicitly if Windows refuses. This is test setup only; production insertion
never brings an old destination back to the foreground. The acceptance variant runs 20 selected-text
replacements each for single-line, multiline and Unicode payloads:

```bash
node scripts/test-native-paste.mjs <built-promptify-cli> --acceptance
node scripts/test-generated-paste.mjs <built-promptify-cli>
```

The second test requires configured local models and observes real adaptive model output in the
native textarea. It does not verify speech recording or global shortcut activation.

The same harness also runs on Linux X11 and supported Wayland desktops. On Wayland, grant paste
permission in the app first; the CLI restores that grant in its own session before the test.
It never injects unless the unique test-window title is focused.
On Linux the progress overlay is hidden immediately before insertion so a compositor cannot
mistake it for the destination. The focused window is then checked normally; Promptify never
reactivates an old destination or bypasses a genuine focus change.

### Build the Windows installer

```powershell
npm run installer      # C:\ptb\release\bundle\nsis\Promptify_1.1.0_x64-setup.exe
```

This additionally needs Visual Studio Build Tools with the Visual C++ toolchain. The build copies the permitted Visual
C++ and OpenMP redistributable DLLs into the application and creates an unsigned per-user NSIS installer.

## Developer CLI

`cargo run -p promptify --bin promptify-cli -- <command>` (or `C:\ptb\debug\promptify-cli.exe` if `CARGO_TARGET_DIR=C:\ptb`):

| Command | Purpose |
| --- | --- |
| `models`, `download <id>` | list or install models |
| `transcribe <file.wav>`, `run <file.wav>` | speech only, or the full pipeline on a recording |
| `live-sim <file.wav>` | replays a recording as if spoken; compares live chunks with one full pass |
| `rewrite "<text>" [--process claude.exe] [--url URL] [--mcp mcp.json] [--mode prompt\|dictation\|answer] [--auto]` | typed text through the pipeline |
| `screen-text` | after 3 s, reads the focused text box of the foreground app, as the app would |
| `eval eval\cases.toml` | structure evaluation (25 opaque cases; `PROMPTIFY_EVAL_SHOW=1` prints prompts and rejected repairs; nonzero exit on failure) |
| `prompt-types` | print the 264-entry taxonomy with activation status, priorities, and input requirements; no model needed |
| `route "<text>" [--process NAME] [--url URL] [--surface SURFACE] [--prompt-type ID]` | inspect adaptive task/surface selection without generation or user-history access |
| `eval-routing eval\adaptive.toml` | deterministic task/form regression benchmark, with per-case failures and macro-F1 |
| `eval-adaptive eval\adaptive.toml [--model ID]` | generate with the selected installed model; validate graphs, numeric preservation, and required user details |
| `serve [--listen 127.0.0.1:47822]` | headless local MCP API; prints its loopback URL |
| `remote pair <link> [--direct]`, `remote send "<text>" [--app claude] [--dictation] [--direct]` | paired-phone test client; requires a `mobile-networking` build |
| `mcp [--api http://127.0.0.1:47821]` | stdio MCP server (see below) |

Set `PROMPTIFY_LOG=1` for diagnostics on stderr. `PROMPTIFY_LLM_NO_PREFIX_CACHE=1` turns off the language model's
prompt-prefix cache for comparison.

`rewrite` and `run` accept `--rendering legacy|adaptive`, `--prompt-type ID`, and `--surface SURFACE`.
Overrides require adaptive rendering and Prompt mode; unknown or proposed task IDs are rejected explicitly.
`--model ID` selects an already installed language model for the current CLI run without saving that preference.
`route` defaults to adaptive rendering; `rewrite`, `run`, and the v1 API retain the existing-profile default.

### Task graphs and check loops

- Complexity guidance asks the model for 2 steps and a short check loop for simple requests, 3-4 steps for moderate
  requests, and 4-8 steps for complex requests, with independent work marked for parallel execution. Terminal prompts
  keep the graph in one paragraph.
- Every graph needs numbered steps, a loop that returns to an existing step with a limit of 1-8 rounds, and a
  non-empty `Done when:` section. Dependencies may refer only to earlier steps. The model is instructed to check the
  completion criteria, stop when they pass, and report unmet criteria when the round limit is reached.
- Graph prompts must open with the goal, not persona boilerplate such as `Act as...` or `You are an expert...`.
  Such openers trigger a repair and fail evaluation. If no valid goal-first repair is produced, the draft is rejected
  with an explicit error rather than pasted or returned to MCP; relevant perspectives inside steps remain allowed.
- Validation checks the finished, sanitized text, including terminal newline handling. A repair shares the original
  generation deadline and is checked the same way. If no repair passes, the draft is rejected with an explicit
  error rather than pasted or returned as a usable prompt. The historical `kept_original` status remains in types
  for compatibility but is no longer emitted for new prompts.
- Dictation and Answer are separate non-prompt modes and do not add graphs.
- Image/video and other generator requests keep their descriptions **inside the graph's creation step**.
  Generator-only fields cannot be assumed to execute a graph or check loop, so these workflows are review-only.
  Use a conversational AI with the appropriate tools, or use Dictation to enter literal content yourself.

### Prompt types and adaptive routing

Enable **Use task-aware prompt adaptation** in Settings -> **Prompt types**. This is local, opt-in rewriting:
Promptify does not run the recipient's tools, upload assets, produce final SQL/formulas/lyrics, or call a cloud LLM.

The bundled [catalog](crates/promptify-core/profiles/prompt-types.toml) contains **264 candidate types in 27 families**.
It is a product taxonomy, not an exhaustive industry standard:

- **37 P1 task recipes plus 2 existing visual-generation recipes are enabled**: explanation, research/source QA,
  synthesis, summaries/actions, extraction/classification, writing/editing, email/replies, brainstorming/stories,
  interview practice, tutoring, decisions/planning, coding/debugging/review/refactoring/tests/CI, SQL, formulas,
  analysis, presentations, UI building, translation, and image/video workflows.
- **225 P2/P3 recipes remain proposed** and searchable. Their metadata is not a claim of validated automatic
  handling or destination capability. An explicit override cannot activate them.
- Domains, tone, language, output formats, and prompting techniques are facets, not duplicate app-profile kinds.

Routing is **target -> input surface -> task**, with conservative deterministic rules. The destination name
does not prove which model, subscription, feature, or field is active. Existing profile matching is reused;
new mixed-purpose sites and IDE/terminal inputs require review or a one-request surface confirmation.
Known conversational AI targets keep the existing browser-and-title fallback when the browser does not
expose its URL (for example Grok in Zen). A known conflicting URL is not overridden by the title, and the
recognized target is checked again immediately before insertion.
Ordinary spreadsheet cells, SQL editors, lyrics fields, and speech scripts are literal content: use Dictation.
No screenshots, browser scraping, new background monitoring, or broader screen-text capture is introduced.

The recording overlay and Advanced Playground show the selected task, form, surface, and warnings. The Playground
uses the same profile preparation and message builder as actual requests. A correction can be queued for the
next Prompt hotkey use only; Dictation, Answer, and isolated onboarding practice do not consume it.
The overlay fits its native window to the content. Long transcripts and results scroll within a bounded panel,
while Copy and Dismiss remain visible. Clipboard and window-control failures appear in the overlay rather
than becoming unhandled errors.

#### Test a prompt type

1. In **Settings -> Prompt types**, turn on **Use task-aware prompt adaptation**.
2. Open **Advanced -> Context playground**. Set the destination process/site, choose task-aware adaptation,
   and enter a request such as `Explain a heat pump to a beginner` or `Debug the checkout crash and add unit tests`.
3. Click **Preview**. Inspect the detected task, destination, surface, graph form, and model messages.
   This is a classification/message preview; it does not run the model or paste anything.
4. To test the full flow, click the actual input in a recognized AI app, press `Ctrl+Alt+Space`, speak the request,
   and stop recording. Keep that app/input focused until insertion finishes. For Grok in Zen, a missing browser
   URL alone does not require a manual override.
5. Confirm the inserted prompt contains numbered steps, a bounded `Loop:`, and `Done when:` criteria.
   If focus or the identified destination changed, or the field is a generator-only/unknown surface, inspect
   the visible reason and use Copy where appropriate rather than bypassing the safety check.

All active forms are `graph` or `inline_graph`. Short tasks use small graphs; interactive tasks use bounded
checks for the current turn without inventing a total number of conversation turns. Unknown tasks retain a
general graph and an explicit uncertainty warning. Mixed requests retain recognized secondary tasks.
If only a prohibition is supplied and no task can be recognized, Promptify asks for the intended task explicitly
rather than inventing a goal or copying a generic example.

Validation checks sanitized output, including graph dependencies, loop bounds, completion criteria, surface
length limits, internal-template leakage, invented numeric constraints, omitted stated numbers, and obvious
final-artifact substitution. It is not a proof of semantic correctness: review the result, especially for
high-stakes uses or when source context is incomplete. Recognition rules currently have English examples;
unrecognized languages fall back to a general graph rather than pretending a confident classification.

Adaptive preferences use versioned `routing-settings.json`, separate from legacy settings. Optional routing
history metadata uses `history.routing.jsonl`, keyed by existing history IDs without duplicating transcript or
output. History disabling, deletion, clearing, and retention apply to both. Only compatible graph examples
are reused; explicit same-app follow-ups can still refer to the previous prompt. No usage telemetry is uploaded.
Unreadable or newer routing settings are reported instead of overwritten; existing-profile behavior remains
available. The graph/loop requirement applies in both policies and cannot be disabled by a task override.

The taxonomy was informed by primary task/capability documentation and cross-product evidence, not Promptify
usage statistics: [NBER/OpenAI usage research](https://www.nber.org/papers/w34255),
[Anthropic usage by product](https://www.anthropic.com/research/economic-index-june-2026-report),
[GitHub Copilot tasks](https://docs.github.com/en/copilot/tutorials/copilot-cookbook),
[Excel AI tasks](https://support.microsoft.com/en-us/excel/copilot/data-insights-with-copilot-in-excel),
[Suno's separate input fields](https://help.suno.com/en/articles/3726721), and
[ElevenLabs sound-effect prompts](https://elevenlabs.io/docs/eleven-creative/playground/sound-effects).
Provider input contracts can change; activate additional recipes only after verifying the particular surface.

### Local adaptive API

Existing `/v1/transform` and `/v1/profiles` shapes remain unchanged. They now enforce the mandatory graph contract
for all Prompt-mode results, including media; invalid-draft fallback is intentionally removed.
Phone protocol v1 retains existing schemas and disabled-by-default networking.

The same loopback token protects:

- `GET /v2/catalog`: catalog version, task definitions/status, surface IDs, rendering policies, and required structure.
- `POST /v2/transform`: existing `text`, `mode`, `app`, `url`, `title` fields plus optional `routing`.
  The v2 default is adaptive; `mode` is `prompt` or `dictation`.

```json
{
  "text": "Debug the checkout crash and add unit tests",
  "app": "cursor",
  "routing": {
    "rendering": "adaptive",
    "task_type": "code.debug",
    "surface": "code_chat"
  }
}
```

The response includes `version`, the original `profile`, `outcome`, `structure`, `routing`, and `validation`.
Routing reports the target, primary/secondary tasks, `graph`/`inline_graph`, resolution reason, warnings,
and whether automatic insertion is permitted. Callers must inspect `outcome` and the insertion policy; an HTTP
200 can contain an explicit generation/validation failure. Request-schema errors return HTTP 400. The API
never enables desktop-only history, screen context, or enrichment hooks.

## Phone networking (paused by default)

Phone networking is unavailable in normal desktop builds, even if older saved settings enabled it. Those settings
and existing pairings are retained for later development, but cannot activate LAN listeners, the `/v1/direct`
WebSocket endpoint, relay connections, pairing, or mDNS. Settings shows **Desktop API** instead of **Phones**;
its switch controls only the token-protected loopback API used by desktop MCP tools.

### Opt-in mobile development

The phone implementation is retained behind the `mobile-networking` Cargo feature. Enable it explicitly only
when developing or testing phone clients:

```powershell
npm run app -- --features mobile-networking
cargo run -p promptify --features mobile-networking --bin promptify-cli -- serve --listen 127.0.0.1:47822
```

Build the CLI with that feature too before using `remote pair` or `remote send`. Without it, phone-specific
`serve` options (`--relay`, `--advertise`, `--offer-file`, `--discoverable`) and remote commands fail explicitly.

In an opt-in build, use Settings → **Desktop API** → *Allow desktop tools and paired devices to use Promptify*. Then:

- **Pair a device**: *Pair a new device* shows a QR code (valid 5 minutes, single use). The phone app scans it.
- **On your network**: turn on *Allow direct connections on this network*. The desktop listens on port 47821.
- **From anywhere**: run the relay (below) and enter its `wss://` address. Plain `ws://` is accepted only for loopback
  and private-network addresses.
- Paired devices are listed with *Rename* and *Remove*. Removing a device ends its session immediately.
- **Find this computer on the network** (optional, needs direct connections): announces the direct port over mDNS so a
  paired phone can find it after the computer's address changes. Only an identifier derived from the pairing room is
  broadcast, under a synthetic host name.
- Client developers: every message is described in [protocol/schema-v1.json](protocol/schema-v1.json) (JSON Schema,
  generated from the code and checked by a test).

### Security model

- Pairing uses Noise `XXpsk3_25519_ChaChaPoly_BLAKE2s`, keyed with the one-time secret in the QR code. The phone checks
  the desktop's public key (also in the QR code) before it reveals anything about itself.
- Later sessions use Noise `IK`. The desktop accepts only the keys of paired devices and sends no reply to unknown keys.
- The relay is blind: it sees only ciphertext and a room ID, `sha256("promptify-room/1" ‖ host secret)`. Only the
  desktop knows the host secret. It caps rooms, channels, rate (512 KB/s, 2 MB burst) and idle time.
- Remote jobs never read or write your history, never call MCP tools, and queue behind your own hotkey jobs. Each
  device is limited to one job at a time and two waiting.
- The local HTTP API (`/v1/transform`, `/v1/profiles`) accepts only loopback callers that present the token in
  `remote\api-token`.

### Self-hosting the relay

```bash
docker build -f deploy/relay/Dockerfile -t promptify-relay .
docker run -d --restart unless-stopped -p 127.0.0.1:8787:8787 promptify-relay
caddy run --config deploy/relay/Caddyfile   # HTTPS + WebSocket in front of the relay
```

Or run it without Docker: `cargo run --release -p promptify-relay -- --listen 127.0.0.1:8787`. `/healthz` returns `ok`.

## MCP

### Promptify as an MCP server

In Settings → **Desktop API**, turn on *Allow desktop MCP tools to use Promptify* (this creates the local API
token), then add this to your MCP client (Claude Desktop, VS Code `mcp.json`, Cursor...):

```json
{ "mcpServers": { "promptify": { "command": "C:\\path\\to\\promptify-cli.exe", "args": ["mcp"] } } }
```

Tools: `transform_prompt(text, app?, url?)` and `clean_dictation(text)`. The API token is sent only to loopback addresses.
No phone-networking feature is needed. For a headless API, run `promptify-cli serve` and give the MCP command
`--api http://127.0.0.1:47822` instead.

When the backend advertises v2 support, `transform_prompt` additionally exposes optional `rendering`, `task_type`,
and `surface` parameters. Adaptive results carry structured routing/validation metadata alongside the prompt.
The MCP server does not advertise these fields for an older backend, never silently ignores an explicit adaptive
request, and rejects incomplete or non-graph Prompt-mode output. `clean_dictation` remains unchanged.

### MCP tools as context for your prompts

Create `%APPDATA%\dev.promptify.app\mcp.json`, then use Settings → *Tools (MCP)* → *Reload* (or restart).
The common `"mcpServers"` format with `"type": "stdio"` or `"http"` works too:

```json
{
  "servers": {
    "docs": {
      "command": "uvx", "args": ["some-mcp-server"],
      "profiles": ["cursor", "claude_code"],
      "timeout_ms": 3000,
      "hooks": [ { "tool": "search", "arguments": { "query": "{transcript}" } } ]
    }
  }
}
```

- `{transcript}`, `{app}` and `{url}` (host only) are replaced in string arguments.
- Use `url` instead of `command` for a Streamable HTTP server. URL servers **never receive the transcript** unless you
  add `"allow_transcript": true`.
- `profiles` limits a server to some target apps; leave it out to run the server for every prompt.
- Results are capped at 3 sources, 2,000 characters each and 6,000 in total. All calls together must finish within
  4 seconds. Results are passed to the model as untrusted reference text inside escaped `<tool_context>` delimiters.
- Hooks run only for hotkey prompts, never for dictation, remote devices or the API. Different servers are queried at
  the same time; `max_chars` caps one server's results (default 2,000).
- **Tool loop** (off unless you add `"loop_tools": ["search"]` to a server): before writing, the model may call those
  tools itself, at most twice. Anything other than exactly one allowed call with known arguments ends the loop. Only
  servers allowed to see the transcript may offer loop tools.
- Settings → *Tools (MCP)* lists the servers, warns about remote servers that receive the transcript, and has a *Test*
  button that starts a server and lists its tools.
- A server that takes longer than its `timeout_ms` to start is stopped and retried on the next prompt. Warm up
  slow launchers (such as a first `uvx` download) once by hand.
- Only connect MCP servers you trust: a local `command` runs with your user rights.

## Tests

```powershell
cargo test --workspace --exclude promptify-llm
cargo test -p promptify-server --features mobile-networking   # opt-in phone tests, including mDNS
cargo clippy --workspace --all-targets
npm run build
npm run test:onboarding
promptify-cli eval-routing eval\adaptive.toml
promptify-cli eval-adaptive eval\adaptive.toml --model qwen3.5-2b-q4km
```

The onboarding browser tests build and serve the production UI with test-only Tauri IPC fixtures. On first use,
install the browser with `npx playwright install chromium --only-shell`. These tests cover guided navigation,
required checks, recovery, paste receipts, persistence, and minimum-window layout; they do not substitute for
native microphone, global-shortcut, or OS clipboard testing. On Windows network shares, run frontend commands
from a mapped drive rather than a UNC working directory.

The same browser suite covers prompt-routing opt-in/rollback, catalog filtering, one-request overrides,
Playground diagnostics, and 560-pixel overlay windows at 150% scale, including long results and clipboard errors.
Core tests include four positive and two confusable negative utterances for every
enabled recipe, a separate 48-case task/form regression dataset, final graph/loop validation, incompatible
surfaces, same-window site changes, and history-sidecar isolation. The measured macro-F1 applies only to the
fixture dataset, not to all real requests. Real-model evaluation checks additional source details and must be
run separately for each installed model tier; it does not replace human review of generated instructions.

`promptify-llm` builds llama.cpp and is covered by the CLI end-to-end runs (`rewrite`, `eval`, and the local MCP API).
Phone pairing and transport tests run only with `mobile-networking`; default tests verify that phone access is
unavailable while the local API still works.

### Isolated first-launch check

Quit other Promptify instances first; the app is single-instance and shortcuts are global. Never delete your normal
application data to simulate a new installation. In a separate terminal:

```powershell
$env:PROMPTIFY_DATA_DIR = Join-Path $env:TEMP ("promptify-onboarding-" + [guid]::NewGuid().ToString("N"))
Remove-Item Env:PROMPTIFY_MODELS_DIR -ErrorAction SilentlyContinue
npm run app
```

Confirm that Settings appears automatically, complete the tour including a real spoken request and paste, then
quit and relaunch with the same data directory. The tour must not reappear after successful completion. Also test
closing/reopening before completion, interrupted downloads, unavailable microphones, shortcut conflicts, cancelled
practice, focus changes, and failed writes. A failed step must remain incomplete and explain how to recover.

For subsequent practice-only checks, set `PROMPTIFY_MODELS_DIR` to an existing genuine model directory before
launching with another fresh data directory. Cached models must not suppress first-launch guidance. This avoids
repeated downloads but does not replace the empty-model installation check. Record hands-on, download/loading,
and total elapsed timings separately; there is no fixed onboarding completion-time limit.

## Morning check

1. `npm run app`. The tray icon appears, and the overlay shows no errors.
2. In Notepad, press `Ctrl+Alt+Space` and say *"plan a three step launch checklist and review it until it's complete"*.
   You should get a numbered task graph with `after` dependencies and a `max N rounds` loop.
3. Press `Ctrl+Alt+Shift+Space` and dictate a sentence with "um" in it. It should paste cleanly.
4. Settings → Desktop API: enable desktop MCP tools. It should show a loopback listener and no phone pairing,
   LAN, relay, or discovery controls. Tools (MCP) remains available for connected tools.
5. Test the local API against a separate headless server (normal builds do not create pairing links):
   ```powershell
   $env:PROMPTIFY_DATA_DIR="$env:TEMP\pf-morning"; $env:PROMPTIFY_MODELS_DIR="$env:APPDATA\dev.promptify.app\models"
   promptify-cli serve --listen 127.0.0.1:47822               # terminal 1; leave running
   # terminal 2, with the same environment variables:
   $token = (Get-Content "$env:PROMPTIFY_DATA_DIR\remote\api-token" -Raw).Trim()
   Invoke-RestMethod http://127.0.0.1:47822/v1/profiles -Headers @{ Authorization = "Bearer $token" }
   ```
6. `promptify-cli rewrite "draft a launch announcement" --process claude.exe --mcp <your mcp.json>`. With
   `PROMPTIFY_LOG=1`, the output includes a `tool context: N items` line.
7. `promptify-cli eval eval\cases.toml`. All 25 cases should pass: 21 task graphs and 4 descriptive media prompts.
8. Hold a hotkey and speak for 10+ seconds with pauses: the overlay shows the text heard so far, and the result arrives
   moments after you let go.
9. Settings → Status: tick *Also start a prompt by holding Ctrl+Shift*, then hold both keys alone for half a second.
   Ctrl+Shift+T in a browser must not start a recording.
10. Settings → Status: set an *Answer hotkey*, press it and ask "what is a mutex?". The answer appears with Copy and is
    not pasted.
11. Settings → Your words: add your own name and a correction, save, then dictate them.
12. Screen text (not verified overnight: a Windows picker held the foreground): add `notepad` under *Apps whose on-screen
    text may be used*, type something in Notepad, then run `promptify-cli screen-text` and click into Notepad within 3 s.
    It should print the text; a password field prints "password field: nothing read".

## Not done yet

- Native iOS and Android apps. Phone networking is paused behind `mobile-networking`; the protocol
  ([schema](protocol/schema-v1.json)), pairing and reference client (`crates/promptify-server/src/client.rs`)
  are retained for client development. An iOS keyboard extension will need *Full Access* for
  network use.
- Screenshot context with a vision model (needs a llama.cpp build with multimodal support and a ~1 GB projector).
- Hold-Ctrl+Shift and screen-text reading on macOS and Linux; a hosted relay, code signing, and Linux/macOS installers.

## License

Promptify is free and open-source software licensed under the [MIT License](LICENSE).
