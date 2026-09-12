/// What is done, what is next, and why anything is refused — derived in one place.
///
/// Before this existed, each control worked out its own enabled state from whatever happened to be
/// in scope. That is how "Re-sign IPA" came to be clickable in a session with no certificate: the
/// button knew about prepared profiles and nothing else, so it offered an action the backend then
/// refused. Every gate now comes from one function over one state, and every refusal carries the
/// sentence explaining it.
import type { Preparation, Report, Signed, TeamStatus } from "../types";

export type StageId =
  | "build"
  | "device"
  | "team"
  | "registration"
  | "certificate"
  | "identifiers"
  | "sign";

export type StageState = "done" | "current" | "waiting";

export type Stage = {
  id: StageId;
  title: string;
  state: StageState;
  /// One line describing the result, shown when the stage is collapsed.
  summary: string;
  /// Why this stage cannot proceed yet. Empty when it can.
  blocked: string;
};

export type Pipeline = {
  report: Report | null;
  ipaPath: string | null;
  deviceId: number | null;
  team: TeamStatus;
  preparation: Preparation | null;
  signed: Signed | null;
};

/// Whether the chosen build contains a Watch app, which the plan will not decide silently.
export function hasWatchApp(report: Report | null) {
  return report?.bundles.some((bundle) => bundle.kind === "Watch app") ?? false;
}

function main(report: Report | null) {
  return report?.bundles.find((bundle) => bundle.path === report.main_path);
}

/// Why re-signing is refused, or an empty string when it is not.
///
/// The order matters: it names the earliest unmet requirement, because that is the one a person
/// can act on. Reporting the last one would send them to a step they cannot reach yet.
export function signBlocked(pipeline: Pipeline): string {
  const { report, ipaPath, team, preparation } = pipeline;
  if (!report || !ipaPath) return "Choose an IPA first.";
  if (!team.signedIn) return "Sign in to Apple first.";
  if (!team.teamId) return "Select a signing team first.";
  if (!team.certificate)
    return "Get a development certificate in step 3 first.";
  if (!preparation)
    return "Register the identifiers and profiles in step 4 first.";
  if (preparation.plan.blockers.length) return preparation.plan.blockers[0];
  if (hasWatchApp(report) && team.watch === "undecided")
    return "Choose what happens to the Watch app.";
  return "";
}

/// Why installing is refused. Separate from signing: an unchanged company build can be installed
/// with no Apple account at all, so this must not inherit re-signing's requirements.
export function installBlocked(
  pipeline: Pipeline,
  signedOnly: boolean,
): string {
  if (!pipeline.ipaPath) return "Choose an IPA first.";
  if (pipeline.deviceId === null) return "Select a connected iPhone first.";
  if (signedOnly && !pipeline.signed) return "Re-sign the build first.";
  return "";
}

function stage(
  id: StageId,
  title: string,
  done: boolean,
  summary: string,
  blocked: string,
): Omit<Stage, "state"> & { done: boolean } {
  return { id, title, done, summary, blocked };
}

/// The stages in order, each marked done, current, or waiting.
///
/// Exactly one stage is current: the first that is not done. Everything after it is waiting, so a
/// person is never offered two next steps at once.
export function stages(pipeline: Pipeline): Stage[] {
  const { report, deviceId, team, preparation } = pipeline;
  const app = main(report);
  const raw = [
    stage(
      "build",
      "Build",
      !!report,
      app
        ? `${app.name} ${app.version ?? ""} · ${report?.bundles.length ?? 0} bundles`
        : "",
      "",
    ),
    stage(
      "device",
      "iPhone",
      deviceId !== null,
      "",
      "Connect and unlock an iPhone, then select it.",
    ),
    stage(
      "team",
      "Team",
      team.signedIn && !!team.teamId,
      team.teamLabel ?? "",
      team.signedIn
        ? "Select the team that should re-sign this build."
        : "Sign in to Apple to list your teams.",
    ),
    stage(
      "registration",
      "Register this iPhone",
      team.registered,
      team.registrationSummary,
      "Select a team and an iPhone first.",
    ),
    stage(
      "certificate",
      "Development certificate",
      team.certificate,
      team.certificateSummary,
      // The key survives a restart; the certificate does not, and that surprises people.
      "Needed once per session. The key stored in this Mac's Keychain reuses the same certificate.",
    ),
    stage(
      "identifiers",
      "Identifiers and profiles",
      !!preparation && preparation.plan.blockers.length === 0,
      preparation
        ? `${preparation.app_ids.length} identifier(s) registered for this team`
        : "",
      "Get a development certificate first.",
    ),
    stage(
      "sign",
      "Re-sign",
      !!pipeline.signed,
      pipeline.signed
        ? `${pipeline.signed.bundles_signed} bundle(s) signed as ${pipeline.signed.identifier}`
        : "",
      signBlocked(pipeline),
    ),
  ];
  const current = raw.findIndex((entry) => !entry.done);
  return raw.map(({ done, ...rest }, index) => ({
    ...rest,
    state: done ? "done" : index === current ? "current" : "waiting",
  }));
}

/// The stage a person should be looking at, or null once everything is done.
export function currentStage(pipeline: Pipeline): Stage | null {
  return stages(pipeline).find((entry) => entry.state === "current") ?? null;
}
