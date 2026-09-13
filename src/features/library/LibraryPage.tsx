import { AppIcon } from "./AppIcon";
import { useLibraryDrop } from "./useLibraryDrop";
import { useRenewal } from "../renew/useRenewal";
import { RenewalBanner } from "../renew/RenewalBanner";
import { ExpiryLine } from "../renew/ExpiryLine";
import { useCallback, useEffect, useRef, useState } from "react";
import { open } from "@tauri-apps/plugin-dialog";
import { Box, Plus, Search, ArrowLeft, ArrowRight, Trash2 } from "lucide-react";
import {
  isTauri,
  libraryList,
  libraryImport,
  libraryRemove,
  libraryReclaim,
  libraryChanged,
} from "../../ipc/commands";
import type { Artifact, Attempt, LibrarySnapshot } from "./types";
import { failure, message } from "../../ipc/failure";

const empty: LibrarySnapshot = {
  apps: [],
  artifacts: [],
  devices: [],
  attempts: [],
  expiries: [],
  storage_bytes: 0,
  unreferenced_bytes: 0,
};
export const bytes = (n: number) => `${(n / 1024 / 1024).toFixed(1)} MB`;
const date = (n: number) => new Date(n * 1000).toLocaleString();
export function expiry(value: string | null) {
  if (!value || !Number.isFinite(Date.parse(value)))
    return "Expiration unknown";
  return `${Date.parse(value) <= Date.now() ? "Expired" : "Expires"} ${new Date(value).toLocaleDateString()}`;
}
const version = (a: Pick<Artifact, "version" | "build">) =>
  `${a.version ?? "Unknown version"} (${a.build ?? "unknown build"})`;
