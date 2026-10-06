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

export function Inference({ state, onChange, guided = false, children }: { state: InferenceState; onChange: () => void; guided?: boolean; children?: ReactNode }) {
  const { config, status, refresh } = state;
  const [draft, setDraft] = useState<InferenceConnectionInput | null>(null);
  const [original, setOriginal] = useState<InferenceConnection | null>(null);
  const [secret, setSecret] = useState("");
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [notice, setNotice] = useState<string | null>(null);
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
      setError(String(reason));
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

  return (
    <section className={guided ? "tour-target" : undefined} aria-label="Prompt writer">
      <h2 id="prompt-writer" tabIndex={-1}>2. Prompt writer</h2>
      <p className="hint">One selected model handles every language-model stage: prompt generation and structural repair. Dictation does not use it. No automatic provider fallback. Speech recognition remains local.</p>
      {!config && !state.error && <p role="status">Loading inference configuration…</p>}
      {state.error && <div role="alert"><p className="error">{state.error}</p><button disabled={busy} onClick={() => void act(state.retry)}>Retry inference configuration</button>
        <button disabled={busy} onClick={() => {
          if (window.confirm("Reset inference configuration to bundled local? Saved connections and their credentials will be removed.")) void act(api.resetInference);
        }}>Reset inference</button>
      </div>}
      {config && <>
        <p role="status"><strong>Selected: </strong>{config.selection.kind === "bundled_local" ? "Bundled local" : `${config.connections.find((connection) => config.selection.kind === "connection" && connection.id === config.selection.connection_id)?.name ?? "Connection"} · ${config.selection.model}`}
          {" · "}{status?.state === "ready" ? "Ready" : status?.state === "error" ? "Error" : "Configured, not yet verified"}</p>
        {status?.message && <p className={status.state === "error" ? "error" : "hint"} role={status.state === "error" ? "alert" : undefined}>{status.message}</p>}
        {config.selection.kind === "connection" && status?.state === "configured" && <p role="alert" className="warn">
          Shortcuts are unavailable until the selected connection passes its test. Click Test below.
          {" "}Successful verification is remembered across restarts. Only changed connection settings or a different unverified model require a new test; model discovery is not a generation test.
        </p>}
        <button disabled={busy || config.selection.kind === "bundled_local"} onClick={() => void act(() => api.selectInference({ kind: "bundled_local" }))}>Use bundled local</button>
        <p className="hint">Bundled local is the default: no account, API key, or external language-model service.</p>
        {children}
        <h3>API and local-server connections</h3>
        <div className="inference-options">
          {PRESETS.map((preset) => <button key={preset.provider} disabled={busy} aria-pressed={draft?.provider === preset.provider} onClick={() => add(preset.provider)}>{preset.provider === "custom" ? "Add custom model" : preset.name}</button>)}
        </div>
        <p className="hint">Provider API keys use API billing, not a ChatGPT or other consumer subscription. Keys are write-only and stored in the operating system credential store, never in browser storage. OAuth subscriptions are not supported here.</p>
        <ul className="inference-connections">
          {config.connections.map((connection) => {
            const activeConnection = config.selection.kind === "connection" && config.selection.connection_id === connection.id;
            const selected = activeConnection && config.selection.kind === "connection" && config.selection.model === connection.model;
            return <li key={connection.id}>
              <strong>{connection.name}</strong> · {connection.model} {selected && <span className="badge">Selected</span>}
              <div className="hint">{connection.base_url} · {connection.protocol} · {connection.credential_present ? "Key saved (never displayed)" : "No key saved"}</div>
              <div className="actions">
                <button disabled={busy || selected} onClick={() => void act(() => api.selectInference({ kind: "connection", connection_id: connection.id, model: connection.model }))}>Use {connection.name}</button>
                <button disabled={busy} onClick={() => edit(connection)}>Edit {connection.name}</button>
                <button disabled={busy} onClick={() => void act(async () => {
                  const result = await api.testInferenceConnection(connection.id, connection.model);
                  if (result.state !== "ready") throw new Error(result.message || "Synthetic inference test did not verify the selected model.");
                  setNotice(`Synthetic test completed for ${connection.name} · ${connection.model}. Connectivity is not a quality guarantee.`);
                })}>Test {connection.name}</button>
                <button disabled={busy || activeConnection} onClick={() => {
                  if (window.confirm(`Remove ${connection.name} and its saved credential?`)) void act(async () => {
                    await api.removeInferenceConnection(connection.id);
                    if (draft?.id === connection.id) { invalidateDiscovery(); setDraft(null); setSecret(""); }
                  });
                }}>Remove {connection.name}</button>
              </div>
              {activeConnection && <p className="hint">Select a replacement before removing the active connection.{!selected && " The saved model changed; select it explicitly to use the new model."}</p>}
            </li>;
          })}
        </ul>
        <p className="hint">Tests send tiny synthetic, non-sensitive content to the exact saved model, not your history or screen text. They may incur API charges or trigger just-in-time model loading. Nothing is tested automatically.</p>
      </>}
      {draft && <fieldset className="inference-editor" disabled={busy}>
        <legend>{original ? `Edit ${original.name}` : `Add ${draft.name}`}</legend>
        {draft.provider === "lm_studio" && <p>In LM Studio, open the Developer tab, start the server, and make a text model available. Models are discovered automatically; choose or enter an ID, save, select it, then test it. Localhost does not prove the model itself is offline: check LM Link or other remote routing.</p>}
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
        <label>Model ID<input value={draft.model} onChange={(event) => patch({ model: event.target.value })} placeholder="Exact text-model identifier" /></label>
        <p className="hint">Manual model entry works even when discovery is unavailable. Discovery lists IDs only; it does not prove a model is loaded or ready.</p>
        {discoveryState === "idle" && <p className="hint">To discover models, enter a valid HTTP(S) endpoint, confirm remote data disclosure if needed, and provide an API key when authentication is enabled.</p>}
        {draft.provider === "google" && draft.protocol !== "openai_chat_completions" && <p className="warn">Google model discovery requires its documented OpenAI Chat Completions protocol.</p>}
        {discoveryState === "pending" && <p role="status">Discovering models…</p>}
        {discoveryState === "ready" && <p role="status">{models.length ? "Models discovered from this draft endpoint. Choose an ID and save before testing." : "No models discovered. Enter a model ID manually."}</p>}
        {discoveryError && <p role="alert" className="error">{discoveryError}</p>}
        <button disabled={!canDiscover || discoveryState === "pending"} onClick={() => { invalidateDiscovery(); setDiscoveryAttempt((attempt) => attempt + 1); }}>{discoveryState === "error" ? "Retry model discovery" : "Refresh models"}</button>
        {models.length > 0 && <label>Discovered model<select aria-label="Discovered model" value={models.some((model) => model.id === draft.model) ? draft.model : ""} onChange={(event) => patch({ model: event.target.value })}>
          <option value="" disabled>Choose a discovered model</option>{models.map((model) => <option key={model.id} value={model.id}>{model.name} ({model.id})</option>)}
        </select></label>}
        <label className="inline"><input type="checkbox" checked={draft.stream} onChange={(event) => patch({ stream: event.target.checked })} />Stream responses</label>
        {policy.remote && <div className="inference-disclosure">
          <p>This non-loopback endpoint receives transcripts, prompt instructions, permitted app/context metadata, opted-in focused screen text, enabled history context, and drafts sent for structural repair. History and screen-text opt-ins still control collection. Local history storage does not prevent outgoing context. Check the endpoint operator's retention and privacy policies.</p>
          <label className="inline"><input type="checkbox" checked={draft.consent_remote} onChange={(event) => patch({ consent_remote: event.target.checked })} />I consent to sending this context to this endpoint.</label>
        </div>}
        {policy.insecure && <p className="warn">HTTP is unencrypted. Other devices on the network may read context and tokens in transit. Use HTTPS when available.</p>}
        <div className="actions">
          <button className="primary" disabled={!canSave} onClick={() => {
            invalidateDiscovery();
            const input: InferenceConnectionInput = {
              id: draft.id, name: draft.name.trim(), provider: draft.provider, base_url: draft.base_url.trim(),
              protocol: draft.protocol, auth: draft.auth, model: draft.model.trim(), stream: draft.stream,
              allow_insecure_lan: false, consent_remote: draft.consent_remote,
              secret: draft.auth === "api_key" && secret ? secret : null, remove_secret: draft.remove_secret,
            };
            setSecret("");
            void act(async () => {
              const next = await api.saveInferenceConnection(input);
              const saved = next.connections.find((connection) => connection.id === input.id);
              if (saved) edit(saved);
              setNotice("Connection saved. Select it explicitly if needed; test only when the selected model is not yet verified.");
            });
          }}>{busy ? "Working…" : "Save connection"}</button>
          <button onClick={() => { invalidateDiscovery(); setDraft(null); setSecret(""); }}>Cancel editing</button>
        </div>
      </fieldset>}
      {notice && <p role="status">{notice}</p>}
      {error && <p role="alert" className="error">{error}</p>}
      {busy && <p role="status">Inference action in progress…</p>}
    </section>
  );
}
