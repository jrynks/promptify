import { useCallback, useEffect, useRef, useState } from "react";
import { createRoot } from "react-dom/client";
import { listen } from "@tauri-apps/api/event";
import { api, type AppInfo, type DownloadEvent, type HistoryEntry, type McpInfo, type ModelStatus, type OfferInfo, type OnboardingStatus, type OnboardingStep, type PreviewOutput, type ProfileSummary, type RemoteInfo, type PromptCatalog, type RoutingOptions } from "../api";
import { OnboardingTour } from "./OnboardingTour";
import { DesktopIntegration } from "./DesktopIntegration";
import { PromptRoutingSettings, RoutingControls, RoutingSummary, automaticRouting } from "./PromptRouting";
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

function HotkeyField({ mode, value, onSaved }: { mode: "prompt" | "dictation" | "answer"; value: string | null; onSaved: () => void }) {
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
          {value ? <kbd>{formatHotkey(value)}</kbd> : <span className="hint">Not set</span>}{" "}
          <button className="link" onClick={() => setCapturing(true)}>{value ? "Change" : "Set"}</button>
        </>
      )}
      {error && <div className="error">{error}</div>}
    </>
  );
}

function ModifierHold({ info, onChange }: { info: AppInfo; onChange: () => void }) {
  const enabled = info.modifier_hold_requested;
  const monitorError = info.modifier_hold_error;
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const setEnabled = async (next: boolean) => {
    setBusy(true);
    setError(null);
    try {
      await api.setModifierHold(next);
      onChange();
    } catch (reason) {
      setError(String(reason));
      onChange();
    } finally {
      setBusy(false);
    }
  };
  const selectKeyboard = async (path: string) => {
    setBusy(true);
    setError(null);
    try {
      await api.setModifierKeyboard(path);
    } catch (reason) {
      setError(String(reason));
    } finally {
      setBusy(false);
      onChange();
    }
  };
  return (
    <div className="stack">
      <label className="inline">
        <input
          type="checkbox"
          checked={enabled}
          disabled={busy}
          onChange={(e) => void setEnabled(e.target.checked)}
        />
        Also start a prompt by holding <kbd>Ctrl</kbd>+<kbd>Shift</kbd> on their own
      </label>
      <div className="hint desc indent-check">Shortcuts like Ctrl+Shift+T and the quick Ctrl+Shift layout switch are ignored.</div>
      <div className="hint desc indent-check">macOS requires Input Monitoring permission. Linux Wayland requires explicit read access to physical keyboards; keyboard access can expose all keystrokes. Promptify never saves typed text or blocks keys.</div>
      {info.modifier_keyboard_devices.length > 0 && <label>
        Ctrl+Shift keyboard
        <select value={info.modifier_keyboard ?? ""} disabled={busy} onChange={(event) => void selectKeyboard(event.target.value)}>
          <option value="" disabled>Select a physical keyboard</option>
          {info.modifier_keyboard && !info.modifier_keyboard_devices.some((device) => device.path === info.modifier_keyboard) &&
            <option value={info.modifier_keyboard} disabled>Selected keyboard disconnected</option>}
          {info.modifier_keyboard_devices.map((device) => <option key={device.path} value={device.path}>{device.name} ({device.path})</option>)}
        </select>
        <span className="hint desc">Only this keyboard is monitored on Wayland. Grant read-only access to its device if needed.</span>
      </label>}
      {(error || monitorError) && <div className="error" role="alert">{error || monitorError}</div>}
      {monitorError && <button disabled={busy} onClick={() => void setEnabled(true)}>Retry Ctrl+Shift monitoring</button>}
    </div>
  );
}

