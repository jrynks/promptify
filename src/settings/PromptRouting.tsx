import { useEffect, useState } from "react";
import { api, type PromptCatalog, type PromptRouting, type Rendering, type RoutingOptions, type RoutingState } from "../api";

export const automaticRouting = (rendering: Rendering): RoutingOptions => ({ rendering, task_type: null, surface: null });
const surfaceLabel = (surface: string) => surface.replaceAll("_", " ");

export function RoutingControls({ catalog, value, onChange, includeRendering = true }: {
  catalog: PromptCatalog;
  value: RoutingOptions;
  onChange: (value: RoutingOptions) => void;
  includeRendering?: boolean;
}) {
  return <div className="grid routing-controls">
    {includeRendering && <label>Rendering
      <select value={value.rendering} onChange={(event) => {
        if (event.target.value === "legacy" || event.target.value === "adaptive") onChange(automaticRouting(event.target.value));
      }}>
        <option value="legacy">Existing target profiles</option>
        <option value="adaptive">Task-aware adaptation</option>
      </select>
    </label>}
    <label>Prompt type
      <select disabled={value.rendering !== "adaptive"} value={value.task_type ?? ""} onChange={(event) => onChange({ ...value, task_type: event.target.value || null })}>
        <option value="">Automatic from the request</option>
        {catalog.tasks.filter((task) => task.status === "enabled").map((task) =>
          <option key={task.id} value={task.id}>{task.id} - {task.label}</option>)}
      </select>
    </label>
    <label>Input surface
      <select disabled={value.rendering !== "adaptive"} value={value.surface ?? ""} onChange={(event) => {
        const surface = catalog.surfaces.find((item) => item === event.target.value);
        if (surface || event.target.value === "") onChange({ ...value, surface: surface ?? null });
      }}>
        <option value="">Automatic; review if uncertain</option>
        {catalog.surfaces.filter((surface) => surface !== "unknown").map((surface) =>
          <option key={surface} value={surface}>{surfaceLabel(surface)}</option>)}
      </select>
    </label>
  </div>;
}

export function RoutingSummary({ routing }: { routing: PromptRouting }) {
  return <div className="routing-summary">
    <p><strong>{routing.target_name}</strong> / {surfaceLabel(routing.surface)} / <strong>{routing.task_type}</strong></p>
    <p className="hint">Format: {surfaceLabel(routing.form)}. Selection: {routing.reason}.
      {routing.conversational ? " One turn at a time." : ""}
      {!routing.auto_paste ? " Review and copy; automatic paste is disabled." : ""}
    </p>
    {routing.secondary_tasks.length > 0 && <p className="hint">Other recognized tasks: {routing.secondary_tasks.join(", ")}</p>}
    {routing.warnings.map((warning) => <p className="hint" key={warning}>{warning}</p>)}
  </div>;
}

