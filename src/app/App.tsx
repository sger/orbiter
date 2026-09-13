import { LibraryPage } from "../features/library/LibraryPage";
import type { Opened } from "../features/library/types";
import { isTauri, libraryOpen, libraryChanged } from "../ipc/commands";
import { useAppearance } from "./appearance";
import { SettingsPage } from "../features/settings/SettingsPage";
import { useEffect, useRef, useState } from "react";
import { Workspace } from "../features/workspace/Workspace";
import { HelpPage } from "../features/help/HelpPage";
import { AppShell } from "./AppShell";
import { useAppRoute } from "./navigation";

export function App() {
  const route = useAppRoute();
  const { appearance, changeAppearance } = useAppearance();
  const [account, setAccount] = useState<string | null>(null);
  const [selected, setSelected] = useState<Opened | null>(null);
  const [busy, setBusy] = useState(false);
  const [opening, setOpening] = useState(false);
  const [error, setError] = useState("");
  const parts = route.split("/");
  const workspace = parts[2] === "workspace" || route === "ipas/workspace";
  const requested = parts[3];
  useEffect(() => {
    if (!requested || requested === selected?.artifact.id) {
      setOpening(false);
      setError("");
      return;
    }
    if (busy) {
      setOpening(false);
      setError(
        "An operation is in progress. Finish it before opening another version.",
      );
      return;
    }
    if (!isTauri()) {
      setError("Open Orbiter on your Mac to load saved IPAs.");
      return;
    }
    let disposed = false;
    setOpening(true);
    setError("");
    libraryOpen(requested)
      .then((value) => {
        if (!disposed) {
          setSelected(value);
          libraryChanged();
        }
      })
      .catch((e) => {
        if (!disposed) setError(String(e));
      })
      .finally(() => {
        if (!disposed) setOpening(false);
      });
    return () => {
      disposed = true;
    };
  }, [requested, selected?.artifact.id, busy]);
  const content = useRef<HTMLDivElement>(null);
  useEffect(() => {
    content.current
      ?.querySelector<HTMLElement>("[data-page-active='true'] h1")
      ?.focus();
  }, [route, selected, opening]);
  return (
    <AppShell route={route} account={account}>
      <div ref={content}>
        {/* Keep native operations and form state alive while reading Help. */}
        {error && (
          <p className="error" role="alert">
            {error}
          </p>
        )}
        {opening && <p role="status">Verifying saved IPA…</p>}
        {selected &&
          (!workspace ||
            (!!requested && requested !== selected.artifact.id)) && (
            <p className="notice">
              <a
                href={`#/ipas/${selected.artifact.app_id}/workspace/${selected.artifact.id}`}
              >
                {busy
                  ? "Operation in progress — return to workspace"
                  : `Resume ${selected.artifact.name}`}
              </a>
            </p>
          )}
        <div
          hidden={
            !workspace ||
            opening ||
            (!!requested && requested !== selected?.artifact.id)
          }
          data-page-active={workspace}
        >
          <a
            className="text-button"
            href={selected ? `#/ipas/${selected.artifact.app_id}` : "#/ipas"}
          >
            Back to library
          </a>
          <Workspace
            onAccount={setAccount}
            selected={selected}
            visible={workspace && !opening}
            onBusy={setBusy}
            onChoose={
              selected
                ? () => {
                    window.location.hash = "/ipas";
                  }
                : undefined
            }
          />
        </div>
        <div
          hidden={workspace || !route.startsWith("ipas")}
          data-page-active={!workspace && route.startsWith("ipas")}
        >
          <LibraryPage
            visible={!workspace && route.startsWith("ipas")}
            appId={workspace ? undefined : parts[1]}
            busy={busy || opening}
            selectedId={selected?.artifact.id}
            onRemoved={(ids) => {
              if (selected && ids.includes(selected.artifact.id))
                setSelected(null);
            }}
          />
        </div>
        {route === "settings" && (
          <div data-page-active="true">
            <SettingsPage
              appearance={appearance}
              onAppearanceChange={changeAppearance}
            />
          </div>
        )}
        {route === "help" && (
          <div data-page-active="true">
            <HelpPage />
          </div>
        )}
      </div>
    </AppShell>
  );
}
