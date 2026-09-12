import { ArrowRight, Clock3 } from "lucide-react";
import type { Bundle, Preparation, Signed, SigningProgress } from "../../types";
import { date } from "./format";

export function SigningPanel({
  signing,
  signed,
  signError,
  signStep,
  preparation,
  app,
  blocked,
  desktop,
  onSign,
}: {
  signing: boolean;
  signed: Signed | null;
  signError: string | null;
  signStep: SigningProgress | null;
  preparation: Preparation | null;
  app: Bundle | undefined;
  blocked: string;
  desktop: boolean;
  onSign: () => void;
}) {
  return (
    <section className="action-bar" tabIndex={-1} data-stage="sign">
      <div>
        <div className="action-label">
          <Clock3 size={16} />
          {signing
            ? "Re-signing"
            : signed
              ? "Re-signed build expires"
              : preparation?.profiles.length
                ? "Prepared profile expiration"
                : app?.profile?.expires_at
                  ? "Embedded profile expiration"
                  : "Ready when the next pieces are."}
        </div>
        <p>
          {signError ??
            (signing
              ? signStep
                ? `${signStep.stage} · ${signStep.done} of ${signStep.total}`
                : "Signing. This reads and rewrites the whole app, so it takes a while."
              : signed
                ? `${date(signed.expires)} · Re-signed ${signed.bundles_signed} bundle(s)${
                    signed.removed.length
                      ? `, removed ${signed.removed.length}`
                      : ""
                  }. Install it below.`
                : preparation?.profiles.length
                  ? `${date(
                      preparation.profiles
                        .map((profile) => profile.expires)
                        .sort()[0],
                    )} · ${
                      blocked ||
                      "Re-sign to produce an installable build. The original IPA is never changed."
                    }`
                  : app?.profile?.expires_at
                    ? `${date(app.profile.expires_at)} · ${app.profile.expired ? "Expired" : "The company build's own profile"}`
                    : blocked)}
        </p>
        {signed && (
          <details className="signing-log">
            <summary>What re-signing did</summary>
            <pre>{(signed.log ?? []).join("\n")}</pre>
          </details>
        )}
      </div>
      <button
        className="primary"
        // One source for the gate and for the sentence beside it, so a refused action can
        // never be offered without its reason.
        disabled={!desktop || signing || blocked !== ""}
        onClick={onSign}
      >
        {signing ? "Re-signing…" : "Re-sign IPA"} <ArrowRight size={17} />
      </button>
    </section>
  );
}
