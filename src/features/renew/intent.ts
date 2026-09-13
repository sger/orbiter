import type { Refresh } from "../library/types";

/// One pending "refresh this build", handed from the library to the workspace.
///
/// The library knows which build expired; the workspace is where re-signing happens. Between them
/// is a hash route, and the route deliberately carries identifiers and nothing else — a screen
/// reached by a pasted or bookmarked link must mean the same thing as one reached by a click, and
/// an intent smuggled through the address bar would break that. So the intent travels beside the
/// navigation rather than inside it.
///
/// It holds answers, never an action. Nothing here signs, opens a plan, or contacts anything: the
/// workspace fills its controls in from this and then waits for the same clicks it always waited
/// for. If it is lost — a reload, a second window, a route taken some other way — the workspace
/// opens on the right build with its usual defaults, which is a worse offer and not a wrong one.
let pending: Refresh | null = null;

/// Record what the next workspace opening should start from. Replaces any intent not yet taken:
/// the one a person just asked for is the one they mean.
export function requestRefresh(refresh: Refresh) {
  pending = refresh;
}

/// Take the intent, if it is about this build. Returns it once and forgets it, so a prefill cannot
/// reapply itself over an answer someone has since changed by hand.
export function claimRefresh(artifactId: string | null): Refresh | null {
  if (!pending || !artifactId || pending.artifact_id !== artifactId)
    return null;
  const claimed = pending;
  pending = null;
  return claimed;
}

/// Drop any intent that was never taken, so leaving the library without following through does not
/// leave one waiting to surprise the next workspace that opens.
export function forgetRefresh() {
  pending = null;
}