export function PromptRoutingSettings() {
  const [state, setState] = useState<RoutingState | null>(null);
  const [catalog, setCatalog] = useState<PromptCatalog | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [notice, setNotice] = useState("");
  const [busy, setBusy] = useState(false);
  const [attempt, setAttempt] = useState(0);
  const [next, setNext] = useState<RoutingOptions>(automaticRouting("adaptive"));
  const [query, setQuery] = useState("");
  const [includeProposed, setIncludeProposed] = useState(false);
  const [limit, setLimit] = useState(12);

  useEffect(() => {
    let disposed = false;
    setError(null);
    void Promise.all([api.routingState(), api.promptCatalog()]).then(([settings, types]) => {
      if (!disposed) { setState(settings); setCatalog(types); }
    }, (reason) => { if (!disposed) setError(`Could not load prompt routing: ${String(reason)}`); });
    return () => { disposed = true; };
  }, [attempt]);

  const save = async (rendering: Rendering) => {
    setBusy(true); setError(null); setNotice("");
    try {
      setState(await api.setRendering(rendering));
      setNotice(rendering === "adaptive" ? "Task-aware adaptation is enabled. Graphs and check loops remain mandatory." : "Existing target profiles are restored. Graphs and check loops remain mandatory.");
    } catch (reason) { setError(String(reason)); }
    finally { setBusy(false); }
  };

  const queue = async () => {
    setBusy(true); setError(null); setNotice("");
    try {
      await api.queuePromptRouting(next);
      setNotice("Selection saved for your next Prompt hotkey use only. Switch to the destination input before recording. Dictation is unchanged.");
    } catch (reason) { setError(String(reason)); }
    finally { setBusy(false); }
  };

  const reset = async () => {
    setBusy(true); setError(null); setNotice("");
    try {
      setState(await api.resetRouting());
      setNotice("Default routing restored. The previous routing file was backed up in the app data folder.");
    } catch (reason) { setError(String(reason)); }
    finally { setBusy(false); }
  };

  const needle = query.trim().toLowerCase();
  const matches = catalog?.tasks.filter((task) =>
    (includeProposed || task.status === "enabled") &&
    `${task.id} ${task.family} ${task.label} ${task.inputs}`.toLowerCase().includes(needle)) ?? [];

  return <section>
    <h2>Prompt types</h2>
    <p className="hint">Adapt the prompt to the identified tool, its input surface, and what you ask it to do. Everything is rewritten locally; the destination does the work.</p>
    {error && <div role="alert"><p className="error">{error}</p>
      {catalog && state && <button disabled={busy} onClick={() => setAttempt((value) => value + 1)}>Retry prompt types</button>}
    </div>}
    {state?.error && <div role="alert"><p className="error">{state.error} Legacy rendering remains active until the routing settings are repaired.</p>
      <p className="hint">Restoring defaults backs up the existing routing file before replacing it.</p>
      <button disabled={busy} onClick={() => void reset()}>Restore default routing</button>
    </div>}
    {!catalog || !state ? <>
      {!error && <p role="status">Loading prompt types...</p>}
      {error && <button onClick={() => setAttempt((value) => value + 1)}>Retry prompt types</button>}
    </> : <>
      <label className="inline">
        <input type="checkbox" checked={state.rendering === "adaptive"} disabled={busy}
          onChange={(event) => void save(event.target.checked ? "adaptive" : "legacy")} />
        Use task-aware prompt adaptation
      </label>
      <p className="hint">Experimental and off by default. Every final prompt still requires numbered steps, a bounded check loop, and Done when criteria. This switch adapts the task content, not that mandate. Single-line fields use inline graphs; generator-only and unconfirmed inputs require review instead of automatic pasting.</p>
      <details>
        <summary>Choose a type or surface for the next prompt</summary>
        <p className="hint">Only confirm the actual AI input. A spreadsheet cell, SQL editor, lyrics field, or speech script is literal content: use Dictation there.</p>
        <RoutingControls catalog={catalog} value={next} onChange={setNext} />
        {next.surface === "literal" && <p className="hint">Use the Dictation hotkey in a literal-content field. An AI prompt will not be generated there.</p>}
        <button disabled={busy || next.surface === "literal"} onClick={() => void queue()}>Use for next prompt</button>
      </details>
      {notice && <p role="status">{notice}</p>}
      <h3>Prompt catalog</h3>
      <p className="hint">{catalog.tasks.length} candidate types; {catalog.tasks.filter((task) => task.status === "enabled").length} enabled. Proposed types are research candidates, not promises of automatic support.</p>
      <label>Find a prompt type
        <input value={query} onChange={(event) => { setQuery(event.target.value); setLimit(12); }} placeholder="Try debugging, spreadsheet, roleplay, or music" />
      </label>
      <label className="inline"><input type="checkbox" checked={includeProposed} onChange={(event) => { setIncludeProposed(event.target.checked); setLimit(12); }} />Include proposed types</label>
      <p className="hint" role="status">{matches.length} matching types</p>
      <ul className="rows prompt-catalog">
        {matches.slice(0, limit).map((task) => <li key={task.id}>
          <strong>{task.id}</strong>
          <p>{task.label}</p>
          <p className="hint">{task.family} / {task.priority} / {task.feasibility} / {task.status}</p>
          <p className="hint">Useful context: {task.inputs}</p>
        </li>)}
      </ul>
      {matches.length > limit && <button onClick={() => setLimit((value) => value + 24)}>Show more types</button>}
    </>}
  </section>;
}
