import { useEffect, useRef } from "react";
import { X } from "lucide-react";
import { sections } from "./content";

/// A slide-over beside the work, not a modal over it.
///
/// Someone reading why push notifications are gone is usually looking at the finding that says so.
/// Covering the screen to explain it would take away the thing being explained.
export function HelpPanel({
  open,
  section,
  onClose,
}: {
  open: boolean;
  /// Which section to scroll to, when opened from a particular stage.
  section: string | null;
  onClose: () => void;
}) {
  const panel = useRef<HTMLDivElement | null>(null);
  useEffect(() => {
    if (!open) return;
    const onKey = (event: KeyboardEvent) => {
      if (event.key === "Escape") onClose();
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [open, onClose]);
  useEffect(() => {
    if (!open || !section) return;
    panel.current
      ?.querySelector(`#help-${section}`)
      ?.scrollIntoView({ block: "start" });
  }, [open, section]);
  if (!open) return null;
  return (
    <div
      className="help-panel"
      role="dialog"
      aria-label="Help"
      aria-modal="false"
      ref={panel}
    >
      <div className="help-head">
        <strong>Help</strong>
        <button
          className="text-button"
          onClick={onClose}
          aria-label="Close help"
        >
          <X size={16} />
        </button>
      </div>
      <div className="help-body">
        {sections.map((entry) => (
          <section key={entry.id} id={`help-${entry.id}`}>
            <h3>{entry.title}</h3>
            {entry.body}
          </section>
        ))}
      </div>
    </div>
  );
}
