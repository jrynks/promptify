# Promptify

Speak a rough request and get a clear, well-structured prompt pasted into the AI app you are using.
By default, everything runs on your own computer: Whisper for speech and a bundled local Qwen model for writing.
No account or API key is needed for bundled inference. Optional connections under **Models** connect
LM Studio, other compatible local servers, or API-key providers directly from the desktop.
Speech recognition stays on-device regardless of the selected language-model backend.
The optional, experimental Jev quality reviewer
is a cloud service and requires a TypeSafe API key and explicit opt-in.

**Latest release: [v1.3.0](https://github.com/jrynks/promptify/releases/tag/v1.3.0)**.
Download a Windows x64 installer or Linux `.deb`, `.rpm`, or AppImage, with SHA-256 checksums.
Models download separately during guided setup. See [Download and install](#download-and-install)
for hardware requirements and platform limitations.

Configurable inference is included in v1.3.0. Bundled on-device models remain the default;
external inference and experimental Jev review require explicit configuration and opt-in.

## Inference: bundled models, LM Studio, and API keys

Bundled models remain the default. **Settings -> Models** groups the pipeline into local
**Speech recognition**, **Prompt writer** (bundled models or API/local-server connections), and
optional **Quality review** (Jev). The **Prompt writer** section has a two-way switch:
**On this device** (bundled models) or **Online or server** (your saved API or local-server
connections). One click switches between them, and the **Now using** line shows the active
writer. You can also switch from the tray menu under **Prompt writer**. One explicit model handles
prompt writing and structural repair throughout each job. Switching applies to subsequent jobs.
Dictation mode does not call a language model.

- **LM Studio:** start the server in LM Studio's Developer tab, make a text model available, and
  add the LM Studio preset. Its default API base URL is `http://localhost:1234/v1`. Refresh the
  model list or enter the exact model identifier, then click **Save and use**. If server
  authentication is enabled, supply its API token; otherwise leave the token blank.
  Promptify does not start LM Studio or download/load/unload its models. Discovered models may
  not already be loaded when LM Studio uses just-in-time loading. A generation test can trigger
  that loading.
- **Other local servers:** use a custom OpenAI-compatible endpoint for Ollama, llama.cpp,
  vLLM, or a compatible gateway. Model discovery is optional; manual identifiers are supported.
  Protocol compatibility varies by server and model.
- **API keys:** configure OpenAI, Anthropic, or Google with a provider key and model identifier.
  Provider API usage is billed to your provider account. A ChatGPT or SuperGrok subscription
  is not an API key; subscription OAuth is not supported in this release.
- **Custom endpoints:** choose OpenAI Chat Completions, OpenAI Responses, or Anthropic Messages.
  Supply the API **base URL**, not a full generation URL. Keep any gateway path prefix; select
  the protocol separately.

Tests use synthetic content, not history or focused screen text, and may incur provider charges.
Listing models alone does not prove the selected model can generate text. There is no automatic
provider fallback or inference retry. **Save and use** on a new connection checks it once with a
tiny synthetic message and switches to it only if the check passes; if it fails, your current
writer stays active. Switching to another saved connection or model works the same way. Successful
checks are saved across restarts for that connection and model, so switching back to an unchanged
connection, or restarting, needs no new check. Editing the active connection selects the edited
model and checks it again. Until that check passes, shortcuts are paused; click **Check now** or
switch to **On this device**. On Wayland, an unchecked selection blocks shortcut activation and
opens Models settings rather than recording. In the tray, connections that have not passed a check
open Settings instead of switching directly.

Credentials are stored in the OS credential store, never in inference settings JSON. If secure
storage is locked or unavailable, resolve the displayed error; there is no plaintext fallback.
HTTP and HTTPS endpoints are supported, including hostname-based gateways. HTTP does not require a
separate acknowledgment; a non-blocking warning explains that requests and tokens are unencrypted.
The general disclosure for sending context to a non-loopback endpoint still applies.
Promptify does not bypass TLS verification or forward credentials through
redirects. Local/LAN requests need no public tunnel or Promptify relay.

Model discovery runs automatically after entering a valid endpoint and supplying any required
authentication and remote-data consent, before saving or entering a model ID. It lists the endpoint's
available model IDs without running inference or selecting a model for you. Choose an ID, save the
connection, and select it explicitly. If discovery is unsupported or fails, an error and manual model
entry remain available. Discovery uses the current edited endpoint, not the previously saved URL;
editing an endpoint cannot silently forward its saved credential to a different endpoint.

For **Agent Maestro**, use a custom connection with the API base
`http://127.0.0.1:23333/api/openai/v1` and OpenAI Chat Completions (or OpenAI Responses).
Its complete model catalogue is served at `/api/v1/lm/chatModels`; the Anthropic `/models` route
lists only a subset and the OpenAI `/models` route is not provided.
Promptify recognizes either gateway base and uses the same host and gateway prefix for discovery,
retaining the `copilot` vendor models that Maestro's generation proxy supports, including GPT,
Grok, Claude and Gemini. It excludes unrelated VS Code providers and recursive custom endpoints.
The catalogue is outside Maestro's LLM-key authentication, so discovery does not send that key.
Generation continues to use
the selected OpenAI protocol and base URL. Keep the Agent Maestro VS Code window running; confirm
the port with **Agent Maestro: Get API Server Status**.

**Privacy changes when you select external inference:** requests go directly to the configured
endpoint. They can include your transcript, prompt instructions, admitted application context,
opted-in focused text, enabled history context, and draft text used for structural repair.
Audio stays on-device. History being stored locally does not mean it cannot enter a remote prompt.
Provider retention and billing are governed by that provider's terms. Even a localhost endpoint
can route upstream (for example, a server using remote model links); verify its configuration.
Removing a connection or replacing its credentials invalidates its active sessions.

- **Prompt mode** (`Ctrl+Alt+Space`): turns what you said into a prompt that fits the target app (ChatGPT, Claude,
  Gemini, Grok, Copilot, Perplexity, Cursor, VS Code, Claude Code, Codex CLI, terminals, image generators...).
  AI requests become a **task graph**, sized to their complexity: numbered steps, explicit dependencies (`after 1, 2`),
  bounded check-and-revise loops (`max N rounds`) and `Done when:` completion checks. **Every final prompt retains this
  structure**, including media, writing, tutoring, and role-play requests. The finished output is validated and,
  if needed, repaired up to twice within the original deadline; an invalid draft is never returned as a usable prompt.
- **Task-aware adaptation** (experimental, off by default): Settings -> **Prompt types** adapts the content to the
  identified tool/site, confirmed input surface, and requested task without removing the graph/loop mandate.
- **Dictation mode** (`Ctrl+Alt+Shift+Space`): pastes what you said with filler words removed. Say "new line",
  "new paragraph" or "scratch that" as their own sentence to edit as you speak.
- **Copy results**: the final prompt dialog expands to show the full text. It scrolls only
  when the content exceeds the current monitor's available height; Copy and Dismiss stay visible.
- Hold the hotkey while speaking, or tap it once to start and again to stop. `Esc` cancels. All are rebindable.
  Optionally, holding **Ctrl+Shift on their own** also starts a prompt on Windows, macOS, and Linux X11
  (requires Input Monitoring permission on macOS). This modifier-only gesture is unavailable on
  Wayland; use the configured Prompt shortcut through desktop integration instead.
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
- **Quality-first recommendations**: Models highlights the highest quality tier with confirmed RAM compatibility.
  Recommendations never automatically switch your selected models or download anything. Larger models may be slower.

Settings includes **General**, **Models**, **Prompt types**, **Your words**, **History**, and **Updates**.
Models brings speech downloads, prompt-writer selection and connections, and optional quality review
into one view. Older Inference recovery links and remembered tabs open this combined view.
Unvisited sections do not initialize their model/history/catalog work; visited sections retain unsaved edits.
Answer mode, the Advanced playground, MCP, desktop HTTP APIs, phone pairing, LAN discovery, and relay support have
been removed. Promptify has no external inference listener or connected-tool execution. Model downloads and update
checks still use HTTPS. Speech stays local; bundled prompt generation uses a child-process worker,
while explicitly selected API/local-server inference uses outgoing requests to that endpoint.

## What's new in v1.3.0

- **One-click prompt writer switching:** choose **On this device** or **Online or server**
  in Models settings, or use the tray's **Prompt writer** menu. Changes apply to subsequent jobs.
- **Bring your own inference:** use LM Studio, compatible local servers, or supported API-key
  providers. Model discovery and manual identifiers are available; new or changed connections
  must pass a generation check before use. Unchanged verified connections persist across restarts.
- **Local speech and secure credentials:** speech recognition and plain dictation stay on-device.
  API keys use the OS credential store with explicit errors, not plaintext fallbacks.
  External generation can send admitted context to the selected endpoint; see the privacy disclosure above.
- **Prompt quality and optional review:** shared task-graph guidance emphasizes faithful expansion
  and meaningful bounded refinement. Experimental Jev review is opt-in cloud processing;
  uncertain or failed reviews retain drafts for manual copy rather than authorizing paste.
- **Fresh platform packages:** Windows x64 setup and Linux x64 deb, rpm and AppImage packages
  include the bundled language-model worker and developer CLI.

### Retained improvements from v1.2.1

- **Clearer settings and recovery:** consistent spacing across all six tabs, shared shortcut errors
  shown once, and nearby actions to retry model loading, check microphones, reconnect desktop
  integration, repair routing settings, or recover a generated result.
- **Remembered Wayland authorization:** after a successful explicit Enable, Promptify saves your
  choice and reconnects desktop integration on later launches. Disabling it stays disabled.
  Your desktop may still ask again if approval expires or is revoked; older profiles need one
  explicit Enable to record combined consent.
- **Safer Linux destination detection:** KDE focus inspection uses the native focused process and
  bounded accessibility metadata, without application-name allowlists. Insertion still requires
  sufficient destination evidence; unsupported fields offer review/copy or an explicit
  per-request input-surface selection, not a universal paste bypass.
- **More reliable recording and history:** microphone capture starts alongside destination
  inspection to buffer early speech, and history IDs remain unique across clears and restarts.
- **Fresh Windows and Linux installers:** verified payloads include the local worker and developer
  CLI. Windows remains unsigned; macOS has no published installer.

If automatic insertion is blocked, the result stays available to copy and dismiss.
Promptify does not retry ambiguous paste delivery or silently reset your configuration.

### Prompt quality development

Every prompt-mode request, even a short question, must become a task graph with explicit
dependencies, a bounded refinement loop and verifiable completion criteria. Plain dictation
does not use this structure. Quality means faithfully expanding the user's intent into useful
work and deliverable-specific checks, not adding arbitrary restrictions or merely passing syntax
validation. Both prompt renderers use shared guidance for this. The shared fallback example
demonstrates concrete suggestions, conditional use of prior discussion and a repair loop; it is
used for adaptive general requests and default profiles without a valid graph example.

The live regression and related held-out requests are in `eval/intent-expansion.toml`.
Run `PROMPTIFY_EVAL_SHOW=1 promptify-cli eval-adaptive eval/intent-expansion.toml` to inspect real
local-model outputs. Its routing, structure and required-detail checks are only a first gate:
read the outputs for invented restrictions, lost intent, vague steps and ineffective loops.
The repeated live request samples multiple generations within the same worker session.

### Optional Jev quality review (experimental)

Settings -> **Models** includes an experimental **Jev quality review** option.
Save your TypeSafe API key in the password field, then enable review after reading the cloud
disclosure. Saving a key alone does not enable review. The key is kept in the OS credential
store, never the settings file, browser storage, or logs; unavailable credential storage is
an explicit error, not a plaintext fallback. Removing the key disables review.
If the credential store becomes unavailable, its error remains visible and review can still be
disabled without unlocking the store.
Enter only the API key; Promptify sends it using Bearer authentication. Saving verifies storage,
not authentication. HTTP 401 means TypeSafe did not accept the credential. HTTP 403 can mean a
missing or unrecognized authentication header, or denied access; it does not prove the key is wrong.

When enabled, desktop Prompt mode sends the transcript and generated draft to
`https://api.typesafe.ai/v1/systemone` using `jev-latest`. No audio, separate screen context,
history records, or model prompts are sent, but the generated draft may itself contain details
from opted-in context. Plain dictation and local CLI evaluations do not call Jev.
TypeSafe's [privacy policy](https://typesafe.ai/legal/privacy-policy) says inputs are not used
to train or fine-tune models; this is still external processing, not an offline feature.

Jev assesses fidelity, invented restrictions, useful graph steps, and meaningful refinement
checks. It does not write prompts or replace deterministic graph validation. A negative review
can trigger **one rewrite using the job's selected inference backend and one more Jev review**, within the original generation
deadline. API errors, uncertain decisions, invalid rewrites, or a second negative review leave
the draft available for manual review/copy and never authorize automatic pasting. Such drafts
are not added to prompt history as examples. Approval still cannot bypass destination safety.

The probability thresholds are experimental policy, not proven prompt-quality guarantees:
all checks must score at least 0.85 to approve; a check at or below 0.20 requests revision;
other results require manual review. A Jev probability is **not** the same as an 8.5/10
human quality grade. Real-service quality and false-approval/rejection rates must be measured
on live and held-out examples before treating this reviewer as reliable.
An inconclusive result means Jev connected and completed its review, not that authentication
failed. The result shows all four scores and the approval rule so you can see why automatic
pasting was withheld. Review failures still retain the draft for manual copy; no threshold is
relaxed just to make a draft pass.

## Download and install

Download the x64 installer for your platform from the
[v1.3.0 release](https://github.com/jrynks/promptify/releases/tag/v1.3.0).
Quit any running Promptify instance before upgrading. Models are downloaded separately on first launch.
Verify your download against the release's **SHA256SUMS.txt**.

### Windows

Promptify supports Windows 10 and 11 on x64 computers with AVX2. Run **Promptify_1.3.0_x64-setup.exe**. The installer is currently
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
| Ubuntu 22.04+, Debian 12+, Linux Mint 21+ | `.deb` | `sudo apt install ./Promptify_1.3.0_amd64.deb` |
| Fedora and compatible RPM desktops with WebKitGTK 4.1 | `.rpm` | `sudo dnf install ./Promptify-1.3.0-1.x86_64.rpm` |
| Bazzite / Fedora Atomic, Arch, and other modern glibc desktops | `.AppImage` | Make executable, then run as your normal user |

For the AppImage:

```bash
chmod +x Promptify_1.3.0_amd64.AppImage
./Promptify_1.3.0_amd64.AppImage
```

If FUSE 2 is unavailable (common on immutable desktops), use
`./Promptify_1.3.0_amd64.AppImage --appimage-extract-and-run` instead. The AppImage avoids
layering application packages on Bazzite; it is not a Flatpak. Your graphics driver's Vulkan
loader and desktop audio services must be available. CPU inference is supported, but the binary
still needs the Vulkan loader. Native `.deb`/`.rpm` installers declare their runtime dependencies.
AppImage portability does not imply that every distribution or compositor has been tested.

#### Linux dependency enforcement

There is **no single all-dependencies installation gate across all Linux formats**.
The current behavior depends on the package and installation command:

| Path | Missing-dependency behavior | Can installation be partial? |
| --- | --- | --- |
| `.deb` through `apt install ./...deb` | APT resolves the package's declared runtime dependencies. An unsatisfied required dependency prevents a successful configured installation; already installed dependencies count as satisfied. | A failed transaction can leave downloaded or unpacked files/packages. Success does not mean models, desktop services, permissions, or every optional feature are ready. |
| `.deb` through `dpkg -i` | `dpkg` does not download dependencies. Missing declared dependencies normally prevent configuration and return failure. | Yes: the application can be unpacked but remain unconfigured. |
| `.rpm` through `dnf install ./...rpm` | DNF resolves declared requirements and normally refuses an unsatisfiable dependency transaction. | No successful dependency-satisfied installation in that case; dependencies already present need not be reinstalled. |
| `.rpm` through `rpm -i` | RPM checks declared requirements but does not fetch them. Missing requirements normally fail the dependency check. | Dependency enforcement can be bypassed explicitly with options such as `--nodeps`; this is not the documented installation path. |
| AppImage | No package-manager dependency installation or exhaustive host preflight is implemented. Copying or extracting the file can succeed without all host requirements. | Yes: the file can be present while launch or features fail because host libraries, drivers, services, or desktop permissions are unavailable. |

Runtime requirements are declared in [the packaging script](./scripts/installer.mjs),
not installed by a custom Linux setup wizard. The `.deb` list includes glibc 2.35,
WebKitGTK 4.1, GTK 3, AppIndicator, ALSA, Vulkan, OpenMP, libxdo, Xtst, Xi, and
xkbcommon; the `.rpm` list names the corresponding runtime packages. These lists
are not proof that every runtime or compositor capability has been enumerated.
Models are downloaded separately during onboarding, and desktop permissions are
requested separately: neither is a package-manager dependency.

The [installer CI workflow](./.github/workflows/installers.yml) is stricter about
**building** artifacts: prerequisite installation, tool/version checks, frontend
dependency installation, tests, packaging, and payload inspection must succeed
before uploading verified installers. Its Linux prerequisite command requests
all listed packages in one APT transaction and a failed step stops that job.
The [payload verifier](./scripts/verify-installers.mjs) requires all three Linux
package formats, checks binaries and their layout, and runs a GUI-free CLI smoke
test on the build host. It records package dependency metadata but does not
assert an exhaustive dependency list or test clean-machine installation with
each runtime dependency removed. Artifact verification success therefore is
not an all-dependencies end-user installation guarantee.

The v1.3.0 packages support X11 and permissioned Wayland integration on desktops providing
the RemoteDesktop and GlobalShortcuts portals, including KDE Plasma 6. KDE uses native KWin
focus evidence; other Wayland desktops use accessibility metadata when available.
An application must expose enough metadata for confirmed-field adaptive insertion.
X11 adaptive field confirmation remains conservative: use review/copy or explicitly select the
input surface for the next request when it cannot be confirmed.
GNOME, other compositors, and Steam Deck Gaming Mode have not been comprehensively certified.
GNOME users may need an AppIndicator extension to see the tray. The packages do not grant
keyboard-device access, install input permission rules, or run the app as root.
See [Wayland integration and keyboard permissions](#fedora--bazzite) below.

Linux settings and models live in `${XDG_DATA_HOME:-~/.local/share}/dev.promptify.app`.
Installed and development builds share that location; do not run both at once.

### Update availability and installation

Promptify checks GitHub's published **latest stable release** when it starts.
It compares the release tag's semantic version with the version embedded in the
running app; drafts, prereleases, tags without releases, and installer workflow
artifacts are not updates. A newer release adds an indicator in Settings and an
**Install update** action in the system tray. It never installs automatically.

Click **Install update** to download the matching installer and verify its size
and SHA-256 against that release's `SHA256SUMS.txt` before installation:

- **Windows x64:** opens the NSIS installer and exits Promptify to release its
  executable files. Follow the installer and any Windows authorization prompts.
  Starting the installer is not reported as proof that installation completed.
- **Linux `.deb` / `.rpm`:** confirms package ownership of the running executable
  and runs APT or DNF through `pkexec`. The system requests authorization; a
  cancelled authorization or nonzero installer exit is shown as an error.
  These paths require the matching package manager and a working Polkit agent.
- **Linux AppImage:** replaces the running AppImage with a verified download
  using a same-directory atomic rename. Its directory must be writable.
  Click **Restart Promptify** after installation.
- **Unsupported architectures, source/unmanaged Linux builds, or platforms
  without a published matching installer:** the new-release indicator remains
  visible, but installation is disabled with an explanation. macOS installer
  publishing is not currently configured.

Settings -> **Updates** also provides **Check for updates**, release notes, and
an option to disable startup checks. Checks do not block model loading or prompt
generation. Successful checks are cached for one hour per running session;
failed checks may retry after one minute. Network failures, invalid release
metadata, missing checksums, download corruption, and installation errors are
displayed, not treated as successful updates. Windows installer downloads are
retained under the application data directory's `updates` folder because the
installer still needs them after Promptify exits.

Update checks contact `api.github.com` without authentication, so GitHub's public
API rate limits apply. Downloads follow HTTPS redirects only to GitHub's release
hosts. The request identifies Promptify's version; it sends no prompts, history,
window titles, or application context. Published SHA-256 checksums protect
download integrity, but are not an independent publisher signature: the trust
boundary is the GitHub repository/release account and HTTPS.

For each new version, publish a stable release tagged `v<version>` with the
existing canonical installer names and `SHA256SUMS.txt` containing exactly one
SHA-256 entry for each downloadable installer. Publishing only CI artifacts or
a Git tag does not make an update available.

On first launch, Promptify opens a required guided tour in Settings:

1. **Models and inference:** download and select a speech model, then choose bundled prompt-writing inference
   or configure and test a local server/API connection. Bundled model recommendations favor the highest
   quality tier with confirmed RAM compatibility. Larger models may take longer; smaller and multilingual
   alternatives remain available. Recommendations never switch existing selections. Downloads can be cancelled
   and resumed. Speech and the selected inference backend must be ready before you continue.
   A server/API connection does not require downloading a bundled prompt model.
   Loading failures offer a retry and, for bundled inference, a CPU option.
2. **General:** check the system-default microphone and Prompt shortcut. Refresh after connecting a microphone,
   allow microphone access in your operating system, and change the shortcut if another app has claimed it.
3. **Practice:** prepare the practice field, use the Prompt shortcut, speak a request, and use the shortcut again
   to finish. Keep the field focused until the generated prompt is pasted into it. Press Esc to cancel. Typing,
   a preview, or an unsuccessful paste does not complete this check.
4. **Finish setup:** unlock normal use after the real voice-to-prompt exercise succeeds and setup is saved.

No account or API key is needed for bundled models or an unauthenticated local server. Internet access is
needed to download bundled models; bundled speech and prompt generation run locally afterward.
An API provider still requires network access and may charge for the practice inference calls.
Optional Jev review requires internet access and a TypeSafe key, including during
practice if review has been enabled. The practice exercise does not save its text to history. A current
graphics driver with Vulkan support is recommended; CPU loading is also supported.

Closing Settings or quitting does not skip setup: the tour resumes when you return. Interrupted downloads can be
resumed from Models. Existing configured users keep their usual tray-first startup and are not forced through the
tour. Autostart and vocabulary customization stay outside required setup.

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
accuracy, and app context can all be changed in Settings.

App data lives in `%APPDATA%\dev.promptify.app` (settings, models, and history); logs are
in `%LOCALAPPDATA%\dev.promptify.app\logs`. Installed and development builds use the same data folder, so do not run both
at the same time. Existing settings retain their model selections and preferences. Retired Answer/network settings
are accepted only for migration and omitted on the next save; old pairing/token/MCP files are left untouched but
are no longer loaded or used.

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
  fallback, not proof of a writable input. On KDE, KWin supplies the authoritative window
  identity first. AT-SPI scans only application trees belonging to that authenticated process;
  unrelated applications' child trees, active flags, and failed PID lookups do not invalidate
  the target scan. Unscoped discovery is capped at 32 registrations; native-scoped discovery
  allows up to 4096, with the same overall deadline and per-target tree bounds.
  The native window is rechecked before and after inspection. KDE's KWin snapshot uses a short-lived script,
  unloaded after each query, accepting replies only from KWin's authenticated D-Bus connection.
- **Permission lifecycle**: a private `remote-desktop-token` can reduce repeated permission
  prompts. The first explicit Enable action acquires both sessions and saves authorization
  only after both succeed. Subsequent Wayland launches reconnect both sessions in a background
  worker using that saved choice and the RemoteDesktop restore token. Fresh profiles and
  profiles with disabled integration never request startup authorization. Older profiles
  need one explicit Enable after this upgrade to record combined consent.
  The desktop still controls approval and may display a dialog again; cancellation, revocation,
  or reconnection failure leaves explicit recovery controls rather than claiming permission.
  Enabling on X11 does not record Wayland consent. Cancellation, denial, explicit disable,
  and session revocation clear saved authorization, so startup does not repeat those dialogs.
  Transient portal transport failures retain prior consent without claiming a live grant.
  Partial sessions are closed on shortcut or settings-save failure. General Settings places
  **Enable desktop integration** beside the permission warning and shows shared shortcut
  errors only once. An unavailable optional modifier gesture offers **Disable Ctrl+Shift hold**
  next to its error without disabling the ordinary Prompt shortcut. **Disable desktop integration**
  persists across launches, blocks insertion, invalidates old jobs even after re-enabling,
  and closes active Wayland input/shortcut sessions. Desktop-wide stored permission can also
  be revoked through the system's permission controls.
- **Other permissions**: modifier-hold preferences and approved screen-text apps persist
  in settings; supported hooks are reapplied on startup. Microphone access, macOS Accessibility
  and Input Monitoring approvals are stored by the OS, not in Promptify's configuration.
  Promptify uses existing OS grants and reports revocation; it cannot persist or bypass a denied
  OS permission. Disable remains disabled across restarts, even with saved authorization.
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

### Recovery controls

Errors that need user action keep their recovery controls nearby:

| Area | Available actions |
| --- | --- |
| Startup / connection | Retry startup or connection; open the app data folder to inspect invalid configuration without resetting it |
| Microphone | Open system microphone settings; refresh microphone and shortcut detection |
| Shortcuts / desktop access | Enable integration, retry focus detection, change shortcuts, resume paused shortcuts, retry or disable optional Ctrl+Shift hold; open macOS Accessibility or Input Monitoring settings when permission is needed |
| Models | Download, resume, cancel, select or delete models; refresh model events; retry loading, use CPU fallback, or choose another model |
| Models: Prompt writer | Switch between on-device and online/server writers; save credentials securely; discover, check, edit, or remove endpoint connections; choose an online model from the discovered list |
| Failed results | Open General, Models or Prompt types as appropriate; dismiss any result; copy retained output and restore the previous clipboard |
| History / vocabulary | Refresh history and its connection; edit invalid correction pairs; retry failed saves |
| Prompt types | Retry loading; explicitly restore default routing when the routing settings need repair |
| Updates | Retry status/event connection or update checks; download manually from the release page when no installer is supported; install and restart |
| Practice | Prepare, cancel, retry the practice connection, return to setup checks or finish after a verified paste |

Actionable overlay errors remain visible until dismissed or replaced by a new request.
Recovery never retries paste into an uncertain destination, grants desktop permission
without consent, or resets the user's configuration automatically. System-settings
shortcuts use fixed OS panels; Linux launchers currently support KDE and GNOME.
Other desktops receive an explicit instruction to open their settings manually.
Settings refreshes read cached focus diagnostics and run hardware/status work off the UI thread.
Only an explicit **Retry focus detection** performs a new scan, and it refuses while a job
owns the context. Recording opens the microphone concurrently with target inspection,
without showing focus-changing UI first; startup and inspection timings are logged separately.

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
Failed enable attempts restore the previous monitoring state and leave the saved option
unchanged. A disabled gesture does not display cached background-monitoring errors or
Retry monitoring controls; the failed action itself still reports its error.

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

#### Application-independent Linux focus discovery

A KDE Wayland reproduction identified the target application successfully but blocked
adaptive insertion with `SurfaceUnconfirmed`: the desktop-wide AT-SPI scan timed out or
encountered a node with 41 children, above its former 32-child bound. This was an inspection
failure before paste dispatch, not proof of a destination application's clipboard defect.

The fix uses native focused-process evidence on KDE and AT-SPI field metadata without
application-name allowlists. It fetches each node's child references through `GetChildren`
instead of one IPC round trip per child. Inspection remains bounded: 256 children per node,
512 nodes, depth 32, 50 ms per call and a 650 ms overall scan budget. Ambiguous focus,
provider errors, expired budgets, process mismatches and password/read-only fields still
fail closed. Outside KDE on Wayland, unique-active-window accessibility discovery remains in use;
Windows/macOS native adapters are unchanged.
On X11, native window handles cannot be equated with synthetic AT-SPI identities.
Field inspection is skipped instead of scanning and then comparing incompatible handles
or trusting a PID alone: same-process windows must remain distinguishable. Native window
focus checks remain intact; adaptive routing uses review/copy unless its input is confirmed.

Regression coverage exercises a 41-child tree under three unrelated service identities,
native process filtering, complete child retention, ambiguous focus, provider failure,
deadline/node/depth limits and protected fields. The live registry batch/PID probe passed
on the affected KDE host. Audit recovery validation passed 93 desktop library tests,
183 core tests and 74 browser tests; four optional live tests are skipped by the default
desktop suite. The read-only live KWin test separately passed, measuring three focus
snapshots at 2, 2 and 1 ms on this host. Those timings are not microphone-startup or
end-to-end paste measurements; no destination-delivery claim follows from them.
A live writable-field check additionally requires the destination
input to be focused and exposed by its accessibility provider:

```bash
cargo test --locked -p promptify --lib destination_linux::tests
cargo test --locked -p promptify --lib live_registry_child_count -- --ignored --nocapture
cargo test --locked -p promptify --lib live_native_target_inspection -- --ignored --nocapture
```

The fix does not turn every application window into an approved AI input. If an application
does not expose the focused field, review/copy or explicitly select the actual AI input
surface for the next request. Enabling that application's accessibility support may help;
Promptify does not change external application settings or grant permissions automatically.

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
| `inference` | show selected inference status without exposing credentials or endpoint URLs |
| `transcribe <file.wav>`, `run <file.wav>` | speech only, or the full pipeline on a recording |
| `live-sim <file.wav>` | replays a recording as if spoken; compares live chunks with one full pass |
| `rewrite "<text>" [--process claude.exe] [--url URL] [--mode prompt\|dictation] [--auto]` | typed text through the pipeline |
| `screen-text` | after 3 s, reads the focused text box of the foreground app, as the app would |
| `eval eval\cases.toml` | structure evaluation (25 opaque cases; `PROMPTIFY_EVAL_SHOW=1` prints prompts and rejected repairs; nonzero exit on failure) |
| `prompt-types` | print the 264-entry taxonomy with activation status, priorities, and input requirements; no model needed |
| `route "<text>" [--process NAME] [--url URL] [--surface SURFACE] [--prompt-type ID]` | inspect adaptive task/surface selection without generation or user-history access |
| `eval-routing eval\adaptive.toml` | deterministic task/form regression benchmark, with per-case failures and macro-F1 |
| `eval-adaptive eval\adaptive.toml [--model ID]` | generate with the selected installed model; validate graphs, numeric preservation, and required user details |

Set `PROMPTIFY_LOG=1` for diagnostics on stderr. `PROMPTIFY_LLM_NO_PREFIX_CACHE=1` turns off the language model's
prompt-prefix cache for comparison.

`rewrite` and `run` accept `--rendering legacy|adaptive`, `--prompt-type ID`, and `--surface SURFACE`.
Overrides require adaptive rendering and Prompt mode; unknown or proposed task IDs are rejected explicitly.
`--model ID` selects an already installed language model for the current CLI run without saving that preference.
Ordinary `rewrite` and `run` commands otherwise use the saved inference selection, including a configured
LM Studio server or API provider. API requests may incur charges. The evaluation commands retain their
documented bundled-local model contracts; they do not silently send cases to the selected API provider.
Dictation mode does not require language-model credentials. Configure keys in the desktop's
Models -> Prompt writer settings, not command-line arguments.
`route` defaults to adaptive rendering; `rewrite` and `run` retain the existing-profile default.
Retired `mcp`, `serve`, and `remote` commands, `--mcp`/network options, and Answer mode fail explicitly before model loading.

### Task graphs and check loops

- Complexity guidance asks the model for 2 steps and a short check loop for simple requests, 3-4 steps for moderate
  requests, and 4-8 steps for complex requests, with independent work marked for parallel execution. Terminal prompts
  keep the graph in one paragraph.
- Every graph needs numbered steps, a loop that returns to an existing step with a limit of 1-8 rounds, and a
  non-empty `Done when:` section. Dependencies may refer only to earlier steps. The model is instructed to check the
  completion criteria, stop when they pass, and report unmet criteria when the round limit is reached.
- Graph prompts must open with the goal, not persona boilerplate such as `Act as...` or `You are an expert...`.
  Such openers trigger a repair and fail evaluation. If no valid goal-first repair is produced, the draft is rejected
  with an explicit error rather than pasted or returned as a usable prompt; relevant perspectives inside steps remain allowed.
- Validation checks the finished, sanitized text, including terminal newline handling. A repair shares the original
  generation deadline and is checked the same way. Quality-first defaults permit two repair attempts, only when
  needed; valid first drafts use no extra generation. If no repair passes, the draft is rejected with an explicit
  error rather than pasted or returned as a usable prompt. The historical `kept_original` status remains in types
  for compatibility but is no longer emitted for new prompts.
- Dictation does not add graphs.
- Image/video and other generator requests keep their descriptions **inside the graph's creation step**.
  Generator-only fields cannot be assumed to execute a graph or check loop, so these workflows are review-only.
  Use a conversational AI with the appropriate tools, or use Dictation to enter literal content yourself.

### Prompt types and adaptive routing

Enable **Use task-aware prompt adaptation** in Settings -> **Prompt types**. This is local, opt-in rewriting:
Promptify does not run the recipient's tools, upload assets, or produce final SQL/formulas/lyrics.
Bundled inference is local; selecting an API provider explicitly permits its text inference calls.
The separate Jev option sends the transcript and generated draft for cloud review only when enabled.

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

The recording overlay shows the selected task, form, surface, and warnings. A correction can be queued in
Prompt types for the next Prompt hotkey use only; Dictation and isolated onboarding practice do not consume it.
The overlay fits its native window to the content. Long transcripts and results scroll within a bounded panel,
while Copy and Dismiss remain visible. Clipboard and window-control failures appear in the overlay rather
than becoming unhandled errors.

#### Test a prompt type

1. In **Settings -> Prompt types**, turn on **Use task-aware prompt adaptation**.
2. Optionally expand **Choose a type or surface for the next prompt** to confirm the actual AI input.
3. Click the actual input in a recognized AI app, press `Ctrl+Alt+Space`, speak a request such as
   `Debug the checkout crash and add unit tests`, and stop recording. Keep that input focused until insertion finishes.
4. Confirm the inserted prompt contains numbered steps, a bounded `Loop:`, and `Done when:` criteria.
   If focus or the identified destination changed, or the field is a generator-only/unknown surface, inspect
   the visible reason and use Copy where appropriate rather than bypassing the safety check.
5. For classification diagnostics without generation, use `promptify-cli route` with the appropriate context.

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
available. **Restore default routing** is an explicit repair command: it first saves the exact existing
bytes to a new `routing-settings.json.backup-N`, never replaces older backups, and only then writes
legacy defaults. Backup/read failures leave the original untouched.
History IDs reserve a durable `history.id` high-water mark under the shared history lock before append.
Clearing, deleting, retention, restarts and clock rollback do not reuse issued IDs; an invalid high-water
mark fails closed rather than resetting the sequence. The graph/loop requirement applies in both policies
and cannot be disabled by a task override.

The taxonomy was informed by primary task/capability documentation and cross-product evidence, not Promptify
usage statistics: [NBER/OpenAI usage research](https://www.nber.org/papers/w34255),
[Anthropic usage by product](https://www.anthropic.com/research/economic-index-june-2026-report),
[GitHub Copilot tasks](https://docs.github.com/en/copilot/tutorials/copilot-cookbook),
[Excel AI tasks](https://support.microsoft.com/en-us/excel/copilot/data-insights-with-copilot-in-excel),
[Suno's separate input fields](https://help.suno.com/en/articles/3726721), and
[ElevenLabs sound-effect prompts](https://elevenlabs.io/docs/eleven-creative/playground/sound-effects).
Provider input contracts can change; activate additional recipes only after verifying the particular surface.

## Tests

```powershell
cargo test --workspace --exclude promptify-llm
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
quality recommendations, migration of remembered settings tabs, retained drafts, and 560-pixel overlay windows at 150% scale, including long results and clipboard errors.
Core tests include four positive and two confusable negative utterances for every
enabled recipe, a separate 48-case task/form regression dataset, final graph/loop validation, incompatible
surfaces, same-window site changes, and history-sidecar isolation. The measured macro-F1 applies only to the
fixture dataset, not to all real requests. Real-model evaluation checks additional source details and must be
run separately for each installed model tier; it does not replace human review of generated instructions.

`promptify-llm` builds llama.cpp and is covered by the CLI end-to-end runs (`rewrite`, `eval`, and `eval-adaptive`).
Native settings tests cover migration of retired integration preferences without resetting surviving settings.
CLI tests verify that removed commands/options fail before model loading.

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
4. Settings has exactly six sections and no Answer, Advanced, Tools/MCP, or Desktop API controls.
5. `promptify-cli rewrite "draft a launch announcement" --process claude.exe` generates a local prompt.
6. `promptify-cli eval eval\cases.toml` checks all 25 task-graph cases.
7. Hold a hotkey and speak for 10+ seconds with pauses: the overlay shows the text heard so far.
8. Settings -> General: enable *Also start a prompt by holding Ctrl+Shift*, then hold both keys alone.
   Ctrl+Shift+T in a browser must not start a recording.
9. Settings -> Your words: add your own name and a correction, save, then dictate them.
10. Models recommends compatible quality tiers without changing your selections or starting downloads.
11. Update checks and explicit model downloads remain available; bundled inference needs no network connection.

## Not done yet

- Screenshot context with a vision model (needs a llama.cpp build with multimodal support and a projector).
- Code signing and a packaged macOS installer; macOS source support remains experimental.

## License

Promptify is free and open-source software licensed under the [MIT License](LICENSE).
