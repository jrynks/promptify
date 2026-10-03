import { useState } from "react";
import { api, type AppInfo } from "../api";

export function DesktopIntegration({ info, onChange }: { info: AppInfo; onChange: () => void }) {
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);

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

  if (info.paste_permission === "not_needed" && !info.desktop_error) return null;

  return (
    <div aria-label="Desktop integration">
      <h3>Desktop integration</h3>
      {info.desktop_error
        ? <p className="error" role="alert">{info.desktop_error}</p>
        : <p>Focused-window detection is working.</p>}
      {info.paste_permission !== "not_needed" && <>
        <p>{info.paste_permission === "granted"
          ? "Automatic paste: keyboard permission is active."
          : "Automatic paste on Wayland needs keyboard permission. Allow keyboard control in the desktop dialog; no screen capture is requested. Your desktop may remember this permission for future launches."}</p>
        {info.paste_permission === "required" &&
          <button disabled={busy} onClick={() => void grant()}>{busy ? "Waiting for desktop permission..." : "Grant paste permission"}</button>}
      </>}
      {error && <p className="error" role="alert">{error}</p>}
    </div>
  );
}
