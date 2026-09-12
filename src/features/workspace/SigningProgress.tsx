import { Check } from "lucide-react";
import type { Stage } from "../../state/pipeline";

export function SigningProgress({ stages }: { stages: Stage[] }) {
  const current = stages.find((stage) => stage.state === "current");
  const done = stages.filter((stage) => stage.state === "done").length;
  return (
    <section className="signing-progress card" aria-label="Signing progress">
      <div className="timeline-heading">
        <h2>Signing progress</h2>
        <span>
          {current ? `Next: ${current.title}` : "Ready to install"} · {done}/
          {stages.length}
        </span>
      </div>
      <ol aria-label="Signing steps">
        {stages.map((stage, index) => (
          <li key={stage.id} data-state={stage.state}>
            <button
              data-state={stage.state}
              aria-label={`${stage.title}: ${stage.state}`}
              aria-current={stage.state === "current" ? "step" : undefined}
              onClick={() => {
                const target =
                  document.querySelector<HTMLElement>(
                    `[data-stage="${stage.id}"]`,
                  ) ?? document.querySelector<HTMLElement>(".accounts");
                target?.scrollIntoView({
                  block: "center",
                  behavior: window.matchMedia(
                    "(prefers-reduced-motion: reduce)",
                  ).matches
                    ? "auto"
                    : "smooth",
                });
                target?.focus({ preventScroll: true });
              }}
            >
              <span className="stage-dot" aria-hidden="true">
                {stage.state === "done" ? <Check size={14} /> : index + 1}
              </span>
              <span className="timeline-label">{stage.title}</span>
              <small>
                {stage.state === "done"
                  ? "Done"
                  : stage.state === "current"
                    ? "Next"
                    : "Waiting"}
              </small>
            </button>
          </li>
        ))}
      </ol>
    </section>
  );
}
