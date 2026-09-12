import { useEffect, useRef } from "react";
import { X } from "lucide-react";
import { HelpContent } from "./HelpContent";

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
      className="fixed inset-y-0 right-0 z-20 flex w-[min(460px,100vw-68px)] flex-col border-l border-line bg-surface help-panel"
      role="dialog"
      aria-label="Help"
      aria-modal="false"
      ref={panel}
    >
      <div className="flex items-center justify-between border-b border-line-soft px-[22px] py-[18px]">
        <strong>Help</strong>
        <button
          className="text-button"
          onClick={onClose}
          aria-label="Close help"
        >
          <X size={16} />
        </button>
      </div>
      <div className="help-body overflow-y-auto px-[22px] pt-1 pb-8 text-body leading-[1.7] text-ink-muted">
        <HelpContent />
      </div>
    </div>
  );
}
