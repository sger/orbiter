import { Compatibility } from "./Compatibility";
import { SigningPanel } from "./SigningPanel";
import { useSigning } from "./useSigning";
import { date } from "./format";
import { useInspection } from "./useInspection";
import { Accounts } from "../team/Accounts";
import { useCallback, useEffect, useRef, useState } from "react";
import {
  Box,
  Check,
  ChevronDown,
  FolderOpen,
  Info,
  Layers3,
  LoaderCircle,
  ShieldCheck,
  Upload,
  X,
} from "lucide-react";
import type { Bundle, Preparation, TeamStatus } from "../../types";
import {
  hasWatchApp,
  signBlocked,
  stages,
  type Pipeline,
} from "../../state/pipeline";
import { Devices } from "../device/Devices";
import { InstallSigned } from "../install/InstallSigned";
import { DeviceLog } from "../diagnose/DeviceLog";
import { HelpPanel } from "../help/HelpPanel";
import { SigningProgress } from "./SigningProgress";
const inspectionStages = [
  "Checking archive",
  "Reading bundles and signatures",
  "Assessing compatibility",
];
/// No account, no team, nothing registered. The starting point every session returns to on
/// sign-out, and what the pipeline reads before anyone has signed in.
const noTeam: TeamStatus = {
  signedIn: false,
  account: null,
  teamId: null,
  teamLabel: null,
  registered: false,
  registrationSummary: "",
  certificate: false,
  certificateSummary: "",
  watch: "undecided",
};
export function Workspace({
  onAccount,
}: {
  onAccount: (account: string | null) => void;
}) {
  const [team, setTeam] = useState<TeamStatus>(noTeam);
  useEffect(() => onAccount(team.account), [team.account, onAccount]);
  const { signed, signing, signError, signStep, invalidate, sign } =
    useSigning();
  // Stable: Accounts clears the preparation whenever this identity changes, so an inline closure
  // here would wipe the result on every render.
  const prepared = useCallback(
    (result: Preparation | null) => {
      // A new plan invalidates anything signed under the previous one.
      invalidate();
      setPreparation(result);
    },
    [invalidate],
  );
  const [preparation, setPreparation] = useState<Preparation | null>(null);
  const [deviceId, setDeviceId] = useState<number | null>(null);
  const [installBusy, setInstallBusy] = useState(false);
  const installActive = useRef(false);
  const installationBusy = useCallback((value: boolean) => {
    installActive.current = value;
    setInstallBusy(value);
  }, []);
  const {
    report,
    busy,
    stage,
    error,
    setError,
    drag,
    cancelled,
    ipaPath,
    desktop,
    choose,
    cancel,
  } = useInspection(installActive);
  const app = report?.bundles.find((b) => b.path === report.main_path);
  const [help, setHelp] = useState(false);
  const [helpSection, setHelpSection] = useState<string | null>(null);
  const openHelp = useCallback((section: string) => {
    setHelpSection(section);
    setHelp(true);
  }, []);
  const pipeline: Pipeline = {
    report,
    ipaPath,
    deviceId,
    team,
    preparation,
    signed,
  };
  const blocked = signBlocked(pipeline);
  const steps = stages(pipeline);
  const frameworks =
    report?.bundles.filter((b) => b.kind === "Framework").length ?? 0;
  const nested =
    report?.bundles.filter(
      (b) => b.kind !== "Framework" && b.kind !== "Main app",
    ).length ?? 0;
  return (
    <>
      <h1 className="page-title" tabIndex={-1}>
        IPAs
      </h1>
      <SigningProgress stages={steps} />
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
          <section className="card source" tabIndex={-1} data-stage="build">
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
                      {((report?.size_bytes ?? 0) / 1024 / 1024).toFixed(1)} MB
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
            <Accounts
              paused={installBusy}
              deviceId={deviceId}
              ipaPath={ipaPath}
              hasWatchApp={hasWatchApp(report)}
              onPrepared={prepared}
              onStatus={setTeam}
              onHelp={openHelp}
            />
            <p className="hint">
              A different account on the same company team shares that team's
              device allowance. Personal teams have their own limits and
              capability restrictions.
            </p>
          </section>
        </section>
        <Compatibility report={report} preparation={preparation} />
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
            {inspectionStages.map((s, i) => (
              <span
                className={
                  report || inspectionStages.indexOf(stage) >= i
                    ? "reached"
                    : ""
                }
                key={s}
              >
                {i + 1}. {s}
              </span>
            ))}
          </div>
        </div>
      )}
      <SigningPanel
        signing={signing}
        signed={signed}
        signError={signError}
        signStep={signStep}
        preparation={preparation}
        app={app}
        blocked={blocked}
        desktop={desktop}
        onSign={() => void sign(ipaPath!, team.watch)}
      />
      <InstallSigned
        path={signed ? signed.path : ipaPath}
        signed={signed !== null}
        deviceId={deviceId}
        onBusy={installationBusy}
      />
      {signed && (
        <section className="card diagnostics">
          <DeviceLog
            deviceId={deviceId}
            subjects={[signed.identifier, app?.name ?? ""].filter(Boolean)}
            superseded={[app?.identifier ?? ""].filter(Boolean)}
            disabled={installBusy}
          />
        </section>
      )}
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
            executable entitlements are reported separately. Signatures and CMS
            trust have not been verified.
          </p>
          {report.bundles.map((b) => (
            <BundleDetails key={b.path} bundle={b} />
          ))}
        </details>
      )}
      <footer>
        <span>Orbiter</span>
      </footer>
      <HelpPanel
        open={help}
        section={helpSection}
        onClose={() => setHelp(false)}
      />
    </>
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
