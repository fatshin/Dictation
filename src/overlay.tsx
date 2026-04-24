import { createRoot } from "react-dom/client";
import { useEffect, useState } from "react";
import { listen } from "@tauri-apps/api/event";

type SessionInfo = {
  id: string;
  stage: string | { error: { stage: string; message: string } };
};

function Overlay() {
  const [stage, setStage] = useState<string>("idle");
  const [transcript, setTranscript] = useState("");
  const [rewrite, setRewrite] = useState("");
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    const unsubs: (() => void)[] = [];

    listen<SessionInfo>("session:state", (e) => {
      const s = e.payload.stage;
      if (typeof s === "string") {
        setStage(s);
        setError(null);
      } else if (s && typeof s === "object" && "error" in s) {
        setStage("error");
        setError(s.error.message);
      }
    }).then((u) => unsubs.push(u));

    listen<string>("asr:final", (e) => {
      setTranscript(e.payload);
    }).then((u) => unsubs.push(u));

    listen<string>("asr:partial", (e) => {
      setTranscript(e.payload);
    }).then((u) => unsubs.push(u));

    listen<string>("llm:token", (e) => {
      setRewrite((prev) => prev + e.payload);
    }).then((u) => unsubs.push(u));

    listen<string>("llm:done", () => {
      setStage("done");
    }).then((u) => unsubs.push(u));

    return () => {
      unsubs.forEach((u) => u());
    };
  }, []);

  const stageLabel: Record<string, string> = {
    idle: "",
    consent_pending: "Waiting for consent...",
    recording: "🎙 Listening...",
    transcribing: "Transcribing...",
    rewriting: "Rewriting...",
    injecting: "Injecting...",
    done: "Done",
    error: "Error",
  };

  return (
    <div
      style={{
        width: 400,
        height: 120,
        background: "rgba(30, 30, 30, 0.92)",
        borderRadius: 12,
        padding: 12,
        color: "#fff",
        fontFamily: "system-ui, sans-serif",
        fontSize: 13,
        display: "flex",
        flexDirection: "column",
        gap: 4,
        overflow: "hidden",
      }}
    >
      <div
        style={{
          fontSize: 11,
          opacity: 0.7,
          display: "flex",
          justifyContent: "space-between",
        }}
      >
        <span>Dictation</span>
        <span>{stageLabel[stage] || stage}</span>
      </div>

      {error ? (
        <div style={{ color: "#ff6b6b", fontSize: 12 }}>{error}</div>
      ) : (
        <>
          <div
            style={{
              flex: 1,
              overflow: "hidden",
              textOverflow: "ellipsis",
              whiteSpace: "nowrap",
              opacity: 0.8,
            }}
          >
            {transcript || "—"}
          </div>
          <div
            style={{
              flex: 1,
              overflow: "hidden",
              textOverflow: "ellipsis",
              whiteSpace: "nowrap",
              fontWeight: 600,
            }}
          >
            {rewrite || "—"}
          </div>
        </>
      )}
    </div>
  );
}

const root = document.getElementById("root");
if (root) {
  createRoot(root).render(<Overlay />);
}
