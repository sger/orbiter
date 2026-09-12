import {
  CircleHelp,
  Settings,
  FileArchive,
  Orbit,
  PanelLeftClose,
  PanelLeftOpen,
} from "lucide-react";
import type { ReactNode } from "react";
import { routeHref, type AppRoute } from "./navigation";

export type NavigationItem = { id: AppRoute; label: string; icon: ReactNode };
const primaryItems: NavigationItem[] = [
  { id: "ipas", label: "IPAs", icon: <FileArchive size={19} /> },
];
const utilityItems: NavigationItem[] = [
  { id: "settings", label: "Settings", icon: <Settings size={19} /> },
  { id: "help", label: "Help", icon: <CircleHelp size={19} /> },
];

export function Rail({
  route,
  expanded,
  onToggle,
  items = primaryItems,
  utilities = utilityItems,
}: {
  route: AppRoute;
  expanded: boolean;
  onToggle: () => void;
  items?: NavigationItem[];
  utilities?: NavigationItem[];
}) {
  const link = (item: NavigationItem) => (
    <a
      key={item.id}
      className="sidebar-item"
      href={routeHref(item.id)}
      title={item.label}
      aria-label={item.label}
      aria-current={route === item.id ? "page" : undefined}
    >
      {item.icon}
      {expanded && <span>{item.label}</span>}
    </a>
  );
  return (
    <aside className="sidebar" aria-label="Workspace sidebar">
      <div className="sidebar-top">
        <a href={routeHref("ipas")} aria-label="Orbiter home">
          <Orbit size={29} />
          {expanded && <span>orbiter</span>}
        </a>
      </div>
      <button
        className="sidebar-toggle"
        onClick={onToggle}
        aria-label={expanded ? "Collapse sidebar" : "Expand sidebar"}
        aria-expanded={expanded}
        title={expanded ? "Collapse sidebar" : "Expand sidebar"}
      >
        {expanded ? <PanelLeftClose size={19} /> : <PanelLeftOpen size={19} />}
        {expanded && <span>Collapse</span>}
      </button>
      <nav className="sidebar-navigation" aria-label="App navigation">
        {items.map(link)}
      </nav>
      <nav className="sidebar-utilities" aria-label="Utilities">
        {utilities.map(link)}
      </nav>
    </aside>
  );
}
