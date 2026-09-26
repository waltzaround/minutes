import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import { useEffect, useRef } from "react";
import { isTauri } from "./invoke";

export const EVENTS = {
  downloadProgress: "download_progress",
  audioLevel: "audio_level",
  transcriptSegment: "transcript_segment",
  processingProgress: "processing_progress",
  analysisProgress: "analysis_progress",
  meetingWarning: "meeting_warning",
  memoryWarning: "memory_warning",
  meetingState: "meeting_state",
} as const;

/** Subscribe to a backend event for the component's lifetime. */
export function useTauriEvent<T>(event: string, handler: (payload: T) => void) {
  const ref = useRef(handler);
  ref.current = handler;
  useEffect(() => {
    if (!isTauri()) return;
    let unlisten: UnlistenFn | undefined;
    let disposed = false;
    listen<T>(event, (e) => ref.current(e.payload)).then((u) => {
      if (disposed) u();
      else unlisten = u;
    });
    return () => {
      disposed = true;
      unlisten?.();
    };
  }, [event]);
}
