import { useCallback, useEffect, useRef, useState } from "react";
import { open } from "@tauri-apps/plugin-dialog";
import {
  ArrowLeft,
  ArrowRight,
  Check,
  LoaderCircle,
  Smartphone,
  Upload,
} from "lucide-react";
import { Checkbox } from "../../components/ui/Checkbox";
import { Select } from "../../components/ui/Select";
import { TextField } from "../../components/ui/TextField";
import { Accounts } from "../team/Accounts";
import { Devices } from "../device/Devices";
import { AppIcon } from "../library/AppIcon";
import { useLibraryDrop } from "../library/useLibraryDrop";
import type { Opened } from "../library/types";
import type { AccountView, TeamStatus, WatchChoice } from "../../types";
import {
  isTauri,
  libraryChanged,
  libraryImport,
  libraryOpen,
  withdrawCertificates,
} from "../../ipc/commands";
import { message } from "../../ipc/failure";
import { useGuidedFlow } from "./useGuidedFlow";
import { HelpPanel } from "../help/HelpPanel";
import type { Consents } from "./types";
import { useLibraryExpiry } from "../renew/useLibraryExpiry";
import { ExpiryLine } from "../renew/ExpiryLine";
import "./guided.css";
const noConsent: Consents = {
  registration: false,
  certificate: false,
  provisioning: false,
};
const ignore = () => {};

