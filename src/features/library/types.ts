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
