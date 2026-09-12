import { useEffect, useState } from "react";
import { Check, CircleHelp } from "lucide-react";
import type { StageState } from "../state/pipeline";

/// One step of the pipeline: a heading, a result once it has one, and its controls while it needs
/// attention.
///
/// A satisfied stage collapses to its summary. The column used to show every control of every step
/// at once, which made the one thing a person had to do next indistinguishable from the six they
/// had already done or could not reach yet.
export function Stage({
  index,
  title,
  state,
  summary,
  blocked,
  help,
  onHelp,
  children,
}: {
  index: number;
  title: string;
  state: StageState;
  summary: string;
  /// Why this stage cannot be acted on. Shown instead of its controls when it is waiting.
  blocked?: string;
  /// Help section this stage is explained in, if any.
  help?: string;
  onHelp?: (section: string) => void;
  children?: React.ReactNode;
}) {
  // A finished stage collapses, but its controls must stay reachable: a team, a Watch choice or a
  // device can all be changed after the fact, and a summary with no way back is a dead end.
  const [reopened, setReopened] = useState(false);
  useEffect(() => {
    if (state !== "done") setReopened(false);
  }, [state]);
  const open = state === "current" || reopened;
  return (
    <section
      className="border-t border-line-soft py-3.5 first-of-type:border-t-0"
      aria-current={open || undefined}
    >
      <div className="flex items-center gap-2.5">
        <span
          className={`grid h-5 w-5 flex-none place-items-center rounded-full border text-[11px] font-semibold ${
            state === "done"
              ? "border-[#bcd9bf] bg-[#e4f1e4] text-go-deep"
              : state === "current"
                ? "border-ink-strong bg-ink-strong text-surface-tint"
                : "border-line-strong bg-surface-soft text-ink-soft"
          }`}
          aria-hidden
        >
          {state === "done" ? <Check size={13} /> : index}
        </span>
        <strong
          className={`text-sm ${state === "waiting" ? "text-ink-faint" : "text-ink-strong"}`}
        >
          {title}
        </strong>
        {summary && state === "done" && !reopened && (
          <span className="stage-summary overflow-hidden text-ellipsis whitespace-nowrap text-[13px] text-ink-soft">
            {summary}
          </span>
        )}
        {state === "done" && (
          <button
            className="ml-auto border-0 bg-transparent px-1 py-0.5 text-[13px] text-[#5d7c63] hover:text-ink-strong hover:underline"
            // Names what it changes: several stages offer one, and so does the build panel.
            aria-label={`${reopened ? "Hide" : "Change"} ${title}`}
            onClick={() => setReopened((value) => !value)}
          >
            {reopened ? "Hide" : "Change"}
          </button>
        )}
        {help && onHelp && (
          <button
            className="ml-1.5 grid place-items-center border-0 bg-transparent p-0.5 text-[#a9b5a6] last:ml-auto hover:text-ink-strong"
            // Deliberately does not repeat the stage title: a control whose accessible name
            // contains another control's name makes both ambiguous to anything matching by name.
            aria-label={`Help, step ${index}`}
            title={`About ${title}`}
            onClick={() => onHelp(help)}
          >
            <CircleHelp size={14} />
          </button>
        )}
      </div>
      {open && <div className="pt-2.5">{children}</div>}
      {state === "waiting" && blocked && (
        <p className="mt-1.5 ml-[29px] text-[13px] text-ink-faint">{blocked}</p>
      )}
    </section>
  );
}
