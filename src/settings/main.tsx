import { useCallback, useEffect, useState } from "react";
import { createRoot } from "react-dom/client";
import { listen } from "@tauri-apps/api/event";
import { api, type AppInfo, type DownloadEvent, type HistoryEntry, type ModelStatus, type OfferInfo, type PreviewOutput, type ProfileSummary, type RemoteInfo } from "../api";
import "./settings.css";

const gb = (bytes: number) => `${(bytes / 1e9).toFixed(bytes < 1e9 ? 2 : 1)} GB`;

function formatHotkey(accelerator: string) {
  const isMac = navigator.userAgent.includes("Mac");
  return accelerator.replace("CommandOrControl", isMac ? "Cmd" : "Ctrl").replace("Alt", isMac ? "Option" : "Alt");
}

const MODIFIER_CODES = new Set(["ControlLeft", "ControlRight", "ShiftLeft", "ShiftRight", "AltLeft", "AltRight", "MetaLeft", "MetaRight"]);

/** Converts a key press into a Tauri accelerator; null while only modifiers are held. */
function toAccelerator(e: React.KeyboardEvent): string | null {
  if (MODIFIER_CODES.has(e.code)) return null;
  const parts: string[] = [];
  if (e.ctrlKey) parts.push("Control");
  if (e.metaKey) parts.push("Super");
  if (e.altKey) parts.push("Alt");
  if (e.shiftKey) parts.push("Shift");
  if (parts.length === 0) return null;
  const key = e.code.startsWith("Key") ? e.code.slice(3) : e.code.startsWith("Digit") ? e.code.slice(5) : e.code;
  return [...parts, key].join("+");
}

function HotkeyField({ mode, value, onSaved }: { mode: "prompt" | "dictation"; value: string; onSaved: () => void }) {
  const [capturing, setCapturing] = useState(false);
  const [error, setError] = useState<string | null>(null);
  return (
    <>
      {capturing ? (
        <input
          autoFocus
          readOnly
          className="capture"
          value="Press the new key combination… (Esc to cancel)"
          onBlur={() => setCapturing(false)}
          onKeyDown={(e) => {
            e.preventDefault();
            if (e.code === "Escape") return setCapturing(false);
            const accelerator = toAccelerator(e);
            if (!accelerator) return;
            setCapturing(false);
            api.setHotkey(mode, accelerator).then(() => {
              setError(null);
              onSaved();
            }, (err) => setError(String(err)));
          }}
        />
      ) : (
        <>
          <kbd>{formatHotkey(value)}</kbd> <button className="link" onClick={() => setCapturing(true)}>Change</button>
        </>
      )}
      {error && <div className="error">{error}</div>}
    </>
  );
}

function ModifierHold({ enabled, onChange }: { enabled: boolean; onChange: () => void }) {
  const [error, setError] = useState<string | null>(null);
  return (
    <>
      <label className="inline">
        <input
          type="checkbox"
          checked={enabled}
          onChange={(e) => void api.setModifierHold(e.target.checked).then(() => { setError(null); onChange(); }, (err) => setError(String(err)))}
        />
        Also start a prompt by holding <kbd>Ctrl</kbd>+<kbd>Shift</kbd> on their own
      </label>
      <span className="hint">Shortcuts like Ctrl+Shift+T and the quick Ctrl+Shift layout switch are ignored.</span>
      {error && <div className="error">{error}</div>}
    </>
  );
}

