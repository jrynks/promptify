import { useEffect, useRef, useState } from "react";
import { createRoot } from "react-dom/client";
import { listen } from "@tauri-apps/api/event";
import { api, type OverlayEvent, type Outcome, type PromptRouting } from "../api";
import "./overlay.css";

type View =
  | { kind: "idle" }
  | { kind: "listening"; mode: string; profile: string; target: string; latched: boolean }
  | { kind: "working"; label: string }
  | { kind: "result"; tone: "ok" | "warn" | "error"; title: string; body?: string; detail?: string; canCopy: boolean; canRestore?: boolean };

const BLOCK_MESSAGES: Record<string, string> = {
  focus_changed: "You switched windows, so nothing was pasted.",
  focus_unknown: "Couldn't confirm the target window, so nothing was pasted.",
  output_truncated: "The result was cut off, so it wasn't pasted.",
  insert_failed: "Couldn't paste into the app.",
  surface_unconfirmed: "AI input not confirmed. Review and copy this prompt.",
  graph_unsupported: "This graph needs a capable AI assistant. Review before copying.",
};

const FAIL_MESSAGES: Record<string, string> = {
  recording_too_long: "That recording was too long. Try a shorter request.",
  transcription_failed: "Couldn't transcribe the recording.",
  generation_failed: "The prompt model stopped unexpectedly.",
  invalid_prompt: "Couldn't produce a valid prompt for this input.",
  timed_out: "The prompt model took too long. Try a shorter request or a smaller model.",
  empty_output: "The prompt model returned nothing.",
  engine_busy: "Promptify is busy with another request. Try again in a moment.",
};

function describe(outcome: Outcome, capped: boolean): View {
  const note = capped ? " Recording hit the length limit; later speech was dropped." : "";
  switch (outcome.kind) {
    case "inserted":
      return { kind: "result", tone: "ok", title: "Paste sent" + note, detail: "Check the destination: sending a paste does not verify that the application received it. Generated text stays on the clipboard until you restore it after checking.", canCopy: true, canRestore: true };
    case "answered":
      return { kind: "result", tone: "ok", title: "Answer" + note, body: outcome.text, canCopy: true };
    case "blocked":
      return { kind: "result", tone: "warn", title: BLOCK_MESSAGES[outcome.reason] + note, body: outcome.text, detail: outcome.detail ?? undefined, canCopy: true };
    case "no_speech":
      return { kind: "result", tone: "warn", title: "Didn't catch any speech.", canCopy: false };
    case "cancelled":
      return { kind: "result", tone: "warn", title: "Cancelled.", canCopy: false };
    case "failed":
      return {
        kind: "result",
        tone: "error",
        title: FAIL_MESSAGES[outcome.reason] ?? "Something went wrong.",
        body: outcome.detail ?? undefined,
        canCopy: false,
      };
  }
}

const STAGE_LABELS: Record<string, string> = {
  transcribing: "Transcribing…",
  researching: "Looking up context…",
  generating: "Writing your prompt…",
  revising: "Checking the prompt's format…",
  inserting: "Pasting…",
};

