import { useLayoutEffect, useState } from "react";

export type Appearance = "system" | "light" | "dark";
const storageKey = "orbiter.appearance";
const systemQuery = "(prefers-color-scheme: dark)";

function readAppearance(): Appearance {
  try {
    const value = localStorage.getItem(storageKey);
    return value === "light" || value === "dark" ? value : "system";
  } catch {
    return "system";
  }
}

function applyAppearance(appearance: Appearance) {
  document.documentElement.dataset.theme =
    appearance === "system"
      ? window.matchMedia(systemQuery).matches
        ? "dark"
        : "light"
      : appearance;
}

// Apply before mounting so a saved dark preference is used on the first render.
export function initializeAppearance() {
  applyAppearance(readAppearance());
}

export function useAppearance() {
  const [appearance, setAppearance] = useState<Appearance>(readAppearance);
  useLayoutEffect(() => {
    const query = window.matchMedia(systemQuery);
    const update = () => applyAppearance(appearance);
    update();
    query.addEventListener("change", update);
    return () => query.removeEventListener("change", update);
  }, [appearance]);
  const changeAppearance = (value: Appearance) => {
    setAppearance(value);
    try {
      localStorage.setItem(storageKey, value);
    } catch {
      /* Still applies for this session. */
    }
  };
  return { appearance, changeAppearance };
}
