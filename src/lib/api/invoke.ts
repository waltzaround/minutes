import { invoke as tauriInvoke } from "@tauri-apps/api/core";
import type { AppErrorPayload } from "@/lib/types";

export class CommandError extends Error {
  readonly code: string;
  constructor(payload: AppErrorPayload) {
    super(payload.message);
    this.code = payload.code;
  }
}

function isPayload(e: unknown): e is AppErrorPayload {
  return typeof e === "object" && e !== null && "code" in e && "message" in e;
}

/** True when running inside the Tauri webview (false in plain `vite` dev). */
export function isTauri(): boolean {
  return typeof window !== "undefined" && "__TAURI_INTERNALS__" in window;
}

export async function invoke<T>(cmd: string, args?: Record<string, unknown>): Promise<T> {
  if (!isTauri()) {
    throw new CommandError({ code: "no_backend", message: "Open this app through Minutes, not a web browser." });
  }
  try {
    return await tauriInvoke<T>(cmd, args);
  } catch (e) {
    if (isPayload(e)) throw new CommandError(e);
    throw new CommandError({ code: "internal", message: String(e) });
  }
}
