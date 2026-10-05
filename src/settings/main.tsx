import { useCallback, useEffect, useRef, useState } from "react";
import { createRoot } from "react-dom/client";
import { listen } from "@tauri-apps/api/event";
import { api, type AppInfo, type DictationTone, type DownloadEvent, type HistoryEntry, type ModelStatus, type OnboardingStatus, type OnboardingStep } from "../api";
import { OnboardingTour } from "./OnboardingTour";
import { DesktopIntegration } from "./DesktopIntegration";
import { UpdateIndicator, UpdatesSettings, useUpdates } from "./Updates";
import { PromptRoutingSettings } from "./PromptRouting";
import "./settings.css";
import { RecoveryButton } from "./RecoveryButton";

const gb = (bytes: number) => `${(bytes / 1e9).toFixed(bytes < 1e9 ? 2 : 1)} GB`;
const DICTATION_TONES: { value: DictationTone; label: string; description: string }[] = [
  { value: "clean_transcript", label: "Clean transcript", description: "Remove fillers and apply spoken editing commands without language-model rewriting." },
  { value: "natural", label: "Natural", description: "Improve grammar and flow while keeping your voice." },
  { value: "casual", label: "Casual", description: "Use relaxed, conversational wording." },
  { value: "formal", label: "Formal", description: "Polish professionally without adding greetings, recipients, or signatures." },
  { value: "concise", label: "Concise", description: "Trim repetition without dropping facts or requested actions." },
  { value: "unhinged", label: "Unhinged", description: "Add playful, irreverent emphasis without new claims or profanity." },
];

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

function HotkeyField({ mode, value, onSaved }: { mode: "prompt" | "dictation"; value: string | null; onSaved: () => void }) {
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
  const monitorError = enabled ? info.modifier_hold_error : null;
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
      <div className="hint desc indent-check">This optional gesture requires supported system input monitoring. If it is unavailable, use the configured Prompt shortcut through desktop integration. Promptify never requires selecting a physical keyboard or granting raw keyboard-device access.</div>
      {(error || monitorError) && <div className="error" role="alert">{error || monitorError}</div>}
      {monitorError && <div className="inline">
        <button disabled={busy} onClick={() => void setEnabled(true)}>Retry Ctrl+Shift monitoring</button>
        {enabled && <button disabled={busy} onClick={() => void setEnabled(false)}>Disable Ctrl+Shift hold</button>}
        {navigator.userAgent.includes("Mac") && <RecoveryButton action={() => api.openSystemSettings("input_monitoring")} onComplete={onChange}>Open Input Monitoring settings</RecoveryButton>}
      </div>}
    </div>
  );
}

