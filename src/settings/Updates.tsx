import { useCallback, useEffect, useRef, useState } from "react";
import { listen } from "@tauri-apps/api/event";
import { api, type UpdateInfo } from "../api";

export function useUpdates() {
  const [info, setInfo] = useState<UpdateInfo | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [connectionError, setConnectionError] = useState<string | null>(null);
  const [listenerError, setListenerError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const [connectionAttempt, setConnectionAttempt] = useState(0);
  const revision = useRef(0);
  const active = useRef(true);
  const actionActive = useRef(false);
  const refresh = useCallback(() => {
    const id = ++revision.current;
    void api.updateInfo().then((next) => {
      if (active.current && id === revision.current) {
        setInfo(next);
        setConnectionError(null);
      }
    }, (reason) => {
      if (active.current && id === revision.current) setConnectionError(`Could not load update status: ${String(reason)}`);
    });
  }, []);

  useEffect(() => {
    active.current = true;
    let disposed = false;
    let stop: (() => void) | undefined;
    void listen("updates-changed", refresh).then((unlisten) => {
      if (disposed) { unlisten(); return; }
      stop = unlisten;
      setListenerError(null);
      refresh();
    }, (reason) => {
      if (!disposed) setListenerError(`Could not connect to update events: ${String(reason)}`);
    });
    return () => {
      disposed = true;
      active.current = false;
      revision.current++;
      stop?.();
    };
  }, [refresh, connectionAttempt]);

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
  const retryConnection = () => {
    setError(null);
    setConnectionError(null);
    setConnectionAttempt((value) => value + 1);
  };
  return { info, error: listenerError || connectionError || error, connectionError: listenerError || connectionError, retryConnection, busy: busy || !!info?.checking || info?.installation === "downloading" || info?.installation === "installing", run };
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
    {(updates.error || info.installation_error || info.error) && <div role="alert">
      <p className="error">{updates.error || info.installation_error || info.error}</p>
      {updates.connectionError && <button disabled={updates.busy} onClick={updates.retryConnection}>Retry update connection</button>}
      {info.error && <button disabled={updates.busy} onClick={() => void updates.run(api.checkForUpdates)}>Retry update check</button>}
    </div>}
    {info.release.install_error && <p className="warn">{info.release.install_error}</p>}
    {(updates.error || info.installation_error || info.release.install_error) &&
      <button disabled={updates.busy} onClick={() => void updates.run(api.openUpdateRelease)}>Download from release page</button>}
  </aside>;
}

export function UpdatesSettings({ updates }: { updates: UpdateState }) {
  const { info, busy, error, run, retryConnection } = updates;
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
    {(error || info?.error) && !info?.release?.update_available && <div role="alert">
      <p className="error">{error || info?.error}</p>
      {error && <button disabled={busy} onClick={retryConnection}>Retry update connection</button>}
      {info?.error && <button disabled={busy} onClick={() => void run(api.checkForUpdates)}>Retry update check</button>}
    </div>}
  </section>;
}
