import { useCallback, useState } from "react";
import { channel, signIpa } from "../../ipc/commands";
import type { Signed, SigningProgress, WatchChoice } from "../../types";

export function useSigning() {
  const [signed, setSigned] = useState<Signed | null>(null);
  const [signing, setSigning] = useState(false);
  const [signError, setSignError] = useState<string | null>(null);
  const [signStep, setSignStep] = useState<SigningProgress | null>(null);
  const invalidate = useCallback(() => {
    setSigned(null);
    setSignError(null);
  }, []);
  async function sign(path: string, watch: WatchChoice) {
    setSigning(true);
    invalidate();
    setSignStep(null);
    try {
      setSigned(
        await signIpa(path, watch, channel<SigningProgress>(setSignStep)),
      );
    } catch (error) {
      setSignError(
        typeof error === "string" ? error : "Signing did not complete.",
      );
    } finally {
      setSigning(false);
      setSignStep(null);
    }
  }
  return { signed, signing, signError, signStep, invalidate, sign };
}
