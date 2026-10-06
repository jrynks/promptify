import { useCallback, useEffect, useRef, useState, type ReactNode } from "react";
import { listen } from "@tauri-apps/api/event";
import { api, type DiscoveredInferenceModel, type InferenceConfig, type InferenceConnection, type InferenceConnectionInput, type InferenceProvider, type InferenceStatus } from "../api";

const PRESETS: { provider: InferenceProvider; name: string; base: string; auth: "none" | "api_key" }[] = [
  { provider: "openai", name: "OpenAI", base: "https://api.openai.com/v1", auth: "api_key" },
  { provider: "anthropic", name: "Anthropic", base: "https://api.anthropic.com/v1", auth: "api_key" },
  { provider: "google", name: "Google", base: "https://generativelanguage.googleapis.com/v1beta/openai", auth: "api_key" },
  { provider: "lm_studio", name: "LM Studio", base: "http://localhost:1234/v1", auth: "none" },
  { provider: "custom", name: "Custom endpoint", base: "", auth: "none" },
];

function endpointPolicy(base: string) {
  try {
    const url = new URL(base.trim());
    const loopback = url.hostname === "localhost" || url.hostname === "[::1]" || /^127\./.test(url.hostname);
    const valid = ["http:", "https:"].includes(url.protocol) && !url.username && !url.password && !url.href.includes("?") && !url.href.includes("#");
    return { valid, endpoint: `${url.origin}${url.pathname.replace(/\/+$/, "")}`, remote: !loopback, insecure: !loopback && url.protocol === "http:" };
  } catch {
    return { valid: false, endpoint: "", remote: true, insecure: false };
  }
}

export function useInference() {
  const [config, setConfig] = useState<InferenceConfig | null>(null);
  const [status, setStatus] = useState<InferenceStatus | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [listenerError, setListenerError] = useState<string | null>(null);
  const [listenerAttempt, setListenerAttempt] = useState(0);
  const sequence = useRef(0);
  const refresh = useCallback(async () => {
    const request = ++sequence.current;
    try {
      const [next, nextStatus] = await Promise.all([api.inferenceConfig(), api.inferenceStatus()]);
      if (request === sequence.current) {
        setConfig(next);
        setStatus(nextStatus);
        setError(null);
      }
    } catch (reason) {
      if (request === sequence.current) setError(`Could not read inference configuration: ${String(reason)}`);
    }
  }, []);
  useEffect(() => {
    void refresh();
    let disposed = false;
    let stop: (() => void) | undefined;
    void listen("inference-changed", () => void refresh()).then((unlisten) => {
      if (disposed) unlisten();
      else { stop = unlisten; setListenerError(null); }
    }, (reason) => { if (!disposed) setListenerError(`Could not listen for inference changes: ${String(reason)}`); });
    return () => { disposed = true; sequence.current++; stop?.(); };
  }, [refresh, listenerAttempt]);
  const retry = async () => { setListenerAttempt((attempt) => attempt + 1); await refresh(); };
  return { config, status, error: listenerError || error, refresh, retry };
}

export type InferenceState = ReturnType<typeof useInference>;

const LAST_ONLINE = "promptify.lastOnlineModel";
function rememberOnline(id: string, model: string) {
  try { localStorage.setItem(LAST_ONLINE, JSON.stringify({ id, model })); } catch { /* preference only */ }
}
function recallOnline(): { id: string; model: string } | null {
  try { return JSON.parse(localStorage.getItem(LAST_ONLINE) ?? "null"); } catch { return null; }
}
function hostOf(base: string) {
  try { return new URL(base).host; } catch { return base; }
}