function Status({ info, onChange }: { info: AppInfo | null; onChange: () => void }) {
  if (!info) return <p>Loading…</p>;
  return (
    <section>
      <h2>Status</h2>
      <dl>
        <dt>Microphone</dt>
        <dd>{info.input_device ? `${info.input_device} (system default)` : "No default microphone found"}</dd>
        <dt>Prompt hotkey</dt>
        <dd><HotkeyField mode="prompt" value={info.hotkeys.prompt} onSaved={onChange} /> hold to talk, or tap to start and tap again to finish</dd>
        <dt></dt>
        <dd>
          <ModifierHold enabled={info.modifier_hold} onChange={onChange} />
        </dd>
        <dt>Dictation hotkey</dt>
        <dd><HotkeyField mode="dictation" value={info.hotkeys.dictation} onSaved={onChange} /></dd>
        <dt>Cancel</dt>
        <dd><kbd>{info.hotkeys.cancel}</kbd> while listening or processing</dd>
        <dt>Local models</dt>
        <dd>{info.engines_ready ? "Ready" : "Download a speech model and a prompt model below to start."}</dd>
        <dt>Acceleration</dt>
        <dd>
          <label className="inline">
            <input type="checkbox" checked={info.use_gpu} onChange={(e) => void api.setUseGpu(e.target.checked).then(() => window.setTimeout(onChange, 4000))} />
            Use the GPU when available
          </label>
          <span className="hint">{info.gpu_device ? `Prompt model running on ${info.gpu_device}` : "Prompt model running on the CPU"}</span>
        </dd>
      </dl>
      {info.hotkey_errors.map((e) => (
        <p key={e} className="error">{e}</p>
      ))}
    </section>
  );
}

function Models({ onChange }: { onChange: () => void }) {
  const [models, setModels] = useState<ModelStatus[]>([]);
  const [progress, setProgress] = useState<Record<string, DownloadEvent>>({});
  const [error, setError] = useState<string | null>(null);

  const refresh = useCallback(() => {
    void api.listModels().then(setModels);
    onChange();
  }, [onChange]);

  useEffect(() => {
    refresh();
    const unlisten = listen<DownloadEvent>("model-download", ({ payload }) => {
      setProgress((p) => ({ ...p, [payload.id]: payload }));
      if (payload.done) {
        if (payload.error) setError(`${payload.id}: ${payload.error}`);
        refresh();
      }
    });
    return () => void unlisten.then((stop) => stop());
  }, [refresh]);

  const act = (fn: () => Promise<unknown>) => {
    setError(null);
    fn().then(refresh, (e) => setError(String(e)));
  };

  const group = (kind: ModelStatus["kind"], title: string) => (
    <>
      <h3>{title}</h3>
      <table>
        <tbody>
          {models.filter((m) => m.kind === kind).map((m) => {
            const p = progress[m.id];
            const pct = p && !p.done ? Math.floor((p.downloaded / p.total) * 100) : null;
            return (
              <tr key={m.id}>
                <td>
                  <input type="radio" name={kind} checked={m.selected} disabled={!m.installed} onChange={() => act(() => api.selectModel(m.id))} aria-label={`Use ${m.display_name}`} />
                </td>
                <td>
                  <strong>{m.display_name}</strong> <span className="hint">{m.tier} · {m.license} · {m.min_ram_gb} GB+ RAM</span>
                </td>
                <td className="actions">
                  {m.installed && !m.downloading && <button onClick={() => act(() => api.deleteModel(m.id))}>Delete</button>}
                  {!m.installed && !m.downloading && (
                    <button onClick={() => act(() => api.downloadModel(m.id))}>
                      {m.partial_bytes > 0 ? `Resume (${gb(m.partial_bytes)} of ${gb(m.size_bytes)})` : `Download ${gb(m.size_bytes)}`}
                    </button>
                  )}
                  {m.downloading && (
                    <>
                      <span>{p?.verifying ? "Verifying…" : pct !== null ? `${pct}%` : "Starting…"}</span>
                      <button onClick={() => act(() => api.cancelDownload(m.id))}>Cancel</button>
                    </>
                  )}
                </td>
              </tr>
            );
          })}
        </tbody>
      </table>
    </>
  );

  return (
    <section>
      <h2>Models</h2>
      <p className="hint">Downloaded from Hugging Face over HTTPS, checked against pinned SHA-256 hashes, and run entirely on this device.</p>
      {group("stt", "Speech to text")}
      {group("llm", "Prompt writer")}
      {error && <p className="error">{error}</p>}
    </section>
  );
}

