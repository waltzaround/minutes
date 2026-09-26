import { useQueryClient } from "@tanstack/react-query";
import { useEffect } from "react";
import { useLocation, useNavigate } from "react-router";
import { toast } from "sonner";
import { meetingsApi } from "@/lib/api";
import { EVENTS, useTauriEvent } from "@/lib/api/events";
import { useRecordingStatus } from "@/lib/api/queries";
import type { MeetingWarning, MemoryWarning } from "@/lib/types";

const isMac = typeof navigator !== "undefined" && /Mac/i.test(navigator.platform);

/**
 * App-wide behaviour: warnings that must be visible from any screen, and
 * keyboard shortcuts.
 *   ⌘/Ctrl+Shift+R  start or go to the recording
 *   ⌘/Ctrl+,        settings
 *   ⌘/Ctrl+1 / 2    new meeting / people
 *   ⌘/Ctrl+K        search (handled by the sidebar)
 */
export function GlobalEffects() {
  const navigate = useNavigate();
  const location = useLocation();
  const qc = useQueryClient();
  const { data: recording } = useRecordingStatus();

  // Keep the sidebar meeting list fresh as meetings are processed.
  useTauriEvent(EVENTS.processingProgress, () => void qc.invalidateQueries({ queryKey: ["meetings"] }));
  useTauriEvent(EVENTS.analysisProgress, () => void qc.invalidateQueries({ queryKey: ["meetings"] }));
  useTauriEvent<MemoryWarning>(EVENTS.memoryWarning, (w) => toast.warning(w.message, { duration: 10_000 }));
  useTauriEvent<MeetingWarning>(EVENTS.meetingWarning, (w) => {
    // The recording screen shows these inline; elsewhere, toast them.
    if (location.pathname !== "/recording") toast.warning(w.message, { duration: 15_000, action: { label: "View", onClick: () => navigate("/recording") } });
  });

  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      const mod = isMac ? e.metaKey : e.ctrlKey;
      if (!mod) return;
      if (e.shiftKey && e.key.toLowerCase() === "r") {
        e.preventDefault();
        if (recording) navigate("/recording");
        else
          meetingsApi.start().then(
            (s) => {
              qc.setQueryData(["recording"], s);
              navigate("/recording");
            },
            (err) => toast.error(err.message),
          );
      } else if (e.key === ",") {
        e.preventDefault();
        navigate("/settings");
      } else if (e.key === "1") {
        e.preventDefault();
        navigate("/");
      } else if (e.key === "2") {
        e.preventDefault();
        navigate("/people");
      }
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [navigate, qc, recording]);

  return null;
}