function Overlay() {
  const [view, setView] = useState<View>({ kind: "idle" });
  const [level, setLevel] = useState(0);
  const [preview, setPreview] = useState("");
  const [routing, setRouting] = useState<PromptRouting | null>(null);
  const [uiError, setUiError] = useState<string | null>(null);
  const [copying, setCopying] = useState(false);
  const [restoring, setRestoring] = useState(false);
  const hideTimer = useRef<number | undefined>(undefined);
  const streamPreview = useRef<HTMLDivElement>(null);
  const eventVersion = useRef(0);
  const mode = useRef("prompt");
  // The transcript is shown until the first token arrives; then the draft replaces it.
  const streaming = useRef(false);

  const scheduleHide = (ms: number) => {
    const version = eventVersion.current;
    window.clearTimeout(hideTimer.current);
    hideTimer.current = window.setTimeout(() => {
      if (version !== eventVersion.current) return;
      void api.hideOverlay().then(() => {
        if (version === eventVersion.current) setView({ kind: "idle" });
      }, (error) => {
        if (version === eventVersion.current) setUiError(`Could not dismiss the overlay: ${String(error)}`);
      });
    }, ms);
  };

  const visible = view.kind !== "idle";
  useEffect(() => {
    if (!visible) return;
    const root = document.getElementById("root");
    if (!root) return;
    let frame = 0;
    let lastHeight = 0;
    let disposed = false;
    const measure = () => {
      window.cancelAnimationFrame(frame);
      frame = window.requestAnimationFrame(() => {
        const height = Math.ceil(root.getBoundingClientRect().height);
        if (height <= 0 || height === lastHeight) return;
        lastHeight = height;
        void api.resizeOverlay(height).catch((error) => {
          if (!disposed) setUiError(`Could not fit the overlay to its content: ${String(error)}`);
        });
      });
    };
    const updateScreenLimit = () => {
      void api.overlayMaxHeight().then((height) => {
        if (disposed) return;
        root.style.setProperty("--overlay-max-height", `${Math.max(1, height - 16)}px`);
        measure();
      }, (error) => {
        if (!disposed) setUiError(`Could not determine the available overlay height: ${String(error)}`);
      });
    };
    const observer = new ResizeObserver(measure);
    observer.observe(root);
    window.addEventListener("resize", updateScreenLimit);
    updateScreenLimit();
    return () => {
      disposed = true;
      window.cancelAnimationFrame(frame);
      observer.disconnect();
      window.removeEventListener("resize", updateScreenLimit);
    };
  }, [visible]);

  useEffect(() => {
    if ((view.kind === "working" || view.kind === "listening") && streamPreview.current) {
      streamPreview.current.scrollTop = streamPreview.current.scrollHeight;
    }
  }, [preview, view.kind]);

  const restoreClipboard = async () => {
    const version = eventVersion.current;
    window.clearTimeout(hideTimer.current);
    setRestoring(true);
    setUiError(null);
    try {
      await api.restoreInsertionClipboard();
      if (version === eventVersion.current) {
        setView((current) => current.kind === "result" ? { ...current, canRestore: false, detail: "Previous clipboard restored." } : current);
      }
    } catch (error) {
      if (version === eventVersion.current) setUiError(`Could not restore the clipboard: ${String(error)}`);
    } finally {
      setRestoring(false);
    }
  };

  const copyResult = async () => {
    const version = eventVersion.current;
    setCopying(true);
    setUiError(null);
    try {
      await api.copyLastResult();
      if (version === eventVersion.current) scheduleHide(600);
    } catch (error) {
      if (version === eventVersion.current) setUiError(`Could not copy the prompt: ${String(error)}`);
    } finally {
      setCopying(false);
    }
  };

  useEffect(() => {
    let disposed = false;
    let stopListening: (() => void) | undefined;
    const unlisten = listen<OverlayEvent>("overlay-event", ({ payload }) => {
      switch (payload.type) {
        case "listening":
          eventVersion.current += 1;
          window.clearTimeout(hideTimer.current);
          if (!payload.latched) setPreview("");
          setUiError(null);
          setRouting(null);
          mode.current = payload.mode;
          setView({ kind: "listening", mode: payload.mode, profile: payload.profile, target: payload.target, latched: payload.latched });
          break;
        case "level":
          setLevel(payload.level);
          break;
        case "partial":
          setPreview(payload.text);
          break;
        case "stage":
          if (payload.stage === "revising") setPreview("");
          if (payload.stage === "generating" || payload.stage === "revising") streaming.current = false;
          setView({ kind: "working", label: payload.stage === "generating" && mode.current === "answer" ? "Answering…" : STAGE_LABELS[payload.stage] });
          break;
        case "transcript":
          streaming.current = false;
          setPreview(payload.text);
          break;
        case "token":
          if (streaming.current) {
            setPreview((prev) => prev + payload.text);
          } else {
            streaming.current = true;
            setPreview(payload.text);
          }
          break;
        case "routing":
          setRouting(payload.routing);
          break;
        case "finished": {
          eventVersion.current += 1;
          window.clearTimeout(hideTimer.current);
          setUiError(null);
          setRouting(payload.report.routing ?? null);
          const next = describe(payload.report.outcome, payload.capped);
          setView(next);
          if (payload.report.outcome.kind === "inserted") scheduleHide(3500);
          else if (next.kind === "result" && !next.canCopy) scheduleHide(next.tone === "ok" ? 1200 : next.tone === "error" ? 6000 : 3500);
          break;
        }
        case "error":
          eventVersion.current += 1;
          setUiError(null);
          setView({ kind: "result", tone: "error", title: payload.message, canCopy: false });
          scheduleHide(4000);
          break;
        case "cancelled":
          eventVersion.current += 1;
          setUiError(null);
          setView({ kind: "result", tone: "warn", title: "Cancelled.", canCopy: false });
          scheduleHide(1200);
          break;
      }
    });
    void unlisten.then((stop) => {
      if (disposed) stop();
      else stopListening = stop;
    }, (error) => {
        if (!disposed) {
          setView({ kind: "result", tone: "error", title: "The overlay could not connect to Promptify.", canCopy: false });
          setUiError(String(error));
        }
      });
    return () => {
      disposed = true;
      window.clearTimeout(hideTimer.current);
      stopListening?.();
    };
  }, []);

  if (view.kind === "idle") return null;

  return (
    <div className={`pill ${view.kind === "result" ? view.tone : ""}`}>
      {view.kind === "listening" && <div className="meter" aria-hidden>
        <span style={{ transform: `scaleX(${Math.min(1, level * 8)})` }} />
      </div>}
      <div className="text">
        {view.kind === "listening" && <>
            <strong role="status">{view.mode === "prompt" ? "Listening for a prompt" : view.mode === "answer" ? "Listening for a question" : "Dictating"}</strong>
            <span className="metadata">
              {view.profile} · {view.target}
              {view.latched ? " · press the hotkey again to finish" : " · release to finish"} · Esc cancels
            </span>
            {preview && <div ref={streamPreview} className="preview" role="region" aria-label="Live transcript" tabIndex={0}>{preview}</div>}
        </>}
        {view.kind === "working" && <>
          <strong role="status">{view.label}</strong>
          {routing && <span className="metadata" title={routing.warnings.join(" ")}>{routing.target_name} / {routing.task_type} / {routing.form.replaceAll("_", " ")}{!routing.auto_paste ? " / review before copying" : ""}</span>}
          {preview && <div ref={streamPreview} className="preview" role="region" aria-label="Prompt preview" tabIndex={0}>{preview}</div>}
        </>}
        {view.kind === "result" && <>
          <strong role="status">{view.title}</strong>
          <div className="result-content" role="region" aria-label="Prompt result" tabIndex={0}>
            {view.detail && <span className="metadata">{view.detail}</span>}
            {routing && <span className="metadata" title={routing.warnings.join(" ")}>{routing.task_type} / {routing.surface.replaceAll("_", " ")}</span>}
            {view.body && <div className="result-text">{view.body}</div>}
          </div>
        </>}
        {uiError && <div role="alert" className="ui-error">{uiError}</div>}
        {view.kind === "result" && view.canCopy && (
            <div className="actions">
              <button disabled={copying} onClick={() => void copyResult()}>{copying ? "Copying..." : "Copy"}</button>
              {view.canRestore && <button disabled={restoring} onClick={() => void restoreClipboard()}>Restore previous clipboard</button>}
              <button onClick={() => scheduleHide(0)}>Dismiss</button>
            </div>
        )}
      </div>
    </div>
  );
}

createRoot(document.getElementById("root")!).render(<Overlay />);
