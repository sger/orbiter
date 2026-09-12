import { CircleHelp, Orbit } from "lucide-react";
import type { Stage } from "../state/pipeline";

/// The left rail: progress through the pipeline, then anything that is not progress.
///
/// The two are kept apart deliberately. Progress is generated from the pipeline's stages, so it
/// grows on its own when a stage is added and nothing here needs editing. Items are a fixed,
/// hand-written list — Help today, settings or a second workspace later — and they sit below a
/// divider so a new one can never be mistaken for another step of the flow. The rail is not
/// navigation: nothing here changes what the main column shows except by opening a panel beside it.
export type RailItem = {
  id: string;
  label: string;
  icon: React.ReactNode;
  active?: boolean;
  onSelect: () => void;
};

export function Rail({
  stages,
  items,
  onSelectStage,
}: {
  stages: Stage[];
  items: RailItem[];
  /// Scrolls the column to that stage. A dot is a shortcut, not a route.
  onSelectStage: (id: string) => void;
}) {
  return (
    <aside className="fixed inset-y-0 left-0 flex w-[52px] flex-col items-center bg-rail px-0 py-[27px] text-rail-ink md:w-[68px]">
      <a
        className="flex text-rail-bright no-underline"
        href="#"
        aria-label="Orbiter home"
      >
        <Orbit size={29} />
      </a>
      <ol
        className="mt-[34px] flex list-none flex-col items-center gap-2.5 p-0"
        aria-label="Progress"
      >
        {stages.map((stage) => (
          <li key={stage.id}>
            <button
              title={`${stage.title} — ${stage.state}`}
              onClick={() => onSelectStage(stage.id)}
              className={`block h-[9px] w-[9px] rounded-full border p-0 ${
                stage.state === "done"
                  ? "border-go bg-go"
                  : stage.state === "current"
                    ? "border-rail-bright bg-rail-bright shadow-[0_0_0_3px_rgba(240,245,222,0.18)]"
                    : "border-rail-line bg-transparent"
              }`}
            >
              <span className="sr-only">
                {stage.title}: {stage.state}
              </span>
            </button>
          </li>
        ))}
      </ol>
      {/* Everything below the divider is not a step. */}
      <div className="mt-auto flex w-full flex-col items-center gap-1 border-t border-rail-line/40 pt-4">
        {items.map((item) => (
          <button
            key={item.id}
            title={item.label}
            aria-label={item.label}
            aria-expanded={item.active}
            onClick={item.onSelect}
            className={`grid place-items-center rounded-lg border-0 bg-transparent p-2 hover:text-rail-bright ${
              item.active ? "bg-rail-line/40 text-rail-bright" : "text-rail-dim"
            }`}
          >
            {item.icon}
          </button>
        ))}
      </div>
    </aside>
  );
}

/// The rail's fixed items. Adding one is adding an entry here.
export function helpItem(active: boolean, onSelect: () => void): RailItem {
  return {
    id: "help",
    label: "Help",
    icon: <CircleHelp size={19} />,
    active,
    onSelect,
  };
}
