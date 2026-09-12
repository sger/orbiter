import type { InputHTMLAttributes, ReactNode } from "react";

export function Checkbox({
  children,
  className = "",
  ...props
}: Omit<InputHTMLAttributes<HTMLInputElement>, "type" | "children"> & {
  children: ReactNode;
}) {
  return (
    <label className={`checkbox-field ${className}`}>
      <input {...props} type="checkbox" />
      <span>{children}</span>
    </label>
  );
}