function History({ info, onChange }: { info: AppInfo | null; onChange: () => void }) {
  const [entries, setEntries] = useState<HistoryEntry[]>([]);
  const [error, setError] = useState<string | null>(null);
  const refresh = useCallback(() => void api.listHistory().then(setEntries), []);

  useEffect(() => {
    refresh();
    const unlisten = listen("history-changed", refresh);
    return () => void unlisten.then((stop) => stop());
  }, [refresh]);

  const act = (fn: () => Promise<unknown>) => {
    setError(null);
    fn().then(() => {
      refresh();
      onChange();
    }, (e) => setError(String(e)));
  };

  return (
    <section>
      <h2>History</h2>
      <p className="hint">
        Saved only on this device. Your past requests and the prompts you used teach Promptify your style, and your last prompt in the same app lets you follow up with things like “make it shorter”. Surrounding screen text and audio are never saved.
      </p>
      <label className="inline">
        <input type="checkbox" checked={info?.history_enabled ?? false} onChange={(e) => act(() => api.setHistoryEnabled(e.target.checked))} />
        Save history and use it as context
      </label>
      <button disabled={entries.length === 0} onClick={() => act(api.clearHistory)}>Clear all</button>
      {error && <p className="error">{error}</p>}
      <ul className="history">
        {entries.map((e) => (
          <li key={e.id}>
            <div className="hint">
              {new Date(e.created_ms).toLocaleString()} · {e.app_key} · {e.mode} · {e.inserted ? "pasted" : "not pasted"}
              <button className="link" onClick={() => act(() => api.deleteHistoryEntry(e.id))}>Delete</button>
            </div>
            <p className="said">“{e.transcript}”</p>
            {e.mode === "prompt" && <pre>{e.output}</pre>}
          </li>
        ))}
      </ul>
    </section>
  );
}

function Playground({ profiles }: { profiles: ProfileSummary[] }) {
  const [transcript, setTranscript] = useState("um so help me plan customer interviews for the new onboarding flow, like goals and questions");
  const [processName, setProcessName] = useState("chrome.exe");
  const [windowTitle, setWindowTitle] = useState("");
  const [url, setUrl] = useState("https://claude.ai/new");
  const [surrounding, setSurrounding] = useState("");
  const [preview, setPreview] = useState<PreviewOutput | null>(null);
  const [error, setError] = useState<string | null>(null);

  const run = async () => {
    setError(null);
    try {
      setPreview(
        await api.previewPrompt({
          transcript,
          processName,
          windowTitle,
          url: url || null,
          surroundingText: surrounding || null,
        }),
      );
    } catch (e) {
      setPreview(null);
      setError(String(e));
    }
  };

  return (
    <section>
      <h2>Context playground</h2>
      <p className="hint">Check which of the {profiles.length} target profiles a window resolves to, and the exact messages the local model will receive.</p>
      <div className="grid">
        <label>Process<input value={processName} onChange={(e) => setProcessName(e.target.value)} /></label>
        <label>Window title<input value={windowTitle} onChange={(e) => setWindowTitle(e.target.value)} /></label>
        <label>URL<input value={url} onChange={(e) => setUrl(e.target.value)} /></label>
      </div>
      <label>Spoken text<textarea rows={3} value={transcript} onChange={(e) => setTranscript(e.target.value)} /></label>
      <label>Surrounding text (optional)<textarea rows={2} value={surrounding} onChange={(e) => setSurrounding(e.target.value)} /></label>
      <button onClick={() => void run()}>Preview</button>
      {error && <p className="error">{error}</p>}
      {preview && (
        <div className="preview">
          <p>Profile: <strong>{preview.profileName}</strong> ({preview.profileId})</p>
          {preview.messages.map((m, i) => (
            <details key={i} open={i === preview.messages.length - 1}>
              <summary>{m.role}</summary>
              <pre>{m.content}</pre>
            </details>
          ))}
        </div>
      )}
    </section>
  );
}

function relayLabel(info: RemoteInfo): string {
  const relay = info.status?.relay;
  if (!relay || relay.state === "disabled") return "No relay (local network only)";
  if (relay.state === "connected") return "Relay connected";
  if (relay.state === "connecting") return "Connecting to relay…";
  return `Relay error: ${relay.message}`;
}

