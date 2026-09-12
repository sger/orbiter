import {
  useCallback,
  useEffect,
  useRef,
  useState,
  type RefObject,
} from "react";
import { getCurrentWebview } from "@tauri-apps/api/webview";
import { open } from "@tauri-apps/plugin-dialog";
import {
  cancelInspection,
  channel,
  inspectIpa,
  isTauri,
} from "../../ipc/commands";
import type { Report } from "../../types";

export function useInspection(installActive: RefObject<boolean>) {
  const [report, setReport] = useState<Report | null>(null),
    [busy, setBusy] = useState(false),
    [stage, setStage] = useState(""),
    [error, setError] = useState(""),
    [drag, setDrag] = useState(false),
    [cancelled, setCancelled] = useState(false);
  const [ipaPath, setIpaPath] = useState<string | null>(null);
  const active = useRef(false);
  const desktop = isTauri();
  const inspect = useCallback(async (path: string) => {
    if (active.current || installActive.current) return;
    active.current = true;
    setBusy(true);
    setReport(null);
    setIpaPath(null);
    setError("");
    setCancelled(false);
    setStage("Checking archive");
    const progress = channel<string>(setStage);
    try {
      setReport(await inspectIpa(path, progress));
      setStage("Inspection complete");
      setIpaPath(path);
    } catch (e) {
      setError(String(e));
      setStage("Inspection stopped");
    } finally {
      active.current = false;
      setBusy(false);
    }
  }, []);
  useEffect(() => {
    if (!desktop) return;
    let disposed = false;
    let cleanup: (() => void) | undefined;
    getCurrentWebview()
      .onDragDropEvent((event) => {
        if (disposed) return;
        setDrag(event.payload.type === "over");
        if (event.payload.type === "drop") {
          if (event.payload.paths.length !== 1) {
            setError("Drop one IPA at a time.");
            return;
          }
          void inspect(event.payload.paths[0]);
        }
      })
      .then((fn) => {
        if (disposed) fn();
        else cleanup = fn;
      })
      .catch(() => setError("Drag and drop is unavailable. Use Choose IPA."));
    return () => {
      disposed = true;
      cleanup?.();
    };
  }, [desktop, inspect]);
  async function choose() {
    try {
      const p = await open({
        multiple: false,
        directory: false,
        filters: [{ name: "iOS application", extensions: ["ipa"] }],
      });
      if (typeof p === "string") await inspect(p);
    } catch {
      setError(
        "The file picker could not open. Try dropping a local IPA into this window.",
      );
    }
  }
  async function cancel() {
    try {
      await cancelInspection();
      setCancelled(true);
    } catch {
      setError(
        "Could not request cancellation. Wait for inspection to finish.",
      );
    }
  }
  return {
    report,
    busy,
    stage,
    error,
    setError,
    drag,
    cancelled,
    ipaPath,
    desktop,
    choose,
    cancel,
  };
}
