import { useEffect, useRef, useState } from "react";
import {
  channel,
  isTauri,
  startDeviceLog,
  stopDeviceLog,
} from "../../ipc/commands";
import type { LogLine, LogSummary } from "../../types";
import { LoaderCircle } from "lucide-react";
import { Checkbox } from "../../components/ui/Checkbox";

/// A line iOS marked as a failure. Severity is carried in the line text as `<Error>`/`<Fault>`,
/// so the filter reads it there rather than from a separate field the capture does not provide.
const isFailure = (text: string) => /<(Error|Fault)>/.test(text);

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
  const [summary, setSummary] = useState<LogSummary | null>(null);
  const [error, setError] = useState<string | null>(null);
  // Show only lines iOS marked as a failure. Off by default so a capture reads in full; on, it
  // cuts the RunningBoard/lifecycle noise down to what actually went wrong.
  const [errorsOnly, setErrorsOnly] = useState(false);
  const mounted = useRef(false);
  useEffect(() => {
    mounted.current = true;
    return () => {
      mounted.current = false;
      // A capture must not outlive the panel that asked for it.
      if (isTauri()) void stopDeviceLog().catch(() => {});
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
    <div className="mt-[18px] border-t border-line-soft pt-4">
      <div className="mb-1.5 flex items-center justify-between gap-3">
        <strong>Device log</strong>
        <button
          className="text-button"
          disabled={disabled || !ready}
          onClick={() => {
            if (running) {
              void stopDeviceLog().catch(() => {});
              return;
            }
            setRunning(true);
            setLines([]);
            setSummary(null);
            setError(null);
            const progress = channel<LogLine>((line) =>
              setLines((previous) => [...previous, line]),
            );
            startDeviceLog(deviceId!, subjects, superseded, progress)
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
          ? `Open the failing screen on the iPhone while this runs. Only lines mentioning ${subjects.join(" or ")} are kept; everything else the iPhone logs is read and discarded. Nothing is written to disk, and the capture stops after five minutes. This shows the system and framework messages about the app; the app's own NSLog or os_log output may not appear here — read that in Console.app's unified log.`
          : "Sign a build and select the iPhone to capture its log for that app."}
      </p>
      {error && (
        <p className="hint" role="alert">
          {error}
        </p>
      )}
      {lines.length > 0 &&
        (() => {
          const failures = lines.filter((line) => isFailure(line.text));
          const shown = errorsOnly ? failures : lines;
          return (
            <>
              <div className="mt-2.5 flex items-center justify-between gap-3">
                <Checkbox
                  checked={errorsOnly}
                  onChange={(event) => setErrorsOnly(event.target.checked)}
                >
                  Errors and faults only
                </Checkbox>
                <span className="text-caption text-ink-faint">
                  {errorsOnly
                    ? `${failures.length} of ${lines.length}`
                    : `${failures.length} error(s)`}
                </span>
              </div>
              {shown.length > 0 ? (
                <pre className="mt-2.5 max-h-[260px] overflow-auto rounded-control border border-line bg-surface-soft px-3 py-2.5 text-caption leading-relaxed break-words whitespace-pre-wrap">
                  {shown.map((line) => line.text).join("\n")}
                </pre>
              ) : (
                <p className="hint" role="status">
                  No errors or faults among the {lines.length} line(s) captured.
                </p>
              )}
            </>
          );
        })()}
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
