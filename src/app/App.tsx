import { useEffect, useRef, useState } from "react";
import { Workspace } from "../features/workspace/Workspace";
import { HelpPage } from "../features/help/HelpPage";
import { AppShell } from "./AppShell";
import { useAppRoute } from "./navigation";

export function App() {
  const route = useAppRoute();
  const [account, setAccount] = useState<string | null>(null);
  const content = useRef<HTMLDivElement>(null);
  useEffect(() => {
    content.current
      ?.querySelector<HTMLElement>("[data-page-active='true'] h1")
      ?.focus();
  }, [route]);
  return (
    <AppShell route={route} account={account}>
      <div ref={content}>
        {/* Keep native operations and form state alive while reading Help. */}
        <div hidden={route !== "ipas"} data-page-active={route === "ipas"}>
          <Workspace onAccount={setAccount} />
        </div>
        {route === "help" && (
          <div data-page-active="true">
            <HelpPage />
          </div>
        )}
      </div>
    </AppShell>
  );
}
