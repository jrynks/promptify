# Promptify

Speak a rough request and get a clear, well-structured prompt pasted into the AI app you are using. Everything runs on your
own computer: Whisper for speech, a local Qwen model for writing. No cloud AI services are used.

- **Prompt mode** (`Ctrl+Alt+Space`): turns what you said into a prompt that fits the target app (ChatGPT, Claude,
  Gemini, Grok, Copilot, Perplexity, Cursor, VS Code, Claude Code, Codex CLI, terminals, image generators...).
  AI requests become a **task graph**, sized to their complexity: numbered steps, explicit dependencies (`after 1, 2`),
  bounded check-and-revise loops (`max N rounds`) and `Done when:` completion checks. Image and video creation prompts
  stay descriptive. The finished output is validated and, if needed, repaired once.
- **Dictation mode** (`Ctrl+Alt+Shift+Space`): pastes what you said with filler words removed. Say "new line",
  "new paragraph" or "scratch that" as their own sentence to edit as you speak.
- **Answer mode** (optional hotkey, set it in Settings): ask a question; the local model's answer is shown with a Copy
  button and never pasted.
- Hold the hotkey while speaking, or tap it once to start and again to stop. `Esc` cancels. All are rebindable.
  Optionally, holding **Ctrl+Shift on their own** also starts a prompt (Windows; shortcuts like Ctrl+Shift+T are ignored).
- **Live transcription**: speech is transcribed at natural pauses while you talk, so the result is ready moments after
  you stop. The overlay shows what was heard so far.
- **Automatic mode** (optional): outside AI apps the prompt hotkey types plain dictation. Start with "prompt:" or
  "dictate:" to choose yourself.
- **Your words**: names and terms Whisper should expect, corrections (`heard => meant`), and the apps whose focused
  text box may be used as context (never password fields, never saved).
- **Phones and remote access** (off by default): pair a phone with a QR code; it sends audio or text to your desktop
  over an end-to-end encrypted channel, directly on your network or through a self-hosted relay.
- **MCP**: Promptify is an MCP server (other AI tools can ask it to write prompts) and an MCP client (your MCP tools can
  add reference facts before a prompt is written).

## Build and run (Windows dev build)

Requirements: Rust 1.85+ (tested 1.97), Node 22+, the Vulkan SDK (`VULKAN_SDK` set), and libclang
(`LIBCLANG_PATH`, e.g. from `pip install libclang`). A Vulkan-capable GPU is recommended; models fall back to CPU.

```powershell
npm install
npm run app            # builds the workers and launches the desktop app (tray icon)
```

