import { CalendarClock, CircleAlert } from "lucide-react";
import type { LibraryExpiry } from "../library/types";

/// The seven-day line for a build Orbiter actually installed.
///
/// A free team's profile lasts seven days and the app then refuses to launch, telling the tester
/// nothing. Orbiter used to say so once, while signing, and never again — and then, briefly, not
/// at all. This is the part that keeps saying it.
///
/// Every word comes from Rust, including the day count: `renewal::line` is the only place that
/// wording exists, so this page, the library and a screenshot of either cannot disagree. Nothing
/// here re-signs on its own or contacts Apple; the banner offers the same click the main control
/// does, and only when that control would accept it.
export function ExpiryLine({
  expiry,
  variant,
  canResign,
  onResign,
}: {
  expiry: LibraryExpiry | null;
  /// `line` sits among the hints of a row; `banner` is the two-state block above an action.
  variant: "line" | "banner";
  canResign?: boolean;
  onResign?: () => void;
}) {
  if (!expiry) return null;
  if (variant === "line") {
    return (
      <p className="hint" data-standing={expiry.standing.state}>
        {expiry.sentence}
        {expiry.device_name && <> · {expiry.device_name}</>}
      </p>
    );
  }
  return (
    <section
      className={`renewal ${expiry.urgent ? "renewal-urgent" : ""}`}
      // Announced, not interrupting: this appears while a person is reading something else.
      role="status"
      data-standing={expiry.standing.state}
      data-bearing={expiry.bearing}
    >
      {expiry.urgent ? <CircleAlert size={17} /> : <CalendarClock size={17} />}
      <p>
        {expiry.sentence}
        {expiry.device_name && <> · {expiry.device_name}</>}
      </p>
      {expiry.urgent && canResign && onResign && (
        <button className="primary" onClick={onResign}>
          Re-sign now
        </button>
      )}
    </section>
  );
}