function Remote() {
  const [info, setInfo] = useState<RemoteInfo | null>(null);
  const [relayUrl, setRelayUrl] = useState("");
  const [offer, setOffer] = useState<OfferInfo | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [now, setNow] = useState(() => Date.now() / 1000);

  const refresh = useCallback(() => {
    void api.remoteInfo().then((next) => {
      setInfo(next);
      if (next.status?.pairing_expires_unix == null) setOffer(null);
    });
  }, []);

  useEffect(() => {
    void api.remoteInfo().then((next) => {
      setInfo(next);
      setRelayUrl(next.relay_url ?? "");
    });
    const timer = window.setInterval(() => {
      setNow(Date.now() / 1000);
      refresh();
    }, 2000);
    return () => window.clearInterval(timer);
  }, [refresh]);

  const act = (fn: () => Promise<unknown>) => {
    setError(null);
    fn().then(refresh, (e) => {
      setError(String(e));
      refresh();
    });
  };

  if (!info) return null;
  const save = (enabled: boolean, lan: boolean) => act(() => api.setRemoteSettings(enabled, relayUrl.trim() || null, lan));
  const secondsLeft = offer ? Math.max(0, Math.round(offer.expires_unix - now)) : 0;

  return (
    <section>
      <h2>Phones and remote access</h2>
      <p className="hint">
        Let your own phones use this computer's local models. Everything between a paired phone and this computer is end-to-end encrypted; a relay only forwards scrambled data it cannot read. Phones never get your history or screen text.
      </p>
      <label className="inline">
        <input type="checkbox" checked={info.enabled} onChange={(e) => save(e.target.checked, info.lan_direct)} />
        Allow paired devices to use Promptify
      </label>
      <label className="inline">
        <input type="checkbox" checked={info.lan_direct} onChange={(e) => save(info.enabled, e.target.checked)} />
        Accept direct connections on this network
      </label>
      <label>
        Relay for access over the internet (optional, self-hosted)
        <input placeholder="wss://relay.example.net" value={relayUrl} onChange={(e) => setRelayUrl(e.target.value)} />
      </label>
      <button onClick={() => save(info.enabled, info.lan_direct)}>Save relay</button>
      {info.enabled && info.status && (
        <p className="hint">
          {relayLabel(info)} · {info.status.sessions} connected · local API on {info.status.listen} (token in {info.api_token_path})
        </p>
      )}
      {info.enabled && (
        <div>
          {offer && secondsLeft > 0 ? (
            <div className="pairing">
              <img alt="Pairing code" width={240} height={240} src={`data:image/svg+xml;utf8,${encodeURIComponent(offer.svg)}`} />
              <p className="hint">Scan with the Promptify app within {secondsLeft}s. The code works once.</p>
              <details>
                <summary>Pairing link</summary>
                <pre>{offer.uri}</pre>
              </details>
              <button onClick={() => act(api.cancelPairing)}>Cancel pairing</button>
            </div>
          ) : (
            <button onClick={() => act(() => api.createPairingOffer().then(setOffer))}>Pair a phone</button>
          )}
        </div>
      )}
      {error && <p className="error">{error}</p>}
      {info.error && <p className="error">{info.error}</p>}
      <ul className="history">
        {info.devices.map((d) => (
          <li key={d.id}>
            <strong>{d.name}</strong>
            <span className="hint">
              {" "}· paired {new Date(d.paired_unix * 1000).toLocaleDateString()}
              {d.last_seen_unix ? ` · last seen ${new Date(d.last_seen_unix * 1000).toLocaleString()}` : ""}
            </span>
            <button className="link" onClick={() => act(() => api.removeDevice(d.id))}>Remove</button>
          </li>
        ))}
      </ul>
    </section>
  );
}

function Settings() {
  const [info, setInfo] = useState<AppInfo | null>(null);
  const [profiles, setProfiles] = useState<ProfileSummary[]>([]);
  const refreshInfo = useCallback(() => void api.appInfo().then(setInfo), []);

  useEffect(() => {
    refreshInfo();
    void api.listProfiles().then(setProfiles);
  }, [refreshInfo]);

  return (
    <main>
      <h1>Promptify</h1>
      <p className="hint">Hold the hotkey anywhere, say what you want, and a structured prompt for the app you're in is written locally on this device. Promptify keeps running in the system tray when you close this window.</p>
      <Status info={info} onChange={refreshInfo} />
      <Models onChange={refreshInfo} />
      <History info={info} onChange={refreshInfo} />
      <Remote />
      <Playground profiles={profiles} />
    </main>
  );
}

createRoot(document.getElementById("root")!).render(<Settings />);
