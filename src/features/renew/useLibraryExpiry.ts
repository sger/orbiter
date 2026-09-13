import { useCallback, useEffect, useState } from "react";
import { isTauri, libraryExpiry } from "../../ipc/commands";
import type { LibraryExpiry } from "../library/types";

/// Where the seven days stand for the build on screen.
///
/// The clock is the only thing that moves, and it moves slowly: a standing changes at a
/// twenty-four-hour boundary, so this re-reads every ten minutes, whenever the window is looked at
/// again, and whenever the library changes — which is the moment an install or a re-sign makes a
/// new record exist. A tighter interval would burn a timer to show the same sentence. Nothing here
/// reaches Apple or the phone; it is a local file and a clock read in Rust.
const INTERVAL = 10 * 60 * 1000;

export function useLibraryExpiry(
  artifactId: string | null,
  teamId: string | null,
) {
  const [expiry, setExpiry] = useState<LibraryExpiry | null>(null);
  const refresh = useCallback(() => {
    if (!isTauri() || !artifactId) {
      setExpiry(null);
      return;
    }
    // A note about an expiry must never be the reason something breaks: failure is silence.
    libraryExpiry(artifactId, teamId)
      .then(setExpiry)
      .catch(() => setExpiry(null));
  }, [artifactId, teamId]);

  useEffect(() => {
    refresh();
    const timer = window.setInterval(refresh, INTERVAL);
    window.addEventListener("focus", refresh);
    window.addEventListener("library-changed", refresh);
    return () => {
      window.clearInterval(timer);
      window.removeEventListener("focus", refresh);
      window.removeEventListener("library-changed", refresh);
    };
  }, [refresh]);

  return { expiry, refresh };
}