export function LibraryPage({
  appId,
  visible = true,
  busy,
  selectedId,
  onRemoved,
  onInstall,
}: {
  appId?: string;
  visible?: boolean;
  busy: boolean;
  selectedId?: string;
  onRemoved: (ids: string[]) => void;
  onInstall: () => void;
}) {
  const { renewal: legacyRenewal, forget: forgetLegacy } = useRenewal(
    null,
    null,
  );
  const [data, setData] = useState(empty);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState("");
  const [query, setQuery] = useState("");
  const [tab, setTab] = useState("Versions");
  const [importing, setImporting] = useState(false);
  const [progress, setProgress] = useState("");
  const [results, setResults] = useState<
    { name: string; message: string; appId?: string; artifactId?: string }[]
  >([]);
  const [removal, setRemoval] = useState<{
    appId: string;
    artifactId: string | null;
  } | null>(null);
  const [removing, setRemoving] = useState(false);
  const [reclaiming, setReclaiming] = useState(false);
  const importingRef = useRef(false);
  const busyRef = useRef(busy);
  busyRef.current = busy;
  const dragging = useLibraryDrop(
    visible,
    (paths) => {
      void importFiles(paths);
    },
    setError,
  );
  const refresh = useCallback(async () => {
    if (!isTauri()) {
      setLoading(false);
      return;
    }
    try {
      setData(await libraryList());
      setError("");
    } catch (e) {
      setError(message(e));
    } finally {
      setLoading(false);
    }
  }, []);
  useEffect(() => {
    void refresh();
    window.addEventListener("library-changed", refresh);
    return () => window.removeEventListener("library-changed", refresh);
  }, [refresh]);
  useEffect(() => {
    void refresh();
    if (!busy) return;
    const timer = window.setInterval(() => void refresh(), 1500);
    return () => window.clearInterval(timer);
  }, [appId, busy, refresh]);
  useEffect(() => {
    setTab("Versions");
    setRemoval(null);
  }, [appId]);
  async function importFiles(droppedPaths?: string[]) {
    if (importingRef.current) {
      setError(
        "An import is already running. Drop more files after it finishes.",
      );
      return;
    }
    importingRef.current = true;
    setImporting(true);
    setResults([]);
    try {
      const chosen =
        droppedPaths ??
        (await open({
          multiple: true,
          directory: false,
          filters: [{ name: "iOS application", extensions: ["ipa"] }],
        }));
      const paths = typeof chosen === "string" ? [chosen] : (chosen ?? []);
      for (const [index, path] of paths.entries()) {
        const name = path.split(/[\\/]/).pop() ?? "IPA";
        setProgress(`Importing ${index + 1} of ${paths.length}: ${name}`);
        try {
          if (!/\.ipa$/i.test(path))
            throw "Only IPA files can be imported. Choose a file ending in .ipa.";
          const result = await libraryImport(path);
          setResults((old) => [
            ...old,
            {
              name,
              message: result.duplicate
                ? "Already saved — existing version reopened"
                : "Imported",
              appId: result.app_id,
              artifactId: result.artifact_id,
            },
          ]);
          if (paths.length === 1)
            window.location.hash =
              result.duplicate && !busyRef.current
                ? `/ipas/${result.app_id}/workspace/${result.artifact_id}`
                : `/ipas/${result.app_id}`;
        } catch (e) {
          // A saved copy that no longer matches its record is repaired by importing the same
          // file again, so the result says that rather than leaving a dead end. An artifact that
          // is simply gone is not repairable, and must not be given the same advice.
          const failed = failure(e);
          setResults((old) => [
            ...old,
            {
              name,
              message:
                failed.code === "artifact_changed"
                  ? `${failed.message} Importing this file again will replace it.`
                  : failed.message,
            },
          ]);
        }
        libraryChanged();
      }
      setProgress(
        paths.length
          ? `Finished importing ${paths.length} selected ${paths.length === 1 ? "file" : "files"}.`
          : "",
      );
    } catch (e) {
      setError(message(e));
    } finally {
      importingRef.current = false;
      setImporting(false);
    }
  }
  async function remove() {
    if (!removal || busy) return;
    setRemoving(true);
    try {
      const affected = data.artifacts
        .filter(
          (a) =>
            a.app_id === removal.appId &&
            (!removal.artifactId ||
              a.id === removal.artifactId ||
              a.source_id === removal.artifactId),
        )
        .map((a) => a.id);
      await libraryRemove(removal.appId, removal.artifactId);
      onRemoved(affected);
      setRemoval(null);
      libraryChanged();
      if (!removal.artifactId) window.location.hash = "/ipas";
    } catch (e) {
      setError(message(e));
    } finally {
      setRemoving(false);
    }
  }
  const app = data.apps.find((a) => a.id === appId);
  const artifacts = data.artifacts
    .filter((a) => a.app_id === appId && !a.deleted)
    .slice()
    .reverse()
    .sort((a, b) => b.added_unix - a.added_unix);
  const attempts = data.attempts
    .filter((a) => a.app_id === appId)
    .slice()
    .reverse()
    .sort((a, b) => b.started_unix - a.started_unix);
  const deviceName = (id: string) =>
    data.devices.find((d) => d.id === id)?.name ?? "Remembered device";
  // Longest-lived first from Rust, so the first entry for an app is the copy still launching.
  const headline = (id: string) =>
    data.expiries.find((e) => e.app_id === id) ?? null;
  const installedExpiry = (attempt?: Attempt) =>
    attempt
      ? `${attempt.signed ? "Installed signed build" : "Installed original profile"}: ${expiry(attempt.expires)}`
      : "No successful installation recorded";
  const artifactRow = (a: Artifact) => (
    <article className="library-version" key={a.id}>
      <div>
        <strong>
          {a.source_id ? "Signed build" : "Original"} · {version(a)}
        </strong>
        <p className="hint">
          Saved {date(a.added_unix)} · {bytes(a.size_bytes)}
        </p>
        <p className="hint">
          {a.source_id ? "Signed profile" : "Imported profile"}:{" "}
          {expiry(a.expires)}
        </p>
        {a.source_id && (
          <p className="hint library-wrap">
            {a.identifier} · Marker: {a.marker || "none"} · Watch: {a.watch} ·
            Team tag: {a.team_tag?.slice(0, 12) ?? "unknown"}
          </p>
        )}
        <details>
          <summary className="hint">Artifact identity</summary>
          <code className="library-wrap">SHA-256: {a.sha256}</code>
          {a.source_id && <p className="hint">Source version: {a.source_id}</p>}
        </details>
      </div>
      <div className="library-actions">
        <a
          className="file-button"
          href={`#/ipas/${a.app_id}/workspace/${a.id}`}
          aria-disabled={busy && selectedId !== a.id}
          onClick={(e) => {
            if (busy && selectedId !== a.id) e.preventDefault();
          }}
        >
          {a.source_id ? "Review installation" : "Open version"}
        </a>
        <button
          className="text-button"
          disabled={busy || removing}
          onClick={() => setRemoval({ appId: a.app_id, artifactId: a.id })}
        >
          <Trash2 size={14} aria-hidden="true" />
          <span>Remove {a.source_id ? "signed build" : "version"}</span>
        </button>
      </div>
    </article>
  );
  return (
    <section className="library-page" data-dragging={dragging}>
      <div className="library-heading">
        <div>
          {appId && (
            <a className="text-button" href="#/ipas">
              <ArrowLeft size={15} /> All apps
            </a>
          )}
          <div className="library-app-heading">
            {app && <AppIcon sha={app.icon_sha} name={app.name} />}
            <div>
              <h1 className="page-title" tabIndex={-1} title={app?.name}>
                {app?.name ?? (appId ? "App details" : "App Library")}
              </h1>
              <p className="hint library-wrap">
                {app
                  ? app.identifier
                  : "Your apps, saved versions, and installation history. Stored on this Mac."}
              </p>
            </div>
          </div>
        </div>
        <div className="library-heading-actions">
          <button
            className="text-button"
            onClick={() => void importFiles()}
            disabled={importing || !isTauri()}
          >
            <Plus size={17} /> Import IPA
          </button>
          <button
            className="primary"
            onClick={onInstall}
            disabled={busy || importing}
          >
            Install an app <ArrowRight size={17} />
          </button>
        </div>
      </div>
      <div className="library-drop-zone" role="status">
        {dragging
          ? importing
            ? "An import is running — wait before dropping more files."
            : "Drop your IPA files to import"
          : "Drag and drop IPA files here, or use Import IPA."}
      </div>
      {!isTauri() && (
        <p className="notice">
          Open Orbiter on your Mac to import and manage local IPAs.
        </p>
      )}
      {data.storage_warning && (
        <p className="notice" role="status">
          {data.storage_warning}
        </p>
      )}
      {error && (
        <div className="error" role="alert">
          {error}
          <button className="text-button" onClick={() => void refresh()}>
            Retry
          </button>
        </div>
      )}
      {progress && <p role="status">{progress}</p>}
      {results.length > 0 && (
        <ul className="library-results" aria-label="Import results">
          {results.map((r, i) => (
            <li key={i}>
              <span className="library-import-message">
                <strong>{r.name}</strong> — {r.message}
              </span>
              {r.appId &&
                r.artifactId &&
                data.artifacts.some(
                  (a) => a.id === r.artifactId && !a.deleted,
                ) && (
                  <a
                    href={`#/ipas/${r.appId}/workspace/${r.artifactId}`}
                    aria-disabled={busy && selectedId !== r.artifactId}
                    onClick={(e) => {
                      if (busy && selectedId !== r.artifactId)
                        e.preventDefault();
                    }}
                  >
                    Open imported version
                  </a>
                )}
            </li>
          ))}
        </ul>
      )}
      {loading ? (
        <p role="status">Loading library…</p>
      ) : appId ? (
        app ? (
          <>
            <nav className="library-tabs" aria-label="App details">
              {["Versions", "Installations", "Devices"].map((t) => (
                <button
                  key={t}
                  aria-pressed={tab === t}
                  onClick={() => setTab(t)}
                >
                  {t}
                </button>
              ))}
            </nav>
            <ExpiryLine expiry={headline(app.id)} variant="banner" />
            {tab === "Versions" && (
              <div className="card library-card">
                <p className="hint">
                  Originals are ordered by import time. Signed builds stay with
                  their source version. Profile expiration does not confirm that
                  an app is installed or working.
                </p>
                {artifacts
                  .filter((a) => !a.source_id)
                  .map((a) => (
                    <div key={a.id}>
                      {artifactRow(a)}
                      {artifacts
                        .filter((s) => s.source_id === a.id)
                        .map(artifactRow)}
                    </div>
                  ))}
                {artifacts.length === 0 && (
                  <p>No saved files. Import an IPA to add a version.</p>
                )}
              </div>
            )}
            {tab === "Installations" && (
              <div className="card library-card">
                <p className="hint">
                  Attempts recorded by Orbiter. A successful result describes
                  that installation, not the app’s current presence or ability
                  to run.
                </p>
                {attempts.length === 0 && (
                  <p>No installation attempts recorded.</p>
                )}
                {attempts.map((a) => (
                  <article className="library-version" key={a.id}>
                    <div>
                      <strong>
                        {deviceName(a.device_id)} · {a.stage}
                      </strong>
                      <p>
                        {version(a)} · {date(a.started_unix)}
                      </p>
                      <p className="hint">{a.message}</p>
                      <p className="hint">
                        {a.stage === "installed"
                          ? installedExpiry(a)
                          : "No installed expiration established by this attempt."}
                      </p>
                      <ExpiryLine
                        expiry={
                          data.expiries.find((e) => e.attempt_id === a.id) ??
                          null
                        }
                        variant="line"
                      />
                      <code className="library-wrap">
                        Artifact: {a.artifact_id}
                      </code>
                      {data.artifacts.find((f) => f.id === a.artifact_id)
                        ?.deleted && (
                        <p className="hint">
                          Saved file removed; history retained.
                        </p>
                      )}
                    </div>
                  </article>
                ))}
              </div>
            )}
            {tab === "Devices" && (
              <div className="card library-card">
                <p className="hint">
                  Devices used for this app’s installation attempts in Orbiter.
                  This is not an inventory of apps on a phone.
                </p>
                {data.devices
                  .filter((d) => attempts.some((a) => a.device_id === d.id))
                  .map((d) => {
                    const history = attempts.filter(
                      (a) => a.device_id === d.id,
                    );
                    return (
                      <article className="library-version" key={d.id}>
                        <div>
                          <strong>{d.name}</strong>
                          <p>
                            {history.length} attempts · Last recorded{" "}
                            {date(history[0].started_unix)}
                          </p>
                          <p className="hint">
                            {installedExpiry(
                              history.find((a) => a.stage === "installed"),
                            )}
                          </p>
                          <ExpiryLine
                            expiry={
                              data.expiries.find((e) => e.device_id === d.id) ??
                              null
                            }
                            variant="line"
                          />
                          <code>Device tag: {d.id.slice(0, 12)}</code>
                        </div>
                      </article>
                    );
                  })}
                {attempts.length === 0 && (
                  <p>No remembered devices for this app yet.</p>
                )}
              </div>
            )}
            <div className="library-storage">
              <span>
                {bytes(
                  artifacts.reduce(
                    (n, a, i, list) =>
                      n +
                      (list.findIndex((b) => b.sha256 === a.sha256) === i
                        ? a.size_bytes
                        : 0),
                    0,
                  ),
                )}{" "}
                of saved files
              </span>
              <button
                className="text-button"
                disabled={busy || removing}
                onClick={() => setRemoval({ appId: app.id, artifactId: null })}
              >
                Remove app and history
              </button>
            </div>
          </>
        ) : (
          <p>
            App not found. <a href="#/ipas">Return to your library</a>.
          </p>
        )
      ) : (
        <>
          <label className="library-search">
            <Search size={18} />
            <input
              type="search"
              aria-label="Search apps"
              placeholder="Search apps or bundle identifiers"
              value={query}
              onChange={(e) => setQuery(e.target.value)}
            />
          </label>
          {data.apps.length === 0 && !error && (
            <div className="card library-empty">
              <Box size={36} />
              <h2>Your library starts here</h2>
              <p>
                Import an IPA to keep a local copy, compare versions, and record
                installations.
              </p>
              <p className="hint">
                Importing never installs an app. Your original files stay
                untouched.
              </p>
            </div>
          )}
          <div className="library-rows">
            {data.apps
              .filter((a) =>
                `${a.name} ${a.identifier}`
                  .toLowerCase()
                  .includes(query.toLowerCase()),
              )
              .map((a) => {
                const versions = data.artifacts.filter(
                  (v) => v.app_id === a.id && !v.source_id && !v.deleted,
                );
                const history = data.attempts
                  .filter((h) => h.app_id === a.id)
                  .slice()
                  .reverse();
                return (
                  <a
                    className="card library-row"
                    href={`#/ipas/${a.id}`}
                    key={a.id}
                  >
                    <AppIcon sha={a.icon_sha} name={a.name} />
                    <div>
                      <h2>{a.name}</h2>
                      <p className="library-wrap">{a.identifier}</p>
                      <p className="hint">
                        {versions.length} saved{" "}
                        {versions.length === 1 ? "version" : "versions"}
                        {history[0]
                          ? ` · Latest attempt: ${history[0].stage} on ${deviceName(history[0].device_id)} · ${date(history[0].started_unix)}`
                          : " · No installation history"}
                      </p>
                      <p className="hint">
                        {installedExpiry(
                          history.find((h) => h.stage === "installed"),
                        )}
                      </p>
                      <ExpiryLine expiry={headline(a.id)} variant="line" />
                      {!history.some((h) => h.stage === "installed") &&
                        versions.length > 0 && (
                          <p className="hint">
                            Latest imported profile:{" "}
                            {expiry(versions[versions.length - 1].expires)}
                          </p>
                        )}
                    </div>
                  </a>
                );
              })}
          </div>
          {data.apps.length > 0 &&
            !data.apps.some((a) =>
              `${a.name} ${a.identifier}`
                .toLowerCase()
                .includes(query.toLowerCase()),
            ) && <p>No apps match “{query}”.</p>}
          {/* The file an older Orbiter wrote. Nothing writes it any more — the library records
              an installed build's expiry now — so this is the one place it is shown, and the one
              place it can be cleared. */}
          <RenewalBanner
            renewal={legacyRenewal}
            canResign={false}
            onResign={() => {}}
            onForget={forgetLegacy}
          />
          <p className="library-storage">
            <span>
              {data.apps.length} apps · {bytes(data.storage_bytes)} of managed
              IPAs
              {data.unreferenced_bytes > 0 && (
                <>
                  {" · "}
                  {bytes(data.unreferenced_bytes)} not referenced by any saved
                  version
                </>
              )}
            </span>
            {/* Left behind when a copy was interrupted. Named and counted rather than deleted:
                Orbiter removes files somebody asked it to remove, and this is the asking. */}
            {data.unreferenced_bytes > 0 && (
              <button
                className="text-button"
                disabled={busy || reclaiming}
                onClick={async () => {
                  setReclaiming(true);
                  setError("");
                  try {
                    await libraryReclaim();
                    libraryChanged();
                  } catch (e) {
                    setError(message(e));
                  } finally {
                    setReclaiming(false);
                  }
                }}
              >
                {reclaiming ? "Reclaiming…" : "Reclaim unreferenced files"}
              </button>
            )}
          </p>
        </>
      )}
      {removal && (
        <section
          className="card library-confirm"
          role="alertdialog"
          aria-modal="false"
          aria-labelledby="remove-title"
        >
          <h2 id="remove-title">
            {removal.artifactId
              ? "Remove saved files?"
              : "Remove app and history?"}
          </h2>
          <p>
            {removal.artifactId
              ? "This removes this file and any signed builds saved from it. Installation history is preserved."
              : "This permanently removes all saved files and installation history for this app."}{" "}
            Nothing is uninstalled from a phone.
          </p>
          <div className="library-actions">
            <button
              autoFocus
              disabled={removing}
              onClick={() => setRemoval(null)}
            >
              Keep files
            </button>
            <button disabled={busy || removing} onClick={() => void remove()}>
              {removing ? "Removing…" : "Confirm removal"}
            </button>
          </div>
        </section>
      )}
    </section>
  );
}
