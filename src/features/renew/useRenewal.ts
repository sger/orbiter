import { useCallback, useEffect, useState } from "react";
import { isTauri, renewalForget, renewalStatus } from "../../ipc/commands";
import type { Renewal } from "../../types";

/// What Orbiter remembers about the last build it put on a phone for this team.
///
/// The clock is the only thing that moves here, and it moves slowly: a standing changes at a
/// twenty-four-hour boundary, so this re-reads every ten minutes and whenever the window is looked
/// at again. A tighter interval would burn a timer to show the same sentence. Nothing here reaches
/// Apple or the phone — it is a local file and `Date.now()` in Rust.
const INTERVAL = 10 * 60 * 1000;

export function useRenewal(teamId: string | null, identifier: string | null) {
  const [renewal, setRenewal] = useState<Renewal | null>(null);
  const refresh = useCallback(() => {
    if (!isTauri()) return;
    // A record of an expiry must never be the reason something breaks: a failure is silence.
    renewalStatus(teamId, identifier)
      .then(setRenewal)
      .catch(() => setRenewal(null));
  }, [teamId, identifier]);

  useEffect(() => {
    refresh();
    const timer = window.setInterval(refresh, INTERVAL);
    // Coming back to the window after a day away is exactly when the sentence has gone stale.
    window.addEventListener("focus", refresh);
    return () => {
      window.clearInterval(timer);
      window.removeEventListener("focus", refresh);
    };
  }, [refresh]);

  const forget = useCallback(() => {
    void renewalForget()
      .catch(() => {})
      .then(refresh);
  }, [refresh]);

  return { renewal, refresh, forget };
}
