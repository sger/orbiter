import { useCallback, useEffect, useRef, useState } from "react";
import { discoverDevices, isTauri } from "../../ipc/commands";
import type { Device, Discovery } from "../../types";
import { ChevronDown, RefreshCw, Smartphone } from "lucide-react";
const labels = {
  paired: "Pairing verified",
  locked: "Locked",
  trust_required: "Trust needed",
  pairing_unverified: "Pairing unverified",
  unavailable: "Unavailable",
};
export function Devices({
  onSelect,
  paused = false,
}: {
  onSelect?: (id: number | null) => void;
  paused?: boolean;
}) {
  const [result, setResult] = useState<Discovery | null>(null);
  const [selected, setSelected] = useState("");
  const [busy, setBusy] = useState(false);
  const active = useRef(false);
  const mounted = useRef(false);
  const desktop = isTauri();
  const refresh = useCallback(async () => {
    if (active.current || !desktop || paused) return;
    active.current = true;
    setBusy(true);
    try {
      const next = await discoverDevices();
      if (!mounted.current) return;
      setResult(next);
      setSelected((current) =>
        next.devices.some((d) => String(d.id) === current)
          ? current
          : next.devices.length === 1
            ? String(next.devices[0].id)
            : "",
      );
    } catch {
      if (mounted.current) {
        setResult({
          devices: [],
          service_available: false,
          message:
            "Device discovery failed. Check the cable and Apple device service, then refresh.",
        });
        setSelected("");
      }
    } finally {
      active.current = false;
      if (mounted.current) setBusy(false);
    }
  }, [desktop, paused]);
  useEffect(() => {
    mounted.current = true;
    void refresh();
    const timer = window.setInterval(() => void refresh(), 5000);
    return () => {
      mounted.current = false;
      window.clearInterval(timer);
    };
  }, [refresh]);
  const device = result?.devices.find((d) => String(d.id) === selected);
  useEffect(() => {
    onSelect?.(
      device?.state === "paired" && device.connection === "USB"
        ? device.id
        : null,
    );
  }, [device?.id, device?.state, device?.connection, onSelect]);
  return (
    <div className="devices">
      <div className="device-label">
        <label htmlFor="device">Physical iPhone</label>
        <button
          type="button"
          className="text-button"
          disabled={!desktop || busy || paused}
          onClick={() => void refresh()}
          aria-label="Refresh devices"
        >
          <RefreshCw size={12} className={busy ? "spin" : ""} />
          {busy ? "Checking…" : "Refresh"}
        </button>
      </div>
      <div className="select-wrap">
        <Smartphone size={17} />
        <select
          id="device"
          disabled={!desktop || !result?.devices.length || busy || paused}
          value={selected}
          onChange={(e) => setSelected(e.target.value)}
        >
          <option value="">
            {!desktop
              ? "Available in desktop app"
              : !result
                ? "Checking connected devices…"
                : result.devices.length
                  ? "Select an iPhone"
                  : result.service_available
                    ? "No iPhone detected"
                    : "Device service unavailable"}
          </option>
          {result?.devices.map((d) => (
            <option value={String(d.id)} key={d.id}>
              {d.name ?? d.product_type ?? "Apple device (identity unverified)"}{" "}
              · {d.connection} · {labels[d.state]}
            </option>
          ))}
        </select>
        <ChevronDown size={14} />
      </div>
      <div className="device-status" role="status" aria-live="polite">
        {device ? (
          <>
            <strong>
              {labels[device.state]}
              {device.ios_version ? ` · iOS ${device.ios_version}` : ""}
            </strong>
            <p>{device.message}</p>
          </>
        ) : (
          <p>
            {result?.message ??
              (desktop && result
                ? "Connect an iPhone by USB, unlock it, and check Finder or Apple's device app if it isn't listed."
                : "Connect an iPhone to inspect its connection and pairing state.")}
          </p>
        )}
      </div>
    </div>
  );
}
