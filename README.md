# Promptify

Speak a rough request and get a clear, well-structured prompt pasted into the AI app you are using. Everything runs on your
own computer: Whisper for speech, a local Qwen model for writing. No cloud AI services are used.

- **Prompt mode** (`Ctrl+Alt+Space`): turns what you said into a prompt that fits the target app (ChatGPT, Claude,
  Gemini, Grok, Copilot, Perplexity, Cursor, VS Code, Claude Code, Codex CLI, terminals, image generators...).
  Multi-step requests become a **task graph**: numbered steps, explicit dependencies (`after 1, 2`) and bounded
  check-and-revise loops (`max N rounds`). The output is validated and, if needed, repaired once.
- **Dictation mode** (`Ctrl+Alt+Shift+Space`): pastes what you said with filler words removed.
- Hold the hotkey while speaking, or tap it once to start and again to stop. `Esc` cancels. Both are rebindable.
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

## Developer CLI

`cargo run -p promptify --bin promptify-cli -- <command>` (or `C:\ptb\debug\promptify-cli.exe` if `CARGO_TARGET_DIR=C:\ptb`):

| Command | Purpose |
| --- | --- |
| `models`, `download <id>` | list or install models |
| `transcribe <file.wav>`, `run <file.wav>` | speech only, or the full pipeline on a recording |
| `rewrite "<text>" [--process claude.exe] [--url URL] [--mcp mcp.json]` | typed text through the prompt writer |
| `eval eval\cases.toml` | structure evaluation (20 opaque cases; `PROMPTIFY_EVAL_SHOW=1` prints prompts) |
| `serve [--relay wss://...] [--listen 127.0.0.1:47822]` | headless remote server; prints a pairing link |
| `remote pair <link> [--direct]`, `remote send "<text>" [--app claude] [--dictation] [--direct]` | act as a paired phone |
| `mcp [--api http://127.0.0.1:47821]` | stdio MCP server (see below) |

Set `PROMPTIFY_LOG=1` for diagnostics on stderr.

## Remote access

Settings → **Phones and remote access** → *Allow paired devices to use Promptify*. Then:

- **Pair a device**: *Pair a new device* shows a QR code (valid 5 minutes, single use). The phone app scans it.
- **On your network**: turn on *Allow direct connections on this network*. The desktop listens on port 47821.
- **From anywhere**: run the relay (below) and enter its `wss://` address. Plain `ws://` is accepted only for loopback
  and private-network addresses.
- Paired devices are listed with *Remove*. Removing a device ends its session immediately.

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

Create `%APPDATA%\dev.promptify.app\mcp.json`, then restart Promptify:

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
- Hooks run only for hotkey prompts, never for dictation, remote devices or the API.
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
7. `promptify-cli eval eval\cases.toml`. The last run scored graph 10/10 and flat 10/10 on Qwen3.5 9B.

## Not done yet

- Native iOS and Android apps. The protocol, pairing and reference client (`crates/promptify-server/src/client.rs`)
  are ready for them. An iOS keyboard extension will need *Full Access* for network use.
- Live streaming transcription, a hosted relay, code signing, installers, and macOS/Linux hotkey and paste backends.
- KV-cache reuse in the language-model worker.
