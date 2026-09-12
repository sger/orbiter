import type { ReactNode } from "react";
import { ChevronDown } from "lucide-react";

export function Select({
  id,
  icon,
  value,
  disabled,
  onChange,
  children,
}: {
  id: string;
  icon?: ReactNode;
  value: string;
  disabled?: boolean;
  onChange?: (value: string) => void;
  children: ReactNode;
}) {
  return (
    <div className="select-control" data-disabled={disabled || undefined}>
      {icon && (
        <span className="select-icon" aria-hidden="true">
          {icon}
        </span>
      )}
      <select
        id={id}
        disabled={disabled}
        value={value}
        onChange={(event) => onChange?.(event.target.value)}
      >
        {children}
      </select>
      <ChevronDown size={14} className="select-chevron" aria-hidden="true" />
    </div>
  );
}
