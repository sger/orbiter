import type { Report, Job, Standing, Bearing } from "../../types";
export interface LibraryApp {
  id: string;
  identifier: string;
  name: string;
  /// The icon's hash; its bytes are fetched separately and cached in the window.
  icon_sha: string | null;
  added_unix: number;
}
export interface Artifact {
  id: string;
  app_id: string;
  source_id: string | null;
  sha256: string;
  name: string;
  identifier: string;
  version: string | null;
  build: string | null;
  size_bytes: number;
  added_unix: number;
  expires: string | null;
  expires_unix: number | null;
  team_tag: string | null;
  watch: string | null;
  marker: string | null;
  deleted: boolean;
}
export interface RememberedDevice {
  id: string;
  name: string;
  last_seen_unix: number;
}
export interface Attempt {
  id: string;
  app_id: string;
  artifact_id: string;
  device_id: string;
  app_name: string;
  identifier: string;
  version: string | null;
  build: string | null;
  sha256: string;
  signed: boolean;
  expires: string | null;
  expires_unix: number | null;
  team_tag: string | null;
  started_unix: number;
  finished_unix: number | null;
  stage: Job["stage"];
  message: string;
}
/// One successful installation and where its seven days stand.
///
/// Every field is decided in Rust, the sentence included. The front end never computes a day
/// count from a date: one implementation of that arithmetic, so the library page, the workspace
/// and a screenshot of either cannot word the same fact differently.
export interface LibraryExpiry {
  app_id: string;
  artifact_id: string;
  attempt_id: string;
  device_id: string;
  device_name: string;
  app_name: string;
  identifier: string;
  signed: boolean;
  expires: string | null;
  expires_unix: number;
  installed_unix: number;
  standing: Standing;
  bearing: Bearing;
  sentence: string;
  urgent: boolean;
  /// Still launching, but not for long. The step before `urgent`, never true beside it.
  soon: boolean;
}
/// The starting point for re-signing a build whose seven days have run out.
///
/// Not a plan and not an action: the answers a person already gave, so the review screen can open
/// with them filled in rather than asking again. Every one stays editable, and that screen still
/// asks for the same acknowledgements it asked for the first time.
export interface Refresh {
  app_id: string;
  /// The original to sign again — never the expiring signed build itself.
  artifact_id: string;
  name: string;
  /// What the expired build was signed under. Shown for confirmation, never applied silently.
  watch: string | null;
  marker: string | null;
  /// The phone that build went to, as this library's tag for it.
  device_id: string | null;
  device_name: string | null;
}
export interface LibrarySnapshot {
  apps: LibraryApp[];
  artifacts: Artifact[];
  devices: RememberedDevice[];
  attempts: Attempt[];
  /// Longest-lived first within each app: the entry a screen leads with is the first for that app.
  expiries: LibraryExpiry[];
  storage_bytes: number;
  /// Bytes no record points at. Reported, never deleted without being asked.
  unreferenced_bytes: number;
  storage_warning?: string | null;
}
export interface Imported {
  app_id: string;
  artifact_id: string;
  duplicate: boolean;
}
export interface Opened {
  artifact: Artifact;
  report: Report;
  path: string;
}
