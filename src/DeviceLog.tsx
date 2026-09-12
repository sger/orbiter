import { useEffect, useRef, useState } from "react";
import { Channel, invoke, isTauri } from "@tauri-apps/api/core";
import { LoaderCircle } from "lucide-react";

type LogLine = { text: string };
type Summary = { matched: number; discarded: number; message: string };

/// Reads the iPhone's log while a person reproduces a failure, keeping only the lines about the
/// app being diagnosed. The rest of the device's log is read and dropped, never shown or stored.
export function DeviceLog({
  deviceId,
  subjects,
  superseded,
  disabled,
}: {
  deviceId: number | null;
  /// What a kept line must mention: the signed build's identifier and its app name.
  subjects: string[];
  /// The identifier this build was made from. Both apps are usually installed side by side and
  /// their processes share a name, so lines naming only that one belong to the other app.
  superseded: string[];
  disabled: boolean;
}) {
  const [lines, setLines] = useState<LogLine[]>([]);
  const [running, setRunning] = useState(false);
  const [summary, setSummary] = useState<Summary | null>(null);
  const [error, setError] = useState<string | null>(null);
  const mounted = useRef(false);
  useEffect(() => {
    mounted.current = true;
    return () => {
      mounted.current = false;
      // A capture must not outlive the panel that asked for it.
      if (isTauri()) void invoke("stop_device_log").catch(() => {});
    };
  }, []);
  const subjectKey = subjects.join(" ");
  useEffect(() => {
    setLines([]);
    setSummary(null);
    setError(null);
  }, [deviceId, subjectKey]);
  const ready = isTauri() && deviceId !== null && subjects.length > 0;
  return (
    <div className="device-log">
      <div className="device-log-head">
        <strong>Device log</strong>
        <button
          className="text-button"
          disabled={disabled || !ready}
          onClick={() => {
            if (running) {
              void invoke("stop_device_log").catch(() => {});
              return;
            }
            setRunning(true);
            setLines([]);
            setSummary(null);
            setError(null);
            const progress = new Channel<LogLine>();
            progress.onmessage = (line) =>
              setLines((previous) => [...previous, line]);
            invoke<Summary>("start_device_log", {
              deviceId,
              subjects,
              superseded,
              progress,
            })
              .then((result) => {
                if (mounted.current) setSummary(result);
              })
              .catch((failure) => {
                if (mounted.current)
                  setError(
                    typeof failure === "string"
                      ? failure
                      : "The iPhone's log could not be read.",
                  );
              })
              .finally(() => {
                if (mounted.current) setRunning(false);
              });
          }}
        >
          {running ? (
            <>
              <LoaderCircle size={14} className="spin" /> Stop capture
            </>
          ) : (
            "Capture while you reproduce it"
          )}
        </button>
      </div>
      <p className="hint">
        {ready
          ? `Open the failing screen on the iPhone while this runs. Only lines mentioning ${subjects.join(" or ")} are kept; everything else the iPhone logs is read and discarded. Nothing is written to disk, and the capture stops after five minutes.`
          : "Sign a build and select the iPhone to capture its log for that app."}
      </p>
      {error && (
        <p className="hint" role="alert">
          {error}
        </p>
      )}
      {lines.length > 0 && (
        <pre className="device-log-lines">
          {lines.map((line) => line.text).join("\n")}
        </pre>
      )}
      {summary && (
        <p className="hint" role="status">
          {summary.message} {summary.matched} line(s) about this app;{" "}
          {summary.discarded} about the rest of the device were discarded.
        </p>
      )}
      {running && lines.length === 0 && (
        <p className="hint" role="status">
          Listening. Nothing about this app yet.
        </p>
      )}
    </div>
  );
}
