import { useEffect, useState } from "react";
import { api, type JevStatus } from "../api";

export function JevSettings() {
  const [status, setStatus] = useState<JevStatus | null>(null);
  const [key, setKey] = useState("");
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);

  const refresh = async () => {
    setBusy(true);
    setError(null);
    try {
      setStatus(await api.jevStatus());
    } catch (reason) {
      setStatus(null);
      setError(String(reason));
    } finally {
      setBusy(false);
    }
  };
  useEffect(() => { void refresh(); }, []);

  const change = async (operation: () => Promise<JevStatus>) => {
    setBusy(true);
    setError(null);
    try {
      setStatus(await operation());
    } catch (reason) {
      setError(String(reason));
      // Removal disables review before touching the key; recover the actual state on failure.
      try {
        setStatus(await api.jevStatus());
      } catch (refreshReason) {
        setStatus(null);
        setError(`${String(reason)} Could not refresh Jev settings: ${String(refreshReason)}`);
      }
    } finally {
      setBusy(false);
    }
  };

  return <section aria-label="Jev remote review">
    <h2>Jev remote review (experimental)</h2>
    <p className="hint" id="jev-consent">Optional desktop prompt review, off by default. Enabling sends your spoken request and generated prompt to TypeSafe for Jev review. The generated prompt may contain context you opted into using. It may run twice after a bounded rewrite. No separate histories or screen context are sent. Plain dictation and CLI evaluations are not reviewed.</p>
    <p className="hint">Review uses experimental conservative thresholds, not probabilities calibrated for your task. Speech recognition stays local; generation and any rewrite use your selected inference backend.</p>
    <p className="hint">Your API key is stored only in the OS credential store, never in settings or browser storage. Saving a key does not enable review or make an API call.</p>
    {status && <p role="status">{status.key_configured ? "API key stored in OS credential store. Stored does not mean verified; no connection test has been made." : "No API key stored."}</p>}
    <label htmlFor="jev-key">TypeSafe API key</label>
    <div className="inline">
      <input id="jev-key" type="password" autoComplete="off" value={key} disabled={busy || !status}
        onChange={(event) => setKey(event.target.value)} />
      <button disabled={busy || !status || !key} onClick={() => {
        const value = key;
        setKey("");
        void change(() => api.saveJevKey(value));
      }}>Save API key</button>
      <button disabled={busy || !status?.key_configured} onClick={() => {
        setKey("");
        void change(api.deleteJevKey);
      }}>Remove API key</button>
    </div>
    <label className="inline">
      <input type="checkbox" checked={status?.enabled ?? false}
        disabled={busy || !status || (!status.key_configured && !status.enabled)}
        aria-describedby="jev-consent"
        onChange={(event) => void change(() => api.setJevEnabled(event.target.checked))} />
      Enable Jev review and send requests and generated prompts to TypeSafe
    </label>
    {error && <p className="error" role="alert">{error}</p>}
    {!status && <button disabled={busy} onClick={() => void refresh()}>Retry Jev settings</button>}
  </section>;
}
