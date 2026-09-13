/// Reading a backend failure.
///
/// Commands used to reject with a bare string, so the interface told failures apart by matching
/// human-readable text — and rewording a sentence could change what the window did. The refactored
/// commands reject with `{ code, message }` instead: the code is a stable contract, the message is
/// the only text meant for a person.
///
/// Both shapes reach here, because the backend is being converted a workflow at a time. Anything
/// that is still a string is reported as `internal`, which is exactly what it is: a failure the
/// backend has not classified yet.
import type { ErrorCode } from "../types";

export type Failure = {
  code: ErrorCode;
  message: string;
};

/// Normalise anything a rejected `invoke` can produce into a failure with a code.
///
/// Never throws, and never returns an empty message: an interface that renders "[object Object]"
/// or an empty line has lost the failure entirely, which is worse than a vague sentence.
export function failure(error: unknown): Failure {
  if (typeof error === "string") {
    return { code: "internal", message: error };
  }
  if (
    error &&
    typeof error === "object" &&
    "message" in error &&
    typeof (error as Failure).message === "string"
  ) {
    const found = error as Failure;
    return {
      code: typeof found.code === "string" ? found.code : "internal",
      message: found.message,
    };
  }
  return {
    code: "internal",
    message: "Something went wrong. Check the log for details.",
  };
}

/// The sentence to show for a rejected `invoke`.
///
/// The common case: most call sites want the text, not the code. Use `failure` directly where the
/// interface has to behave differently for a particular code.
export function message(error: unknown): string {
  return failure(error).message;
}
