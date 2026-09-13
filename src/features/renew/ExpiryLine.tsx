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
  onRefresh,
  refreshLabel = "Re-sign now",
}: {
  expiry: LibraryExpiry | null;
  /// `line` sits among the hints of a row; `banner` is the two-state block above an action.
  variant: "line" | "banner";
  canResign?: boolean;
  onResign?: () => void;
  /// Offered where re-signing is not on this screen: it opens the one that does it, with the
  /// previous answers filled in, and asks for nothing on the way. Absent when the original the
  /// build was made from is gone, because there would be nothing for that screen to open.
  onRefresh?: () => void;
  refreshLabel?: string;
}) {
  if (!expiry) return null;
  if (variant === "line") {
    return (
      <p
        className="hint"
        data-standing={expiry.standing.state}
        // Rust decides when a countdown stops being background; this only weights it.
        data-soon={expiry.soon || undefined}
      >
        {expiry.sentence}
        {expiry.device_name && <> · {expiry.device_name}</>}
      </p>
    );
  }
  return (
    <section
      className={`renewal ${expiry.urgent ? "renewal-urgent" : expiry.soon ? "renewal-soon" : ""}`}
      // Announced, not interrupting: this appears while a person is reading something else.
      role="status"
      data-standing={expiry.standing.state}
      data-bearing={expiry.bearing}
    >
      {expiry.urgent || expiry.soon ? (
        <CircleAlert size={17} />
      ) : (
        <CalendarClock size={17} />
      )}
      <p>
        {expiry.sentence}
        {expiry.device_name && <> · {expiry.device_name}</>}
      </p>
      {expiry.urgent && canResign && onResign && (
        <button className="primary" onClick={onResign}>
          Re-sign now
        </button>
      )}
      {onRefresh && (
        <button
          className={expiry.urgent || expiry.soon ? "primary" : "text-button"}
          onClick={onRefresh}
        >
          {refreshLabel}
        </button>
      )}
    </section>
  );
}
