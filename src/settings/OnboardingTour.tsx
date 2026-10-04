import { useEffect, useRef, useState } from "react";
import { listen } from "@tauri-apps/api/event";
import { api, type AppInfo, type EngineStatus, type OnboardingStatus, type OnboardingStep, type PracticeEvent } from "../api";
import { DesktopIntegration } from "./DesktopIntegration";
import { RecoveryButton } from "./RecoveryButton";

const STEPS: { id: OnboardingStep; label: string }[] = [
  { id: "models", label: "Local models" },
  { id: "input", label: "Microphone and shortcut" },
  { id: "practice", label: "Your first prompt" },
];

function engineLabel(engine: EngineStatus): string {
  switch (engine.state) {
    case "missing": return "Download and select a model below";
    case "loading": return "Loading the selected model...";
    case "ready": return "Loaded successfully";
    case "error": return engine.message;
  }
}

interface Receipt {
  attempt: number;
  job: number;
  text: string;
}

export function OnboardingTour({ status, info, onChange }: { status: OnboardingStatus; info: AppInfo; onChange: () => void }) {
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const [listening, setListening] = useState(false);
  const [listenerError, setListenerError] = useState<string | null>(null);
  const [listenerAttempt, setListenerAttempt] = useState(0);
  const [level, setLevel] = useState(0);
  const [transcript, setTranscript] = useState("");
  const [activity, setActivity] = useState("");
  const [value, setValue] = useState("");
  const [pasted, setPasted] = useState<string | null>(null);
  const [receipt, setReceipt] = useState<Receipt | null>(null);
  const field = useRef<HTMLTextAreaElement>(null);
  const heading = useRef<HTMLHeadingElement>(null);
  const attempt = useRef<number | null>(null);
  const confirming = useRef<string | null>(null);
  const activeStatus = useRef(status);
  activeStatus.current = status;

  useEffect(() => {
    const stopPractice = () => {
      void api.disarmOnboardingPractice().catch((reason) => console.error("Could not stop practice while leaving setup:", reason));
    };
    window.addEventListener("pagehide", stopPractice);
    return () => {
      window.removeEventListener("pagehide", stopPractice);
      stopPractice();
    };
  }, []);

  useEffect(() => {
    heading.current?.focus();
    setError(null);
  }, [status.step]);

  useEffect(() => {
    let disposed = false;
    let stop: (() => void) | undefined;
    setListening(false);
    setListenerError(null);
    void listen<PracticeEvent>("onboarding-practice", ({ payload }) => {
      if (disposed || payload.attempt_id !== attempt.current) return;
      const event = payload.event;
      switch (event.type) {
        case "listening": setActivity("Listening. Speak your request, then press the shortcut again."); break;
        case "level": setLevel(Math.max(0, Math.min(1, event.level))); break;
        case "partial":
        case "transcript": setTranscript(event.text); break;
        case "stage": setActivity(event.stage === "inserting" ? "Pasting into this field..." : `${event.stage}...`); break;
        case "finished":
          setLevel(0);
          if (event.report.outcome.kind === "inserted") {
            setReceipt({ attempt: payload.attempt_id, job: event.report.job_id, text: event.report.outcome.text });
            setActivity("Checking the pasted result...");
          } else {
            setActivity("Practice did not finish successfully. Check the message below and try again.");
          }
          onChange();
          break;
        case "error": setError(event.message); setLevel(0); onChange(); break;
        case "cancelled": setActivity("Cancelled. You can try again."); setLevel(0); onChange(); break;
        case "token": break;
      }
    }).then((unlisten) => {
      if (disposed) unlisten();
      else { stop = unlisten; setListening(true); }
    }, (reason) => {
      if (!disposed) setListenerError(`Could not listen for practice results: ${String(reason)}`);
    });
    return () => { disposed = true; stop?.(); };
  }, [onChange, listenerAttempt]);

  useEffect(() => {
    if (!receipt || status.practice.attempt_id !== receipt.attempt) return;
    const normalize = (text: string) => text.replace(/\r\n/g, "\n");
    if (pasted === null || normalize(pasted) !== normalize(receipt.text) || normalize(value) !== normalize(receipt.text)) return;
    const key = `${receipt.attempt}:${receipt.job}`;
    if (confirming.current === key) return;
    confirming.current = key;
    void api.confirmOnboardingPaste(receipt.attempt, receipt.job, value).then(() => {
      setActivity("Verified: your voice became a prompt and was pasted successfully.");
      onChange();
    }, (reason) => {
      setError(String(reason));
      setActivity("The result could not be verified. Try practice again.");
    });
  }, [receipt, pasted, value, status.practice.attempt_id, onChange]);

  const act = async (action: () => Promise<unknown>) => {
    if (busy) return;
    setBusy(true);
    setError(null);
    try {
      await action();
      onChange();
    } catch (reason) {
      setError(String(reason));
    } finally {
      setBusy(false);
    }
  };

  const arm = () => act(async () => {
    setReceipt(null);
    setPasted(null);
    setValue("");
    setTranscript("");
    setLevel(0);
    confirming.current = null;
    field.current?.focus();
    const next = await api.armOnboardingPractice();
    if (document.activeElement !== field.current || !document.hasFocus()) {
      await api.disarmOnboardingPractice();
      throw new Error("Practice lost focus while preparing. Keep the practice field focused and try again.");
    }
    attempt.current = next.practice.attempt_id;
    setActivity("Ready. Press your Prompt shortcut to start speaking.");
  });

  const disarm = () => {
    if (activeStatus.current.practice.phase === "passed") return;
    void api.disarmOnboardingPractice().then(onChange, (reason) => setError(`Could not stop practice: ${String(reason)}`));
  };

  const index = STEPS.findIndex((step) => step.id === status.step);
  const failedLoad = status.speech.state === "error" || status.language.state === "error";
  const inputReady = info.engines_ready && !!info.input_device && !info.prompt_hotkey_error && !info.hotkeys_paused && info.paste_permission !== "required" && !info.desktop_error && info.desktop_integration_enabled !== false;
  const passed = status.practice.phase === "passed";
  const modelReady = info.engines_ready && status.speech.state === "ready" && status.language.state === "ready";
  const active = status.practice.phase === "recording" || status.practice.phase === "processing";
  const shortcut = info.activation_bindings?.find((binding) => binding.id === "prompt")?.trigger_description
    ?? info.hotkeys.prompt.replace("CommandOrControl", navigator.userAgent.includes("Mac") ? "Cmd" : "Ctrl");
  const microphoneHelp = navigator.userAgent.includes("Windows")
    ? "In Windows Settings, check System > Sound > Input and Privacy & security > Microphone, including desktop app access."
    : navigator.userAgent.includes("Mac")
      ? "In System Settings, check Sound > Input and Privacy & Security > Microphone and Accessibility."
      : "In your system sound settings, choose a default input and allow microphone access. Desktop shortcuts and insertion also require a supported desktop session.";

  if (status.startup_error) {
    return (
      <section className="tour" aria-labelledby="setup-recovery-title">
        <h1 id="setup-recovery-title">Setup needs attention</h1>
        <p role="alert" className="error">{status.startup_error}</p>
        <p>Check the settings file and folder permissions. Your existing configuration will not be replaced with defaults. After fixing the problem, retry to restart Promptify safely.</p>
        <button disabled={busy} onClick={() => void act(api.retryOnboardingStartup)}>Retry startup</button>
        <RecoveryButton action={api.openDataFolder}>Open app data folder</RecoveryButton>
        {error && <p role="alert" className="error">{error}</p>}
      </section>
    );
  }

  return (
    <section className="tour" aria-labelledby="setup-title">
      <p className="hint">Welcome to Promptify - required first-time setup</p>
      <ol className="tour-steps" aria-label="Setup progress">
        {STEPS.map((step, i) => <li key={step.id} aria-current={i === index ? "step" : undefined}>{i + 1}. {step.label}</li>)}
      </ol>
      <h1 id="setup-title" ref={heading} tabIndex={-1}>{STEPS[index].label}</h1>
      {status.step === "models" && (
        <>
          <p>Everything runs on this computer. No account or API key is needed. Choose a speech model and a prompt model below, then let them load. <strong>Recommended</strong> marks the highest quality tier supported by your RAM.</p>
          <p className="hint">Quality models may be slower and need more memory. Smaller and multilingual choices remain available. Internet is needed for downloads, not for dictation or prompt writing.</p>
          <ul className="tour-checks" aria-live="polite">
            <li>Speech: {engineLabel(status.speech)}</li>
            <li>Prompt writer: {engineLabel(status.language)}</li>
          </ul>
          {failedLoad && <div className="tour-actions">
            <button disabled={busy} onClick={() => void act(api.retryModelLoading)}>Retry model loading</button>
            {info.use_gpu && <button disabled={busy} onClick={() => void act(() => api.setUseGpu(false))}>Try loading on CPU</button>}
          </div>}
          <button className="primary" disabled={busy || !modelReady} onClick={() => void act(() => api.setOnboardingStep("input"))}>Continue to microphone and shortcut</button>
        </>
      )}
      {status.step === "input" && (
        <>
          <p>Check the highlighted microphone and Prompt shortcut below. Change the shortcut if another app has claimed it. We will test real recording and pasting next.</p>
          <p>{info.input_device ? `Detected: ${info.input_device}. Recording access has not been tested yet.` : "No default microphone was detected. Connect or enable one, then refresh."}</p>
          <p className="hint">{microphoneHelp}</p>
          <div className="tour-actions">
            <RecoveryButton action={() => api.openSystemSettings("microphone")} onComplete={onChange}>Open microphone settings</RecoveryButton>
            <button disabled={busy} onClick={onChange}>Refresh microphone and shortcut</button>
            {info.hotkeys_paused && <button disabled={busy} onClick={() => void act(api.resumeOnboardingHotkeys)}>Resume shortcuts</button>}
          </div>
          {info.prompt_hotkey_error && <p className="error" role="alert">{info.prompt_hotkey_error}</p>}
          <DesktopIntegration info={info} onChange={onChange} />
          <div className="tour-actions">
            <button disabled={busy} onClick={() => void act(() => api.setOnboardingStep("models"))}>Back to models</button>
            <button className="primary" disabled={busy || !inputReady} onClick={() => void act(() => api.setOnboardingStep("practice"))}>Continue to practice</button>
          </div>
        </>
      )}
      {status.step === "practice" && (
        <>
          <p>Choose <strong>Prepare practice</strong>, then press <kbd>{shortcut}</kbd>, say <em>&quot;Write a friendly greeting for a new colleague&quot;</em>, and press the same shortcut again. You can also hold to talk. Press <kbd>Esc</kbd> to cancel.</p>
          <p className="hint">Keep the practice field focused until the prompt appears. This exercise stays local and does not use connected tools or save its text to history.</p>
          {!inputReady && <p role="alert" className="error">The required input or model configuration changed. Go back to check it before trying again.</p>}
          {(!inputReady && (info.paste_permission === "required" || info.desktop_error || info.desktop_integration_enabled === false)) && <DesktopIntegration info={info} onChange={onChange} />}
          <button disabled={busy || active || !inputReady || !listening} onClick={() => void arm()}>Prepare practice</button>
          {active && <RecoveryButton action={api.disarmOnboardingPractice} onComplete={onChange}>Cancel practice</RecoveryButton>}
          <label className="practice-label" htmlFor="setup-practice">Practice field
            <textarea
              id="setup-practice"
              ref={field}
              rows={5}
              value={value}
              placeholder="Your spoken request will become a prompt and be pasted here."
              onChange={(event) => setValue(event.target.value)}
              onPaste={(event) => setPasted(event.clipboardData.getData("text/plain"))}
              onBlur={disarm}
              spellCheck={false}
              aria-describedby="practice-status"
            />
          </label>
          <label className="hint">Microphone level <meter min={0} max={1} value={level} /></label>
          <p id="practice-status" role="status">{passed ? "Success! Your microphone, shortcut, models, and paste are working." : activity || "Prepare the field to begin. Typing here does not complete the exercise."}</p>
          {transcript && <p><strong>Heard:</strong> {transcript}</p>}
          {receipt && !passed && (pasted === null || pasted.replace(/\r\n/g, "\n") !== receipt.text.replace(/\r\n/g, "\n") || value.replace(/\r\n/g, "\n") !== receipt.text.replace(/\r\n/g, "\n")) &&
            <p className="warn">The model produced a result, but no matching paste reached this field. Keep it focused and try again.</p>}
          <p className="hint">{microphoneHelp}</p>
          <div className="tour-actions">
            <button disabled={busy || active} onClick={() => void act(() => api.setOnboardingStep(info.engines_ready ? "input" : "models"))}>Back to setup checks</button>
            <button className="primary" disabled={busy || !passed || !inputReady} onClick={() => void act(api.completeOnboarding)}>Finish setup</button>
          </div>
          {passed && <p>Promptify will keep running in the system tray. Use the Prompt shortcut in a text box, the Dictation shortcut for plain speech, and Esc to cancel. All settings remain available from the tray.</p>}
        </>
      )}
      {listenerError && <div role="alert" className="error"><p>{listenerError}</p><button onClick={() => setListenerAttempt((value) => value + 1)}>Retry practice connection</button></div>}
      {(error || status.practice.error) && <p role="alert" className="error">{error || status.practice.error}</p>}
      <p className="hint tour-foot">You can close this window or quit from the tray. Setup resumes when you return; closing it does not complete setup.</p>
    </section>
  );
}
