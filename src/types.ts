export interface Profile {
  name: string | null;
  team_name: string | null;
  team_id: string | null;
  expires_at: string | null;
  expires_unix: number | null;
  expired: boolean | null;
  device_count: number | null;
  distribution: string;
  entitlements: Record<string, unknown>;
  trust: string;
}
export interface Slice {
  architecture: string;
  encrypted: boolean;
  entitlements: Record<string, unknown>;
  xml_entitlements_present: boolean;
  der_entitlements_present: boolean;
}
export interface Bundle {
  path: string;
  kind: string;
  name: string;
  identifier: string;
  version: string | null;
  build: string | null;
  minimum_os: string | null;
  supported_platforms: string[];
  device_families: number[];
  slices: Slice[];
  profile: Profile | null;
  issues: string[];
}
export interface Finding {
  status:
    "not_verified" | "requires_configuration" | "unsupported" | "preserved";
  title: string;
  detail: string;
  bundle: string | null;
}
export interface Report {
  size_bytes: number;
  main_path: string;
  bundles: Bundle[];
  findings: Finding[];
  icon_data_url: string | null;
}

/// What the signer produced. The original IPA is never modified.
export type Signed = {
  path: string;
  identifier: string;
  expires: string;
  expires_unix: number;
  bundles_signed: number;
  removed: string[];
  message: string;
  /// What each signing stage did, in order.
  log: string[];
};

/// How far along a signing run is. Counts of real things, never an invented percentage.
export type SigningProgress = {
  stage: string;
  done: number;
  total: number;
};

/// A connected iPhone as discovery reports it. `id` is an ephemeral transport identifier, never
/// a UDID — nothing that crosses this boundary identifies a device durably.
export type Device = {
  id: number;
  name: string | null;
  product_type: string | null;
  ios_version: string | null;
  connection: string;
  state:
    | "paired"
    | "locked"
    | "trust_required"
    | "pairing_unverified"
    | "unavailable";
  message: string;
};
export type Discovery = {
  devices: Device[];
  service_available: boolean;
  message: string | null;
};

export type Team = {
  id: string;
  name: string;
  kind: string | null;
  free: boolean | null;
  membership: string | null;
};
export type AccountView = {
  stage: "signed_out" | "signing_in" | "two_factor" | "signed_in" | "failed";
  account: string | null;
  teams: Team[];
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
export type Certificate = {
  reused: boolean;
  expires: string | null;
  active: number;
  message: string;
};
export type Registration = {
  registration: "already_registered" | "registered";
  team_devices: number;
  message: string;
};
export type WatchChoice = "undecided" | "remove" | "sign";
export type Capability = {
  key: string;
  action: "keep" | "rewrite" | "remove";
  reason: string;
  consequence: string;
};
export type Preparation = {
  plan: {
    new_main_identifier: string;
    blockers: string[];
    consequences: string[];
    app_ids_required: number;
    bundles: {
      name: string;
      identifier: string;
      new_identifier: string;
      capabilities: Capability[];
    }[];
  };
  app_ids: {
    identifier: string;
    created: boolean;
    capabilities: string[];
    remaining: number | null;
  }[];
  profiles: { identifier: string; expires: string; uuid: string }[];
};

export type Review = {
  token: string;
  app_name: string;
  bundle_id: string;
  version: string | null;
  device_name: string;
  size_bytes: number;
  sha256: string;
  existing_app: { version: string | null; build: string | null } | null;
  blockers: string[];
  notes: string[];
};
export type Job = {
  id: string;
  stage:
    | "preparing"
    | "transferring"
    | "installing"
    | "installed"
    | "failed"
    | "cancelled"
    | "unknown";
  message: string;
  transferred_bytes: number;
  total_bytes: number;
  device_percent: number | null;
  cleanup_pending: boolean;
};

export type LogLine = { text: string };
export type LogSummary = {
  matched: number;
  discarded: number;
  message: string;
};

/// What the team panel reports upward, so the pipeline can derive gates in one place instead of
/// each control working its own state out from whatever is in scope.
export type TeamStatus = {
  signedIn: boolean;
  account: string | null;
  teamId: string | null;
  teamLabel: string | null;
  registered: boolean;
  registrationSummary: string;
  certificate: boolean;
  certificateSummary: string;
  watch: WatchChoice;
};

/// How far a remembered build has left of a free team's seven days.
///
/// Rust decides all of this, including the sentence: the same words appear in the window, in a
/// screenshot of it, and in the log, rather than each surface phrasing the arithmetic its own way.
export type Standing =
  | { state: "valid"; days: number }
  | { state: "expires_today" }
  | { state: "expired"; days: number }
  | { state: "long_expired" };

/// Whether the remembered build is the one on screen. Anything but `same_app` means the countdown
/// is about something else, so no countdown is shown.
export type Bearing = "same_app" | "other_app" | "other_team" | "unknown";

export type Renewal = {
  identifier: string;
  app_name: string;
  watch: WatchChoice;
  standing: Standing;
  bearing: Bearing;
  sentence: string;
  /// The build on screen has run out. The one case that moves above the signing controls.
  urgent: boolean;
};

/// A backend failure's stable classification.
///
/// Mirrors `orbiter_core::domain::errors::ErrorCode`. The window switches on these; it never
/// parses the message. Adding one is a change to a contract both sides share.
export type ErrorCode =
  | "artifact_missing"
  | "artifact_changed"
  | "operation_in_progress"
  | "review_stale"
  | "acknowledgement_required"
  | "device_unavailable"
  | "authentication_required"
  | "storage_read"
  | "storage_write"
  | "storage_corrupt"
  | "storage_unsupported_version"
  | "cancelled"
  | "outcome_unknown"
  | "invalid_request"
  | "internal";
