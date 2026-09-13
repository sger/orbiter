import { useCallback, useEffect, useRef, useState } from "react";
import type { Opened } from "../library/types";
import type { Job, Review, WatchChoice } from "../../types";
import {
  cancelInstall,
  channel,
  discardInstall,
  discardPreparation,
  executeInstall,
  executePreparation,
  installationStatus,
  isTauri,
  libraryChanged,
  libraryOpen,
  libraryList,
  libraryPrepareInstall,
  preparationStatus,
  reviewPreparation,
} from "../../ipc/commands";
import { failure, message } from "../../ipc/failure";
import type {
  Consents,
  FlowStage,
  PreparationReview,
  PreparationStatus,
} from "./types";

export function useGuidedFlow(
  selected: Opened | null | undefined,
  deviceId: number | null,
  accountIdentity: string,
  watch: WatchChoice,
  marker: string,
  /// Absolute paths of libraries to inject into the app; empty for a plain re-sign.
  dylibs: string[] = [],
) {
  const [stage, setStage] = useState<FlowStage>("choose");
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState("");
  const [errorCode, setErrorCode] = useState("");
  const [review, setReview] = useState<Review | null>(null);
  const [preparation, setPreparation] = useState<PreparationReview | null>(
    null,
  );
  const [progress, setProgress] = useState<PreparationStatus | null>(null);
  const [output, setOutput] = useState<Opened | null>(null);
  const [job, setJob] = useState<Job | null>(null);
  const operation = useRef(false),
    generation = useRef(0);
  const uncertain = useRef(false);
  const recovering = useRef(false);
  const [reconnect, setReconnect] = useState(0);
  const [recovered, setRecovered] = useState(false);
  const [recoveryName, setRecoveryName] = useState<string | null>(null);
  const currentReview = useRef<Review | null>(null),
    currentPlan = useRef<PreparationReview | null>(null);
  const clearReview = useCallback(() => {
    const previous = currentReview.current;
    currentReview.current = null;
    setReview(null);
    if (previous) void discardInstall(previous.token).catch(() => {});
  }, []);
  const clearPlan = useCallback(() => {
    const previous = currentPlan.current;
    currentPlan.current = null;
    setPreparation(null);
    if (previous) void discardPreparation(previous.token).catch(() => {});
  }, []);
  useEffect(() => {
    if (operation.current || recovering.current) return;
    generation.current++;
    clearReview();
    clearPlan();
    setOutput(null);
    setRecovered(false);
    setRecoveryName(null);
    setError("");
    setJob(null);
    setProgress(null);
    setStage("choose");
  }, [selected?.artifact.id, deviceId, clearReview, clearPlan]);
  useEffect(() => {
    if (operation.current || recovering.current) return;
    generation.current++;
    clearPlan();
    clearReview();
    setOutput(null);
    setStage((current) =>
      ["prepare", "review"].includes(current) ? "account" : current,
    );
  }, [accountIdentity, watch, marker, dylibs, clearPlan, clearReview]);
  useEffect(
    () => () => {
      generation.current++;
      clearReview();
      clearPlan();
    },
    [clearReview, clearPlan],
  );

  // Reconnect to native work after a webview reload. Never restart an interrupted operation.
  useEffect(() => {
    if (!isTauri()) return;
    let disposed = false;
    let timer: ReturnType<typeof setTimeout>;
    let previous: "install" | "sign" | null = null;
    let recoveredJob: string | null = null;
    async function poll() {
      if (operation.current) {
        timer = setTimeout(poll, 1000);
        return;
      }
      try {
        const [install, signing] = await Promise.all([
          installationStatus(),
          preparationStatus(),
        ]);
        if (disposed || operation.current) return;
        const installing =
          !!install &&
          ["preparing", "transferring", "installing"].includes(install.stage);
        const signingActive = [
          "checking",
          "registration",
          "certificate",
          "provisioning",
          "signing",
        ].includes(signing?.stage);
        recovering.current = installing || signingActive;
        setBusy(installing || signingActive);
        if (installing && recoveredJob !== install.id) {
          const snapshot = await libraryList();
          const attempt = snapshot.attempts.find(
            (attempt) => attempt.id === install.id,
          );
          if (disposed || operation.current) return;
          setRecovered(true);
          setRecoveryName(attempt?.app_name ?? "Previous installation");
          if (attempt) {
            try {
              setOutput(await libraryOpen(attempt.artifact_id));
            } catch {
              setOutput(null);
            }
          }
          recoveredJob = install.id;
        }
        if (installing) {
          previous = "install";
          setJob(install);
          setStage("install");
        } else if (signingActive) {
          if (previous !== "sign" && signing.source_artifact_id) {
            try {
              setOutput(await libraryOpen(signing.source_artifact_id));
            } catch {
              setOutput(null);
            }
          }
          previous = "sign";
          setProgress(signing);
          setStage("prepare");
        } else if (previous === "install" && install) {
          setJob(install);
          setStage("result");
        } else if (previous === "sign") {
          setProgress(signing);
          if (signing.artifact_id) {
            setOutput(await libraryOpen(signing.artifact_id));
            setStage("check");
          }
        } else if (
          install &&
          ["unknown", "failed", "cancelled"].includes(install.stage)
        ) {
          setJob(install);
          setStage("result");
        }
        uncertain.current = false;
        setBusy(installing || signingActive);
        if (installing || signingActive) timer = setTimeout(poll, 1000);
      } catch {
        if (!disposed) {
          uncertain.current = true;
          setBusy(true);
          timer = setTimeout(poll, 3000);
          setError(
            "Could not read previous operation status. Reconnecting before allowing another operation…",
          );
        }
      }
    }
    void poll();
    return () => {
      disposed = true;
      clearTimeout(timer);
    };
  }, [reconnect]);

  async function run(action: (epoch: number) => Promise<void>) {
    if (operation.current || recovering.current || busy) return;
    operation.current = true;
    setBusy(true);
    setError("");
    setErrorCode("");
    const epoch = generation.current;
    try {
      await action(epoch);
    } catch (error) {
      if (epoch === generation.current) {
        setError(message(error));
        setErrorCode(failure(error).code);
      }
    } finally {
      operation.current = false;
      setBusy(uncertain.current);
      libraryChanged();
    }
  }
  async function check(target = output ?? selected) {
    if (!target || deviceId === null) return;
    await run(async (epoch) => {
      clearReview();
      clearPlan();
      setJob(null);
      setStage("check");
      const next = await libraryPrepareInstall(target.artifact.id, deviceId);
      if (epoch !== generation.current) {
        void discardInstall(next.token);
        return;
      }
      currentReview.current = next;
      setReview(next);
    });
  }
  function navigate(next: FlowStage) {
    if (operation.current || recovering.current || busy) return;
    if (
      next === "review" &&
      (!review || review.blockers.length || review.readiness !== "direct")
    )
      return;
    if (next === "account") clearReview();
    if (next === "choose") {
      if (recovered) setOutput(null);
      setRecovered(false);
      setRecoveryName(null);
      clearReview();
      clearPlan();
    }
    setError("");
    setStage(next);
  }
  async function plan() {
    if (!selected || deviceId === null) return;
    await run(async (epoch) => {
      clearPlan();
      setProgress(null);
      setStage("prepare");
      const next = await reviewPreparation(
        selected.artifact.id,
        deviceId,
        watch,
        marker,
        dylibs,
      );
      if (epoch !== generation.current) {
        void discardPreparation(next.token);
        return;
      }
      currentPlan.current = next;
      setPreparation(next);
    });
  }
  async function prepareAndSign(consents: Consents) {
    if (!preparation || preparation.plan.blockers.length) return;
    await run(async (epoch) => {
      const token = preparation.token;
      currentPlan.current = null;
      setPreparation(null);
      setProgress({
        stage: "checking",
        message: "Preparing the reviewed signing actions…",
        completed: [],
        artifact_id: null,
      });
      let result;
      try {
        result = await executePreparation(
          token,
          consents,
          channel<PreparationStatus>((step) => {
            if (epoch === generation.current) setProgress(step);
          }),
        );
      } catch (error) {
        setProgress((current) => ({
          stage: "failed",
          message: message(error),
          completed: current?.completed ?? [],
          artifact_id: null,
        }));
        uncertain.current = true;
        setReconnect((value) => value + 1);
        throw error;
      }
      const opened = await libraryOpen(result.artifact.id);
      if (epoch !== generation.current) return;
      setOutput(opened);
      setStage("check");
      if (deviceId === null) return;
      const next = await libraryPrepareInstall(opened.artifact.id, deviceId);
      if (epoch !== generation.current) {
        void discardInstall(next.token);
        return;
      }
      currentReview.current = next;
      setReview(next);
    });
  }
  async function install(accepted: boolean) {
    if (
      !review ||
      !accepted ||
      review.blockers.length ||
      review.readiness !== "direct"
    )
      return;
    await run(async (epoch) => {
      setJob(null);
      setStage("install");
      try {
        const result = await executeInstall(
          review.token,
          accepted,
          channel<Job>((next) => {
            if (epoch === generation.current) setJob(next);
          }),
        );
        if (epoch === generation.current) setJob(result);
      } catch (error) {
        uncertain.current = true;
        setReconnect((value) => value + 1);
        throw error;
      } finally {
        clearReview();
        if (epoch === generation.current) setStage("result");
      }
    });
  }
  async function cancel() {
    try {
      if (!(await cancelInstall()))
        setError(
          "iOS installation has started or already finished. Wait for the reported outcome.",
        );
    } catch (error) {
      setError(message(error));
    }
  }
  return {
    stage,
    recovered,
    recoveryName,
    busy,
    errorCode,
    error,
    setError,
    review,
    preparation,
    progress,
    output,
    job,
    check,
    navigate,
    plan,
    prepareAndSign,
    install,
    cancel,
  };
}
