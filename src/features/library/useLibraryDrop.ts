import { useEffect, useRef, useState } from "react";
import { getCurrentWebview } from "@tauri-apps/api/webview";
import { isTauri } from "../../ipc/commands";

export function useLibraryDrop(
  enabled: boolean,
  onDrop: (paths: string[]) => void,
  onError: (error: string) => void,
) {
  const [dragging, setDragging] = useState(false);
  const handlers = useRef({ onDrop, onError });
  handlers.current = { onDrop, onError };
  useEffect(() => {
    setDragging(false);
    if (!enabled) return;
    // Browser file drops must not navigate away from Orbiter. Native events provide the paths.
    const prevent = (event: DragEvent) => event.preventDefault();
    window.addEventListener("dragover", prevent);
    window.addEventListener("drop", prevent);
    let disposed = false;
    let cleanup: (() => void) | undefined;
    if (isTauri()) {
      getCurrentWebview()
        .onDragDropEvent((event) => {
          // Hash navigation can happen before React cleans up the previous page's listener.
          const hash = window.location.hash;
          const libraryRoute =
            !hash ||
            hash === "#/ipas" ||
            /^#\/ipas\/(?!workspace$)[^/]+$/.test(hash);
          if (disposed || !libraryRoute) return;
          setDragging(
            event.payload.type === "enter" || event.payload.type === "over",
          );
          if (event.payload.type === "drop")
            handlers.current.onDrop(event.payload.paths);
        })
        .then((unlisten) => {
          if (disposed) unlisten();
          else cleanup = unlisten;
        })
        .catch(() => {
          if (!disposed)
            handlers.current.onError(
              "Drag and drop is unavailable. Use Import IPA to choose files.",
            );
        });
    }
    return () => {
      disposed = true;
      cleanup?.();
      window.removeEventListener("dragover", prevent);
      window.removeEventListener("drop", prevent);
    };
  }, [enabled]);
  return dragging;
}
