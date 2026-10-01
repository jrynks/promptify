import { useEffect, useRef, useState } from "react";
import { createRoot } from "react-dom/client";
import { listen } from "@tauri-apps/api/event";
import { api, type OverlayEvent, type Outcome } from "../api";
import "./overlay.css";

type View =
  | { kind: "idle" }
  | { kind: "listening"; mode: string; profile: string; target: string; latched: boolean }
  | { kind: "working"; label: string }
  | { kind: "result"; tone: "ok" | "warn" | "error"; title: string; body?: string; canCopy: boolean };

const BLOCK_MESSAGES: Record<string, string> = {
  focus_changed: "You switched windows, so nothing was pasted.",
  focus_unknown: "Couldn't confirm the target window, so nothing was pasted.",
  output_truncated: "The result was cut off, so it wasn't pasted.",
  insert_failed: "Couldn't paste into the app.",
};

const FAIL_MESSAGES: Record<string, string> = {
  recording_too_long: "That recording was too long. Try a shorter request.",
  transcription_failed: "Couldn't transcribe the recording.",
  generation_failed: "The prompt model stopped unexpectedly.",
  timed_out: "The prompt model took too long. Try a shorter request or a smaller model.",
  empty_output: "The prompt model returned nothing.",
  engine_busy: "Promptify is busy with another request. Try again in a moment.",
};

function describe(outcome: Outcome, capped: boolean): View {
  const note = capped ? " Recording hit the length limit; later speech was dropped." : "";
  switch (outcome.kind) {
    case "inserted":
      return { kind: "result", tone: "ok", title: "Prompt ready" + note, canCopy: false };
    case "answered":
      return { kind: "result", tone: "ok", title: "Answer" + note, body: outcome.text, canCopy: true };
    case "blocked":
      return { kind: "result", tone: "warn", title: BLOCK_MESSAGES[outcome.reason] + note, body: outcome.text, canCopy: true };
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
  revising: "Fixing the prompt's step structure…",
  inserting: "Pasting…",
};

function Overlay() {
  const [view, setView] = useState<View>({ kind: "idle" });
  const [level, setLevel] = useState(0);
  const [preview, setPreview] = useState("");
  const hideTimer = useRef<number | undefined>(undefined);
  const mode = useRef("prompt");
  // The transcript is shown until the first token arrives; then the draft replaces it.
  const streaming = useRef(false);

  const scheduleHide = (ms: number) => {
    window.clearTimeout(hideTimer.current);
    hideTimer.current = window.setTimeout(() => {
      setView({ kind: "idle" });
      void api.hideOverlay();
    }, ms);
  };

  useEffect(() => {
    const unlisten = listen<OverlayEvent>("overlay-event", ({ payload }) => {
      switch (payload.type) {
        case "listening":
          window.clearTimeout(hideTimer.current);
          if (!payload.latched) setPreview("");
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
        case "finished": {
          const next = describe(payload.report.outcome, payload.capped);
          setView(next);
          if (next.kind === "result" && !next.canCopy) scheduleHide(next.tone === "ok" ? 1200 : next.tone === "error" ? 6000 : 3500);
          break;
        }
        case "error":
          setView({ kind: "result", tone: "error", title: payload.message, canCopy: false });
          scheduleHide(4000);
          break;
        case "cancelled":
          setView({ kind: "result", tone: "warn", title: "Cancelled.", canCopy: false });
          scheduleHide(1200);
          break;
      }
    });
    return () => {
      void unlisten.then((stop) => stop());
    };
  }, []);

  if (view.kind === "idle") return null;

  return (
    <div className={`pill ${view.kind === "result" ? view.tone : ""}`}>
      {view.kind === "listening" && (
        <>
          <div className="meter" aria-hidden>
            <span style={{ transform: `scaleX(${Math.min(1, level * 8)})` }} />
          </div>
          <div className="text">
            <strong>{view.mode === "prompt" ? "Listening for a prompt" : view.mode === "answer" ? "Listening for a question" : "Dictating"}</strong>
            <span>
              {view.profile} · {view.target}
              {view.latched ? " · press the hotkey again to finish" : " · release to finish"} · Esc cancels
            </span>
            {preview && <span className="preview">{preview}</span>}
          </div>
        </>
      )}
      {view.kind === "working" && (
        <div className="text">
          <strong>{view.label}</strong>
          {preview && <span className="preview">{preview}</span>}
        </div>
      )}
      {view.kind === "result" && (
        <div className="text">
          <strong>{view.title}</strong>
          {view.body && <span className="preview">{view.body}</span>}
          {view.canCopy && (
            <div className="actions">
              <button onClick={() => void api.copyLastResult().then(() => scheduleHide(600))}>Copy</button>
              <button onClick={() => scheduleHide(0)}>Dismiss</button>
            </div>
          )}
        </div>
      )}
    </div>
  );
}

createRoot(document.getElementById("root")!).render(<Overlay />);
