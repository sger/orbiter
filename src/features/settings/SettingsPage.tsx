import { Monitor, Moon, Sun } from "lucide-react";
import type { Appearance } from "../../app/appearance";

const choices = [
  {
    value: "system",
    label: "System",
    description: "Follow your device’s appearance.",
    icon: Monitor,
  },
  {
    value: "light",
    label: "Light",
    description: "Use a light gray workspace.",
    icon: Sun,
  },
  {
    value: "dark",
    label: "Dark",
    description: "Use a dark gray workspace.",
    icon: Moon,
  },
] as const;

export function SettingsPage({
  appearance,
  onAppearanceChange,
}: {
  appearance: Appearance;
  onAppearanceChange: (value: Appearance) => void;
}) {
  return (
    <section aria-label="Settings">
      <h1 className="page-title" tabIndex={-1}>
        Settings
      </h1>
      <div className="card settings-card">
        <fieldset>
          <legend>Appearance</legend>
          <p className="settings-description">
            Choose how Orbiter looks. Your preference is saved on this device.
          </p>
          <div className="appearance-options">
            {choices.map(({ value, label, description, icon: Icon }) => (
              <label className="appearance-option" key={value}>
                <input
                  type="radio"
                  name="appearance"
                  value={value}
                  checked={appearance === value}
                  onChange={() => onAppearanceChange(value)}
                />
                <Icon size={22} aria-hidden="true" />
                <strong>{label}</strong>
                <span>{description}</span>
              </label>
            ))}
          </div>
        </fieldset>
      </div>
    </section>
  );
}
