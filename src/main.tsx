import { Accounts, type Preparation } from "./Accounts";
import React, { useCallback, useEffect, useRef, useState } from "react";
import { createRoot } from "react-dom/client";
import { Channel, invoke, isTauri } from "@tauri-apps/api/core";
import { getCurrentWebview } from "@tauri-apps/api/webview";
import { open } from "@tauri-apps/plugin-dialog";
import {
  ArrowRight,
  Box,
  Check,
  ChevronDown,
  Clock3,
  FileArchive,
  FolderOpen,
  Info,
  Layers3,
  LoaderCircle,
  Orbit,
  ShieldCheck,
  Smartphone,
  Upload,
  X,
} from "lucide-react";
import type { Report, Bundle, Signed, SigningProgress } from "./types";
import "./styles.css";
import { SigningIdentities } from "./SigningIdentities";
import { Devices } from "./Devices";
import { InstallSigned } from "./InstallSigned";
const stages = [
  "Checking archive",
  "Reading bundles and signatures",
  "Assessing compatibility",
];
const labels = {
  not_verified: "Not yet verified",
  requires_configuration: "Configuration required",
  unsupported: "Unsupported",
  preserved: "Supported & preserved",
};
/// Findings the prepared team actually establishes. Once Apple has answered, "not yet verified"
/// is no longer true of these capabilities: they are decided, and the decision is what to show.
function preparedFindings(preparation: Preparation) {
  const findings: {
    status: keyof typeof labels;
    title: string;
    /// Overrides the status word when the status label would misdescribe a settled fact.
    label?: string;
    detail: string;
    bundle: string | null;
  }[] = [];
  const plan = preparation.plan;
  if (plan.blockers.length) {
    for (const blocker of plan.blockers)
      findings.push({
        status: "unsupported",
        title: "Re-signing blocked",
        detail: blocker,
        bundle: null,
      });
    return findings;
  }
  const expiry = preparation.profiles
    .map((profile) => profile.expires)
    .sort()[0];
  findings.push({
    status: "preserved",
    title: "Identifiers and profiles prepared",
    detail: `Apple registered ${preparation.app_ids.length} identifier(s) for this team and returned their profiles${
      expiry ? `, valid until ${date(expiry)}` : ""
    }. The build will install under a rewritten identifier, not the company one.`,
    bundle: plan.new_main_identifier,
  });
  // One finding per capability the team could not carry, stated as decided rather than unknown.
  const removed = new Map<string, { detail: string; bundle: string }>();
  const removedConsequences = new Set<string>();
  for (const bundle of plan.bundles)
    for (const capability of bundle.capabilities)
      if (capability.action === "remove" && capability.consequence) {
        removed.set(capability.key, {
          detail: capability.consequence,
          bundle: bundle.new_identifier,
        });
        removedConsequences.add(capability.consequence);
      }
  for (const [key, entry] of removed)
    findings.push({
      status: "unsupported",
      title: capabilityTitle(key),
      detail: entry.detail,
      bundle: entry.bundle,
    });
  // Consequences the whole plan carries — the seven-day expiry, and what was decided about a
  // Watch app. These are established facts about the prepared build, not open configuration.
  for (const consequence of plan.consequences)
    if (!removedConsequences.has(consequence))
      findings.push({
        status: "requires_configuration",
        title: consequence.includes("seven days")
          ? "Weekly re-signing"
          : "Watch app",
        label: consequence.includes("seven days")
          ? "Every seven days"
          : consequence.includes("removed")
            ? "Removed by your choice"
            : "Kept, unverified",
        detail: consequence,
        bundle: null,
      });
  return findings;
}
function capabilityTitle(key: string) {
  const titles: Record<string, string> = {
    "aps-environment": "Push notifications",
    "com.apple.developer.aps-environment": "Push notifications",
    "com.apple.developer.associated-domains": "Associated domains",
    "com.apple.developer.in-app-payments": "Apple Pay",
    "com.apple.security.application-groups": "App groups",
  };
  return titles[key] ?? key;
}
function date(value: string | null) {
  return value
    ? new Date(value).toLocaleString(undefined, {
        dateStyle: "medium",
        timeStyle: "short",
      })
    : "Not available";
}
function App() {
  const [report, setReport] = useState<Report | null>(null),
    [busy, setBusy] = useState(false),
    [stage, setStage] = useState(""),
    [error, setError] = useState(""),
    [drag, setDrag] = useState(false),
    [cancelled, setCancelled] = useState(false);
  const [ipaPath, setIpaPath] = useState<string | null>(null);
  const [watch, setWatch] = useState<"undecided" | "remove" | "sign">(
    "undecided",
  );
  const [signed, setSigned] = useState<Signed | null>(null);
  const [certificate, setCertificate] = useState(false);
  const [signing, setSigning] = useState(false);
  const [signError, setSignError] = useState<string | null>(null);
  const [signStep, setSignStep] = useState<SigningProgress | null>(null);
  // Stable: Accounts clears the preparation whenever this identity changes, so an inline closure
  // here would wipe the result on every render.
  const prepared = useCallback((result: Preparation | null) => {
    // A new plan invalidates anything signed under the previous one.
    setSigned(null);
    setSignError(null);
    setPreparation(result);
  }, []);
  const [preparation, setPreparation] = useState<Preparation | null>(null);
  const [account, setAccount] = useState<string | null>(null);
  const [deviceId, setDeviceId] = useState<number | null>(null);
  const [installBusy, setInstallBusy] = useState(false);
  const installActive = useRef(false);
  const installationBusy = useCallback((value: boolean) => {
    installActive.current = value;
    setInstallBusy(value);
  }, []);
  const active = useRef(false);
  const desktop = isTauri();
  const inspect = useCallback(async (path: string) => {
    if (active.current || installActive.current) return;
    active.current = true;
    setBusy(true);
    setReport(null);
    setIpaPath(null);
    setError("");
    setCancelled(false);
    setStage(stages[0]);
    const progress = new Channel<string>();
    progress.onmessage = setStage;
    try {
      setReport(await invoke<Report>("inspect_ipa", { path, progress }));
      setStage("Inspection complete");
      setIpaPath(path);
    } catch (e) {
      setError(String(e));
      setStage("Inspection stopped");
    } finally {
      active.current = false;
      setBusy(false);
    }
  }, []);
  useEffect(() => {
    if (!desktop) return;
    let disposed = false;
    let cleanup: (() => void) | undefined;
    getCurrentWebview()
      .onDragDropEvent((event) => {
        if (disposed) return;
        setDrag(event.payload.type === "over");
        if (event.payload.type === "drop") {
          if (event.payload.paths.length !== 1) {
            setError("Drop one IPA at a time.");
            return;
          }
          void inspect(event.payload.paths[0]);
        }
      })
      .then((fn) => {
        if (disposed) fn();
        else cleanup = fn;
      })
      .catch(() => setError("Drag and drop is unavailable. Use Choose IPA."));
    return () => {
      disposed = true;
      cleanup?.();
    };
  }, [desktop, inspect]);
  async function choose() {
    try {
      const p = await open({
        multiple: false,
        directory: false,
        filters: [{ name: "iOS application", extensions: ["ipa"] }],
      });
      if (typeof p === "string") await inspect(p);
    } catch {
      setError(
        "The file picker could not open. Try dropping a local IPA into this window.",
      );
    }
  }
  async function cancel() {
    try {
      await invoke("cancel_inspection");
      setCancelled(true);
    } catch {
      setError(
        "Could not request cancellation. Wait for inspection to finish.",
      );
    }
  }
  const app = report?.bundles.find((b) => b.path === report.main_path);
  const frameworks =
    report?.bundles.filter((b) => b.kind === "Framework").length ?? 0;
  const nested =
    report?.bundles.filter(
      (b) => b.kind !== "Framework" && b.kind !== "Main app",
    ).length ?? 0;
  return (
    <div className="shell">
      <aside className="rail">
        <a className="brand" href="#" aria-label="Orbiter home">
          <Orbit size={29} />
        </a>
        <div className="rail-active" title="Install workspace">
          <Box size={21} />
        </div>
        <div className="rail-bottom" title="Local inspection">
          <ShieldCheck size={19} />
        </div>
      </aside>
      <main>
        <header>
          <div className="wordmark">orbiter</div>
          {account ? (
            <div className="header-account">
              <span className="header-account-name" title={account}>
                {account}
              </span>
              <button
                className="text-button"
                onClick={() => void invoke("account_sign_out").catch(() => {})}
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
        {!desktop && (
          <div className="notice">
            <Info size={17} />
            <span>
              Browser preview. Launch the desktop app with{" "}
              <code>npm run tauri dev</code> to inspect a local IPA.
            </span>
          </div>
        )}
        <div className="workspace">
          <section className="left-column">
            <section className="card source">
              <div className="section-heading">
                <h2>
                  <span>01</span> Application
                </h2>
                <span className="subtle">.ipa</span>
              </div>
              {!app ? (
                <button
                  className={`drop ${drag ? "drag" : ""}`}
                  disabled={busy || installBusy || !desktop}
                  onClick={choose}
                >
                  <div className="upload-icon">
                    {busy ? (
                      <LoaderCircle className="spin" size={26} />
                    ) : (
                      <Upload size={26} />
                    )}
                  </div>
                  <strong>
                    {busy ? "Inspecting your build…" : "Drop your IPA here"}
                  </strong>
                  <span>
                    {busy
                      ? "Reading locally. Your original stays untouched."
                      : "or choose a file from your computer"}
                  </span>
                  <span className="file-button">
                    <FolderOpen size={15} /> Choose IPA
                  </span>
                </button>
              ) : (
                <div className="app-summary">
                  <div className="app-icon">
                    {report?.icon_data_url ? (
                      <img
                        src={report.icon_data_url}
                        alt="App icon"
                        onError={(e) => {
                          e.currentTarget.style.display = "none";
                        }}
                      />
                    ) : (
                      <Box size={30} />
                    )}
                  </div>
                  <div className="app-title">
                    <h3>{app.name}</h3>
                    <p>
                      {app.version ?? "Unknown version"}{" "}
                      <span>({app.build ?? "unknown build"})</span>
                    </p>
                  </div>
                  <button
                    className="text-button"
                    onClick={choose}
                    disabled={busy || installBusy}
                  >
                    Change
                  </button>
                  <code className="bundle-id">{app.identifier}</code>
                  <dl className="app-meta">
                    <div>
                      <dt>IPA size</dt>
                      <dd>
                        {((report?.size_bytes ?? 0) / 1024 / 1024).toFixed(1)}{" "}
                        MB
                      </dd>
                    </div>
                    <div>
                      <dt>Minimum OS</dt>
                      <dd>{app.minimum_os ?? "Unknown"}</dd>
                    </div>
                    <div>
                      <dt>Architecture</dt>
                      <dd>
                        {app.slices.map((s) => s.architecture).join(", ") ||
                          "Unverified"}
                      </dd>
                    </div>
                  </dl>
                  <div className="inventory-line">
                    <Layers3 size={15} />
                    {nested} nested {nested === 1 ? "app" : "apps / extensions"}
                    <span>·</span>
                    {frameworks} frameworks
                  </div>
                </div>
              )}
              <div className="source-footer">
                <ShieldCheck size={14} /> Inspection stays local. Original IPA
                unchanged.
              </div>
            </section>
            <section className="card destination">
              <div className="section-heading">
                <h2>
                  <span>02</span> Destination & identity
                </h2>
              </div>
              <Devices onSelect={setDeviceId} paused={installBusy} />
              <SigningIdentities paused={installBusy} />
              <Accounts
                paused={installBusy}
                deviceId={deviceId}
                ipaPath={ipaPath}
                hasWatchApp={
                  report?.bundles.some((b) => b.kind === "Watch app") ?? false
                }
                onPrepared={prepared}
                onWatch={setWatch}
                onCertificate={setCertificate}
                onAccount={setAccount}
              />
              <p className="hint">
                A different account on the same company team shares that team's
                device allowance. Personal teams have their own limits and
                capability restrictions.
              </p>
            </section>
            <details className="card advanced">
              <summary>
                Advanced options <ChevronDown size={16} />
              </summary>
              <div>
                <p>
                  Bundle changes, profile import, and Watch or extension removal
                  require a reviewable signing plan. These options are not
                  available yet.
                </p>
                <p>Your app identifiers and capabilities are left intact.</p>
              </div>
            </details>
          </section>
          <section className="card compatibility">
            <div className="section-heading">
              <h2>
                <span>03</span> Compatibility
              </h2>
              <ShieldCheck size={18} />
            </div>
            <div className="assessment-label">
              <i className={report ? "amber" : ""} />
              {report ? "Review required" : "Awaiting application"}
            </div>
            {report ? (
              <>
                <h3>Know what needs attention.</h3>
                <p className="assessment-intro">
                  {preparation
                    ? "What the prepared team establishes for this build. Runtime behavior is still not verified."
                    : "Static findings from this build. Target-team compatibility and runtime behavior are not verified."}
                </p>
                <div className="findings">
                  {(preparation
                    ? preparedFindings(preparation)
                    : report.findings
                  ).map((f, i) => (
                    <article className={`finding ${f.status}`} key={i}>
                      <div className="finding-icon">
                        {f.status === "unsupported" ? (
                          <X size={15} />
                        ) : (
                          <Info size={15} />
                        )}
                      </div>
                      <div>
                        <div className="finding-heading">
                          <h4>{f.title}</h4>
                          <span>
                            {(f as { label?: string }).label ??
                              labels[f.status]}
                          </span>
                        </div>
                        <p>{f.detail}</p>
                        {f.bundle && <code>{f.bundle}</code>}
                      </div>
                    </article>
                  ))}
                </div>
              </>
            ) : (
              <div className="empty-assessment">
                <div className="orbit-art">
                  <Orbit size={58} strokeWidth={1} />
                  <div />
                </div>
                <h3>
                  A little inspection.
                  <br />A lot fewer surprises.
                </h3>
                <p>
                  Add an IPA to review its provisioning,
                  <br />
                  embedded apps, and signing capabilities.
                </p>
                <ul>
                  <li>
                    <Check size={14} /> Executable & encryption checks
                  </li>
                  <li>
                    <Check size={14} /> Profiles & capability inventory
                  </li>
                  <li>
                    <Check size={14} /> Watch & extension discovery
                  </li>
                </ul>
              </div>
            )}
            <div className="compatibility-footer">
              <Info size={15} />
              <span>
                Installing successfully does not prove the app's features work
                correctly.
              </span>
            </div>
          </section>
        </div>
        {error && (
          <div className="error" role="alert">
            <Info size={18} />
            <span>{error}</span>
            <button onClick={() => setError("")} aria-label="Dismiss error">
              <X size={16} />
            </button>
          </div>
        )}
        {(busy || report) && (
          <div className="progress-area" role="status" aria-live="polite">
            <div className="progress-title">
              {busy ? (
                <LoaderCircle className="spin" size={15} />
              ) : (
                <Check size={15} />
              )}{" "}
              {cancelled && busy ? "Cancellation requested…" : stage}
              {busy && (
                <button
                  className="text-button"
                  disabled={cancelled}
                  onClick={cancel}
                >
                  Cancel inspection
                </button>
              )}
            </div>
            <div className="stages">
              {stages.map((s, i) => (
                <span
                  className={
                    report || stages.indexOf(stage) >= i ? "reached" : ""
                  }
                  key={s}
                >
                  {i + 1}. {s}
                </span>
              ))}
            </div>
          </div>
        )}
        <section className="action-bar">
          <div>
            <div className="action-label">
              <Clock3 size={16} />
              {signing
                ? "Signing"
                : signed
                  ? "Signed build expires"
                  : preparation?.profiles.length
                    ? "Prepared profile expiration"
                    : app?.profile?.expires_at
                      ? "Embedded profile expiration"
                      : "Ready when the next pieces are."}
            </div>
            <p>
              {signError ??
                (signing
                  ? signStep
                    ? `${signStep.stage} · ${signStep.done} of ${signStep.total}`
                    : "Signing. This reads and rewrites the whole app, so it takes a while."
                  : signed
                    ? `${date(signed.expires)} · Signed ${signed.bundles_signed} bundle(s)${
                        signed.removed.length
                          ? `, removed ${signed.removed.length}`
                          : ""
                      }. Install it below.`
                    : preparation?.profiles.length
                      ? `${date(
                          preparation.profiles
                            .map((profile) => profile.expires)
                            .sort()[0],
                        )} · ${
                          certificate
                            ? "Sign to produce an installable build. The original IPA is never changed."
                            : "Get a signing certificate in step 3 before signing."
                        }`
                      : app?.profile?.expires_at
                        ? `${date(app.profile.expires_at)} · ${app.profile.expired ? "Expired" : "Renewal not implemented"}`
                        : "Re-signing requires a signing certificate and prepared profiles. Use the existing-signature flow below for an authorized build.")}
            </p>
            {signed && (
              <details className="signing-log">
                <summary>What signing did</summary>
                <pre>{(signed.log ?? []).join("\n")}</pre>
              </details>
            )}
          </div>
          <button
            className="primary"
            disabled={
              !isTauri() ||
              signing ||
              !ipaPath ||
              !certificate ||
              !preparation?.profiles.length ||
              preparation.plan.blockers.length > 0
            }
            onClick={() => {
              setSigning(true);
              setSignError(null);
              setSigned(null);
              setSignStep(null);
              const progress = new Channel<SigningProgress>();
              progress.onmessage = setSignStep;
              invoke<Signed>("account_sign_ipa", {
                path: ipaPath,
                watch,
                progress,
              })
                .then(setSigned)
                .catch((error) =>
                  setSignError(
                    typeof error === "string"
                      ? error
                      : "Signing did not complete.",
                  ),
                )
                .finally(() => {
                  setSigning(false);
                  setSignStep(null);
                });
            }}
          >
            {signing ? "Signing…" : "Sign IPA"} <ArrowRight size={17} />
          </button>
        </section>
        <InstallSigned
          path={signed ? signed.path : ipaPath}
          signed={signed !== null}
          deviceId={deviceId}
          onBusy={installationBusy}
        />
        {report && (
          <details className="card inventory">
            <summary>
              Bundle inspection details{" "}
              <span>{report.bundles.length} bundles</span>
              <ChevronDown size={16} />
            </summary>
            <p className="hint">
              Profile device counts are embedded snapshots, not portal inventory
              or remaining quota. Profile entitlements are authorizations;
              executable entitlements are reported separately. Signatures and
              CMS trust have not been verified.
            </p>
            {report.bundles.map((b) => (
              <BundleDetails key={b.path} bundle={b} />
            ))}
          </details>
        )}
        <footer>
          <span>
            Orbiter <span className="footer-dot">/</span> Company build
            workspace
          </span>
          <span>Inspection & install transport · macOS & Windows targets</span>
        </footer>
      </main>
    </div>
  );
}
function BundleDetails({ bundle: b }: { bundle: Bundle }) {
  return (
    <details className="bundle-detail">
      <summary>
        <span className="bundle-kind">{b.kind}</span>
        <span>{b.identifier}</span>
        <ChevronDown size={14} />
      </summary>
      <div className="bundle-body">
        <code>{b.path}</code>
        <p>
          Platforms: {b.supported_platforms.join(", ") || "Unspecified"} ·
          Device families: {b.device_families.join(", ") || "Unspecified"} (app
          metadata)
        </p>
        {b.issues.map((s, i) => (
          <p key={i}>{s}</p>
        ))}
        {b.profile && (
          <>
            <dl>
              <div>
                <dt>Profile</dt>
                <dd>{b.profile.name ?? "Unnamed"}</dd>
              </div>
              <div>
                <dt>Original team</dt>
                <dd>
                  {b.profile.team_name} ({b.profile.team_id ?? "Unknown"})
                </dd>
              </div>
              <div>
                <dt>Distribution</dt>
                <dd>{b.profile.distribution}</dd>
              </div>
              <div>
                <dt>Expires</dt>
                <dd>{date(b.profile.expires_at)}</dd>
              </div>
              <div>
                <dt>Allowlist snapshot</dt>
                <dd>{b.profile.device_count ?? "No"} devices</dd>
              </div>
            </dl>
            <p>{b.profile.trust}</p>
            <h4>Profile authorizations</h4>
            <pre>{JSON.stringify(b.profile.entitlements, null, 2)}</pre>
          </>
        )}
        {b.slices.map((s, i) => (
          <div key={i}>
            <h4>
              {s.architecture} ·{" "}
              {s.encrypted ? "Encrypted" : "No encryption flag"}
            </h4>
            <p>
              {s.xml_entitlements_present
                ? "Executable XML entitlements"
                : s.der_entitlements_present
                  ? "DER entitlements present; decoding unavailable"
                  : "No XML or DER entitlements found"}
            </p>
            {s.xml_entitlements_present && (
              <pre>{JSON.stringify(s.entitlements, null, 2)}</pre>
            )}
          </div>
        ))}
      </div>
    </details>
  );
}
createRoot(document.getElementById("root")!).render(
  <React.StrictMode>
    <App />
  </React.StrictMode>,
);
