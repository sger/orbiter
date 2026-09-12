import { useState } from "react";
import { invoke, isTauri } from "@tauri-apps/api/core";

type Inventory = {
  identities: { fingerprint: string; name: string }[];
  available: boolean;
  message: string;
};

export function SigningIdentities({ paused }: { paused: boolean }) {
  const [inventory, setInventory] = useState<Inventory | null>(null);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  async function check() {
    setBusy(true);
    setInventory(null);
    setError(null);
    try {
      setInventory(await invoke<Inventory>("discover_signing_identities"));
    } catch {
      setError(
        "Could not check local signing identities. Check Keychain Access and retry.",
      );
    } finally {
      setBusy(false);
    }
  }
  return (
    <div className="local-signing">
      <div className="device-label">
        <strong>Local signing identities</strong>
        <button
          className="text-button"
          type="button"
          disabled={!isTauri() || busy || paused}
          onClick={() => void check()}
        >
          {busy ? "Checking…" : "Check Keychain"}
        </button>
      </div>
      <p className="hint">
        Use an existing signing certificate and private key on this Mac. This
        check lists identities without exporting keys or signing the app.
      </p>
      <div role="status" aria-live="polite">
        {inventory && <p className="hint">{inventory.message}</p>}
      </div>
      {error && <p role="alert">{error}</p>}
      {!!inventory?.identities.length && (
        <ul className="signing-identities">
          {inventory.identities.map((identity) => (
            <li key={identity.fingerprint}>
              <strong>{identity.name}</strong>
              <span>Certificate fingerprint: {identity.fingerprint}</span>
            </li>
          ))}
        </ul>
      )}
      <p className="hint">
        Profile selection and re-signing are not available yet.
      </p>
    </div>
  );
}
