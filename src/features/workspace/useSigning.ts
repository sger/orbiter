import { useCallback, useState } from "react";
import {
  channel,
  signIpa,
  librarySign,
  libraryChanged,
} from "../../ipc/commands";
import type { Signed, SigningProgress, WatchChoice } from "../../types";
import { message } from "../../ipc/failure";

export function useSigning(artifactId?: string) {
  const [signed, setSigned] = useState<Signed | null>(null);
  const [signedArtifactId, setSignedArtifactId] = useState<
    string | undefined
  >();
  const [signing, setSigning] = useState(false);
  const [signError, setSignError] = useState<string | null>(null);
  const [signStep, setSignStep] = useState<SigningProgress | null>(null);
  const invalidate = useCallback(() => {
    setSigned(null);
    setSignedArtifactId(undefined);
    setSignError(null);
  }, []);
  async function sign(path: string, watch: WatchChoice, marker: string) {
    setSigning(true);
    invalidate();
    setSignStep(null);
    try {
      if (artifactId) {
        const saved = await librarySign(
          artifactId,
          watch,
          marker,
          channel<SigningProgress>(setSignStep),
        );
        setSigned(saved.signed);
        setSignedArtifactId(saved.artifact.id);
        libraryChanged();
      } else
        setSigned(
          await signIpa(
            path,
            watch,
            marker,
            channel<SigningProgress>(setSignStep),
          ),
        );
    } catch (error) {
      setSignError(message(error));
    } finally {
      setSigning(false);
      setSignStep(null);
    }
  }
  return {
    signedArtifactId,
    signed,
    signing,
    signError,
    signStep,
    invalidate,
    sign,
  };
}
