import { ChevronDown } from "lucide-react";

/// Every dropdown in the interface, drawn the same way.
///
/// There were two: the device selector drew its own bordered row with an icon and a chevron, while
/// the team and Watch selects kept the platform's native control. Two dropdowns a few pixels apart
/// looking like different kinds of thing is the sort of difference a person reads as meaning
/// something. The native chevron is replaced by one of ours so the row's metrics are ours too.
export function Select({
  id,
  icon,
  value,
  disabled,
  onChange,
  children,
}: {
  id: string;
  /// Optional leading glyph, as the device selector has.
  icon?: React.ReactNode;
  value: string;
  disabled?: boolean;
  onChange: (value: string) => void;
  children: React.ReactNode;
}) {
  return (
    <div
      className={`flex items-center gap-2.5 rounded-control border border-line px-[11px] py-2.5 ${
        disabled
          ? "bg-surface-soft text-ink-faint"
          : "bg-surface-soft text-ink-muted"
      }`}
    >
      {icon}
      <select
        id={id}
        disabled={disabled}
        value={value}
        onChange={(event) => onChange(event.target.value)}
        className="w-full min-w-0 appearance-none border-0 bg-transparent text-sm text-ink disabled:text-ink-faint"
      >
        {children}
      </select>
      <ChevronDown size={14} className="flex-none" />
    </div>
  );
}
