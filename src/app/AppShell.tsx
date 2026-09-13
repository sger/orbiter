import { useEffect, useState, type ReactNode } from "react";
import { accountSignOut } from "../ipc/commands";
import type { AppRoute } from "./navigation";
import { Rail } from "./Rail";

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
        {children}
      </main>
    </div>
  );
}
