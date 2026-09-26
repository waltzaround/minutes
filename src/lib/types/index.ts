export type * from "./generated";

/** Error shape returned by every Tauri command (see src-tauri/src/error.rs). */
export interface AppErrorPayload {
  code: string;
  message: string;
}