function Status({ info, onChange, guidedStep }: { info: AppInfo | null; onChange: () => void; guidedStep?: OnboardingStep }) {
  const [error, setError] = useState<string | null>(null);
  if (!info) return <p>Loading…</p>;
  const guided = guidedStep !== undefined;
  const integrationNeedsPermission = info.paste_permission === "required";
  return (
    <section className={`general${guidedStep === "input" ? " tour-target" : ""}`} hidden={guidedStep === "practice"} aria-label="General settings">
      <h2>General</h2>
      <p className="hint">Hold a hotkey anywhere, say what you want, and Promptify writes it into the app you're using. Everything runs on this computer, and Promptify keeps running in the system tray when you close this window.</p>
      {[...new Set(guided ? (info.prompt_hotkey_error ? [info.prompt_hotkey_error] : []) : info.hotkey_errors)].map((e) => (
        <p key={e} className="error">{e}</p>
      ))}
      {!guided && integrationNeedsPermission && <DesktopIntegration info={info} onChange={onChange} />}
      <h3>Hotkeys</h3>
      {!guided && info.hotkeys_paused && <div role="alert">
        <p className="warn">Shortcuts are paused.</p>
        <RecoveryButton action={api.resumeOnboardingHotkeys} onComplete={onChange}>Resume shortcuts</RecoveryButton>
      </div>}
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
          <div className="hint desc">Rewrites your words in the selected tone after recording; it does not answer questions or execute dictated instructions.</div>
        </dd>
        <dt>Dictation tone</dt>
        <dd>
          <label htmlFor="dictation-tone">Choose how dictated text is polished</label>
          <select
            id="dictation-tone"
            aria-label="Dictation tone"
            value={info.dictation_tone}
            onChange={(event) => {
              const tone = DICTATION_TONES.find((option) => option.value === event.target.value)?.value;
              if (!tone) {
                setError("The selected dictation tone is not supported.");
                return;
              }
              setError(null);
              void api.setDictationTone(tone).then(onChange, (reason) => setError(`Could not save dictation tone: ${String(reason)}`));
            }}
          >
            {DICTATION_TONES.map((tone) => <option key={tone.value} value={tone.value}>{tone.label}{tone.value === "natural" ? " (default)" : ""}</option>)}
          </select>
          <div className="hint desc">{DICTATION_TONES.find((tone) => tone.value === info.dictation_tone)?.description}</div>
          <div className="hint desc">Rewrite tones use your selected local language model and add processing time. Clean transcript stays deterministic; choose it when the model is unavailable or you prefer no rewrite.</div>
        </dd>
        <dt>Cancel</dt>
        <dd>
          <kbd>{info.hotkeys.cancel}</kbd>
          <div className="hint desc">While listening or working.</div>
        </dd>
        </>}
      </dl>
      {!!info.activation_bindings?.length && <div aria-label="Active desktop shortcuts">
        <p className="hint">Your system assigned these active shortcuts:</p>
        <ul>{info.activation_bindings.map((binding) => <li key={binding.id}>{binding.id}: <kbd>{binding.trigger_description}</kbd></li>)}</ul>
        <p className="hint">The system may reserve these shortcuts while desktop integration is enabled.</p>
      </div>}
      {!guided && <>
      <h3>Behaviour</h3>
      <label className="inline">
        <input type="checkbox" checked={info.auto_mode} onChange={(e) => void api.setAutoMode(e.target.checked).then(onChange)} />
        Outside AI apps, the prompt hotkey types what you said instead of writing a prompt
      </label>
      <p className="hint indent">Start with “prompt:” or “dictate:” to choose yourself.</p>
      </>}
      <h3>This computer</h3>
      {!guided && <p className="hint">Automatic input inspection is independent of the app or site. With task-aware adaptation, a confirmed writable input can receive your prompt without an app-specific setting. Keep that input focused. Unconfirmed inputs require review; protected inputs are never pasted into.</p>}
      {!guided && !integrationNeedsPermission && <DesktopIntegration info={info} onChange={onChange} />}
      <dl>
        <dt>Microphone</dt>
        <dd>
          {info.input_device ? `${info.input_device} (system default)` : <span className="warn">No default microphone found</span>}
          {!guided && <div className="actions">
            <RecoveryButton action={() => api.openSystemSettings("microphone")} onComplete={onChange}>Open microphone settings</RecoveryButton>
            <button onClick={onChange}>Refresh microphone and shortcut</button>
          </div>}
        </dd>
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
      {!guided && !info.engines_ready && <div className="actions">
        <RecoveryButton action={() => api.openRecoverySettings("models")}>Choose models</RecoveryButton>
        {info.models_installed && info.engine_error && <>
          <RecoveryButton action={api.retryModelLoading} onComplete={onChange}>Retry model loading</RecoveryButton>
          {info.use_gpu && <RecoveryButton action={() => api.setUseGpu(false)} onComplete={onChange}>Try loading on CPU</RecoveryButton>}
        </>}
      </div>}
      {error && <p className="error" role="alert">{error}</p>}
    </section>
  );
}