function Status({ info, onChange, guidedStep }: { info: AppInfo | null; onChange: () => void; guidedStep?: OnboardingStep }) {
  const [error, setError] = useState<string | null>(null);
  if (!info) return <p>Loading…</p>;
  const guided = guidedStep !== undefined;
  return (
    <section className={`general${guidedStep === "input" ? " tour-target" : ""}`} hidden={guidedStep === "practice"} aria-label="General settings">
      <h2>General</h2>
      <p className="hint">Hold a hotkey anywhere, say what you want, and Promptify writes it into the app you're using. Everything runs on this computer, and Promptify keeps running in the system tray when you close this window.</p>
      {(guided ? (info.prompt_hotkey_error ? [info.prompt_hotkey_error] : []) : info.hotkey_errors).map((e) => (
        <p key={e} className="error">{e}</p>
      ))}
      <h3>Hotkeys</h3>
      <dl>
        <dt>Prompt</dt>
        <dd>
          <HotkeyField mode="prompt" value={info.hotkeys.prompt} onSaved={onChange} />
          <div className="hint desc">Hold to talk, or tap to start and tap again to finish.</div>
          {!guided && <ModifierHold info={info} onChange={onChange} />}
        </dd>
        {!guided && <>
        <dt>Dictation</dt>
        <dd>
          <HotkeyField mode="dictation" value={info.hotkeys.dictation} onSaved={onChange} />
          <div className="hint desc">Types what you said.</div>
        </dd>
        <dt>Answer</dt>
        <dd>
          <HotkeyField mode="answer" value={info.hotkeys.answer} onSaved={onChange} />
          <div className="hint desc">Ask a question; the answer is shown, never pasted.</div>
        </dd>
        <dt>Cancel</dt>
        <dd>
          <kbd>{info.hotkeys.cancel}</kbd>
          <div className="hint desc">While listening or working.</div>
        </dd>
        </>}
      </dl>
      {!guided && <>
      <h3>Behaviour</h3>
      <label className="inline">
        <input type="checkbox" checked={info.auto_mode} onChange={(e) => void api.setAutoMode(e.target.checked).then(onChange)} />
        Outside AI apps, the prompt hotkey types what you said instead of writing a prompt
      </label>
      <p className="hint indent">Start with “prompt:” or “dictate:” to choose yourself.</p>
      </>}
      <h3>This computer</h3>
      {!guided && <>
        <label className="inline">
          <input type="checkbox" checked={info.code_chat_paste}
            onChange={(event) => {
              setError(null);
              void api.setCodeChatPaste(event.target.checked).then(onChange, (reason) => setError(String(reason)));
            }} />
          Allow automatic prompt paste in VS Code and Cursor AI chat
        </label>
        <p className="hint">Remembered on this computer. With task-aware adaptation, these mixed-purpose apps otherwise require review. Only use the Prompt hotkey while the AI chat input is focused: Promptify cannot distinguish it from an editor or terminal field. Focus-change checks still apply. Disable this to restore per-request confirmation.</p>
      </>}
      {!guided && <DesktopIntegration info={info} onChange={onChange} />}
      <dl>
        <dt>Microphone</dt>
        <dd>{info.input_device ? `${info.input_device} (system default)` : <span className="warn">No default microphone found</span>}</dd>
        <dt>Models</dt>
        <dd>{info.engines_ready ? "Loaded and ready" : <span className="warn">{info.engine_error || (info.models_installed ? "Loading the selected models..." : "Download a speech model and a prompt model under Models to start.")}</span>}</dd>
        <dt>Acceleration</dt>
        <dd>
          <label className="inline">
            <input type="checkbox" checked={info.use_gpu} onChange={(e) => void api.setUseGpu(e.target.checked).then(() => { setError(null); onChange(); }, (reason) => setError(String(reason)))} />
            Use the GPU when available
          </label>
          <div className="hint desc">{info.gpu_device ? `Prompt model running on ${info.gpu_device}` : "Prompt model running on the CPU"}</div>
        </dd>
      </dl>
      {error && <p className="error" role="alert">{error}</p>}
    </section>
  );
}

