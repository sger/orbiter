import { useCallback, useEffect, useRef, useState } from "react";
import { discoverDevices, isTauri } from "../../ipc/commands";
import type { Discovery, Transport } from "../../types";
import { RefreshCw, Smartphone } from "lucide-react";
import { Select } from "../../components/ui/Select";
/// What each transport is called where a person reads it. "Wi-Fi" is the word people use for it;
/// "Network" is what the device daemon calls it and means nothing to anyone else.
const transports = {
  usb: "USB",
  network: "Wi-Fi",
  unknown: "Unknown connection",
};
const labels = {
  paired: "Pairing verified",
  locked: "Locked",
  trust_required: "Trust needed",
  pairing_unverified: "Pairing unverified",
  unavailable: "Unavailable",
};
export function Devices({
  onSelect,
  onConnection,
  paused = false,
}: {
  onSelect?: (id: number | null) => void;
  /// How the selected phone is being reached, for screens that keep saying so after this one.
  /// Reported separately from the id, and as a plain value, so it cannot churn on every poll.
  onConnection?: (connection: Transport | null) => void;
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
            "Device discovery failed. Check the connection and the Apple device service, then refresh.",
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
      device?.state === "paired" ? device.id : null,
    );
    onConnection?.(device?.state === "paired" ? device.connection : null);
  }, [device?.id, device?.state, device?.connection, onSelect, onConnection]);
  return (
    <div className="devices" tabIndex={-1} data-stage="device">
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
      <Select
        id="device"
        icon={<Smartphone size={17} className="flex-none" />}
        disabled={!desktop || !result?.devices.length || busy || paused}
        value={selected}
        onChange={setSelected}
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
            {d.name ?? d.product_type ?? "Apple device (identity unverified)"} ·{" "}
            {transports[d.connection]} · {labels[d.state]}
          </option>
        ))}
      </Select>
      <div className="device-status" role="status" aria-live="polite">
        {device ? (
          <>
            <strong>
              {transports[device.connection]} · {labels[device.state]}
              {device.ios_version ? ` · iOS ${device.ios_version}` : ""}
            </strong>
            <p>{device.message}</p>
            {/* The same phone, reachable two ways. Said rather than listed twice: a list where
                every phone appears once per cable is a list nobody can read. */}
            {device.alternate && (
              <p className="hint">
                Also reachable over {transports[device.alternate]}. Orbiter uses{" "}
                {transports[device.connection]} because it is faster and does not
                depend on staying in range.
              </p>
            )}
            {device.connection === "network" && device.state === "paired" && (
              <p className="hint">
                Installing over Wi-Fi works and takes longer than a cable. If the
                connection drops part-way, the transfer starts again — Orbiter
                never repeats it on its own.
              </p>
            )}
          </>
        ) : (
          <p>
            {result?.message ??
              (desktop && result
                ? "Connect an iPhone by cable and unlock it. To use one over Wi-Fi, connect it by cable once, trust this Mac, and turn on “Show this iPhone when on Wi-Fi” in Finder — an iPhone that has never been trusted here cannot be reached over Wi-Fi at all."
                : "Connect an iPhone to inspect its connection and pairing state.")}
          </p>
        )}
      </div>
    </div>
  );
}
