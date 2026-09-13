import { useCallback, useEffect, useState } from "react";
import { isTauri, libraryRefresh } from "../../ipc/commands";
import type { Refresh } from "../library/types";
import { requestRefresh } from "./intent";

/// Whether an expiring build can still be re-signed, and the click that goes and does it.
///
/// The question is answered in Rust and only there: a signed build has to be traced back to the
/// original it was made from, and that original has to still exist. Working that out again here
/// would be a second copy of the rule, free to drift until the window offers an action the backend
/// refuses — which is the shape of bug this project already wrote `state/pipeline.ts` to end.
///
/// So this asks, and until it has an answer it offers nothing. A build whose original is gone gets
/// no button rather than a button that fails: the countdown is still worth showing, because the
/// app on someone's phone has still stopped working, but "re-sign it" is no longer something
/// Orbiter can do.
///
/// Asking costs one call per opened app and reads the manifest. Nothing here signs, opens a plan,
/// or contacts Apple or the phone.
export function useRefresh(artifactId: string | null) {
  const [target, setTarget] = useState<Refresh | null>(null);

  useEffect(() => {
    if (!isTauri() || !artifactId) {
      setTarget(null);
      return;
    }
    let disposed = false;
    // Not knowing and not being able to are the same offer — none — so a failure is silence.
    libraryRefresh(artifactId)
      .then((value) => {
        if (!disposed) setTarget(value);
      })
      .catch(() => {
        if (!disposed) setTarget(null);
      });
    return () => {
      disposed = true;
    };
  }, [artifactId]);

  /// Hand the previous answers to the workspace and open it on the original.
  ///
  /// Navigation only. The workspace it opens starts where it always starts and waits for the same
  /// clicks; what it gains is not having to ask a person what they chose a week ago.
  const start = useCallback(() => {
    if (!target) return;
    requestRefresh(target);
    window.location.hash = `/ipas/${target.app_id}/workspace/${target.artifact_id}`;
  }, [target]);

  return { target, start };
}
