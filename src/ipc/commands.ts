/// Every call into the Rust core, in one place, typed.
///
/// Components never name a command string. That keeps the IPC surface visible — the whole list of
/// what this interface can ask the backend to do is this file — and it means a command's name or
/// arguments change in one place rather than wherever someone happened to call it.
import { Channel, invoke, isTauri } from "@tauri-apps/api/core";
import type {
  AccountView,
  Certificate,
  Discovery,
  Job,
  LogLine,
  LogSummary,
  Preparation,
  Registration,
  Renewal,
  Report,
  Review,
  Signed,
  SigningProgress,
  WatchChoice,
} from "../types";

export { isTauri };

/// A progress channel. Callers hand it a sink and pass it straight to the command.
export function channel<T>(onMessage: (value: T) => void) {
  const progress = new Channel<T>();
  progress.onmessage = onMessage;
  return progress;
}

// Inspection — local only, no network, never writes the chosen IPA.
export const inspectIpa = (path: string, progress: Channel<string>) =>
  invoke<Report>("inspect_ipa", { path, progress });
export const cancelInspection = () => invoke("cancel_inspection");

// Devices — reads existing pairing records over the local transport.
export const discoverDevices = () => invoke<Discovery>("discover_devices");

// Apple account. Everything here reaches Apple; nothing here happens without a click.
export const accountStatus = () => invoke<AccountView>("account_status");
export const accountSignIn = (
  email: string,
  password: string,
  consent: boolean,
) => invoke<AccountView>("account_sign_in", { email, password, consent });
export type AccountAnswer =
  | { action: "code"; value: string }
  | { action: "sms"; value: number }
  | { action: "devices" | "resend" };
export const accountAnswer = (challengeId: string, answer: AccountAnswer) =>
  invoke<AccountView>("account_answer", { challengeId, answer });
export const accountSignOut = () => invoke<AccountView>("account_sign_out");
export const accountSelectTeam = (id: string) =>
  invoke<AccountView>("account_select_team", { id });
export const accountRefreshTeams = () =>
  invoke<AccountView>("account_refresh_teams");

// Account writes. Each takes its own acknowledgement, and the backend refuses without it.
export const registerDevice = (deviceId: number, acknowledged: boolean) =>
  invoke<Registration>("account_register_device", { deviceId, acknowledged });
export const requestCertificate = (acknowledged: boolean) =>
  invoke<Certificate>("account_request_certificate", { acknowledged });
export const withdrawCertificates = (acknowledged: boolean) =>
  invoke<string>("account_withdraw_certificates", { acknowledged });
export const forgetSigningKey = () =>
  invoke<string>("account_forget_signing_key");
export const prepareProvisioning = (
  path: string,
  acknowledged: boolean,
  watch: WatchChoice,
  /// Absolute paths of libraries to inject into the app; empty for a plain re-sign.
  dylibs: string[] = [],
) =>
  invoke<Preparation>("account_prepare_provisioning", {
    path,
    acknowledged,
    watch,
    dylibs,
  });

// Re-signing. Produces a new IPA; the chosen one is only ever read.
export const signIpa = (
  path: string,
  watch: WatchChoice,
  /// Prefix for the signed app's display name; "" leaves every name alone. Rust decides what is
  /// usable, so whatever is typed here is sent as typed.
  marker: string,
  /// Absolute paths of libraries to inject into the app; empty for a plain re-sign.
  dylibs: string[],
  progress: Channel<SigningProgress>,
) => invoke<Signed>("account_sign_ipa", { path, watch, marker, dylibs, progress });

// The seven-day clock. Reads a local file and a clock; never Apple, never the phone.
export const renewalStatus = (
  teamId: string | null,
  identifier: string | null,
) => invoke<Renewal | null>("renewal_status", { teamId, identifier });
export const renewalForget = () => invoke("renewal_forget");

// Installation over a cable or Wi-Fi, whichever the chosen iPhone is reachable on.
export const prepareInstall = (path: string, deviceId: number) =>
  invoke<Review>("prepare_install", { path, deviceId });
export const discardInstall = (token: string) =>
  invoke("discard_install", { token });
