import { useState } from "react";

export function RecoveryButton({ children, action, onComplete }: {
  children: React.ReactNode;
  action: () => Promise<unknown>;
  onComplete?: () => void;
}) {
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const run = async () => {
    setBusy(true);
    setError(null);
    try {
      await action();
      onComplete?.();
    } catch (reason) {
      setError(String(reason));
    } finally {
      setBusy(false);
    }
  };
  return <span>
    <button disabled={busy} onClick={() => void run()}>{children}</button>
    {error && <span className="error" role="alert">{error}</span>}
  </span>;
}
