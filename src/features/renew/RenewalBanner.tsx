import { CalendarClock, CircleAlert } from "lucide-react";
import type { Renewal } from "../../types";

/// The seven-day line.
///
/// A free team's profile lasts seven days and the app then refuses to launch, showing the tester
/// nothing that explains why. Orbiter used to say so once, while signing, and never again. This is
/// the part that keeps saying it.
///
/// It states a fact and offers the one action that answers it. It never re-signs on its own and
/// never contacts Apple: every step that does still waits for a click, as everything else here
/// does. Two shapes, because a countdown and a dead app are not the same news — a build that has
/// run out is announced, and anything else is a quiet line under the signing controls.
export function RenewalBanner({
  renewal,
  canResign,
  onResign,
  onForget,
}: {
  renewal: Renewal | null;
  /// Whether re-signing is possible right now. A prompt for an action that is blocked would be
  /// telling a person to do something the next control refuses.
  canResign: boolean;
  onResign: () => void;
  onForget: () => void;
}) {
  if (!renewal) return null;
  const urgent = renewal.urgent;
  return (
    <section
      className={`renewal ${urgent ? "renewal-urgent" : ""}`}
      // Announced, not interrupting: this appears while a person is reading something else.
      role="status"
      data-standing={renewal.standing.state}
      data-bearing={renewal.bearing}
    >
      {urgent ? <CircleAlert size={17} /> : <CalendarClock size={17} />}
      <p>{renewal.sentence}</p>
      {urgent && canResign && (
        <button className="primary" onClick={onResign}>
          Re-sign now
        </button>
      )}
      <button className="text-button" onClick={onForget}>
        Forget
      </button>
    </section>
  );
}
