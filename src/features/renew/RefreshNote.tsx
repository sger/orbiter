import { History } from "lucide-react";
import type { Refresh } from "../library/types";

/// What this workspace was opened to replace, when it was opened from a countdown.
///
/// A person arriving here did not choose this IPA from a list; they clicked a sentence about an
/// app that had stopped launching on someone's phone. This says so, because a workspace that looks
/// identical to a fresh one leaves them to work out whether the controls were filled in for them
/// or left at their defaults — and a filled-in answer nobody was told about is a decision made on
/// their behalf.
///
/// The phone line is the part worth being careful with. Three states, not two: the same phone, a
/// different one, and not knowing. Orbiter cannot always identify what is on the cable, and
/// announcing "this is not the phone" when the truth is "I could not tell" would send a person
/// looking for a problem that does not exist. So not knowing says nothing.
export function RefreshNote({
  refresh,
  sameDevice,
}: {
  refresh: Refresh | null;
  /// `null` when no phone is selected, or when the one attached cannot be identified.
  sameDevice: boolean | null;
}) {
  if (!refresh) return null;
  return (
    <section className="renewal" role="status" data-refresh="true">
      <History size={17} />
      <div>
        <p>
          Signing {refresh.name} again to replace the build that expired
          {refresh.device_name ? ` on ${refresh.device_name}` : ""}. The
          previous Watch decision and name marker are filled in below; change
          either before continuing.
        </p>
        {sameDevice === false && (
          <p className="hint">
            The connected iPhone is not the one that build was installed to.
            Installing here adds the app to this phone; it does not replace
            anything on the other one.
          </p>
        )}
      </div>
    </section>
  );
}
