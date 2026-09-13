import { useCallback, useEffect, useState } from "react";
import { isTauri, libraryList } from "../../ipc/commands";
import type { LibraryExpiry } from "../library/types";

/// A standing changes at a twenty-four-hour boundary, so this re-reads slowly and on the moments
/// that actually matter: coming back to the window, and anything changing the library. The same
/// interval the per-build countdowns use, for the same reason — a tighter one would burn a timer
/// to render the same sentence.
const INTERVAL = 10 * 60 * 1000;

/// Which installed builds need attention, across the whole library rather than one open app.
///
/// The countdowns already exist, but only where a person has navigated to them: a build quietly
/// running out on a tester's phone is invisible to someone who opened Orbiter to do something
/// else. This is what the shell reads so that opening the application at all is enough to be told.
///
/// It decides nothing. Which builds are running out, and the words for each, are Rust's; this
/// selects the ones already flagged and keeps them newest-deadline-first, as the library returned
/// them. Nothing here contacts Apple or a phone — it is one local manifest read on a slow timer.
export function useAttention() {
  const [expiries, setExpiries] = useState<LibraryExpiry[]>([]);
  const read = useCallback(() => {
    if (!isTauri()) return;
    // Silence on failure: a note about an expiry must never be the reason a screen breaks, and a
    // library that cannot be read is reported by the library page, which owns that message.
    libraryList()
      .then((snapshot) => setExpiries(snapshot.expiries ?? []))
      .catch(() => setExpiries([]));
  }, []);

  useEffect(() => {
    read();
    const timer = window.setInterval(read, INTERVAL);
    window.addEventListener("focus", read);
    window.addEventListener("library-changed", read);
    return () => {
      window.clearInterval(timer);
      window.removeEventListener("focus", read);
      window.removeEventListener("library-changed", read);
    };
  }, [read]);

  return {
    /// Stopped launching, or stops today.
    urgent: expiries.filter((e) => e.urgent),
    /// Still launching, but not for long.
    soon: expiries.filter((e) => e.soon),
  };
}
