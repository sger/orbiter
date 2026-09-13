import { useEffect, useState } from "react";
import { Box } from "lucide-react";
import { isTauri, libraryIcon } from "../../ipc/commands";

/// Icons fetched this session, by hash. Icon bytes used to ride inside every library snapshot,
/// which is polled while an install runs — so the same few megabytes crossed the IPC again and
/// again. A hash never changes what it points at, so one fetch each is enough.
const cache = new Map<string, string | null>();

export function AppIcon({
  src,
  sha,
  name,
}: {
  /// A data URL the caller already has, as live inspection does.
  src?: string | null;
  /// A hash to fetch from the library instead.
  sha?: string | null;
  name: string;
}) {
  const [fetched, setFetched] = useState<string | null>(
    sha ? (cache.get(sha) ?? null) : null,
  );
  const [failed, setFailed] = useState<string | null>(null);
  useEffect(() => {
    if (!sha || !isTauri() || cache.has(sha)) {
      setFetched(sha ? (cache.get(sha) ?? null) : null);
      return;
    }
    let disposed = false;
    // An icon is decoration: a failure is a placeholder, never an error on screen.
    libraryIcon(sha)
      .catch(() => null)
      .then((value) => {
        cache.set(sha, value);
        if (!disposed) setFetched(value);
      });
    return () => {
      disposed = true;
    };
  }, [sha]);
  const url = src ?? fetched;
  return (
    <div className="app-icon library-icon">
      {url && url !== failed ? (
        <img src={url} alt={`${name} icon`} onError={() => setFailed(url)} />
      ) : (
        <Box size={28} aria-label="App icon unavailable" />
      )}
    </div>
  );
}