export function Inference({ state, onChange, guided = false, localModel = null, children }: { state: InferenceState; onChange: () => void; guided?: boolean; localModel?: string | null; children?: ReactNode }) {
  const { config, status, refresh } = state;
  const [draft, setDraft] = useState<InferenceConnectionInput | null>(null);
  const [original, setOriginal] = useState<InferenceConnection | null>(null);
  const [secret, setSecret] = useState("");
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [notice, setNotice] = useState<string | null>(null);
  const [phase, setPhase] = useState<string | null>(null);
  const [models, setModels] = useState<DiscoveredInferenceModel[]>([]);
  const [discoveryState, setDiscoveryState] = useState<"idle" | "pending" | "ready" | "error">("idle");
  const [discoveryError, setDiscoveryError] = useState<string | null>(null);
  const [discoveryAttempt, setDiscoveryAttempt] = useState(0);
  const discoverySequence = useRef(0);
  const lock = useRef(false);
  const invalidateDiscovery = () => {
    discoverySequence.current++;
    setModels([]);
    setDiscoveryState("idle");
    setDiscoveryError(null);
  };

  const act = async (action: () => Promise<unknown>) => {
    if (lock.current) return;
    lock.current = true;
    setBusy(true);
    setError(null);
    setNotice(null);
    try {
      await action();
      await refresh();
      onChange();
    } catch (reason) {
      setError(String(reason).replace(/^Error: /, ""));
    } finally {
      lock.current = false;
      setBusy(false);
    }
  };

  const edit = (connection: InferenceConnection) => {
    invalidateDiscovery();
    setOriginal(connection);
    const { id, name, provider, base_url, protocol, auth, model, stream, allow_insecure_lan, consent_remote } = connection;
    setDraft({ id, name, provider, base_url, protocol, auth, model, stream, allow_insecure_lan, consent_remote, secret: null, remove_secret: false });
    setSecret("");
    setModels([]);
    setError(null);
    setNotice(null);
  };
  const add = (provider: InferenceProvider) => {
    invalidateDiscovery();
    const preset = PRESETS.find((candidate) => candidate.provider === provider)!;
    setOriginal(null);
    setDraft({
      id: crypto.randomUUID(), name: preset.name, provider, base_url: preset.base,
      protocol: provider === "anthropic" ? "anthropic_messages" : "openai_chat_completions",
      auth: preset.auth, model: "", stream: true, allow_insecure_lan: false, consent_remote: false,
      secret: null, remove_secret: false,
    });
    setSecret("");
    setModels([]);
    setError(null);
    setNotice(null);
  };

  const policy = endpointPolicy(draft?.base_url ?? "");
  const reusableCredential = !!draft && !!original?.credential_present && original.id === draft.id &&
    endpointPolicy(original.base_url).endpoint === policy.endpoint &&
    original.protocol === draft.protocol && original.provider === draft.provider &&
    original.auth === draft.auth && !draft.remove_secret;
  const endpointChanged = !!original?.credential_present && !!draft &&
    (original.base_url !== draft.base_url || original.protocol !== draft.protocol || original.provider !== draft.provider);
  const canSave = !!draft && !!draft.name.trim() && !!draft.base_url.trim() && !!draft.model.trim() &&
    policy.valid && (!policy.remote || draft.consent_remote) &&
    (!endpointChanged || !!secret || draft.remove_secret) &&
    (draft.auth === "none" || !!secret || reusableCredential);
  const canDiscover = !!draft && policy.valid && (!policy.remote || draft.consent_remote) &&
    (draft.auth === "none" || !!secret || reusableCredential) &&
    (draft.provider !== "google" || draft.protocol === "openai_chat_completions");
  const patch = (update: Partial<InferenceConnectionInput>) => {
    if (["base_url", "protocol", "provider", "auth", "remove_secret", "consent_remote", "allow_insecure_lan"].some((key) => key in update)) invalidateDiscovery();
    setDraft((current) => current && { ...current, ...update });
  };

  useEffect(() => {
    const request = ++discoverySequence.current;
    setModels([]);
    setDiscoveryError(null);
    if (!draft || !canDiscover || busy) {
      setDiscoveryState("idle");
      return;
    }
    setDiscoveryState("pending");
    const timer = window.setTimeout(() => {
      void api.discoverInferenceDraft({
        id: draft.id, name: "", model: "", provider: draft.provider,
        base_url: draft.base_url.trim(), protocol: draft.protocol, auth: draft.auth,
        stream: draft.stream, consent_remote: draft.consent_remote, allow_insecure_lan: false,
        remove_secret: draft.remove_secret, secret: draft.auth === "api_key" && secret ? secret : null,
      }).then((discovered) => {
        if (request !== discoverySequence.current) return;
        setModels(discovered);
        setDiscoveryState("ready");
      }, (reason) => {
        if (request !== discoverySequence.current) return;
        setDiscoveryError(`Model discovery failed: ${String(reason)} Enter a model ID manually or retry.`);
        setDiscoveryState("error");
      });
    }, 600);
    return () => { window.clearTimeout(timer); discoverySequence.current++; };
  }, [draft?.id, draft?.base_url, draft?.protocol, draft?.provider, draft?.auth, draft?.remove_secret,
    draft?.consent_remote, draft?.allow_insecure_lan, secret, reusableCredential, canDiscover, busy, discoveryAttempt]);

  const selection = config?.selection;
  const activeId = selection?.kind === "connection" ? selection.connection_id : null;
  const activeConnection = config?.connections.find((connection) => connection.id === activeId) ?? null;
  const online = selection?.kind === "connection";
  const [view, setView] = useState<"local" | "online">(online ? "online" : "local");
  useEffect(() => { if (selection) setView(selection.kind === "connection" ? "online" : "local"); }, [selection?.kind]);
  const isVerified = (connection: InferenceConnection, model: string) =>
    !!config?.verified.some((tested) => tested.connection_id === connection.id && tested.model === model && tested.revision === connection.revision);
  const preferredModel = (connection: InferenceConnection) => {
    if (selection?.kind === "connection" && selection.connection_id === connection.id) return selection.model;
    const last = recallOnline();
    return last?.id === connection.id && last.model ? last.model : connection.model;
  };
  const onlineTarget = () => activeConnection ?? config?.connections.find((connection) => connection.id === recallOnline()?.id) ?? config?.connections[0] ?? null;

  // Verify before selecting so a failing endpoint never replaces a working prompt writer.
  const switchOnline = async (connection: InferenceConnection, model: string) => {
    if (!isVerified(connection, model)) {
      setPhase(`Checking ${connection.name} · ${model}…`);
      try {
        const result = await api.testInferenceConnection(connection.id, model);
        if (result.state !== "ready") throw new Error(result.message || "The test did not complete.");
      } catch (reason) {
        const keep = selection?.kind === "connection" && selection.connection_id === connection.id && status?.state !== "ready";
        throw new Error(`${connection.name} · ${model} could not be reached: ${String(reason).replace(/^Error: /, "")} ${keep ? "Shortcuts stay paused until it passes. Fix the connection or switch to On this device." : `You are still using ${currentLabel}.`}`);
      } finally { setPhase(null); }
    }
    await api.selectInference({ kind: "connection", connection_id: connection.id, model });
    rememberOnline(connection.id, model);
    setNotice(`Now using ${connection.name} · ${model}.`);
  };
  const check = async (connection: InferenceConnection, model: string) => {
    setPhase(`Checking ${connection.name} · ${model}…`);
    try {
      const result = await api.testInferenceConnection(connection.id, model);
      if (result.state !== "ready") throw new Error(`${connection.name} · ${model} check failed: ${result.message || "no details"}`);
    } finally { setPhase(null); }
    setNotice(`${connection.name} · ${model} responded. A working connection does not guarantee prompt quality.`);
  };
  const switchLocal = async () => {
    await api.selectInference({ kind: "bundled_local" });
    setNotice(`Now using ${localModel} on this device.`);
  };
  const chooseLocal = () => {
    setView("local");
    if (!localModel) { setNotice("Download a prompt model below to write prompts on this device."); return; }
    if (online) void act(switchLocal);
  };
  const chooseOnline = () => {
    setView("online");
    const target = onlineTarget();
    if (!target) { setNotice("Set up an online or server model below. It is checked once, then used for prompts."); return; }
    if (!online) void act(() => switchOnline(target, preferredModel(target)));
  };

  const [activeModels, setActiveModels] = useState<DiscoveredInferenceModel[] | null>(null);
  useEffect(() => {
    setActiveModels(null);
    if (!activeConnection) return;
    let current = true;
    api.discoverInferenceModels(activeConnection.id).then((found) => { if (current) setActiveModels(found); }, () => { if (current) setActiveModels([]); });
    return () => { current = false; };
  }, [activeConnection?.id, activeConnection?.revision]);

  const localLabel = localModel ? `On this device · ${localModel}` : "On this device";
  const currentLabel = !config ? "" : selection?.kind === "connection" ? `${activeConnection?.name ?? "Connection"} · ${selection.model}` : localLabel;
  const readiness = status?.state === "ready" ? "Ready" : status?.state === "error" ? "Needs attention" : online ? "Not checked yet" : localModel ? "Ready when used" : "No model downloaded";

  return (
    <section className={guided ? "tour-target" : undefined} aria-label="Prompt writer">
      <h2 id="prompt-writer" tabIndex={-1}>2. Prompt writer</h2>
      <p className="hint">Choose where prompts are written. Switch any time; speech recognition always stays on this device and plain dictation never uses the prompt writer.</p>
      {!config && !state.error && <p role="status">Loading inference configuration…</p>}
      {state.error && <div role="alert"><p className="error">{state.error}</p><button disabled={busy} onClick={() => void act(state.retry)}>Retry inference configuration</button>
        <button disabled={busy} onClick={() => {
          if (window.confirm("Reset inference configuration to bundled local? Saved connections and their credentials will be removed.")) void act(api.resetInference);
        }}>Reset inference</button>
      </div>}
      {config && <>
        <div className="writer-switch" role="radiogroup" aria-label="Prompt writer location">
          <button role="radio" aria-checked={!online} className={view === "local" ? "viewing" : undefined} disabled={busy} onClick={chooseLocal}>
            <strong>On this device</strong>
            <span>{localModel ?? "No prompt model downloaded"}</span>
            <span className="hint">Private · free · works offline</span>
          </button>
          <button role="radio" aria-checked={online} className={view === "online" ? "viewing" : undefined} disabled={busy} onClick={chooseOnline}>
            <strong>Online or server</strong>
            <span>{activeConnection ? `${activeConnection.name} · ${selection?.kind === "connection" ? selection.model : ""}` : onlineTarget() ? `${onlineTarget()!.name} · ${preferredModel(onlineTarget()!)}` : "Not set up yet"}</span>
            <span className="hint">Your API key or local server</span>
          </button>
        </div>
        <p role="status" className="writer-now"><strong>Now using: </strong>{currentLabel} · {readiness}</p>
        {status?.message && <p className={status.state === "error" ? "error" : "hint"} role={status.state === "error" ? "alert" : undefined}>{status.message}</p>}
        {online && activeConnection && status && status.state !== "ready" && <p role="alert" className="warn">
          Shortcuts are paused until {activeConnection.name} passes a quick check.{" "}
          <button disabled={busy} onClick={() => void act(() => check(activeConnection, selection!.kind === "connection" ? selection!.model : activeConnection.model))}>Check now</button>
          {" "}A successful check is remembered across restarts until the connection changes.
        </p>}

        {view === "local" && <div className="writer-panel" aria-label="On-device prompt models">
          {online && localModel && <p className="hint">Still using {currentLabel}. <button disabled={busy} onClick={() => void act(switchLocal)}>Use {localModel} on this device</button></p>}
          {children}
        </div>}

        {view === "online" && <div className="writer-panel" aria-label="Online prompt models">
          {config.connections.length > 0 && <ul className="inference-connections" aria-label="Saved online models">
            {config.connections.map((connection) => {
              const active = connection.id === activeId;
              const model = preferredModel(connection);
              const options = active && activeModels?.length ? activeModels : [];
              return <li key={connection.id} className={active ? "active" : undefined}>
                <label className="inline writer-choice">
                  <input type="radio" name="online-connection" checked={active} disabled={busy} onChange={() => void act(() => switchOnline(connection, model))} aria-label={`Use ${connection.name}`} />
                  <strong>{connection.name}</strong>
                  <span className="hint">{hostOf(connection.base_url)} · {connection.credential_present ? "Key saved (never displayed)" : "No key saved"}</span>
                  {active && status?.state === "ready" && <span className="badge ok">Ready</span>}
                  {!active && isVerified(connection, model) && <span className="badge ok">Checked</span>}
                </label>
                {active ? <label className="writer-model">Model
                  <select aria-label={`${connection.name} model`} disabled={busy} value={model} onChange={(event) => void act(() => switchOnline(connection, event.target.value))}>
                    {!options.some((option) => option.id === model) && <option value={model}>{model}</option>}
                    {options.map((option) => <option key={option.id} value={option.id}>{option.name === option.id ? option.id : `${option.name} (${option.id})`}</option>)}
                  </select>
                  {activeModels === null && <span className="hint">Loading available models…</span>}
                </label> : <div className="hint">{model}</div>}
                <div className="actions">
                  {active && <button disabled={busy} onClick={() => void act(() => check(connection, model))}>Check {connection.name}</button>}
                  <button disabled={busy} onClick={() => edit(connection)}>Edit {connection.name}</button>
                  <button disabled={busy || active} title={active ? "Switch to another prompt writer first" : undefined} onClick={() => {
                    if (window.confirm(`Remove ${connection.name} and its saved credential?`)) void act(async () => {
                      await api.removeInferenceConnection(connection.id);
                      if (draft?.id === connection.id) { invalidateDiscovery(); setDraft(null); setSecret(""); }
                    });
                  }}>Remove {connection.name}</button>
                </div>
              </li>;
            })}
          </ul>}
          <h3>{config.connections.length ? "Add another" : "Set up an online or server model"}</h3>
          <div className="inference-options">
            {PRESETS.map((preset) => <button key={preset.provider} disabled={busy} aria-pressed={!original && draft?.provider === preset.provider} onClick={() => add(preset.provider)}>{preset.provider === "custom" ? "Add custom model" : preset.name}</button>)}
          </div>
          <p className="hint">Use “Add custom model” for Agent Maestro, Ollama, or any OpenAI-compatible server. API keys use API billing, are write-only, and are stored in the operating system credential store.</p>
          <p className="hint">Switching to a model that has not been used before sends one tiny synthetic message to check it works; your history and screen text are not sent. If the check fails, your current prompt writer stays active.</p>
        </div>}
      </>}
      {draft && <fieldset className="inference-editor" disabled={busy}>
        <legend>{original ? `Edit ${original.name}` : `Add ${draft.name}`}</legend>
        {draft.provider === "lm_studio" && <p>In LM Studio, open the Developer tab, start the server, and make a text model available. Models are discovered automatically. Localhost does not prove the model itself is offline: check LM Link or other remote routing.</p>}
        <label>Connection name<input value={draft.name} onChange={(event) => patch({ name: event.target.value })} /></label>
        <label>API base URL<input type="url" value={draft.base_url} onChange={(event) => { patch({ base_url: event.target.value, consent_remote: false, allow_insecure_lan: false }); setModels([]); }} placeholder="http://localhost:1234/v1" /></label>
        <p className="hint">Enter an HTTP(S) API base, not a full generation path. HTTPS encrypts data in transit; HTTP does not.</p>
        <label>Protocol<select aria-label="Protocol" value={draft.protocol} onChange={(event) => patch({ protocol: event.target.value as InferenceConnectionInput["protocol"] })}>
          <option value="openai_chat_completions">OpenAI Chat Completions</option>
          <option value="openai_responses">OpenAI Responses</option>
          <option value="anthropic_messages">Anthropic Messages</option>
        </select></label>
        <label>Authentication<select aria-label="Authentication" value={draft.auth} onChange={(event) => {
          const auth = event.target.value as "none" | "api_key";
          patch({ auth, remove_secret: auth === "none" });
          setSecret("");
        }}><option value="none">No token (optional authentication)</option><option value="api_key">API key / bearer token</option></select></label>
        {draft.auth === "api_key" && <label>{original?.credential_present ? "Replace saved API key / token (leave blank to keep)" : "API key / token"}
          <input type="password" autoComplete="new-password" spellCheck={false} value={secret} onChange={(event) => { invalidateDiscovery(); setSecret(event.target.value); }} />
        </label>}
        {original?.credential_present && <label className="inline"><input type="checkbox" checked={draft.remove_secret} onChange={(event) => {
          patch({ remove_secret: event.target.checked, auth: event.target.checked ? "none" : original.auth });
          setSecret("");
        }} />Remove saved key / token</label>}
        {endpointChanged && <p className="warn">Changing the endpoint or protocol cannot silently reuse a saved key. Supply a new key or explicitly remove it.</p>}
        {policy.remote && <div className="inference-disclosure">
          <p>This non-loopback endpoint receives transcripts, prompt instructions, permitted app/context metadata, opted-in focused screen text, enabled history context, and drafts sent for structural repair. History and screen-text opt-ins still control collection. Local history storage does not prevent outgoing context. Check the endpoint operator's retention and privacy policies.</p>
          <label className="inline"><input type="checkbox" checked={draft.consent_remote} onChange={(event) => patch({ consent_remote: event.target.checked })} />I consent to sending this context to this endpoint.</label>
        </div>}
        {policy.insecure && <p className="warn">HTTP is unencrypted. Other devices on the network may read context and tokens in transit. Use HTTPS when available.</p>}
        {discoveryState === "idle" && <p className="hint">Available models appear automatically once the URL{policy.remote ? ", consent" : ""}{draft.auth === "api_key" ? " and key" : ""} are filled in.</p>}
        {draft.provider === "google" && draft.protocol !== "openai_chat_completions" && <p className="warn">Google model discovery requires its documented OpenAI Chat Completions protocol.</p>}
        {discoveryState === "pending" && <p role="status">Discovering models…</p>}
        {discoveryState === "ready" && <p role="status">{models.length ? `${models.length} models found. Choose one below.` : "No models discovered. Enter a model ID manually."}</p>}
        {discoveryError && <p role="alert" className="error">{discoveryError}</p>}
        {models.length > 0 && <label>Discovered model<select aria-label="Discovered model" value={models.some((model) => model.id === draft.model) ? draft.model : ""} onChange={(event) => patch({ model: event.target.value })}>
          <option value="" disabled>Choose a discovered model</option>{models.map((model) => <option key={model.id} value={model.id}>{model.name} ({model.id})</option>)}
        </select></label>}
        <label>Model ID<input value={draft.model} onChange={(event) => patch({ model: event.target.value })} placeholder="Exact text-model identifier" /></label>
        <button disabled={!canDiscover || discoveryState === "pending"} onClick={() => { invalidateDiscovery(); setDiscoveryAttempt((attempt) => attempt + 1); }}>{discoveryState === "error" ? "Retry model discovery" : "Refresh models"}</button>
        <label className="inline"><input type="checkbox" checked={draft.stream} onChange={(event) => patch({ stream: event.target.checked })} />Stream responses</label>
        <div className="actions">
          <button className="primary" disabled={!canSave} onClick={() => {
            invalidateDiscovery();
            const input: InferenceConnectionInput = {
              id: draft.id, name: draft.name.trim(), provider: draft.provider, base_url: draft.base_url.trim(),
              protocol: draft.protocol, auth: draft.auth, model: draft.model.trim(), stream: draft.stream,
              allow_insecure_lan: false, consent_remote: draft.consent_remote,
              secret: draft.auth === "api_key" && secret ? secret : null, remove_secret: draft.remove_secret,
            };
            const wasActive = input.id === activeId;
            const isNew = !original;
            setSecret("");
            void act(async () => {
              const next = await api.saveInferenceConnection(input);
              const saved = next.connections.find((connection) => connection.id === input.id);
              invalidateDiscovery();
              setDraft(null);
              setOriginal(null);
              if (saved && isNew) await switchOnline(saved, saved.model);
              else if (saved && wasActive) {
                const model = selection?.kind === "connection" && original?.model === saved.model ? selection.model : saved.model;
                await api.selectInference({ kind: "connection", connection_id: saved.id, model });
                rememberOnline(saved.id, model);
                if (!next.verified.some((tested) => tested.connection_id === saved.id && tested.model === model && tested.revision === saved.revision)) await check(saved, model);
                else setNotice(`${saved.name} saved and still in use.`);
              } else setNotice(`${input.name} saved.`);
            });
          }}>{busy ? "Working…" : original ? "Save connection" : "Save and use"}</button>
          <button onClick={() => { invalidateDiscovery(); setDraft(null); setSecret(""); }}>Cancel editing</button>
        </div>
      </fieldset>}
      {phase && <p role="status">{phase}</p>}
      {notice && <p role="status">{notice}</p>}
      {error && <p role="alert" className="error">{error}</p>}
    </section>
  );
}