On first launch the settings window opens to download a speech model and a language model.
App data lives in `%APPDATA%\dev.promptify.app` (settings, models, history, `remote\`, optional `mcp.json`);
logs are in `%LOCALAPPDATA%\dev.promptify.app\logs`. `PROMPTIFY_DATA_DIR` and `PROMPTIFY_MODELS_DIR` override them
for isolated test runs.

## Build and run (Fedora / Bazzite)

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

On Wayland, the taskbar and titlebar icons are resolved through a desktop entry, not the window's embedded PNG.
For an unbundled development build, a user-local `promptify.desktop` entry must match `StartupWMClass=promptify`
and point `Icon` to this checkout's `src-tauri/icons/icon.png`. Keep that development entry hidden with
`NoDisplay=true` and continue launching through the task above; Linux bundles install their own desktop entry and icons.

## Windows installer

```powershell
npm run installer      # C:\ptb\release\bundle\nsis\Promptify_0.1.0_x64-setup.exe
```

Needs Visual Studio Build Tools (its Visual C++ and OpenMP runtime DLLs are installed next to the app). The installer
is unsigned, so Windows SmartScreen asks for confirmation (*More info* → *Run anyway*). It installs per user, includes
the llama.cpp worker and `promptify-cli.exe`, and downloads WebView2 if it is missing. The target computer needs a
current graphics driver (for `vulkan-1.dll`); models are downloaded on first launch. Installed and dev builds share the
same app data folder, so do not run both on one computer at the same time.

## Developer CLI

`cargo run -p promptify --bin promptify-cli -- <command>` (or `C:\ptb\debug\promptify-cli.exe` if `CARGO_TARGET_DIR=C:\ptb`):

| Command | Purpose |
| --- | --- |
| `models`, `download <id>` | list or install models |
| `transcribe <file.wav>`, `run <file.wav>` | speech only, or the full pipeline on a recording |
| `live-sim <file.wav>` | replays a recording as if spoken; compares live chunks with one full pass |
| `rewrite "<text>" [--process claude.exe] [--url URL] [--mcp mcp.json] [--mode prompt\|dictation\|answer] [--auto]` | typed text through the pipeline |
| `screen-text` | after 3 s, reads the focused text box of the foreground app, as the app would |
| `eval eval\cases.toml` | structure evaluation (24 opaque cases; `PROMPTIFY_EVAL_SHOW=1` prints prompts and rejected repairs; nonzero exit on failure) |
| `serve [--relay wss://...] [--listen 127.0.0.1:47822] [--discoverable]` | headless remote server; prints a pairing link |
| `remote pair <link> [--direct]`, `remote send "<text>" [--app claude] [--dictation] [--direct]` | act as a paired phone |
| `mcp [--api http://127.0.0.1:47821]` | stdio MCP server (see below) |

Set `PROMPTIFY_LOG=1` for diagnostics on stderr. `PROMPTIFY_LLM_NO_PREFIX_CACHE=1` turns off the language model's
prompt-prefix cache for comparison.

### Task graphs and check loops

- Complexity guidance asks the model for 2 steps and a short check loop for simple requests, 3-4 steps for moderate
  requests, and 4-8 steps for complex requests, with independent work marked for parallel execution. Terminal prompts
  keep the graph in one paragraph.
- Every graph needs numbered steps, a loop that returns to an existing step with a limit of 1-8 rounds, and a
  non-empty `Done when:` section. Dependencies may refer only to earlier steps. The model is instructed to check the
  completion criteria, stop when they pass, and report unmet criteria when the round limit is reached.
- Validation checks the finished, sanitized text, including terminal newline handling. A repair shares the original
  generation deadline and is checked the same way. If no repair passes, the original sanitized draft is retained,
  marked `kept_original` in the report, and a warning is logged; it is not marked as a valid graph.
- Dictation and answer mode do not add graphs. Image/video generator sites and requests to create images or videos
  in chat apps use descriptive prompts instead.

## Remote access

Settings → **Phones and remote access** → *Allow paired devices to use Promptify*. Then:

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

Turn on remote access (this creates the local API token), then add this to your MCP client (Claude Desktop,
VS Code `mcp.json`, Cursor...):

```json
{ "mcpServers": { "promptify": { "command": "C:\\path\\to\\promptify-cli.exe", "args": ["mcp"] } } }
```

Tools: `transform_prompt(text, app?, url?)` and `clean_dictation(text)`. The API token is sent only to loopback addresses.

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
cargo clippy --workspace --all-targets
npm run build
```

`promptify-llm` builds llama.cpp and is covered by the CLI end-to-end runs (`rewrite`, `eval`, `serve` + `remote send`).

## Morning check

1. `npm run app`. The tray icon appears, and the overlay shows no errors.
2. In Notepad, press `Ctrl+Alt+Space` and say *"plan a three step launch checklist and review it until it's complete"*.
   You should get a numbered task graph with `after` dependencies and a `max N rounds` loop.
3. Press `Ctrl+Alt+Shift+Space` and dictate a sentence with "um" in it. It should paste cleanly.
4. Settings → Phones and remote access: turn it on, enable direct connections, then *Pair a new device*. A QR code appears.
5. Test a phone connection with the CLI against a separate headless server:
   ```powershell
   $env:PROMPTIFY_DATA_DIR="$env:TEMP\pf-morning"; $env:PROMPTIFY_MODELS_DIR="$env:APPDATA\dev.promptify.app\models"
   promptify-cli serve --offer-file $env:TEMP\pf-offer.txt      # terminal 1; leave running
   promptify-cli remote pair (Get-Content $env:TEMP\pf-offer.txt) --direct   # terminal 2, same variables
   promptify-cli remote send "plan a two step code review with a fix loop" --app claude --direct
   ```
6. `promptify-cli rewrite "draft a launch announcement" --process claude.exe --mcp <your mcp.json>`. With
   `PROMPTIFY_LOG=1`, the output includes a `tool context: N items` line.
7. `promptify-cli eval eval\cases.toml`. All 24 cases should pass: 20 task graphs and 4 descriptive media prompts.
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

- Native iOS and Android apps. The protocol ([schema](protocol/schema-v1.json)), pairing and reference client
  (`crates/promptify-server/src/client.rs`) are ready for them. An iOS keyboard extension will need *Full Access* for
  network use.
- Screenshot context with a vision model (needs a llama.cpp build with multimodal support and a ~1 GB projector).
- Hold-Ctrl+Shift and screen-text reading on macOS and Linux; a hosted relay, code signing and installers.