function Models({ onChange, guided = false }: { onChange: () => void; guided?: boolean }) {
  const [models, setModels] = useState<ModelStatus[]>([]);
  const [progress, setProgress] = useState<Record<string, DownloadEvent>>({});
  const [error, setError] = useState<string | null>(null);
  const totalRam = models.find((m) => m.compatibility.total_ram_bytes !== null)?.compatibility.total_ram_bytes;
  const [listenerAttempt, setListenerAttempt] = useState(0);
  const [listenerError, setListenerError] = useState<string | null>(null);

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
      else { stop = unlisten; setListenerError(null); refresh(); }
    }, (reason) => { if (!disposed) setListenerError(`Could not listen for model downloads: ${String(reason)}`); });
    return () => { disposed = true; stop?.(); };
  }, [refresh, listenerAttempt]);

  const act = (fn: () => Promise<unknown>) => {
    setError(null);
    fn().then(refresh, (e) => setError(String(e)));
  };

  const group = (kind: ModelStatus["kind"], title: string) => {
    const candidates = models.filter((m) => m.kind === kind && m.compatibility.supported === true);
    const recommendation = candidates.find((m) => m.tier === "quality") ?? candidates.find((m) => m.tier === "balanced") ?? candidates.find((m) => m.tier === "small");
    return (
    <div className="settings-group">
      <h3>{title}</h3>
      {!recommendation && models.some((m) => m.kind === kind) && <p className="hint">No confirmed compatible recommendation. Check the RAM requirements before choosing a model.</p>}
      <table aria-label={title}>
        <tbody>
          {models.filter((m) => m.kind === kind).map((m) => {
            const p = progress[m.id];
            const pct = p && !p.done && p.total > 0 ? Math.floor((p.downloaded / p.total) * 100) : null;
            const unsupported = m.compatibility.supported === false;
            const recommended = m.id === recommendation?.id;
            return (
              <tr key={m.id} className={[unsupported ? "model-unsupported" : "", guided && recommended ? "tour-model" : ""].filter(Boolean).join(" ")}>
                <td>
                  <input type="radio" name={kind} checked={m.selected} disabled={!m.installed} onChange={() => act(() => api.selectModel(m.id))} aria-label={`Use ${m.display_name}`} />
                </td>
                <td>
                  <strong>{m.display_name}</strong> {recommended && <span className="badge">Recommended</span>}{" "}
                  <span className="hint">{m.tier} · {m.license} · {m.min_ram_gb} GB+ RAM</span>
                  {recommended && <div className="hint">Highest quality tier supported by your RAM. {m.selected ? "Currently selected." : "Download or select it when you choose; your current model is unchanged."}</div>}
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
    </div>
    );
  };

  return (
    <section className={guided ? "tour-target" : undefined} aria-label="Model settings">
      <h2>Models</h2>
      <p className="hint">Downloaded from Hugging Face over HTTPS, checked against pinned SHA-256 hashes, and run entirely on this device.</p>
      <p className="hint">Recommendations favor quality over speed. Larger models can take longer; nothing is downloaded or switched automatically just because it is recommended.</p>
      <details className="model-compatibility">
      <summary>Hardware compatibility{totalRam != null ? ` (${gb(totalRam)} RAM)` : ""}</summary>
      <p className="hint">
        {totalRam != null && `Detected system RAM: ${gb(totalRam)}. `}
        Compatibility compares total system RAM with each model's minimum. GPU acceleration is optional; VRAM and CPU core count are not requirements.
        {" "}Grayed-out models are below the RAM requirement. Downloads and existing selections remain available; this check does not guarantee successful loading.
      </p>
      </details>
      {group("stt", "Speech to text")}
      {group("llm", "Prompt writer")}
      {(listenerError || error) && <div role="alert"><p className="error">{listenerError || error}</p><button onClick={() => {
        setError(null);
        setListenerAttempt((value) => value + 1);
        refresh();
      }}>Refresh models</button></div>}
    </section>
  );
}

function History({ info, onChange }: { info: AppInfo | null; onChange: () => void }) {
  const [entries, setEntries] = useState<HistoryEntry[]>([]);
  const [error, setError] = useState<string | null>(null);
  const [listenerError, setListenerError] = useState<string | null>(null);
  const [listenerAttempt, setListenerAttempt] = useState(0);
  const refresh = useCallback(() => void api.listHistory().then((next) => {
    setEntries(next);
    setError(null);
  }, (reason) => setError(`Could not load history: ${String(reason)}`)), []);

  useEffect(() => {
    refresh();
    let disposed = false;
    let stop: (() => void) | undefined;
    void listen("history-changed", refresh).then((unlisten) => {
      if (disposed) unlisten();
      else {
        stop = unlisten;
        setListenerError(null);
      }
    }, (reason) => { if (!disposed) setListenerError(`Could not listen for history changes: ${String(reason)}`); });
    return () => { disposed = true; stop?.(); };
  }, [refresh, listenerAttempt]);

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
      {(listenerError || error) && <div role="alert"><p className="error">{listenerError || error}</p><button onClick={() => {
        setListenerAttempt((value) => value + 1);
        refresh();
      }}>Refresh history</button></div>}
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

function Words({ info, onChange }: { info: AppInfo | null; onChange: () => void }) {
  const [words, setWords] = useState("");
  const [fixes, setFixes] = useState("");
  const [apps, setApps] = useState("");
  const [status, setStatus] = useState<string | null>(null);
  const [saveError, setSaveError] = useState<string | null>(null);
  const [saving, setSaving] = useState(false);
  const dirty = useRef(false);

  useEffect(() => {
    if (!info || dirty.current) return;
    setWords(info.vocabulary.words.join("\n"));
    setFixes(info.vocabulary.replacements.map((r) => `${r.from} => ${r.to}`).join("\n"));
    setApps(info.screen_text_apps.join("\n"));
  }, [info]);

  if (!info) return null;
  const lines = (text: string) => text.split("\n").map((l) => l.trim()).filter(Boolean);
  const save = () => {
    setSaveError(null);
    setStatus(null);
    const invalid = lines(fixes).find((line) => {
      const [from, ...rest] = line.split("=>");
      return !from.trim() || !rest.length || !rest.join("=>").trim();
    });
    if (invalid) {
      setSaveError(`Invalid correction "${invalid}". Use nonempty "heard => meant" pairs, then save again.`);
      return;
    }
    const replacements = lines(fixes).flatMap((l) => {
      const [from, ...rest] = l.split("=>");
      return rest.length ? [{ from: from.trim(), to: rest.join("=>").trim() }] : [];
    });
    setSaving(true);
    Promise.all([api.setVocabulary({ words: lines(words), replacements }), api.setScreenTextApps(lines(apps))]).then(
      () => {
        dirty.current = false;
        setStatus("Saved.");
        onChange();
      },
      (err) => setSaveError(String(err)),
    ).finally(() => setSaving(false));
  };

  return (
    <section>
      <h2>Your words</h2>
      <p className="hint">Help speech recognition with names and terms, fix words it keeps getting wrong, and choose which apps may share their on-screen text.</p>
      <label>
        <span>Names and terms to expect, one per line</span>
        <textarea rows={4} value={words} onChange={(e) => { dirty.current = true; setWords(e.target.value); }} placeholder={"Promptify\nKubernetes\nPriya"} />
      </label>
      <label>
        <span>Corrections, one per line as <code>heard =&gt; meant</code></span>
        <textarea rows={3} value={fixes} onChange={(e) => { dirty.current = true; setFixes(e.target.value); }} placeholder="prompt if I => Promptify" />
      </label>
      <p className="hint">In dictation, say “new line”, “new paragraph” or “scratch that” as their own sentence.</p>
      <label>
        <span>Apps whose on-screen text may be used as context, one per line (for example <code>chatgpt.com</code> or <code>outlook</code>)</span>
        <textarea rows={3} value={apps} onChange={(e) => { dirty.current = true; setApps(e.target.value); }} />
      </label>
      <p className="hint">Only the focused text box is read, only when you press a hotkey in one of these apps, and never password fields. It is not saved in history.</p>
      <button className="primary" disabled={saving} onClick={save}>{saving ? "Saving..." : saveError ? "Retry save" : "Save"}</button> {status && <span className="hint" role="status">{status}</span>}
      {saveError && <p role="alert" className="error">{saveError}</p>}
    </section>
  );
}

type Pane = "general" | "models" | "prompts" | "words" | "history" | "updates";

const PANES: { id: Pane; label: string }[] = [
  { id: "general", label: "General" },
  { id: "models", label: "Models" },
  { id: "prompts", label: "Prompt types" },
  { id: "words", label: "Your words" },
  { id: "history", label: "History" },
  { id: "updates", label: "Updates" },
];

const PANE_KEY = "promptify.settings.pane";

function Settings() {
  const updates = useUpdates();
  const [info, setInfo] = useState<AppInfo | null>(null);
  const [onboarding, setOnboarding] = useState<OnboardingStatus | null>(null);
  const [loadError, setLoadError] = useState<string | null>(null);
  const [connectionAttempt, setConnectionAttempt] = useState(0);
  const [pane, setPane] = useState<Pane>(() => {
    const saved = localStorage.getItem(PANE_KEY);
    const current = PANES.find((candidate) => candidate.id === saved);
    if (saved && !current) {
      console.warn(`Retired or unknown settings section "${saved}"; opening General.`);
      localStorage.removeItem(PANE_KEY);
    }
    return current?.id ?? "general";
  });
  const [visited, setVisited] = useState<Pane[]>([]);
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
  const guided = onboarding?.required ?? false;
  const visiblePane = guided ? (onboarding?.step === "models" ? "models" : "general") : pane;

  useEffect(() => {
    if (info && onboarding && !onboarding.startup_error) {
      setVisited((current) => current.includes(visiblePane) ? current : [...current, visiblePane]);
    }
  }, [visiblePane, info, onboarding]);

  useEffect(() => {
    let disposed = false;
    let stop: (() => void) | undefined;
    void listen("onboarding-changed", refreshInfo).then((unlisten) => {
      if (disposed) { unlisten(); return; }
      stop = unlisten;
      refreshInfo();
    }, (reason) => setLoadError(`Could not connect to setup events: ${String(reason)}`));
    window.addEventListener("focus", refreshInfo);
    return () => {
      disposed = true;
      stop?.();
      window.removeEventListener("focus", refreshInfo);
    };
  }, [refreshInfo, connectionAttempt]);

  useEffect(() => {
    let disposed = false;
    let stop: (() => void) | undefined;
    void listen<"general" | "models" | "prompts">("open-settings-section", ({ payload }) => {
      if (disposed) return;
      if (payload !== "general" && payload !== "models" && payload !== "prompts") {
        setLoadError("An invalid recovery section was requested. Retry the connection.");
        return;
      }
      setPane(payload);
      localStorage.setItem(PANE_KEY, payload);
      window.scrollTo(0, 0);
    }).then((unlisten) => {
      if (disposed) unlisten();
      else stop = unlisten;
    }, (reason) => { if (!disposed) setLoadError(`Could not connect to recovery controls: ${String(reason)}`); });
    return () => { disposed = true; stop?.(); };
  }, [connectionAttempt]);

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

  const visiblePanes = guided ? PANES.filter((p) => p.id === "general" || p.id === "models") : PANES;

  // Keep visited panes mounted so edits survive switching; unvisited panes do no work.
  const panes: Record<Pane, React.ReactNode> = {
    general: <Status info={info} onChange={refreshInfo} guidedStep={guided ? onboarding.step : undefined} />,
    models: <Models onChange={refreshInfo} guided={guided} />,
    words: <Words info={info} onChange={refreshInfo} />,
    history: <History info={info} onChange={refreshInfo} />,
    prompts: <PromptRoutingSettings />,
    updates: <UpdatesSettings updates={updates} />,
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
        <p className="sidebar-foot hint">Prompt and Dictation.<br />Local models. No account.<br />Keeps running in the system tray.</p>
      </nav>
      <main>
        <UpdateIndicator updates={updates} />
        {guided && <OnboardingTour status={onboarding} info={info} onChange={refreshInfo} />}
        {!onboarding.startup_error && visiblePanes.filter((p) => p.id === visiblePane || visited.includes(p.id)).map((p) => (
          <div key={p.id} className="settings-pane" hidden={visiblePane !== p.id}>
            {panes[p.id]}
          </div>
        ))}
      </main>
    </div>
  );
}

createRoot(document.getElementById("root")!).render(<Settings />);
