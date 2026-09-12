import { useEffect, useRef, useState } from "react";
import { invoke, isTauri } from "@tauri-apps/api/core";

type AccountView = {
  stage: "signed_out" | "signing_in" | "two_factor" | "signed_in" | "failed";
  account: string | null;
  teams: {
    id: string;
    name: string;
    kind: string | null;
    free: boolean | null;
    membership: string | null;
  }[];
  selected_team: string | null;
  challenge: {
    id: string;
    sms: boolean;
    unknown: boolean;
    retry: boolean;
    numbers: { id: number; label: string }[];
  } | null;
  message: string;
};
const initial: AccountView = {
  stage: "signed_out",
  account: null,
  teams: [],
  selected_team: null,
  challenge: null,
  message: "",
};
export function Accounts({ paused }: { paused: boolean }) {
  const [view, setView] = useState<AccountView>(initial);
  const [support, setSupport] = useState<{
    available: boolean;
    message: string;
  } | null>(null);
  const [checkingSupport, setCheckingSupport] = useState(false);
  const [email, setEmail] = useState("");
  const [password, setPassword] = useState("");
  const [code, setCode] = useState("");
  const [consent, setConsent] = useState(false);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [statusError, setStatusError] = useState<string | null>(null);
  const [ready, setReady] = useState(false);
  const inFlight = useRef(false);
  const mounted = useRef(false);
  const desktop = isTauri();
  useEffect(() => {
    mounted.current = true;
    let polling = false;
    async function poll() {
      if (!desktop || inFlight.current || polling) return;
      polling = true;
      const epoch = generation.current;
      try {
        const next = await invoke<AccountView>("account_status");
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
  useEffect(() => {
    setCode("");
  }, [view.challenge?.id]);
  async function command(name: string, args?: Record<string, unknown>) {
    if (inFlight.current) return;
    inFlight.current = true;
    generation.current += 1;
    setBusy(true);
    setError(null);
    try {
      const next = await invoke<AccountView>(name, args);
      if (mounted.current) setView(next);
    } catch {
      if (mounted.current)
        setError(
          "The account action did not complete. Check the current status and retry.",
        );
    } finally {
      inFlight.current = false;
      if (mounted.current) setBusy(false);
    }
  }
  async function checkSupport() {
    setCheckingSupport(true);
    setSupport(null);
    try {
      setSupport(await invoke("local_auth_support"));
    } catch {
      setSupport({
        available: false,
        message: "Local authentication support could not be checked.",
      });
    } finally {
      setCheckingSupport(false);
    }
  }
  const active = view.stage === "signing_in" || view.stage === "two_factor";
  const signedIn = view.stage === "signed_in";
  const disabled = !desktop || !ready || busy || paused;
  const signInBlocker = !desktop
    ? "Open the desktop app to sign in."
    : !ready
      ? "Waiting for account service availability."
      : busy
        ? "An account action is in progress."
        : paused
          ? "Wait for the current operation to finish before signing in."
          : checkingSupport
            ? "Checking local authentication support…"
            : support?.available === false
              ? "Local support is unavailable. Use Check local support to retry."
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
          <p className="hint">
            Sign in to list your developer teams. Re-signing and provisioning
            are still being built. Local authentication support is checked when
            you sign in; the separate check below is optional.
          </p>
          <button
            type="button"
            className="secondary"
            disabled={disabled || checkingSupport}
            onClick={() => void checkSupport()}
          >
            {checkingSupport
              ? "Checking local support…"
              : "Check local support"}
          </button>
          {support && (
            <p className="hint" role="status">
              {support.message}
            </p>
          )}
          <details className="auth-disclosure">
            <summary>Local authentication & Apple communication</summary>
            <p>
              Authentication support is generated using Apple frameworks on this
              Mac. Orbiter’s authentication requests go directly to Apple over
              HTTPS; remote support servers and proxy settings are disabled.
            </p>
            <p>
              Your Apple account email, authentication exchange, verification
              code, and local device authentication data are used with Apple.
              Passwords and codes are not saved. Account sessions stay in memory
              and expire locally after 30 minutes or when Orbiter closes. macOS
              manages its own authentication support data.
            </p>
            <p>
              This preview uses private macOS APIs that may change. If local
              support fails, sign-in stops. Windows local authentication is not
              implemented.
            </p>
          </details>
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
          <button
            type="submit"
            className="secondary"
            disabled={signInBlocker !== null}
            aria-describedby="sign-in-help"
          >
            Sign in to Apple
          </button>
          <p id="sign-in-help" className="hint" role="status">
            {signInBlocker ?? "Ready to sign in with Apple."}
          </p>
        </form>
      )}
      <p className="hint" role="status" aria-live="polite">
        {desktop
          ? view.message
          : "Apple sign-in is available in the desktop app."}
      </p>
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
          <p className="account-name">{view.account}</p>
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
                    : "Apple's answer does not establish whether this is a free personal team or a paid membership, so expiry and capability limits are unknown."}
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
      {(active || signedIn) && (
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
          {active ? "Cancel sign-in" : "Sign out"}
        </button>
      )}
      {(statusError || error) && <p role="alert">{statusError || error}</p>}
    </section>
  );
}
