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
) =>
  invoke<Preparation>("account_prepare_provisioning", {
    path,
    acknowledged,
    watch,
  });

// Re-signing. Produces a new IPA; the chosen one is only ever read.
export const signIpa = (
  path: string,
  watch: WatchChoice,
  progress: Channel<SigningProgress>,
) => invoke<Signed>("account_sign_ipa", { path, watch, progress });

// Installation over USB.
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
