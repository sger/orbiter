import type { Preparation } from "../../types";
export type FlowStage =
  "choose" | "check" | "account" | "prepare" | "review" | "install" | "result";
export type PreparationReview = {
  token: string;
  artifact_id: string;
  device_name: string;
  registration: boolean;
  certificate: boolean;
  provisioning: boolean;
  plan: Preparation["plan"];
  marker: string;
};
export type Consents = {
  registration: boolean;
  certificate: boolean;
  provisioning: boolean;
};
export type PreparationStatus = {
  stage:
    | "idle"
    | "checking"
    | "registration"
    | "certificate"
    | "provisioning"
    | "signing"
    | "complete"
    | "failed";
  message: string;
  completed: string[];
  artifact_id: string | null;
  source_artifact_id?: string | null;
};
