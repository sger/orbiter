import { useEffect, useRef, useState } from "react";
import { accountStatus, accountSelectTeam, isTauri } from "../../ipc/commands";
import type {
  AccountView,
  Certificate,
  Preparation,
  Registration,
  TeamStatus,
  WatchChoice,
} from "../../types";
import type { StageState } from "../../state/pipeline";
const initial: AccountView = {
  stage: "signed_out",
  account: null,
  teams: [],
  selected_team: null,
  challenge: null,
  message: "",
};
export function useAccounts({
  paused,
  deviceId,
  ipaPath,
  onPrepared,
  onStatus,
}: {
  paused: boolean;
  deviceId: number | null;
  ipaPath: string | null;
  onPrepared: (preparation: Preparation | null) => void;
  /// Everything above this panel needs to know, in one shape: the pipeline derives its gates from
  /// it, the header shows the account, and the signer is given the same Watch choice the plan had.
  onStatus: (status: TeamStatus) => void;
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
    const text =
      typeof error === "string"
        ? error.trim()
        : error && typeof error === "object" && "message" in error
          ? String(error.message)
          : "";
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
  async function command(action: () => Promise<AccountView>) {
    if (inFlight.current) return;
    inFlight.current = true;
    generation.current += 1;
    setBusy(true);
    setError(null);
    try {
      const next = await action();
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
  useEffect(() => {
    if (!signedIn || view.selected_team || disabled) return;
    const supported = view.teams.filter(
      (team) =>
        team.free !== null &&
        !team.membership?.toLowerCase().includes("enterprise"),
    );
    if (view.teams.length === 1 && supported.length === 1) {
      void command(() => accountSelectTeam(supported[0].id));
    }
  }, [signedIn, view.selected_team, view.teams, paused]);
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
  return {
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
    team,
    stageState,
    teamLabel,
    active,
    disabled,
    signInBlocker,
    reason,
    command,
  };
}
