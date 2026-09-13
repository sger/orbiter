import { CircleAlert } from "lucide-react";
import type { LibraryExpiry } from "../library/types";

/// What is running out anywhere in the library, said once, wherever a person happens to be.
///
/// Every countdown Orbiter has shown until now lives on the screen for the build it is about, so
/// hearing it required already having gone looking. That is backwards for the one fact worth
/// arriving unprompted: a build stops launching on someone else's phone, and the person who can
/// fix it is the one who opened this window to do something unrelated.
///
/// One line, and only the worst state it has. A build that has stopped working outranks one that
/// is about to, and saying both at once would make neither legible. Nothing is announced twice:
/// this is a link to the app, not a second copy of its countdown.
///
/// For a single build the sentence is Rust's own, unchanged — the same words the library page and
/// the workspace use, so three screens cannot describe one fact three ways. Only a count across
/// several builds is assembled here, because no single record is about all of them.
export function AttentionNotice({
  urgent,
  soon,
  route,
}: {
  urgent: LibraryExpiry[];
  soon: LibraryExpiry[];
  /// Where the person already is. A build's own pages carry its countdown, so repeating it above
  /// them would be the same sentence twice on one screen.
  route: string;
}) {
  const worst = urgent.length ? urgent : soon;
  if (!worst.length) return null;
  const one = worst.length === 1 ? worst[0] : null;
  if (one && route.startsWith(`ipas/${one.app_id}`)) return null;
  return (
    <p
      className={`notice ${urgent.length ? "notice-urgent" : "notice-soon"}`}
      // Announced where a person is already reading, never interrupting what they are doing.
      role="status"
      data-attention={urgent.length ? "urgent" : "soon"}
    >
      <CircleAlert size={16} />
      <a href={`#/ipas/${worst[0].app_id}`}>
        {one
          ? // Rust's sentence, verbatim. The phone is named because the tester holding it is the
            // reason this is worth interrupting a different task for.
            `${one.sentence}${one.device_name ? ` · ${one.device_name}` : ""}`
          : urgent.length
            ? `${urgent.length} installed builds have stopped launching. Sign and install them again.`
            : `${soon.length} installed builds stop launching soon.`}
      </a>
    </p>
  );
}