function Models({ onChange, guided = false }: { onChange: () => void; guided?: boolean }) {
  const [models, setModels] = useState<ModelStatus[]>([]);
  const [progress, setProgress] = useState<Record<string, DownloadEvent>>({});
  const [error, setError] = useState<string | null>(null);
  const totalRam = models.find((m) => m.compatibility.total_ram_bytes !== null)?.compatibility.total_ram_bytes;

  const refresh = useCallback(() => {
    void api.listModels().then(setModels, (reason) => setError(`Could not read models: ${String(reason)}`));
    onChange();
  }, [onChange]);

  useEffect(() => {
    let disposed = false;
    let stop: (() => void) | undefined;
    const unlisten = listen<DownloadEvent>("model-download", ({ payload }) => {
      if (disposed) return;
      setProgress((p) => ({ ...p, [payload.id]: payload }));
      if (payload.done) {
        if (payload.error) setError(`${payload.id}: ${payload.error}`);
        refresh();
      }
    });
    void unlisten.then((unlisten) => {
      if (disposed) unlisten();
      else { stop = unlisten; refresh(); }
    }, (reason) => setError(`Could not listen for model downloads: ${String(reason)}`));
    return () => { disposed = true; stop?.(); };
  }, [refresh]);

  const act = (fn: () => Promise<unknown>) => {
    setError(null);
    fn().then(refresh, (e) => setError(String(e)));
  };

  const group = (kind: ModelStatus["kind"], title: string) => {
    // On first setup, point at one sensible download per kind instead of making every button shout.
    const noneInstalled = !models.some((m) => m.kind === kind && m.installed);
    const candidates = models.filter((m) => m.kind === kind && m.compatibility.supported !== false);
    const recommendation = candidates.find((m) => m.tier === "balanced") ?? candidates.find((m) => m.tier === "small") ?? candidates[0];
    return (
    <>
      <h3>{title}</h3>
      <table>
        <tbody>
          {models.filter((m) => m.kind === kind).map((m) => {
            const p = progress[m.id];
            const pct = p && !p.done && p.total > 0 ? Math.floor((p.downloaded / p.total) * 100) : null;
            const unsupported = m.compatibility.supported === false;
            const recommended = noneInstalled && m.id === recommendation?.id;
            return (
              <tr key={m.id} className={[unsupported ? "model-unsupported" : "", guided && recommended ? "tour-model" : ""].filter(Boolean).join(" ")}>
                <td>
                  <input type="radio" name={kind} checked={m.selected} disabled={!m.installed} onChange={() => act(() => api.selectModel(m.id))} aria-label={`Use ${m.display_name}`} />
                </td>
                <td>
                  <strong>{m.display_name}</strong> {recommended && <span className="badge">Recommended</span>}{" "}
                  <span className="hint">{m.tier} · {m.license} · {m.min_ram_gb} GB+ RAM</span>
                  {m.compatibility.reason && <div className="hint">{m.compatibility.reason}</div>}
                </td>
                <td className="actions">
                  {m.installed && !m.downloading && (
                    <button
                      onClick={() => {
                        const note = m.selected ? " It is the model Promptify is using now." : "";
                        if (window.confirm(`Delete ${m.display_name} (${gb(m.size_bytes)})?${note} You can download it again later.`)) act(() => api.deleteModel(m.id));
                      }}
                    >
                      Delete
                    </button>
                  )}
                  {!m.installed && !m.downloading && (
                    <button className={recommended ? "primary" : ""} onClick={() => act(() => api.downloadModel(m.id))}>
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
  };

  return (
    <section className={guided ? "tour-target" : undefined} aria-label="Model settings">
      <h2>Models</h2>
      <p className="hint">Downloaded from Hugging Face over HTTPS, checked against pinned SHA-256 hashes, and run entirely on this device.</p>
      <p className="hint">
        {totalRam != null && `Detected system RAM: ${gb(totalRam)}. `}
        Compatibility compares total system RAM with each model's minimum. GPU acceleration is optional; VRAM and CPU core count are not requirements.
        {" "}Grayed-out models are below the RAM requirement. Downloads and existing selections remain available; this check does not guarantee successful loading.
      </p>
      {group("stt", "Speech to text")}
      {group("llm", "Prompt writer")}
      {error && <div role="alert"><p className="error">{error}</p><button onClick={refresh}>Refresh models</button></div>}
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
      <button disabled={entries.length === 0} onClick={() => window.confirm(`Delete all ${entries.length} history entries? This cannot be undone.`) && act(api.clearHistory)}>Clear all</button>
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

function Playground({ profiles, routingRevision }: { profiles: ProfileSummary[]; routingRevision: number }) {
  const [transcript, setTranscript] = useState("um so help me plan customer interviews for the new onboarding flow, like goals and questions");
  const [processName, setProcessName] = useState("chrome.exe");
  const [windowTitle, setWindowTitle] = useState("");
  const [url, setUrl] = useState("https://claude.ai/new");
  const [surrounding, setSurrounding] = useState("");
  const [preview, setPreview] = useState<PreviewOutput | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [catalog, setCatalog] = useState<PromptCatalog | null>(null);
  const [routing, setRouting] = useState<RoutingOptions>(automaticRouting("legacy"));

  useEffect(() => {
    let disposed = false;
    void Promise.all([api.promptCatalog(), api.routingState()]).then(([types, settings]) => {
      if (!disposed) { setCatalog(types); setRouting(automaticRouting(settings.rendering)); }
    }, (reason) => { if (!disposed) setError(`Could not load routing controls: ${String(reason)}`); });
    return () => { disposed = true; };
  }, [routingRevision]);

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
          routing,
        }),
      );
    } catch (e) {
      setPreview(null);
      setError(String(e));
    }
  };

  return (
    <section>
      <h2>Advanced</h2>
      <h3>Context playground</h3>
      <p className="hint">Check which of the {profiles.length} target profiles a window resolves to, and the exact messages the local model will receive.</p>
      <div className="grid">
        <label>Process<input value={processName} onChange={(e) => setProcessName(e.target.value)} /></label>
        <label>Window title<input value={windowTitle} onChange={(e) => setWindowTitle(e.target.value)} /></label>
        <label>URL<input value={url} onChange={(e) => setUrl(e.target.value)} /></label>
      </div>
      <label>Spoken text<textarea rows={3} value={transcript} onChange={(e) => setTranscript(e.target.value)} /></label>
      <label>Surrounding text (optional)<textarea rows={2} value={surrounding} onChange={(e) => setSurrounding(e.target.value)} /></label>
      {catalog && <RoutingControls catalog={catalog} value={routing} onChange={setRouting} />}
      <button onClick={() => void run()}>Preview</button>
      {error && <p className="error">{error}</p>}
      {preview && (
        <div className="preview">
          <p>Profile: <strong>{preview.profileName}</strong> ({preview.profileId})</p>
          {preview.routing && <RoutingSummary routing={preview.routing} />}
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

function Words({ info, onChange }: { info: AppInfo | null; onChange: () => void }) {
  const [words, setWords] = useState("");
  const [fixes, setFixes] = useState("");
  const [apps, setApps] = useState("");
  const [status, setStatus] = useState<string | null>(null);

  useEffect(() => {
    if (!info) return;
    setWords(info.vocabulary.words.join("\n"));
    setFixes(info.vocabulary.replacements.map((r) => `${r.from} => ${r.to}`).join("\n"));
    setApps(info.screen_text_apps.join("\n"));
  }, [info]);

  if (!info) return null;
  const lines = (text: string) => text.split("\n").map((l) => l.trim()).filter(Boolean);
  const save = () => {
    const replacements = lines(fixes).flatMap((l) => {
      const [from, ...rest] = l.split("=>");
      return rest.length ? [{ from: from.trim(), to: rest.join("=>").trim() }] : [];
    });
    Promise.all([api.setVocabulary({ words: lines(words), replacements }), api.setScreenTextApps(lines(apps))]).then(
      () => {
        setStatus("Saved.");
        onChange();
      },
      (err) => setStatus(String(err)),
    );
  };

  return (
    <section>
      <h2>Your words</h2>
      <p className="hint">Help speech recognition with names and terms, fix words it keeps getting wrong, and choose which apps may share their on-screen text.</p>
      <label>
        <span>Names and terms to expect, one per line</span>
        <textarea rows={4} value={words} onChange={(e) => setWords(e.target.value)} placeholder={"Promptify\nKubernetes\nPriya"} />
      </label>
      <label>
        <span>Corrections, one per line as <code>heard =&gt; meant</code></span>
        <textarea rows={3} value={fixes} onChange={(e) => setFixes(e.target.value)} placeholder="prompt if I => Promptify" />
      </label>
      <p className="hint">In dictation, say “new line”, “new paragraph” or “scratch that” as their own sentence.</p>
      <label>
        <span>Apps whose on-screen text may be used as context, one per line (for example <code>chatgpt.com</code> or <code>outlook</code>)</span>
        <textarea rows={3} value={apps} onChange={(e) => setApps(e.target.value)} />
      </label>
      <p className="hint">Only the focused text box is read, only when you press a hotkey in one of these apps, and never password fields. It is not saved in history.</p>
      <button className="primary" onClick={save}>Save</button> {status && <span className="hint">{status}</span>}
    </section>
  );
}

/** Character offset of serde_json's "at line L column C" in `text`, for jumping to the error. */
function errorOffset(text: string, message: string): number | null {
  const match = /line (\d+) column (\d+)/.exec(message);
  if (!match) return null;
  const lines = text.split("\n");
  const line = Math.min(Number(match[1]), lines.length) - 1;
  const before = lines.slice(0, line).reduce((n, l) => n + l.length + 1, 0);
  return before + Math.max(0, Math.min(Number(match[2]) - 1, lines[line]?.length ?? 0));
}

function McpEditor({ onSaved }: { onSaved: (info: McpInfo) => void }) {
  const [text, setText] = useState("");
  const [original, setOriginal] = useState<string | null>(null);
  const [template, setTemplate] = useState("");
  const [check, setCheck] = useState<{ ok: true; servers: number } | { ok: false; error: string } | null>(null);
  const [status, setStatus] = useState<{ tone: "hint" | "error"; text: string } | null>(null);
  const area = useRef<HTMLTextAreaElement>(null);

  const load = useCallback(() => {
    api.mcpRead().then(
      (file) => {
        setText(file.text);
        setOriginal(file.text);
        setTemplate(file.template);
        setStatus(file.exists ? null : { tone: "hint", text: "mcp.json does not exist yet. Start from the example or paste your own." });
      },
      (e) => setStatus({ tone: "error", text: String(e) }),
    );
  }, []);

  // Picks up edits made outside the app: shows the file and applies it.
  const reloadFromDisk = () => {
    load();
    void api.mcpReload().then(onSaved);
  };

  useEffect(load, [load]);

  // Validated by the same code that loads the file, a moment after typing stops.
  useEffect(() => {
    if (original === null) return;
    const timer = window.setTimeout(() => {
      api.mcpValidate(text).then((servers) => setCheck({ ok: true, servers }), (e) => setCheck({ ok: false, error: String(e) }));
    }, 300);
    return () => window.clearTimeout(timer);
  }, [text, original]);

  if (original === null) return status ? <p className={status.tone}>{status.text}</p> : null;
  const dirty = text !== original;
  const canSave = dirty && check?.ok === true;

  const save = () => {
    if (!canSave) return;
    api.mcpSave(text, original).then(
      (info) => {
        setOriginal(text);
        setStatus({ tone: "hint", text: info.error ? `Saved, but tools are off: ${info.error}` : info.active ? "Saved. Tools are on." : "Saved. No tools are active." });
        onSaved(info);
      },
      (e) => setStatus({ tone: "error", text: String(e) }),
    );
  };

  const format = () => {
    try {
      setText(JSON.stringify(JSON.parse(text), null, 2) + "\n");
    } catch {
      setStatus({ tone: "error", text: "Fix the JSON error before formatting." });
    }
  };

  const goToError = () => {
    if (check?.ok !== false || !area.current) return;
    const at = errorOffset(text, check.error);
    if (at === null) return;
    area.current.focus();
    area.current.setSelectionRange(at, at + 1);
  };

  const onKeyDown = (e: React.KeyboardEvent<HTMLTextAreaElement>) => {
    if ((e.ctrlKey || e.metaKey) && e.key.toLowerCase() === "s") {
      e.preventDefault();
      save();
    } else if (e.key === "Tab" && !e.shiftKey) {
      e.preventDefault();
      const el = e.currentTarget;
      const { selectionStart: start, selectionEnd: end } = el;
      setText(text.slice(0, start) + "  " + text.slice(end));
      requestAnimationFrame(() => el.setSelectionRange(start + 2, start + 2));
    }
  };

  return (
    <div className="editor">
      <textarea
        ref={area}
        aria-label="mcp.json"
        spellCheck={false}
        wrap="off"
        rows={16}
        value={text}
        placeholder={template}
        onChange={(e) => setText(e.target.value)}
        onKeyDown={onKeyDown}
      />
      <div className="editor-bar">
        <button className="primary" onClick={save} disabled={!canSave} title="Ctrl+S">Save</button>
        <button onClick={format} disabled={!text.trim()}>Format</button>
        <button onClick={() => setText(original)} disabled={!dirty}>Revert</button>
        {!text.trim() && <button onClick={() => setText(template)}>Start from example</button>}
        <button className="link" onClick={() => (!dirty || window.confirm("Discard your unsaved changes and reload mcp.json from disk?")) && reloadFromDisk()}>
          Reload from disk
        </button>
        <span className={check?.ok === false ? "error" : "hint"}>
          {check === null ? "" : check.ok ? `Valid · ${check.servers} server${check.servers === 1 ? "" : "s"}` : check.error}
          {check?.ok === false && errorOffset(text, check.error) !== null && (
            <button className="link" onClick={goToError}>Go to error</button>
          )}
        </span>
        {dirty && <span className="hint">· Unsaved changes</span>}
      </div>
      {status && <p className={status.tone}>{status.text}</p>}
    </div>
  );
}

function Tools() {
  const [info, setInfo] = useState<McpInfo | null>(null);
  const [tests, setTests] = useState<Record<string, string>>({});

  useEffect(() => {
    void api.mcpInfo().then(setInfo);
  }, []);

  const test = (name: string) => {
    setTests((t) => ({ ...t, [name]: "Starting… the first start of an npx or uvx server can take up to a minute." }));
    api.mcpTest(name).then(
      (tools) => setTests((t) => ({ ...t, [name]: tools.length ? `Connected. Tools: ${tools.join(", ")}` : "Connected, but it offers no tools." })),
      (err) => setTests((t) => ({ ...t, [name]: `Failed: ${String(err)}` })),
    );
  };

  if (!info) return null;
  return (
    <section>
      <h2>Tools (MCP)</h2>
      <p className="hint">
        Connected tools can add reference facts before a prompt is written. Edit <code>{info.path}</code> below; saving
        applies it right away. Only hotkey prompts use tools; dictation, phones and the local API never do.
      </p>
      {info.error && <p className="error">{info.error}</p>}
      {info.servers.length === 0 && !info.error && <p>No tools configured.</p>}
      <ul className="rows">
        {info.servers.map((s) => (
          <li key={s.name}>
            <div>
              <strong>{s.name}</strong> {s.enabled ? "" : "(off) "}
              <span className="hint">
                {s.remote ? "remote server" : "runs on this computer"} · {s.hooks} lookups
                {s.loop_tools.length > 0 && ` · the model may call ${s.loop_tools.join(", ")}`}
                {s.profiles.length > 0 && ` · only for ${s.profiles.join(", ")}`}
              </span>
            </div>
            {s.remote && s.transcript_allowed && (
              <p className="warn">What you say is sent to this remote server. Set allow_transcript to false to keep it on this computer.</p>
            )}
            <div className="row-actions">
              <button onClick={() => test(s.name)}>Test</button> {tests[s.name] && <span className="hint">{tests[s.name]}</span>}
            </div>
          </li>
        ))}
      </ul>
      {info.servers.some((s) => !s.remote) && <p className="hint">Servers that run on this computer run with your user rights. Only add servers you trust.</p>}
      <p className="hint">{info.active ? "Tools are on." : "No tools are active."}</p>
      <h3>mcp.json</h3>
      <McpEditor onSaved={setInfo} />
    </section>
  );
}

function Remote() {
  const [info, setInfo] = useState<RemoteInfo | null>(null);
  const [relayUrl, setRelayUrl] = useState("");
  const [offer, setOffer] = useState<OfferInfo | null>(null);
  const [renaming, setRenaming] = useState<{ id: string; name: string } | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [now, setNow] = useState(() => Date.now() / 1000);

  const refresh = useCallback(() => {
    void api.remoteInfo().then((next) => {
      setInfo(next);
      if (next.status?.pairing_expires_unix == null) setOffer(null);
    }, (e) => setError(String(e)));
  }, []);

  useEffect(() => {
    void api.remoteInfo().then((next) => {
      setInfo(next);
      setRelayUrl(next.relay_url ?? "");
    }, (e) => setError(String(e)));
  }, []);

  useEffect(() => {
    if (!info?.mobile_available) return;
    const timer = window.setInterval(() => {
      setNow(Date.now() / 1000);
      refresh();
    }, 2000);
    return () => window.clearInterval(timer);
  }, [info?.mobile_available, refresh]);

  const act = (fn: () => Promise<unknown>) => {
    setError(null);
    fn().then(refresh, (e) => {
      setError(String(e));
      refresh();
    });
  };

  if (!info) return error ? <p className="error">{error}</p> : null;
  const save = (enabled: boolean, lan: boolean, discovery = info.lan_discovery) => act(() => api.setRemoteSettings(enabled, relayUrl.trim() || null, lan, discovery));
  const secondsLeft = offer ? Math.max(0, Math.round(offer.expires_unix - now)) : 0;

  return (
    <section>
      <h2>{info.mobile_available ? "Desktop API and phone access" : "Desktop API (MCP)"}</h2>
      <p className="hint">
        {info.mobile_available
          ? "Desktop MCP tools and paired phones can use this computer's local models. Phone traffic is end-to-end encrypted; phones never get your history or screen text."
          : "Desktop MCP tools can use this computer's models through a token-protected loopback API. Phone networking is paused until the iOS and Android apps are ready."}
      </p>
      <label className="inline">
        <input type="checkbox" checked={info.enabled} onChange={(e) => save(e.target.checked, info.lan_direct)} />
        {info.mobile_available ? "Allow desktop tools and paired devices to use Promptify" : "Allow desktop MCP tools to use Promptify"}
      </label>
      {info.mobile_available && (
        <>
          <label className="inline">
            <input type="checkbox" checked={info.lan_direct} onChange={(e) => save(info.enabled, e.target.checked)} />
            Accept direct connections on this network
          </label>
          {info.lan_direct && (
            <label className="inline">
              <input type="checkbox" checked={info.lan_discovery} onChange={(e) => save(info.enabled, info.lan_direct, e.target.checked)} />
              Let paired phones find this computer if its network address changes (mDNS)
            </label>
          )}
          <label>
            Relay for access over the internet (optional, self-hosted)
            <input placeholder="wss://relay.example.net" value={relayUrl} onChange={(e) => setRelayUrl(e.target.value)} />
          </label>
          <button onClick={() => save(info.enabled, info.lan_direct)}>Save relay</button>
          {info.enabled && info.status && (
            <p className="hint">
              {relayLabel(info)} · {info.status.sessions} connected now
            </p>
          )}
        </>
      )}
      {info.mobile_available && info.enabled && (
        <div>
          {offer && secondsLeft > 0 ? (
            <div className="pairing">
              <img alt="Pairing code" width={240} height={240} src={`data:image/svg+xml;utf8,${encodeURIComponent(offer.svg)}`} />
              <p className="hint">Scan this code from a Promptify phone client within {secondsLeft}s. It works once. The phone apps are not released yet; to try pairing now, give the link below to <code>promptify-cli remote pair</code>.</p>
              <details>
                <summary>Pairing link</summary>
                <pre>{offer.uri}</pre>
              </details>
              <button onClick={() => act(api.cancelPairing)}>Cancel pairing</button>
            </div>
          ) : (
            <button className="primary" onClick={() => act(() => api.createPairingOffer().then(setOffer))}>Pair a phone</button>
          )}
        </div>
      )}
      {error && <p className="error">{error}</p>}
      {info.error && <p className="error">{info.error}</p>}
      {info.mobile_available && <ul className="rows">
        {info.devices.map((d) => (
          <li key={d.id}>
            {renaming?.id === d.id ? (
              <input
                autoFocus
                maxLength={64}
                value={renaming.name}
                onChange={(e) => setRenaming({ id: d.id, name: e.target.value })}
                onKeyDown={(e) => {
                  if (e.key === "Escape") setRenaming(null);
                  if (e.key === "Enter" && renaming.name.trim()) {
                    const name = renaming.name.trim();
                    setRenaming(null);
                    act(() => api.renameDevice(d.id, name));
                  }
                }}
                onBlur={() => setRenaming(null)}
              />
            ) : (
              <strong>{d.name}</strong>
            )}
            <span className="hint">
              {" "}· paired {new Date(d.paired_unix * 1000).toLocaleDateString()}
              {d.last_seen_unix ? ` · last seen ${new Date(d.last_seen_unix * 1000).toLocaleString()}` : ""}
            </span>
            <button className="link" onClick={() => setRenaming({ id: d.id, name: d.name })}>Rename</button>
            <button className="link" onClick={() => window.confirm(`Remove ${d.name}? It is disconnected now and must be paired again to use Promptify.`) && act(() => api.removeDevice(d.id))}>Remove</button>
          </li>
        ))}
      </ul>}
      {info.enabled && info.status && (
        <details open={!info.mobile_available}>
          <summary>Local API for tools on this computer</summary>
          <p className="hint">
            Listening on {info.status.listen}. Tools must send the token stored in <code>{info.api_token_path}</code>.
          </p>
        </details>
      )}
    </section>
  );
}

type Pane = "general" | "models" | "prompts" | "words" | "history" | "phones" | "tools" | "advanced";

const PANES: { id: Pane; label: string }[] = [
  { id: "general", label: "General" },
  { id: "models", label: "Models" },
  { id: "prompts", label: "Prompt types" },
  { id: "words", label: "Your words" },
  { id: "history", label: "History" },
  { id: "phones", label: "Desktop API" },
  { id: "tools", label: "Tools (MCP)" },
  { id: "advanced", label: "Advanced" },
];

const PANE_KEY = "promptify.settings.pane";

function Settings() {
  const [info, setInfo] = useState<AppInfo | null>(null);
  const [onboarding, setOnboarding] = useState<OnboardingStatus | null>(null);
  const [loadError, setLoadError] = useState<string | null>(null);
  const [connectionAttempt, setConnectionAttempt] = useState(0);
  const [profiles, setProfiles] = useState<ProfileSummary[]>([]);
  const [routingRevision, setRoutingRevision] = useState(0);
  const [pane, setPane] = useState<Pane>(() => (localStorage.getItem(PANE_KEY) as Pane | null) ?? "general");
  const refreshId = useRef(0);
  const refreshInfo = useCallback(() => {
    const id = ++refreshId.current;
    void Promise.all([api.appInfo(), api.onboardingStatus()]).then(([nextInfo, nextOnboarding]) => {
      if (id !== refreshId.current) return;
      setInfo(nextInfo);
      setOnboarding(nextOnboarding);
      setLoadError(null);
    }, (reason) => {
      if (id === refreshId.current) setLoadError(`Could not load setup state: ${String(reason)}`);
    });
  }, []);
  const firstInfo = useRef(true);

  useEffect(() => {
    let disposed = false;
    let stop: (() => void) | undefined;
    void listen("onboarding-changed", refreshInfo).then((unlisten) => {
      if (disposed) { unlisten(); return; }
      stop = unlisten;
      refreshInfo();
      void api.listProfiles().then(setProfiles, (reason) => setLoadError(`Could not load profiles: ${String(reason)}`));
    }, (reason) => setLoadError(`Could not connect to setup events: ${String(reason)}`));
    window.addEventListener("focus", refreshInfo);
    return () => {
      disposed = true;
      stop?.();
      window.removeEventListener("focus", refreshInfo);
    };
  }, [refreshInfo, connectionAttempt]);

  // Until models are installed nothing works, so the first view goes straight there.
  useEffect(() => {
    if (info && firstInfo.current) {
      firstInfo.current = false;
      if (!info.models_installed) setPane("models");
    }
  }, [info]);

  const open = (next: Pane) => {
    setPane(next);
    localStorage.setItem(PANE_KEY, next);
    window.scrollTo(0, 0);
  };

  const badge = (id: Pane) => {
    if (id === "models" && info && !info.models_installed) return <span className="badge">Set up</span>;
    if (id === "general" && info && info.hotkey_errors.length > 0) return <span className="badge">!</span>;
    return null;
  };

  if (loadError || !info || !onboarding) {
    return <main className="startup">
      <h1>Promptify setup</h1>
      {loadError ? <><p role="alert" className="error">{loadError}</p><button onClick={() => setConnectionAttempt((n) => n + 1)}>Retry connection</button></> : <p role="status">Loading your setup...</p>}
    </main>;
  }

  const guided = onboarding.required;
  const visiblePane = guided ? (onboarding.step === "models" ? "models" : "general") : pane;
  const visiblePanes = guided ? PANES.filter((p) => p.id === "general" || p.id === "models") : PANES;

  // Panes stay mounted so unsaved edits (for example in mcp.json) survive switching.
  const panes: Record<Pane, React.ReactNode> = {
    general: <Status info={info} onChange={refreshInfo} guidedStep={guided ? onboarding.step : undefined} />,
    models: <Models onChange={refreshInfo} guided={guided} />,
    words: <Words info={info} onChange={refreshInfo} />,
    history: <History info={info} onChange={refreshInfo} />,
    phones: <Remote />,
    tools: <Tools />,
    prompts: <PromptRoutingSettings onSaved={() => setRoutingRevision((value) => value + 1)} />,
    advanced: <Playground profiles={profiles} routingRevision={routingRevision} />,
  };

  return (
    <div className="app">
      <nav className="sidebar" aria-label="Settings sections">
        <div className="brand">Promptify</div>
        {PANES.map((p) => (
          <button key={p.id} className={visiblePane === p.id ? "active" : ""} aria-current={visiblePane === p.id ? "page" : undefined} disabled={guided && visiblePane !== p.id} onClick={() => { if (!guided) open(p.id); }}>
            {p.label}
            {badge(p.id)}
          </button>
        ))}
        <p className="sidebar-foot hint">Keeps running in the system tray.</p>
      </nav>
      <main>
        {guided && <OnboardingTour status={onboarding} info={info} onChange={refreshInfo} />}
        {!onboarding.startup_error && visiblePanes.map((p) => (
          <div key={p.id} hidden={visiblePane !== p.id}>
            {panes[p.id]}
          </div>
        ))}
      </main>
    </div>
  );
}

createRoot(document.getElementById("root")!).render(<Settings />);
