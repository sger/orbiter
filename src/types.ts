export interface Profile {
  name: string | null;
  team_name: string | null;
  team_id: string | null;
  expires_at: string | null;
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
  bundles_signed: number;
  removed: string[];
  message: string;
};
