import { useCallback, useEffect, useRef, useState } from "react";
import { listen } from "@tauri-apps/api/event";
import { api, type UpdateInfo } from "../api";

export function useUpdates() {
  const [info, setInfo] = useState<UpdateInfo | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const revision = useRef(0);
  const active = useRef(true);
  const actionActive = useRef(false);
  const refresh = useCallback(() => {
    const id = ++revision.current;
    void api.updateInfo().then((next) => {
      if (active.current && id === revision.current) setInfo(next);
    }, (reason) => {
      if (active.current && id === revision.current) setError(`Could not load update status: ${String(reason)}`);
    });
  }, []);

  useEffect(() => {
    active.current = true;
    let disposed = false;
    let stop: (() => void) | undefined;
    void listen("updates-changed", refresh).then((unlisten) => {
      if (disposed) { unlisten(); return; }
      stop = unlisten;
      refresh();
    }, (reason) => {
      if (!disposed) setError(`Could not connect to update events: ${String(reason)}`);
    });
    return () => {
      disposed = true;
      active.current = false;
      revision.current++;
      stop?.();
    };
  }, [refresh]);

  const run = async (action: () => Promise<unknown>) => {
    if (actionActive.current) return;
    actionActive.current = true;
    setBusy(true);
    setError(null);
    try {
      await action();
    } catch (reason) {
      if (active.current) setError(String(reason));
    } finally {
      actionActive.current = false;
      if (active.current) {
        setBusy(false);
        refresh();
      }
    }
  };
  return { info, error, busy: busy || !!info?.checking || info?.installation === "downloading" || info?.installation === "installing", run };
}

type UpdateState = ReturnType<typeof useUpdates>;

function InstallButton({ updates }: { updates: UpdateState }) {
  const { info, busy, run } = updates;
  if (info?.installation === "installed") {
    return <button disabled={busy} onClick={() => void run(api.restartAfterUpdate)}>Restart Promptify</button>;
  }
  if (!info?.release?.update_available) return null;
  const label = info.installation === "downloading" ? "Downloading update..."
    : info.installation === "installing" ? "Installing update..." : `Install update ${info.release.version}`;
  return <button disabled={busy || !!info.release.install_error} onClick={() => void run(api.installUpdate)}>{label}</button>;
}

export function UpdateIndicator({ updates }: { updates: UpdateState }) {
  const { info } = updates;
  if (!info?.release?.update_available) return null;
  return <aside className="update-notice" aria-label="Available update">
    <p role="status">{info.installation === "installed"
      ? "Update installed. Restart Promptify to use the new version."
      : `Promptify ${info.release.version} is available.`}</p>
    <InstallButton updates={updates} />
    {(updates.error || info.installation_error) && <p className="error" role="alert">{updates.error || info.installation_error}</p>}
    {info.release.install_error && <p className="warn">{info.release.install_error}</p>}
  </aside>;
}

export function UpdatesSettings({ updates }: { updates: UpdateState }) {
  const { info, busy, error, run } = updates;
  return <section aria-label="Update settings">
    <h2>Updates</h2>
    <p>Installed version: {info?.current_version ?? "Loading..."}</p>
    <p className="hint">Checks published stable releases on GitHub. Updates are downloaded and installed only when you click Install update. System authorization or installer confirmation may be required.</p>
    <label className="inline">
      <input type="checkbox" checked={info?.check_on_startup ?? false} disabled={!info || busy}
        onChange={(event) => void run(() => api.setUpdateChecks(event.target.checked))} />
      Check for new versions when Promptify starts
    </label>
    <p className="hint">Successful checks are cached for one hour in this session; failed checks can retry after one minute. No prompts, history, or application context are sent.</p>
    <div className="actions">
      <button disabled={!info || busy} onClick={() => void run(api.checkForUpdates)}>
        {info?.checking ? "Checking for updates..." : "Check for updates"}
      </button>
      {info?.release?.update_available && <button disabled={busy} onClick={() => void run(api.openUpdateRelease)}>View release notes</button>}
    </div>
    {info && !info.checking && info.checked && !info.error && !info.release?.update_available
      && <p role="status">{info.release ? "You are up to date." : "No published stable release is available."}</p>}
    {(error || info?.error) && <p className="error" role="alert">{error || info?.error}</p>}
  </section>;
}
