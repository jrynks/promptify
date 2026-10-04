import { useState } from "react";
import { api, type AppInfo } from "../api";

export function DesktopIntegration({ info, onChange }: { info: AppInfo; onChange: () => void }) {
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const enabled = info.desktop_integration_enabled !== false;

  const grant = async () => {
    setBusy(true);
    setError(null);
    try {
      await api.grantPastePermission();
      onChange();
    } catch (reason) {
      setError(String(reason));
      onChange();
    } finally {
      setBusy(false);
    }
  };

  const disable = async () => {
    setBusy(true);
    setError(null);
    try {
      await api.disableDesktopIntegration();
      onChange();
    } catch (reason) {
      setError(String(reason));
    } finally {
      setBusy(false);
    }
  };

  const restore = async () => {
    setBusy(true);
    setError(null);
    try {
      await api.restoreInsertionClipboard();
    } catch (reason) {
      setError(String(reason));
    } finally {
      onChange();
      setBusy(false);
    }
  };

  return (
    <div aria-label="Desktop integration">
      <h3>Desktop integration</h3>
      {info.desktop_error
        ? <p className="error" role="alert">{info.desktop_error}</p>
        : <p>Focused-window detection is working.</p>}
      {!enabled && <p>Desktop integration is disabled. Generated text remains available for review.</p>}
      {info.paste_permission !== "not_needed" && enabled && <>
        <p>{info.paste_permission === "granted"
          ? "Desktop integration permission is active."
          : "Desktop integration needs permission. Follow the system dialogs to authorize input and shortcuts. No screen capture is requested. Your system may remember permission for future launches."}</p>
      </>}
      {(!enabled || info.paste_permission === "required") &&
        <button disabled={busy} onClick={() => void grant()}>{busy ? "Waiting for desktop permission..." : "Enable desktop integration"}</button>}
      {enabled &&
        <button disabled={busy} onClick={() => void disable()}>Disable desktop integration</button>}
      {info.clipboard_restore_pending && <>
        <p>Generated text remains on the clipboard so a slow application can read it. After checking the destination, you can restore the previous clipboard. The saved snapshot is kept only until this app exits.</p>
        <button disabled={busy} onClick={() => void restore()}>Restore previous clipboard</button>
      </>}
      {error && <p className="error" role="alert">{error}</p>}
    </div>
  );
}
