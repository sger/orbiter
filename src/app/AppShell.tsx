import { useEffect, useState, type ReactNode } from "react";
import { accountSignOut } from "../ipc/commands";
import type { AppRoute } from "./navigation";
import { Rail } from "./Rail";
import { AttentionNotice } from "../features/renew/AttentionNotice";
import { useAttention } from "../features/renew/useAttention";

export function AppShell({
  busy = false,
  activeOperation,
  route,
  account,
  children,
}: {
  busy?: boolean;
  activeOperation?: { route: AppRoute; label: string };
  route: AppRoute;
  account: string | null;
  children: ReactNode;
}) {
  const [wide, setWide] = useState(
    () => window.matchMedia("(min-width: 1100px)").matches,
  );
  const [preference, setPreference] = useState<boolean | null>(null);
  useEffect(() => {
    const query = window.matchMedia("(min-width: 1100px)");
    const update = () => setWide(query.matches);
    query.addEventListener("change", update);
    return () => query.removeEventListener("change", update);
  }, []);
  const expanded = preference ?? wide;
  const attention = useAttention();
  return (
    <div className="shell" data-expanded={expanded}>
      <Rail
        route={route}
        activeOperation={activeOperation}
        expanded={expanded}
        onToggle={() => setPreference(!expanded)}
      />
      <main className="app-main">
        <header>
          <div className="wordmark">
            <img
              className="brand-icon"
              src="/orbiter.png"
              alt=""
              width={40}
              height={40}
            />
            orbiter
          </div>
          {account ? (
            <div className="header-account">
              <span className="header-account-name" title={account}>
                {account}
              </span>
              <button
                className="text-button"
                disabled={busy}
                onClick={() => void accountSignOut().catch(() => {})}
              >
                Sign out
              </button>
            </div>
          ) : (
            <span className="local">
              <i /> Local workspace
            </span>
          )}
        </header>
        {/* Wherever a person happens to be, and not while they are in the middle of something:
            an operation already has their attention, and a build that has run out will still have
            run out when it finishes. */}
        {!busy && (
          <AttentionNotice
            urgent={attention.urgent}
            soon={attention.soon}
            route={route}
          />
        )}
        {children}
      </main>
    </div>
  );
}
