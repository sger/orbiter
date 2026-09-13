import type { Report, Job } from "../../types";
export interface LibraryApp {
  id: string;
  identifier: string;
  name: string;
  icon_data_url: string | null;
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
  started_unix: number;
  finished_unix: number | null;
  stage: Job["stage"];
  message: string;
}
export interface LibrarySnapshot {
  apps: LibraryApp[];
  artifacts: Artifact[];
  devices: RememberedDevice[];
  attempts: Attempt[];
  storage_bytes: number;
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