export const executeInstall = (
  token: string,
  acknowledged: boolean,
  progress: Channel<Job>,
) => invoke<Job>("execute_install", { token, acknowledged, progress });
export const cancelInstall = () => invoke<boolean>("cancel_install");
export const installationStatus = () =>
  invoke<Job | null>("installation_status");

// Device log. Keeps only lines about the subjects it is given.
export const startDeviceLog = (
  deviceId: number,
  subjects: string[],
  superseded: string[],
  progress: Channel<LogLine>,
) =>
  invoke<LogSummary>("start_device_log", {
    deviceId,
    subjects,
    superseded,
    progress,
  });
export const stopDeviceLog = () => invoke("stop_device_log");

import type {
  LibrarySnapshot,
  LibraryExpiry,
  Imported,
  Opened,
  Artifact,
  Refresh,
} from "../features/library/types";
export const libraryChanged = () =>
  window.dispatchEvent(new Event("library-changed"));
/// Where the seven days stand for one saved build, or null when it was never installed.
export const libraryExpiry = (artifactId: string, teamId: string | null) =>
  invoke<LibraryExpiry | null>("library_expiry", { artifactId, teamId });
/// What a re-sign of an expiring build would start from, or null when the original is gone.
/// Reading this begins nothing; the review screen still asks for every acknowledgement.
export const libraryRefresh = (artifactId: string) =>
  invoke<Refresh | null>("library_refresh", { artifactId });
/// This library's tag for the phone on the cable, for telling a remembered device from another
/// one. Rejects rather than guesses when the phone cannot be identified.
export const libraryDeviceTag = (deviceId: number) =>
  invoke<string>("library_device_tag", { deviceId });
/// One app icon, by its hash. Null when the file is missing or unreadable.
export const libraryIcon = (sha: string) =>
  invoke<string | null>("library_icon", { sha });
/// Delete managed files no record points at, returning the bytes freed.
export const libraryReclaim = () => invoke<number>("library_reclaim");
export const libraryList = () => invoke<LibrarySnapshot>("library_list");
export const libraryImport = (path: string) =>
  invoke<Imported>("library_import", { path });
export const libraryOpen = (artifactId: string) =>
  invoke<Opened>("library_open", { artifactId });
export const libraryRemove = (appId: string, artifactId: string | null) =>
  invoke<void>("library_remove", { appId, artifactId });
export const libraryPrepareInstall = (artifactId: string, deviceId: number) =>
  invoke<Review>("library_prepare_install", { artifactId, deviceId });
export const libraryPrepareProvisioning = (
  artifactId: string,
  acknowledged: boolean,
  watch: WatchChoice,
  /// Absolute paths of libraries to inject into the app; empty for a plain re-sign.
  dylibs: string[] = [],
) =>
  invoke<Preparation>("library_prepare_provisioning", {
    artifactId,
    acknowledged,
    watch,
    dylibs,
  });
export const librarySign = (
  artifactId: string,
  watch: WatchChoice,
  marker: string,
  /// Absolute paths of libraries to inject into the app; empty for a plain re-sign.
  dylibs: string[],
  progress: Channel<SigningProgress>,
) =>
  invoke<{ signed: Signed; artifact: Artifact }>("library_sign", {
    artifactId,
    watch,
    marker,
    dylibs,
    progress,
  });

import type {
  PreparationReview,
  Consents,
  PreparationStatus,
} from "../features/guided/types";
export const reviewPreparation = (
  artifactId: string,
  deviceId: number,
  watch: WatchChoice,
  marker: string,
  /// Absolute paths of libraries to inject into the app; empty for a plain re-sign.
  dylibs: string[] = [],
) =>
  invoke<PreparationReview>("library_review_preparation", {
    artifactId,
    deviceId,
    watch,
    marker,
    dylibs,
  });
export const executePreparation = (
  token: string,
  consents: Consents,
  progress: Channel<PreparationStatus>,
) =>
  invoke<{ signed: Signed; artifact: Artifact }>(
    "library_execute_preparation",
    { token, consents, progress },
  );
export const discardPreparation = (token: string) =>
  invoke<void>("library_discard_preparation", { token });
export const preparationStatus = () =>
  invoke<PreparationStatus>("library_preparation_status");
