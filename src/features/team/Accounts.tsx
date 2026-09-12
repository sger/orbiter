import { useEffect, useRef, useState } from "react";
import {
  accountRefreshTeams,
  accountStatus,
  forgetSigningKey,
  isTauri,
  prepareProvisioning,
  registerDevice,
  requestCertificate,
  withdrawCertificates,
} from "../../ipc/commands";
import { invoke } from "@tauri-apps/api/core";
import { Users, Watch } from "lucide-react";
import { Select } from "../../app/Select";
import { Stage } from "../../app/Stage";
import type { StageState } from "../../state/pipeline";
import type {
  AccountView,
  Certificate,
  Preparation,
  Registration,
  TeamStatus,
  WatchChoice,
} from "../../types";

const initial: AccountView = {
  stage: "signed_out",
  account: null,
  teams: [],
  selected_team: null,
  challenge: null,
  message: "",
};
export function Accounts({
  paused,
  deviceId,
  ipaPath,
  hasWatchApp,
  onPrepared,
  onStatus,
  onHelp,
}: {
  paused: boolean;
  deviceId: number | null;
  ipaPath: string | null;
  /// Whether the selected IPA contains a Watch app, so the choice is only asked when it applies.
  hasWatchApp: boolean;
  onPrepared: (preparation: Preparation | null) => void;
  /// Everything above this panel needs to know, in one shape: the pipeline derives its gates from
  /// it, the header shows the account, and the signer is given the same Watch choice the plan had.
  onStatus: (status: TeamStatus) => void;
  /// Opens the help panel at the section that explains this step.
  onHelp: (section: string) => void;
}) {
  const [view, setView] = useState<AccountView>(initial);
  const [email, setEmail] = useState("");
  const [password, setPassword] = useState("");
  const [code, setCode] = useState("");
  const [consent, setConsent] = useState(false);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [statusError, setStatusError] = useState<string | null>(null);
  const [ready, setReady] = useState(false);
  const [registerAck, setRegisterAck] = useState(false);
  const [registering, setRegistering] = useState(false);
  const [registration, setRegistration] = useState<Registration | null>(null);
  const [registerError, setRegisterError] = useState<string | null>(null);
  const [certAck, setCertAck] = useState(false);
  const [certBusy, setCertBusy] = useState(false);
  const [certificate, setCertificate] = useState<Certificate | null>(null);
  const [certError, setCertError] = useState<string | null>(null);
  const [withdrawAck, setWithdrawAck] = useState(false);
  const [provAck, setProvAck] = useState(false);
  // Apple's watchOS provisioning under a personal team is unverified and a Watch bundle spends
  // App IDs from a small weekly budget, so nothing is registered until this is chosen.
  const [watch, setWatch] = useState<WatchChoice>("undecided");
  const [provBusy, setProvBusy] = useState(false);
  const [preparation, setPreparation] = useState<Preparation | null>(null);
  const [provError, setProvError] = useState<string | null>(null);
  const inFlight = useRef(false);
  const mounted = useRef(false);
  const desktop = isTauri();
  // Backend messages are curated and allowlisted, and they name the actual cause — an account
  // limit, a refusal, what to check. Showing a generic sentence instead hides all of it.
  function reason(error: unknown, fallback: string) {
    const text = typeof error === "string" ? error.trim() : "";
    return text ? text.slice(0, 600) : fallback;
  }
  useEffect(() => {
    mounted.current = true;
    let polling = false;
    async function poll() {
      if (!desktop || inFlight.current || polling) return;
      polling = true;
      const epoch = generation.current;
      try {
        const next = await accountStatus();
        if (mounted.current && epoch === generation.current) {
          setView(next);
          setReady(true);
          setStatusError(null);
        }
      } catch {
        if (mounted.current && epoch === generation.current) {
          setReady(false);
          setStatusError(
            "Account status is unavailable. Restart Orbiter before signing in.",
          );
        }
      } finally {
        polling = false;
      }
    }
    void poll();
    const timer = window.setInterval(() => void poll(), 1000);
    return () => {
      mounted.current = false;
      window.clearInterval(timer);
    };
  }, [desktop]);
  const generation = useRef(0);
  const signedIn = view.stage === "signed_in";
  const team = view.teams.find(
    (candidate) => candidate.id === view.selected_team,
  );
  // One report upward, recomputed whenever any part of it changes.
  useEffect(() => {
    onStatus({
      signedIn,
      account: signedIn ? view.account : null,
      teamId: view.selected_team,
      teamLabel: team
        ? `${team.name}${
            team.free === true
              ? " · Free personal team"
              : team.free === false
                ? ` · ${team.membership ?? "Paid membership"}`
                : ""
          }`
        : null,
      registered: registration !== null,
      registrationSummary: registration?.message ?? "",
      certificate: certificate !== null,
      certificateSummary: certificate?.message ?? "",
      watch,
    });
  }, [
    onStatus,
    signedIn,
    view.account,
    view.selected_team,
    team,
    registration,
    certificate,
    watch,
  ]);
  useEffect(() => {
    setCode("");
  }, [view.challenge?.id]);
  useEffect(() => {
    // A result describes one device on one team; it must not outlive either choice.
    setRegistration(null);
    setRegisterError(null);
    setRegisterAck(false);
    setCertificate(null);
    setCertError(null);
    setCertAck(false);
    setPreparation(null);
    setProvError(null);
    setProvAck(false);
    onPrepared(null);
  }, [deviceId, view.selected_team, ipaPath, onPrepared]);
  async function command(name: string, args?: Record<string, unknown>) {
    if (inFlight.current) return;
    inFlight.current = true;
    generation.current += 1;
    setBusy(true);
    setError(null);
    try {
      const next = await invoke<AccountView>(name, args);
      if (mounted.current) setView(next);
    } catch (failure) {
      if (mounted.current)
        setError(
          reason(
            failure,
            "The account action did not complete. Check the current status and retry.",
          ),
        );
    } finally {
      inFlight.current = false;
      if (mounted.current) setBusy(false);
    }
  }
  /// A step collapses to its result once it is done, and stays open until then.
  ///
  /// It does not impose an order beyond the real one. Apple does not require a registered device
  /// before issuing a certificate, and identifiers can be registered before either; pretending
  /// otherwise would block work that is actually allowed. Only the prerequisites that genuinely
  /// exist — an account, and a chosen team — gate a step, and those already gate the whole panel.
  const stageState = (done: boolean, reachable: boolean): StageState =>
    done ? "done" : reachable ? "current" : "waiting";
  const teamLabel = team
    ? `${team.name}${
        team.free === true
          ? " · Free personal team"
          : team.free === false
            ? ` · ${team.membership ?? "Paid membership"}`
            : ""
      }`
    : null;
  const active = view.stage === "signing_in" || view.stage === "two_factor";
  const disabled = !desktop || !ready || busy || paused;
  const signInBlocker = !desktop
    ? "Open the desktop app to sign in."
    : !ready
      ? "Waiting for account service availability."
      : busy
        ? "An account action is in progress."
        : paused
          ? "Wait for the current operation to finish before signing in."
          : !consent
            ? "Check the agreement above to enable Apple authentication."
            : !email.trim()
              ? "Enter your Apple account email to sign in."
              : !password
                ? "Enter your Apple account password to sign in. Passwords are cleared after each attempt, so signing in again needs it retyped."
                : null;
  return (
    <section className="accounts" aria-label="Apple account authentication">
      <strong>Apple account</strong>
      {!active && !signedIn && (
        <form
          onSubmit={(e) => {
            e.preventDefault();
            if (signInBlocker) return;
            const enteredPassword = password;
            setPassword("");
            void command("account_sign_in", {
              email,
              password: enteredPassword,
              consent,
            });
          }}
        >
          {/* One line. What this means in full is in Help, one click away, rather than four
              paragraphs a person must read past every time to reach the password field. */}
          <p className="hint">
            Sign in with the Apple ID this build should be re-signed for.
            Orbiter talks to Apple directly; passwords are never saved.{" "}
            <button
              type="button"
              className="text-button underline"
              onClick={() => onHelp("account")}
            >
              What is stored
            </button>
          </p>
          <label className="auth-consent">
            <input
              type="checkbox"
              checked={consent}
              disabled={disabled}
              onChange={(e) => setConsent(e.target.checked)}
            />
            I agree to authenticate directly with Apple using local macOS
            support.
          </label>
          <div className="identity-grid">
            <div>
              <label htmlFor="account">Apple account email</label>
              <input
                id="account"
                type="email"
                autoComplete="username"
                maxLength={254}
                required
                disabled={disabled}
                value={email}
                onChange={(e) => setEmail(e.target.value)}
              />
            </div>
            <div>
              <label htmlFor="apple-password">Password</label>
              <input
                id="apple-password"
                type="password"
                autoComplete="current-password"
                maxLength={1024}
                required
                disabled={disabled}
                value={password}
                onChange={(e) => setPassword(e.target.value)}
              />
            </div>
          </div>
          {/* The action and the reason it is unavailable belong on one line: a button with its
              explanation stranded below it reads as two unrelated things. */}
          <div className="mt-3 flex flex-wrap items-center gap-x-3 gap-y-1.5">
            <button
              type="submit"
              className="secondary !mt-0"
              disabled={signInBlocker !== null}
              aria-describedby="sign-in-help"
            >
              Sign in to Apple
            </button>
            <p id="sign-in-help" className="hint min-w-0 flex-1" role="status">
              {signInBlocker ?? "Ready to sign in with Apple."}
            </p>
          </div>
        </form>
      )}
      {/* Only while it says something the stages do not. Once signed in they carry the state, and
          a third line repeating it is noise between the form and the first step. */}
      {(!signedIn || view.stage === "failed") && (
        <p className="hint" role="status" aria-live="polite">
          {desktop
            ? view.message
            : "Apple sign-in is available in the desktop app."}
        </p>
      )}
      {view.challenge && (
        <div className="two-factor">
          <p>
            {view.challenge.retry
              ? "The previous verification attempt was not accepted. Try again or choose another method."
              : view.challenge.sms
                ? "Enter the code sent to your trusted phone."
                : "Enter the code shown on a trusted Apple device."}
          </p>
          {!view.challenge.unknown && (
            <form
              onSubmit={(e) => {
                e.preventDefault();
                const value = code;
                setCode("");
                void command("account_answer", {
                  challengeId: view.challenge!.id,
                  answer: { action: "code", value },
                });
              }}
            >
              <label htmlFor="verification-code">Verification code</label>
              <input
                id="verification-code"
                inputMode="numeric"
                autoComplete="one-time-code"
                maxLength={6}
                value={code}
                disabled={disabled}
                onChange={(e) => setCode(e.target.value.replace(/\D/g, ""))}
              />
              <button
                type="submit"
                className="secondary"
                disabled={disabled || !/^\d{6}$/.test(code)}
              >
                Verify code
              </button>
            </form>
          )}
          <div className="auth-actions">
            <button
              className="text-button"
              disabled={disabled}
              onClick={() =>
                void command("account_answer", {
                  challengeId: view.challenge!.id,
                  answer: { action: "devices" },
                })
              }
            >
              Send to trusted devices
            </button>
            {!view.challenge.unknown && (
              <button
                className="text-button"
                disabled={disabled}
                onClick={() =>
                  void command("account_answer", {
                    challengeId: view.challenge!.id,
                    answer: { action: "resend" },
                  })
                }
              >
                Resend code
              </button>
            )}
            {view.challenge.numbers.map((number) => (
              <button
                className="text-button"
                key={number.id}
                disabled={disabled}
                onClick={() =>
                  void command("account_answer", {
                    challengeId: view.challenge!.id,
                    answer: { action: "sms", value: number.id },
                  })
                }
              >
                Send SMS · {number.label}
              </button>
            ))}
          </div>
        </div>
      )}
      {signedIn && (
        <>
          <Stage
            id="team"
            index={1}
            title="Signing team"
            state={stageState(!!view.selected_team, true)}
            summary={teamLabel ?? ""}
            help="what"
            onHelp={onHelp}
          >
            <label htmlFor="team">Signing team</label>
            <select
              id="team"
              disabled={disabled || !view.teams.length}
              value={view.selected_team ?? ""}
              onChange={(e) =>
                void command("account_select_team", { id: e.target.value })
              }
            >
              <option value="" disabled>
                Select a team
              </option>
              {view.teams.map((team) => (
                <option value={team.id} key={team.id}>
                  {team.name} · {team.id}
                  {team.kind ? ` · ${team.kind}` : ""}
                  {team.free === true
                    ? " · Free personal team"
                    : team.free === false
                      ? ` · ${team.membership ?? "Paid membership"}`
                      : ""}
                </option>
              ))}
            </select>
            {!view.teams.length && (
              <p className="hint">
                Apple returned no developer teams for this account. If it has
                never been used for development, accept the Apple Developer
                Agreement once at developer.apple.com and refresh.
              </p>
            )}
            {(() => {
              const team = view.teams.find((t) => t.id === view.selected_team);
              if (!team) return null;
              // What the chosen team means for re-signing, stated before any signing exists.
              return (
                <p className="hint" role="status">
                  {team.free === true
                    ? "Free personal team: profiles expire after seven days, so the app must be re-signed weekly, and capabilities this team cannot create are removed from the build."
                    : team.free === false
                      ? "Paid membership: installs use this team's device allowance, which is shared with everyone signing on it."
                      : `Apple's answer does not establish whether this is a free personal team or a paid membership, so expiry and capability limits are unknown.${
                          team.membership
                            ? ` Apple reported the membership as "${team.membership}".`
                            : " Apple reported no membership."
                        }`}
                </p>
              );
            })()}
            <button
              className="text-button"
              disabled={disabled}
              onClick={() => void command("account_refresh_teams")}
            >
              Refresh teams
            </button>
          </Stage>
          {view.selected_team && (
            <div className="register-device">
              <Stage
                id="registration"
                index={2}
                title="Register this iPhone"
                state={stageState(!!registration, true)}
                summary={registration?.message ?? ""}
                help="what"
                onHelp={onHelp}
              >
                <p className="hint">
                  Re-signing for a device requires it registered on the signing
                  team. This writes to your Apple account; nothing else is
                  changed, and the device identifier is sent only to Apple.
                </p>
                <label className="auth-consent">
                  <input
                    type="checkbox"
                    checked={registerAck}
                    disabled={disabled || registering || !deviceId}
                    onChange={(e) => setRegisterAck(e.target.checked)}
                  />
                  I understand a free personal team allows three devices, and a
                  paid team consumes one of its 100 slots for the membership
                  year, which removing the device later does not return.
                </label>
                <button
                  className="secondary"
                  disabled={
                    disabled || registering || !deviceId || !registerAck
                  }
                  onClick={() => {
                    setRegistering(true);
                    setRegisterError(null);
                    setRegistration(null);
                    registerDevice(deviceId!, registerAck)
                      .then((result) => {
                        if (mounted.current) setRegistration(result);
                      })
                      .catch((error) => {
                        if (mounted.current)
                          setRegisterError(
                            reason(
                              error,
                              "Registration did not complete. Check the account at developer.apple.com before trying again.",
                            ),
                          );
                      })
                      .finally(() => {
                        if (mounted.current) setRegistering(false);
                      });
                  }}
                >
                  {registering ? "Registering…" : "Register iPhone on team"}
                </button>
                <p className="hint" role="status">
                  {registerError ??
                    registration?.message ??
                    (!deviceId
                      ? "Select a connected iPhone above first."
                      : !registerAck
                        ? "Acknowledge the device allowance to enable registration."
                        : "Ready to register.")}
                </p>
                {registration && (
                  <p className="hint">
                    Devices on this team after the check:{" "}
                    {registration.team_devices}.
                  </p>
                )}
              </Stage>
              <Stage
                id="certificate"
                index={3}
                title="Development certificate"
                state={stageState(!!certificate, true)}
                summary={certificate?.message ?? ""}
                help="account"
                onHelp={onHelp}
              >
                <p className="hint">
                  The signing key is generated on this Mac and never leaves it;
                  only a certificate request goes to Apple. It is kept in this
                  Mac's Keychain for this account and team, so a restart reuses
                  the same certificate instead of spending another slot.
                </p>
                <label className="auth-consent">
                  <input
                    type="checkbox"
                    checked={certAck}
                    disabled={disabled || certBusy}
                    onChange={(e) => setCertAck(e.target.checked)}
                  />
                  I understand this uses one of the team's few active
                  certificate slots, and that Orbiter will never revoke a
                  certificate, because revoking invalidates every app already
                  signed with it.
                </label>
                <button
                  className="secondary"
                  disabled={disabled || certBusy || !certAck}
                  onClick={() => {
                    setCertBusy(true);
                    setCertError(null);
                    setCertificate(null);
                    requestCertificate(certAck)
                      .then((result) => {
                        if (mounted.current) setCertificate(result);
                      })
                      .catch((error) => {
                        if (mounted.current)
                          setCertError(
                            reason(
                              error,
                              "The certificate request did not complete. Check developer.apple.com before requesting another.",
                            ),
                          );
                      })
                      .finally(() => {
                        if (mounted.current) setCertBusy(false);
                      });
                  }}
                >
                  {certBusy
                    ? "Requesting certificate…"
                    : "Get development certificate"}
                </button>
                <p className="hint" role="status">
                  {certError ??
                    certificate?.message ??
                    (certAck
                      ? "Ready to request. Generating the key takes a moment."
                      : "Acknowledge the certificate limit to enable the request.")}
                </p>
                {certificate && (
                  <p className="hint">
                    Active development certificates on this team:{" "}
                    {certificate.active}
                    {certificate.expires
                      ? `. This one expires ${certificate.expires}.`
                      : "."}
                  </p>
                )}
                {certError?.includes("which is its maximum") && (
                  <>
                    <label className="auth-consent">
                      <input
                        type="checkbox"
                        checked={withdrawAck}
                        disabled={disabled || certBusy}
                        onChange={(e) => setWithdrawAck(e.target.checked)}
                      />
                      I understand withdrawing this team's certificate stops
                      every app already signed with it from launching, on every
                      device, and that this cannot be undone.
                    </label>
                    <button
                      className="secondary"
                      disabled={disabled || certBusy || !withdrawAck}
                      onClick={() => {
                        setCertBusy(true);
                        setCertError(null);
                        setCertificate(null);
                        withdrawCertificates(withdrawAck)
                          .then((message) => {
                            if (mounted.current)
                              setCertError(
                                reason(
                                  message,
                                  "The certificate was withdrawn.",
                                ),
                              );
                          })
                          .catch((error) => {
                            if (mounted.current)
                              setCertError(
                                reason(
                                  error,
                                  "The certificate could not be withdrawn.",
                                ),
                              );
                          })
                          .finally(() => {
                            if (mounted.current) setCertBusy(false);
                          });
                      }}
                    >
                      Withdraw the team's certificate
                    </button>
                  </>
                )}
                <button
                  className="text-button"
                  disabled={disabled || certBusy}
                  onClick={() => {
                    setCertBusy(true);
                    setCertError(null);
                    setCertificate(null);
                    forgetSigningKey()
                      .then((message) => {
                        if (mounted.current)
                          setCertError(reason(message, "Signing key removed."));
                      })
                      .catch((error) => {
                        if (mounted.current)
                          setCertError(
                            reason(
                              error,
                              "The stored signing key could not be removed.",
                            ),
                          );
                      })
                      .finally(() => {
                        if (mounted.current) setCertBusy(false);
                      });
                  }}
                >
                  Forget stored signing key
                </button>
              </Stage>
              <Stage
                id="identifiers"
                index={4}
                title="App identifiers and profiles"
                state={stageState(
                  !!preparation && preparation.plan.blockers.length === 0,
                  true,
                )}
                summary={
                  preparation
                    ? `${preparation.app_ids.length} identifier(s) registered`
                    : ""
                }
                help="losses"
                onHelp={onHelp}
              >
                <p className="hint">
                  Registers the plan's rewritten identifiers on this team and
                  downloads their profiles. Apple, not Orbiter, decides which
                  capabilities the identifiers may carry.
                </p>
                {hasWatchApp && (
                  <>
                    <label htmlFor="watch-choice">Watch app</label>
                    <Select
                      id="watch-choice"
                      icon={<Watch size={16} className="flex-none" />}
                      value={watch}
                      disabled={disabled || provBusy}
                      onChange={(value) => setWatch(value as WatchChoice)}
                    >
                      <option value="undecided">
                        Choose what happens to it
                      </option>
                      <option value="remove">
                        Remove it — the iPhone app installs without the Watch
                        app
                      </option>
                      <option value="sign">
                        Sign it too — unverified on this team, and it spends
                        another identifier
                      </option>
                    </Select>
                    <p className="hint">
                      This build includes a Watch app. It is never dropped
                      silently, so choose before identifiers are registered.
                    </p>
                  </>
                )}
                <label className="auth-consent">
                  <input
                    type="checkbox"
                    checked={provAck}
                    disabled={disabled || provBusy || !ipaPath}
                    onChange={(e) => setProvAck(e.target.checked)}
                  />
                  I understand ten identifiers per seven days is the limit on a
                  free personal team, and that an identifier cannot be reused by
                  another team afterwards.
                </label>
                <button
                  className="secondary"
                  disabled={
                    disabled ||
                    provBusy ||
                    !ipaPath ||
                    !provAck ||
                    (hasWatchApp && watch === "undecided")
                  }
                  onClick={() => {
                    setProvBusy(true);
                    setProvError(null);
                    setPreparation(null);
                    prepareProvisioning(ipaPath!, provAck, watch)
                      .then((result) => {
                        if (!mounted.current) return;
                        setPreparation(result);
                        onPrepared(result);
                      })
                      .catch((error) => {
                        if (mounted.current)
                          setProvError(
                            reason(
                              error,
                              "Provisioning did not complete. Check developer.apple.com before retrying.",
                            ),
                          );
                      })
                      .finally(() => {
                        if (mounted.current) setProvBusy(false);
                      });
                  }}
                >
                  {provBusy
                    ? "Provisioning…"
                    : "Prepare identifiers & profiles"}
                </button>
                <p className="hint" role="status">
                  {provError ??
                    (!ipaPath
                      ? "Select an IPA first."
                      : !provAck
                        ? "Acknowledge the identifier limit to enable provisioning."
                        : hasWatchApp && watch === "undecided"
                          ? "Choose what happens to the Watch app."
                          : preparation
                            ? `Plan identifier: ${preparation.plan.new_main_identifier}`
                            : "Ready to provision.")}
                </p>
                {preparation && preparation.plan.blockers.length > 0 && (
                  <ul className="hint">
                    {preparation.plan.blockers.map((blocker) => (
                      <li key={blocker}>{blocker}</li>
                    ))}
                  </ul>
                )}
                {preparation?.app_ids.map((appId) => (
                  <p className="hint" key={appId.identifier}>
                    {appId.identifier} ·{" "}
                    {appId.created ? "registered" : "reused"}
                    {appId.capabilities.length
                      ? ` · Apple enabled: ${appId.capabilities.join(", ")}`
                      : " · Apple enabled no capabilities"}
                    {appId.remaining !== null
                      ? ` · ${appId.remaining} identifier(s) left this week`
                      : ""}
                  </p>
                ))}
                {preparation?.profiles.map((profile) => (
                  <p className="hint" key={profile.uuid}>
                    Profile for {profile.identifier} expires {profile.expires}.
                  </p>
                ))}
              </Stage>
            </div>
          )}
        </>
      )}
      {!signedIn && (
        <>
          <label htmlFor="team">Signing team</label>
          <select id="team" disabled>
            <option>Sign in to load teams</option>
          </select>
        </>
      )}
      {active && (
        <button
          className="secondary"
          disabled={busy}
          onClick={() => {
            // Clear the secrets. The account address and the consent given in this session are
            // kept, so signing back in does not mean retyping everything; both stay visible and
            // editable, and the backend still requires consent on every attempt.
            setPassword("");
            setCode("");
            void command("account_sign_out");
          }}
        >
          Cancel sign-in
        </button>
      )}
      {(statusError || error) && <p role="alert">{statusError || error}</p>}
    </section>
  );
}
