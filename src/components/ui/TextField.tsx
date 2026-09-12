import type { InputHTMLAttributes } from "react";

export function TextField({
  label,
  id,
  ...props
}: InputHTMLAttributes<HTMLInputElement> & { id: string; label: string }) {
  return (
    <div className="text-field">
      <label htmlFor={id}>{label}</label>
      <input {...props} id={id} />
    </div>
  );
}