export function GuidedWorkspace({
  selected,
  visible,
  onBusy,
  onOperation,
  onAccount,
  onOpen,
}: {
  selected?: Opened | null;
  visible: boolean;
  onBusy: (value: boolean) => void;
  onOperation: (value: string | null) => void;
  onAccount: (value: string | null) => void;
  onOpen: (value: Opened) => void;
}) {
  const [deviceId, setDeviceId] = useState<number | null>(null);
  const [accountView, setAccountView] = useState<AccountView | null>(null);
  const [accountBusy, setAccountBusy] = useState(false);
  const [importing, setImporting] = useState(false);
  const [importSummary, setImportSummary] = useState("");
  const importLock = useRef(false);
  const [watch, setWatch] = useState<WatchChoice>("undecided"),
    [marker, setMarker] = useState("test");
  const [consents, setConsents] = useState<Consents>(noConsent),
    [accepted, setAccepted] = useState(false);
  const [withdrawAck, setWithdrawAck] = useState(false);
  const [help, setHelp] = useState<string | null>(null);
  const flow = useGuidedFlow(
    selected,
    deviceId,
    `${accountView?.account ?? ""}/${accountView?.selected_team ?? ""}/${accountView?.stage ?? ""}`,
    watch,
    marker,
  );
  const locked = flow.busy || accountBusy || importing;
  useEffect(() => {
    onOperation(
      flow.busy && ["install", "result"].includes(flow.stage)
        ? "Installation in progress"
        : flow.busy && flow.stage === "prepare" && flow.progress
          ? "Signing in progress"
          : null,
    );
  }, [flow.busy, flow.stage, flow.progress, onOperation]);
  const item = flow.output ?? (flow.recovered ? null : selected);
  const app = item?.report.bundles.find(
    (bundle) => bundle.path === item.report.main_path,
  );
  const selectedTeam = accountView?.teams.find(
    (team) => team.id === accountView.selected_team,
  );
  const supportedTeam =
    !!selectedTeam &&
    selectedTeam.free !== null &&
    !selectedTeam.membership?.toLowerCase().includes("enterprise");
  const hasWatch =
    selected?.report.bundles.some((bundle) => bundle.kind === "Watch app") ??
    false;
  const sourceMissing = !!selected?.artifact.source_id;
  const { expiry, refresh } = useLibraryExpiry(
    item?.artifact.id ?? null,
    accountView?.selected_team ?? null,
  );
  useEffect(() => {
    onBusy(locked);
  }, [locked, onBusy]);
  useEffect(() => {
    if (flow.job && !flow.busy) refresh();
  }, [flow.job?.stage, flow.busy, refresh]);
  useEffect(() => {
    setAccepted(false);
  }, [flow.review?.token]);
  useEffect(() => {
    setConsents(noConsent);
    setWithdrawAck(false);
  }, [flow.preparation?.token]);
  const heading = useRef<HTMLHeadingElement>(null);
  useEffect(() => {
    if (visible) heading.current?.focus();
  }, [flow.stage, visible]);
  const accountStatus = useCallback(
    (status: TeamStatus) => onAccount(status.account),
    [onAccount],
  );

  async function importPaths(paths: string[]) {
    if (locked || importLock.current || !paths.length) return;
    importLock.current = true;
    setImporting(true);
    flow.setError("");
    const errors: string[] = [];
    let first: Opened | null = null;
    try {
      for (const [index, path] of paths.entries()) {
        setImportSummary(
          `Importing ${index + 1} of ${paths.length}: ${path.split(/[\\/]/).pop()}`,
        );
        try {
          if (!path.toLowerCase().endsWith(".ipa"))
            throw new Error("Choose an IPA file.");
          const imported = await libraryImport(path);
          if (!first) first = await libraryOpen(imported.artifact_id);
        } catch (error) {
          errors.push(message(error));
        }
      }
      libraryChanged();
      if (first) onOpen(first);
      setImportSummary(
        `Imported ${paths.length - errors.length} of ${paths.length} selected files.${errors.length ? ` ${errors.length} failed: ${errors.join(" ")}` : ""}`,
      );
    } finally {
      importLock.current = false;
      setImporting(false);
    }
  }
  const dragging = useLibraryDrop(
    visible && flow.stage === "choose" && !locked,
    (paths) => void importPaths(paths),
    flow.setError,
    true,
  );
  async function choose() {
    try {
      const paths = await open({
        multiple: true,
        filters: [{ name: "iOS application", extensions: ["ipa"] }],
      });
      if (paths) await importPaths(Array.isArray(paths) ? paths : [paths]);
    } catch (error) {
      flow.setError(message(error));
    }
  }
  async function signInstead() {
    if (locked) return;
    if (selected?.artifact.source_id) {
      setImporting(true);
      try {
        onOpen(await libraryOpen(selected.artifact.source_id));
      } catch {
        flow.setError(
          "The original source is unavailable. Import its original IPA to sign again.",
        );
      } finally {
        setImporting(false);
      }
    } else flow.navigate("account");
  }
  async function withdraw() {
    if (!withdrawAck || locked) return;
    setImporting(true);
    try {
      await withdrawCertificates(true);
      setWithdrawAck(false);
      flow.setError(
        "Certificate withdrawn. Review preparation again before creating a replacement.",
      );
    } catch (error) {
      flow.setError(message(error));
    } finally {
      setImporting(false);
    }
  }
  const stageIndex = {
    choose: 0,
    check: 1,
    account: 1,
    prepare: 1,
    review: 2,
    install: 3,
    result: 3,
  }[flow.stage];
  const title = {
    choose: "Choose an app and iPhone",
    check:
      flow.review?.readiness === "direct"
        ? "Ready for installation review"
        : "Check app and iPhone",
    account:
      accountView?.stage === "signed_in"
        ? "Choose your signing team"
        : "Sign in to prepare this app",
    prepare: "Prepare & sign",
    review: "Review installation",
    install: "Installing on your iPhone",
    result:
      flow.job?.stage === "installed"
        ? "Installation complete"
        : "Installation result",
  }[flow.stage];
  return (
    <div className="guided-flow">
      <h1 className="page-title" tabIndex={-1}>
        Install an app
      </h1>
      <ol className="guided-steps" aria-label="Installation steps">
        {["Choose", "Prepare if needed", "Review", "Install"].map(
          (label, index) => (
            <li
              key={label}
              aria-current={stageIndex === index ? "step" : undefined}
              data-done={index < stageIndex}
            >
              <span>
                {index < stageIndex ? <Check size={14} /> : index + 1}
              </span>
              {label}
            </li>
          ),
        )}
      </ol>
      <div className="guided-layout">
        <section className="card guided-card">
          <div className="guided-heading">
            <p className="guided-eyebrow">
              {flow.stage === "account" ? "Apple account" : "Local workspace"}
            </p>
            <h2 ref={heading} tabIndex={-1}>
              {title}
            </h2>
          </div>
          {flow.stage === "choose" && (
            <div className="guided-section">
              <p>
                Select an IPA and a connected iPhone. Orbiter will check which
                steps are needed.
              </p>
              <button
                className={
                  selected
                    ? "secondary"
                    : `guided-drop ${dragging ? "drag" : ""}`
                }
                disabled={locked || !isTauri()}
                onClick={() => void choose()}
              >
                <Upload size={24} />
                <strong>
                  {importing
                    ? "Importing selected files…"
                    : selected
                      ? "Import another IPA"
                      : "Drop IPA files here"}
                </strong>
                {!selected && <span>or browse files on your Mac</span>}
              </button>
              <a className="text-button" href="#/ipas">
                Choose a saved version from App Library
              </a>
              {selected && (
                <div className="guided-app">
                  <AppIcon
                    src={selected.report.icon_data_url}
                    name={selected.artifact.name}
                  />
                  <div>
                    <strong>{selected.artifact.name}</strong>
                    <p>
                      {selected.artifact.version ?? "Unknown version"} ·{" "}
                      {selected.artifact.build ?? "Unknown build"}
                    </p>
                  </div>
                </div>
              )}
            </div>
          )}
          <div hidden={flow.stage !== "choose"} className="guided-section">
            <Devices
              onSelect={setDeviceId}
              paused={locked || flow.stage !== "choose"}
            />
          </div>
          {flow.stage === "choose" && (
            <div className="guided-actions">
              <button
                className="primary"
                disabled={locked || !selected || deviceId === null}
                onClick={() => void flow.check()}
              >
                Check app & iPhone <ArrowRight size={16} />
              </button>
            </div>
          )}
          {flow.stage === "check" && (
            <div className="guided-section">
              {flow.busy && (
                <p role="status">
                  <LoaderCircle size={18} className="spin" /> Checking the saved
                  IPA and verified iPhone…
                </p>
              )}
              {flow.review && (
                <>
                  {flow.review.readiness === "direct" ? (
                    <>
                      <p>
                        This IPA passed the local installation checks. Continue
                        using its existing signature, or re-sign a copy with
                        your Apple account.
                      </p>
                      <p className="hint">
                        iOS will validate its signatures during installation.
                        Passing these checks does not guarantee the app will
                        launch.
                      </p>
                      <button
                        className="primary"
                        disabled={locked}
                        onClick={() => flow.navigate("review")}
                      >
                        Review installation <ArrowRight size={16} />
                      </button>
                    </>
                  ) : (
                    <>
                      <p>
                        {flow.review.readiness === "needs_signing"
                          ? "This app needs a profile prepared for your iPhone. Signing may resolve these issues."
                          : "Resolve these issues before continuing."}
                      </p>
                      <ul>
                        {flow.review.blockers.map((issue, i) => (
                          <li key={i}>{issue}</li>
                        ))}
                      </ul>
                    </>
                  )}
                  {flow.review.readiness !== "blocked" && (
                    <button
                      className={
                        flow.review.readiness === "direct"
                          ? "text-button"
                          : "primary"
                      }
                      disabled={locked}
                      onClick={() => void signInstead()}
                    >
                      {sourceMissing
                        ? "Open original to sign again"
                        : flow.review.readiness === "direct"
                          ? "Re-sign with my Apple account"
                          : "Continue with Apple account"}
                    </button>
                  )}
                </>
              )}
              {!flow.busy && !flow.review && (
                <button className="secondary" onClick={() => void flow.check()}>
                  Check again
                </button>
              )}
              {!flow.busy && (
                <button
                  className="text-button"
                  onClick={() => flow.navigate("choose")}
                >
                  <ArrowLeft size={16} /> Change app or iPhone
                </button>
              )}
            </div>
          )}
          <div hidden={flow.stage !== "account"} className="guided-section">
            <Accounts
              authenticationOnly
              onView={setAccountView}
              paused={flow.busy || importing}
              onBusy={setAccountBusy}
              deviceId={deviceId}
              ipaPath={selected?.path ?? null}
              hasWatchApp={hasWatch}
              onPrepared={ignore}
              onStatus={accountStatus}
              onHelp={setHelp}
            />
            {accountView?.stage === "signed_in" && (
              <>
                <p className="hint">
                  Membership comes from Apple’s team information, not your email
                  address. Profiles and permissions determine what this team can
                  sign.
                </p>
                {selectedTeam && !supportedTeam && (
                  <p role="alert">
                    This team’s membership is unknown or unsupported. Refresh
                    teams or select a Personal Team or Apple Developer Program
                    team.
                  </p>
                )}
                {hasWatch && (
                  <div className="guided-option">
                    <label htmlFor="guided-watch">Included Watch app</label>
                    <Select
                      id="guided-watch"
                      value={watch}
                      disabled={locked}
                      onChange={(value) => setWatch(value as WatchChoice)}
                    >
                      <option value="undecided">
                        Choose what happens to the Watch app
                      </option>
                      <option value="remove">Remove the Watch app</option>
                      <option value="sign">Keep and sign the Watch app</option>
                    </Select>
                    <p className="hint">
                      Keeping it requires additional profiles and identifiers.
                      Watch support must be verified for your team.
                    </p>
                  </div>
                )}
                <details className="guided-advanced">
                  <summary>Advanced signing options</summary>
                  <TextField
                    id="guided-marker"
                    label="App name marker"
                    value={marker}
                    disabled={locked}
                    maxLength={40}
                    onChange={(event) => setMarker(event.target.value)}
                  />
                  <p className="hint">
                    Leave empty to keep the original display name. Required
                    capability changes are shown before signing.
                  </p>
                </details>
                <button
                  className="primary"
                  disabled={
                    locked ||
                    !supportedTeam ||
                    deviceId === null ||
                    !selected ||
                    (hasWatch && watch === "undecided") ||
                    sourceMissing
                  }
                  onClick={() => void flow.plan()}
                >
                  Review preparation <ArrowRight size={16} />
                </button>
              </>
            )}
            <button
              className="text-button"
              disabled={locked}
              onClick={() => flow.navigate("choose")}
            >
              <ArrowLeft size={16} /> Back to app and iPhone
            </button>
          </div>
          {flow.stage === "prepare" && (
            <div className="guided-section">
              {flow.progress && (
                <div role="status" aria-live="polite">
                  <strong>{flow.progress.message}</strong>
                  <ol className="guided-progress">
                    {flow.progress.completed.map((step) => (
                      <li key={step}>
                        <Check size={15} />
                        {step}
                      </li>
                    ))}
                  </ol>
                </div>
              )}
              {flow.busy && !flow.progress && (
                <p role="status">
                  Checking team resources. Nothing is being registered or
                  created.
                </p>
              )}
              {flow.preparation && (
                <>
                  <p>
                    {selectedTeam?.free
                      ? "Personal Team"
                      : (selectedTeam?.membership ?? "Signing team")}{" "}
                    · {selectedTeam?.name}
                  </p>
                  <p>
                    Only the required actions below will run. Existing verified
                    resources are reused.
                  </p>
                  <ul className="guided-resource-list">
                    <li>
                      {flow.preparation.registration
                        ? "Register this iPhone on the selected team"
                        : "Reuse this iPhone’s existing registration"}
                    </li>
                    <li>
                      {flow.preparation.certificate
                        ? "Obtain a development certificate for this Mac"
                        : "Reuse the existing signing key and certificate"}
                    </li>
                    <li>
                      {flow.preparation.provisioning
                        ? "Reserve missing app identifiers and obtain signing profiles"
                        : "Reuse verified signing profiles"}
                    </li>
                    <li>Sign a managed copy and save it to your library</li>
                  </ul>
                  <p className="hint">
                    Signed identifier:{" "}
                    <code>{flow.preparation.plan.new_main_identifier}</code>
                  </p>
                  <ul>
                    {flow.preparation.plan.consequences.map(
                      (consequence, i) => (
                        <li key={i}>{consequence}</li>
                      ),
                    )}
                  </ul>
                  <details>
                    <summary>Capability changes</summary>
                    {flow.preparation.plan.bundles.map((bundle) => (
                      <div key={bundle.identifier}>
                        <strong>{bundle.name}</strong>
                        <ul>
                          {bundle.capabilities
                            .filter(
                              (capability) =>
                                capability.consequence ||
                                capability.action === "remove",
                            )
                            .map((capability) => (
                              <li key={capability.key}>
                                {capability.key}:{" "}
                                {capability.consequence || capability.reason}
                              </li>
                            ))}
                        </ul>
                      </div>
                    ))}
                  </details>
                  {flow.preparation.plan.blockers.length > 0 ? (
                    <ul role="alert">
                      {flow.preparation.plan.blockers.map((blocker, i) => (
                        <li key={i}>{blocker}</li>
                      ))}
                    </ul>
                  ) : (
                    <>
                      {flow.preparation.registration && (
                        <Checkbox
                          checked={consents.registration}
                          disabled={locked}
                          onChange={(e) =>
                            setConsents({
                              ...consents,
                              registration: e.target.checked,
                            })
                          }
                        >
                          I approve registering this iPhone. Registration uses
                          this team’s device allowance; removing a paid-team
                          device does not restore its slot for the membership
                          year.
                        </Checkbox>
                      )}
                      {flow.preparation.certificate && (
                        <Checkbox
                          checked={consents.certificate}
                          disabled={locked}
                          onChange={(e) =>
                            setConsents({
                              ...consents,
                              certificate: e.target.checked,
                            })
                          }
                        >
                          I approve obtaining a development certificate, using a
                          certificate slot if needed. No existing certificate
                          will be revoked.
                        </Checkbox>
                      )}
                      <Checkbox
                        checked={consents.provisioning}
                        disabled={locked}
                        onChange={(e) =>
                          setConsents({
                            ...consents,
                            provisioning: e.target.checked,
                          })
                        }
                      >
                        {flow.preparation.provisioning
                          ? "I approve reserving missing identifiers, obtaining profiles, and signing with the changes shown above. Identifiers consume the team’s allowance and cannot be reused by another team."
                          : "I approve signing with the changes shown above using the existing profiles."}
                      </Checkbox>
                      <button
                        className="primary"
                        disabled={
                          locked ||
                          !consents.provisioning ||
                          (flow.preparation.registration &&
                            !consents.registration) ||
                          (flow.preparation.certificate &&
                            !consents.certificate)
                        }
                        onClick={() => void flow.prepareAndSign(consents)}
                      >
                        Prepare & sign <ArrowRight size={16} />
                      </button>
                    </>
                  )}
                </>
              )}
              {!locked && (
                <>
                  <button
                    className="text-button"
                    onClick={() => flow.navigate("account")}
                  >
                    <ArrowLeft size={16} /> Change signing setup
                  </button>
                  <button
                    className="secondary"
                    onClick={() => void flow.plan()}
                  >
                    Refresh preparation
                  </button>
                </>
              )}
              {!locked && flow.errorCode === "certificate_conflict" && (
                <details className="guided-advanced">
                  <summary>Certificate conflict recovery</summary>
                  <p>
                    Only use this if the team’s certificate limit prevents
                    signing and you cannot use the existing key. Withdrawal
                    affects apps signed outside Orbiter too.
                  </p>
                  <Checkbox
                    checked={withdrawAck}
                    onChange={(e) => setWithdrawAck(e.target.checked)}
                  >
                    I understand withdrawing this team’s development
                    certificates stops every app signed with them on every
                    device, and cannot be undone.
                  </Checkbox>
                  <button
                    className="secondary"
                    disabled={!withdrawAck || locked}
                    onClick={() => void withdraw()}
                  >
                    Withdraw development certificates
                  </button>
                </details>
              )}
            </div>
          )}
          {flow.stage === "review" && flow.review && (
            <div className="guided-section">
              <div className="guided-app">
                <AppIcon
                  src={item?.report.icon_data_url}
                  name={flow.review.app_name}
                />
                <div>
                  <strong>
                    {flow.review.app_name} {flow.review.version}
                  </strong>
                  <p>{flow.review.device_name}</p>
                </div>
              </div>
              <dl className="guided-facts">
                <div>
                  <dt>Bundle identifier</dt>
                  <dd>{flow.review.bundle_id}</dd>
                </div>
                <div>
                  <dt>Profile expiration</dt>
                  <dd>{item?.artifact.expires ?? "Unknown"}</dd>
                </div>
                <div>
                  <dt>IPA size</dt>
                  <dd>
                    {(flow.review.size_bytes / 1024 / 1024).toFixed(1)} MB
                  </dd>
                </div>
              </dl>
              <p>
                {flow.review.existing_app
                  ? `An app with this identifier is installed (${flow.review.existing_app.version ?? "unknown version"}). This may replace it.`
                  : "No app with this identifier was found at review time."}
              </p>
              {flow.review.notes.map((note, index) => (
                <p className="hint" key={index}>
                  {note}
                </p>
              ))}
              <details>
                <summary>Reviewed artifact fingerprint</summary>
                <code>{flow.review.sha256}</code>
                <p className="hint">
                  Review expires in 10 minutes. Orbiter rechecks its validity
                  before installation.
                </p>
              </details>
              <Checkbox
                checked={accepted}
                disabled={locked}
                onChange={(e) => setAccepted(e.target.checked)}
              >
                I authorize installation on this iPhone, including replacement
                of the same app if present. I understand data retention is not
                guaranteed.
              </Checkbox>
              <button
                className="primary"
                disabled={locked || !accepted}
                onClick={() => void flow.install(accepted)}
              >
                Install on iPhone <ArrowRight size={16} />
              </button>
              <button
                className="text-button"
                disabled={locked}
                onClick={() => flow.navigate("choose")}
              >
                <ArrowLeft size={16} /> Change app or iPhone
              </button>
            </div>
          )}
          {(flow.stage === "install" || flow.stage === "result") && (
            <div className="guided-section">
              <div role="status" aria-live="polite">
                <strong>
                  {flow.job?.stage === "unknown"
                    ? "Installation outcome unknown"
                    : (flow.job?.message ??
                      "Waiting for the backend’s installation result…")}
                </strong>
              </div>
              {flow.job?.stage === "transferring" && (
                <>
                  <progress
                    value={flow.job.transferred_bytes}
                    max={flow.job.total_bytes}
                  />
                  <p>
                    {(flow.job.transferred_bytes / 1024 / 1024).toFixed(1)} /{" "}
                    {(flow.job.total_bytes / 1024 / 1024).toFixed(1)} MB
                  </p>
                </>
              )}
              {flow.job?.stage === "installing" && (
                <p>
                  iOS is installing the app
                  {flow.job.device_percent === null
                    ? "."
                    : ` · ${flow.job.device_percent}%`}
                  . Keep the iPhone connected.
                </p>
              )}
              {flow.busy &&
                ["preparing", "transferring"].includes(
                  flow.job?.stage ?? "",
                ) && (
                  <button
                    className="secondary"
                    onClick={() => void flow.cancel()}
                  >
                    Cancel transfer
                  </button>
                )}
              {flow.job?.stage === "installed" && (
                <>
                  <p>
                    iOS confirmed installation. Open the app on your iPhone to
                    verify that it works.
                  </p>
                  {item?.artifact.source_id && (
                    <p className="hint">
                      If iOS asks you to trust the developer, open Settings →
                      General → VPN &amp; Device Management and review the
                      developer entry.
                    </p>
                  )}
                  <ExpiryLine expiry={expiry} variant="banner" />
                </>
              )}
              {flow.stage === "result" && flow.job?.stage !== "installed" && (
                <p>
                  Check the iPhone before trying again. Orbiter will require a
                  fresh review; it will not automatically repeat the
                  installation.
                </p>
              )}
              {!locked && (
                <button
                  className="secondary"
                  onClick={() => flow.navigate("choose")}
                >
                  Back to app and iPhone
                </button>
              )}
              <a
                className="text-button"
                href={
                  selected ? `#/ipas/${selected.artifact.app_id}` : "#/ipas"
                }
              >
                View in library
              </a>
            </div>
          )}
          {importSummary && flow.stage === "choose" && (
            <p className="guided-import-status" role="status">
              {importSummary}
            </p>
          )}
          {flow.error && (
            <p className="guided-error" role="alert">
              {flow.error}
            </p>
          )}
        </section>
        <aside className="guided-context" aria-label="Installation context">
          <div className="guided-app">
            <AppIcon
              src={item?.report.icon_data_url}
              name={item?.artifact.name ?? "Application"}
            />
            <div>
              <strong>
                {item?.artifact.name ?? flow.recoveryName ?? "No app selected"}
              </strong>
              <p>
                {item?.artifact.version ?? "Choose an IPA to begin"}
                {item?.artifact.build ? ` (${item.artifact.build})` : ""}
              </p>
            </div>
          </div>
          {app && (
            <p className="hint">
              <code>{app.identifier}</code>
            </p>
          )}
          <div className="guided-phone">
            <Smartphone size={20} />
            <span>
              {flow.review?.device_name ??
                flow.preparation?.device_name ??
                (deviceId === null
                  ? "Choose a connected iPhone"
                  : "USB iPhone selected")}
            </span>
          </div>
          <p className="hint">
            Original IPA unchanged. Signing creates a separate saved build.
          </p>
          <p className="hint">
            Only installations recorded by Orbiter appear in your library
            history.
          </p>
          <ExpiryLine expiry={expiry} variant="line" />
          {flow.stage !== "account" && (
            <button className="text-button" onClick={() => setHelp("account")}>
              What is stored
            </button>
          )}
          {selected && (
            <a
              className="text-button"
              href={`#/ipas/${selected.artifact.app_id}`}
            >
              App details
            </a>
          )}
        </aside>
      </div>
      {help && <HelpPanel open section={help} onClose={() => setHelp(null)} />}
    </div>
  );
}
