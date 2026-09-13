import { Checkbox } from "../../components/ui/Checkbox";
import { useEffect, useRef, useState } from "react";
import {
  cancelInstall,
  channel,
  discardInstall,
  executeInstall,
  installationStatus,
  isTauri,
  prepareInstall,
  libraryPrepareInstall,
  libraryChanged,
} from "../../ipc/commands";
import type { Job, Review } from "../../types";
import { ArrowRight, LoaderCircle } from "lucide-react";
import { message } from "../../ipc/failure";
export function InstallSigned({
  paused = false,
  artifactId,
  path,
  signed,
  deviceId,
  onBusy,
}: {
  paused?: boolean;
  artifactId?: string;
  path: string | null;
  /// True when `path` is the build Orbiter just signed rather than the chosen IPA.
  signed: boolean;
  deviceId: number | null;
  onBusy: (busy: boolean) => void;
}) {
  const [review, setReview] = useState<Review | null>(null),
    [job, setJob] = useState<Job | null>(null),
    [working, setWorking] = useState(false),
    [preparing, setPreparing] = useState(false),
    [accepted, setAccepted] = useState(false),
    [error, setError] = useState("");
  const operation = useRef(false);
  const generation = useRef(0);
  const current = useRef<Review | null>(null);
  function discard() {
    const old = current.current;
    current.current = null;
    if (old) void discardInstall(old.token).catch(() => {});
  }
  useEffect(() => {
    generation.current++;
    discard();
    setReview(null);
    setAccepted(false);
    setError("");
    return () => {
      generation.current++;
      discard();
    };
  }, [path, deviceId, artifactId]);
  useEffect(() => {
    if (!isTauri()) return;
    let disposed = false;
    let timer: ReturnType<typeof setTimeout> | undefined;
    async function readStatus() {
      if (disposed) return;
      if (operation.current) {
        timer = setTimeout(readStatus, 1000);
        return;
      }
      try {
        const status = await installationStatus();
        if (disposed) return;
        if (operation.current) {
          timer = setTimeout(readStatus, 1000);
          return;
        }
        setJob(status ?? null);
        const active =
          !!status &&
          ["preparing", "transferring", "installing"].includes(status.stage);
        setWorking(active);
        onBusy(active);
        if (active) timer = setTimeout(readStatus, 1000);
      } catch {
        if (!disposed)
          setError(
            "Could not read the previous installation status. Check the phone before retrying.",
          );
      }
    }
    void readStatus();
    return () => {
      disposed = true;
      clearTimeout(timer);
    };
  }, [onBusy]);
  async function prepare() {
    if (paused || !path || deviceId === null) return;
    const version = ++generation.current;
    discard();
    setReview(null);
    setAccepted(false);
    setError("");
    operation.current = true;
    setPreparing(true);
    onBusy(true);
    try {
      const next = await (artifactId
        ? libraryPrepareInstall(artifactId, deviceId)
        : prepareInstall(path, deviceId));
      if (generation.current !== version) {
        void discardInstall(next.token);
        return;
      }
      current.current = next;
      setReview(next);
    } catch (e) {
      if (generation.current === version) setError(message(e));
    } finally {
      operation.current = false;
      setPreparing(false);
      onBusy(false);
      libraryChanged();
    }
  }
  async function install() {
    if (paused || !review || !accepted || review.blockers.length) return;
    operation.current = true;
    setWorking(true);
    onBusy(true);
    setError("");
    setJob(null);
    const progress = channel<Job>(setJob);
    try {
      setJob(await executeInstall(review.token, accepted, progress));
    } catch (e) {
      setError(message(e));
      try {
        setJob(await installationStatus());
      } catch {
        /* Keep the actionable execution error. */
      }
    } finally {
      operation.current = false;
      setWorking(false);
      onBusy(false);
      libraryChanged();
      setReview(null);
      current.current = null;
      setAccepted(false);
    }
  }
  async function cancel() {
    try {
      const accepted = await cancelInstall();
      if (!accepted)
        setError(
          "iOS installation has already started or the job finished. Check the reported outcome.",
        );
    } catch {
      setError(
        "Could not request cancellation. Keep the phone connected and wait for the result.",
      );
    }
  }
  return (
    <section className="card signed-install">
      <div className="section-heading">
        <h2>
          {signed ? "Install the signed build" : "Install existing signature"}
        </h2>
        <span className="subtle">
          {signed ? "SIGNED BUILD" : "TRANSPORT PREVIEW"}
        </span>
      </div>
      <div className="install-body">
        <p>
          {signed
            ? "Install the build Orbiter just signed on the selected USB iPhone. The review below checks it the same way as any other IPA, including whether this iPhone is in its profile."
            : "Install the selected IPA unchanged on the selected USB iPhone. No Apple account is needed. The build must already be provisioned for this device."}
        </p>
        <button
          className="review-button"
          disabled={
            paused ||
            !isTauri() ||
            !path ||
            deviceId === null ||
            preparing ||
            working
          }
          onClick={prepare}
        >
          {preparing ? (
            <>
              <LoaderCircle size={16} className="spin" />
              Checking IPA and iPhone…
            </>
          ) : (
            <>
              Review installation <ArrowRight size={16} />
            </>
          )}
        </button>
        {review && (
          <div className="install-review">
            <h3>
              {review.app_name} {review.version} → {review.device_name}
            </h3>
            <code>{review.bundle_id}</code>
            <p>
              {(review.size_bytes / 1024 / 1024).toFixed(1)} MB · Review expires
              in 10 minutes
            </p>
            <p>
              {review.existing_app
                ? `An app with this bundle ID is already installed (${review.existing_app.version ?? "unknown version"}). This installation may replace it.`
                : "No app with this bundle ID was found at review time."}
            </p>
            <details>
              <summary>Reviewed IPA fingerprint</summary>
              <code>{review.sha256}</code>
            </details>
            {review.notes.map((note, i) => (
              <p key={i}>{note}</p>
            ))}
            {review.blockers.length > 0 ? (
              <div className="install-blockers" role="alert">
                <strong>Installation blocked</strong>
                <ul>
                  {review.blockers.map((b, i) => (
                    <li key={i}>{b}</li>
                  ))}
                </ul>
              </div>
            ) : (
              <>
                <Checkbox
                  checked={accepted}
                  onChange={(e) => setAccepted(e.target.checked)}
                  disabled={working}
                >
                  I authorize installation on this iPhone, including replacement
                  of the same app if present. I understand data retention is not
                  guaranteed.
                </Checkbox>
                <button
                  className="review-button"
                  disabled={paused || !accepted || working}
                  onClick={install}
                >
                  {signed ? "Install signed IPA" : "Install unchanged IPA"}{" "}
                  <ArrowRight size={16} />
                </button>
              </>
            )}
          </div>
        )}
        {error && (
          <p className="install-error" role="alert">
            {error}
          </p>
        )}
        {job && (
          <div className="install-job" role="status" aria-live="polite">
            <strong>
              {job.stage === "installed"
                ? "iOS reported installation complete"
                : job.stage === "unknown"
                  ? "Installation outcome unknown"
                  : `Installation: ${job.stage}`}
            </strong>
            <p>{job.message}</p>
            {signed && job.stage === "installed" && (
              // iOS refuses to run a build signed by a free personal team until the certificate
              // is trusted on the device. It is asked once per certificate, so a weekly re-sign
              // with the same certificate does not ask again.
              <div className="install-next">
                <strong>
                  Trust the developer on the iPhone before launching
                </strong>
                <p>
                  Settings → General → VPN &amp; Device Management → the
                  developer entry for your Apple account → Trust. The iPhone
                  needs internet access to verify it. This is asked once per
                  signing certificate, not once per app.
                </p>
              </div>
            )}
            {job.stage === "transferring" && (
              <>
                <progress max={job.total_bytes} value={job.transferred_bytes} />
                <p>
                  {(job.transferred_bytes / 1024 / 1024).toFixed(1)} /{" "}
                  {(job.total_bytes / 1024 / 1024).toFixed(1)} MB transferred
                </p>
              </>
            )}
            {job.stage === "installing" && job.device_percent !== null && (
              <p>iOS-reported progress: {job.device_percent}%</p>
            )}
            {working &&
              (job.stage === "preparing" || job.stage === "transferring") && (
                <button className="text-button" onClick={cancel}>
                  Cancel transfer
                </button>
              )}
          </div>
        )}
      </div>
    </section>
  );
}
