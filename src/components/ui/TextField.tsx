import { useEffect, useState, type InputHTMLAttributes } from "react";

export function TextField({
  label,
  id,
  type,
  ...props
}: InputHTMLAttributes<HTMLInputElement> & { id: string; label: string }) {
  const [visible, setVisible] = useState(false);
  const password = type === "password";
  useEffect(() => {
    if (!props.value || props.disabled) setVisible(false);
  }, [props.value, props.disabled]);
  return (
    <div className="text-field">
      <label htmlFor={id}>{label}</label>
      <div className={password ? "password-control" : undefined}>
        <input {...props} type={password && visible ? "text" : type} id={id} />
        {password && (
          <button
            type="button"
            className="password-toggle"
            aria-label={visible ? "Hide password" : "Show password"}
            aria-controls={id}
            disabled={props.disabled}
            onClick={() => setVisible((value) => !value)}
          >
            {visible ? "Hide" : "Show"}
          </button>
        )}
      </div>
    </div>
  );
}
