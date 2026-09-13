import { useEffect } from "react";
import { libraryPrepareProvisioning } from "../../ipc/commands";
import { TextField } from "../../components/ui/TextField";
import { Checkbox } from "../../components/ui/Checkbox";
import {
  accountRefreshTeams,
  accountSignIn,
  accountAnswer,
  accountSignOut,
  accountSelectTeam,
  forgetSigningKey,
  prepareProvisioning,
  registerDevice,
  requestCertificate,
  withdrawCertificates,
} from "../../ipc/commands";
import { Users, Watch } from "lucide-react";
import { Select } from "../../components/ui/Select";
import { Stage } from "../../app/Stage";
import type { Preparation, TeamStatus, WatchChoice } from "../../types";

import { useAccounts } from "./useAccounts";
export function Accounts({
  artifactId,
  onBusy,
  paused,
  deviceId,
  ipaPath,
  hasWatchApp,
  onPrepared,
  onStatus,
  onHelp,
}: {
  artifactId?: string;
  onBusy?: (value: boolean) => void;
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
  const {
    view,
    email,
    setEmail,
    password,
    setPassword,
    code,
    setCode,
    consent,
    setConsent,
    busy,
    error,
    statusError,
    registerAck,
    setRegisterAck,
    registering,
    setRegistering,
    registration,
    setRegistration,
    registerError,
    setRegisterError,
    certAck,
    setCertAck,
    certBusy,
    setCertBusy,
    certificate,
    setCertificate,
    certError,
    setCertError,
    withdrawAck,
    setWithdrawAck,
    provAck,
    setProvAck,
    watch,
    setWatch,
    provBusy,
    setProvBusy,
    preparation,
    setPreparation,
    provError,
    setProvError,
    mounted,
    desktop,
    signedIn,
    stageState,
    teamLabel,
    active,
    disabled,
    signInBlocker,
    reason,
    command,
  } = useAccounts({ paused, deviceId, ipaPath, onPrepared, onStatus });
  useEffect(() => {
    onBusy?.(busy || registering || certBusy || provBusy);
  }, [busy, registering, certBusy, provBusy, onBusy]);
  // Emphasize the earliest unfinished action that can actually run now.
  // This changes presentation only; acknowledgement and backend gates stay intact.
  const nextAction =
    !registration && !disabled && !registering && deviceId && registerAck
      ? "register"
      : !certificate && !disabled && !certBusy && certAck
        ? "certificate"
        : !preparation &&
            !disabled &&
            !provBusy &&
            ipaPath &&
            provAck &&
            (!hasWatchApp || watch !== "undecided")
          ? "provision"
          : null;
  return (
    <section
      tabIndex={-1}
      className="accounts"
      aria-label="Apple account authentication"
    >
      <strong className="account-title">Apple account</strong>
      {!active && !signedIn && (
        <form
          onSubmit={(e) => {
            e.preventDefault();
            if (signInBlocker) return;
            const enteredPassword = password;
            setPassword("");
            void command(() => accountSignIn(email, enteredPassword, consent));
          }}
        >
          {/* One line. What this means in full is in Help, one click away, rather than four
              paragraphs a person must read past every time to reach the password field. */}
          <p className="hint">
            Sign in with the Apple ID this build should be re-signed for.
            Orbiter talks to Apple directly; passwords are never saved.
          </p>
          <button
            type="button"
            className="text-button privacy-link"
            onClick={() => onHelp("account")}
          >
            What is stored
          </button>

          <div className="auth-fields">
            <TextField
              label="Apple account email"
              id="account"
              type="email"
              autoComplete="username"
              maxLength={254}
              required
              disabled={disabled}
              value={email}
              onChange={(e) => setEmail(e.target.value)}
            />
            <TextField
              label="Password"
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
          <Checkbox
            checked={consent}
            disabled={disabled}
            onChange={(e) => setConsent(e.target.checked)}
          >
            I agree to authenticate directly with Apple using local macOS
            support.
          </Checkbox>
          {/* The action and the reason it is unavailable belong on one line: a button with its
              explanation stranded below it reads as two unrelated things. */}
          <div className="mt-3 flex flex-wrap items-center gap-x-3 gap-y-1.5">
            <button
              type="submit"
              className="secondary action-emphasis !mt-0"
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
                void command(() =>
                  accountAnswer(view.challenge!.id, { action: "code", value }),
                );
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
                className="secondary action-emphasis"
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
                void command(() =>
                  accountAnswer(view.challenge!.id, { action: "devices" }),
                )
              }
            >
              Send to trusted devices
            </button>
            {!view.challenge.unknown && (
              <button
                className="text-button"
                disabled={disabled}
                onClick={() =>
                  void command(() =>
                    accountAnswer(view.challenge!.id, { action: "resend" }),
                  )
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
                  void command(() =>
                    accountAnswer(view.challenge!.id, {
                      action: "sms",
                      value: number.id,
                    }),
                  )
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
            <Select
              icon={<Users size={16} />}
              id="team"
              disabled={disabled || !view.teams.length}
              value={view.selected_team ?? ""}
              onChange={(value) => void command(() => accountSelectTeam(value))}
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
            </Select>
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
              onClick={() => void command(accountRefreshTeams)}
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
                <Checkbox
                  checked={registerAck}
                  disabled={disabled || registering || !deviceId}
                  onChange={(e) => setRegisterAck(e.target.checked)}
                >
                  I understand a free personal team allows three devices, and a
                  paid team consumes one of its 100 slots for the membership
                  year, which removing the device later does not return.
                </Checkbox>
                <button
                  className={`secondary ${nextAction === "register" ? "action-emphasis" : ""}`}
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
                <Checkbox
                  checked={certAck}
                  disabled={disabled || certBusy}
                  onChange={(e) => setCertAck(e.target.checked)}
                >
                  I understand this uses one of the team's few active
                  certificate slots, and that Orbiter will never revoke a
                  certificate, because revoking invalidates every app already
                  signed with it.
                </Checkbox>
                <button
                  className={`secondary ${nextAction === "certificate" ? "action-emphasis" : ""}`}
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
                    <Checkbox
                      checked={withdrawAck}
                      disabled={disabled || certBusy}
                      onChange={(e) => setWithdrawAck(e.target.checked)}
                    >
                      I understand withdrawing this team's certificate stops
                      every app already signed with it from launching, on every
                      device, and that this cannot be undone.
                    </Checkbox>
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
                <Checkbox
                  checked={provAck}
                  disabled={disabled || provBusy || !ipaPath}
                  onChange={(e) => setProvAck(e.target.checked)}
                >
                  I understand ten identifiers per seven days is the limit on a
                  free personal team, and that an identifier cannot be reused by
                  another team afterwards.
                </Checkbox>
                <button
                  className={`secondary ${nextAction === "provision" ? "action-emphasis" : ""}`}
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
                    (artifactId
                      ? libraryPrepareProvisioning(artifactId, provAck, watch)
                      : prepareProvisioning(ipaPath!, provAck, watch)
                    )
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
          <Select id="team" icon={<Users size={16} />} value="" disabled>
            <option value="">Sign in to load teams</option>
          </Select>
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
            void command(accountSignOut);
          }}
        >
          Cancel sign-in
        </button>
      )}
      {(statusError || error) && <p role="alert">{statusError || error}</p>}
    </section>
  );
}
