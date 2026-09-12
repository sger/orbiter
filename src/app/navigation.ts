import { useEffect, useState } from "react";

export type AppRoute = "ipas" | "help";
export const routeHref = (route: AppRoute) => `#/${route}`;

function readRoute(): AppRoute {
  return window.location.hash === "#/help" ? "help" : "ipas";
}

// Hash routes work with both the desktop asset protocol and the browser preview.
export function useAppRoute() {
  const [route, setRoute] = useState(readRoute);
  useEffect(() => {
    const update = () => {
      const next = readRoute();
      if (window.location.hash !== routeHref(next)) {
        window.history.replaceState(null, "", routeHref(next));
      }
      setRoute(next);
    };
    update();
    window.addEventListener("hashchange", update);
    return () => window.removeEventListener("hashchange", update);
  }, []);
  return route;
}
