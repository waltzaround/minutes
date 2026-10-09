import { useQuery } from "@tanstack/react-query";
import { meetingDetailApi } from "@/lib/api";
import { useEffect, useRef, useState } from "react";
import { EVENTS, useTauriEvent } from "@/lib/api/events";
import type { LiveSegment } from "@/lib/types";
import { formatClock } from "@/lib/utils/format";

/** Provisional transcript shown while recording. Speaker names may change after the meeting. */
export function LiveTranscript({ meetingId }: { meetingId: string }) {
  const { data: saved } = useQuery({ queryKey: ["liveTranscript", meetingId], queryFn: () => meetingDetailApi.get(meetingId) });
  const [segments, setSegments] = useState<LiveSegment[]>([]);
  const endRef = useRef<HTMLDivElement>(null);
  const stick = useRef(true);

  useTauriEvent<LiveSegment>(EVENTS.transcriptSegment, (s) => {
    if (s.meetingId !== meetingId) return;
    setSegments((prev) => [...prev, s].sort((a, b) => a.startMs - b.startMs));
  });

  useEffect(() => {
    if (stick.current) endRef.current?.scrollIntoView({ block: "end" });
  }, [segments]);

  const all = [...new Map([
    ...(saved?.segments ?? []).map((s): [string, LiveSegment] => [s.id, { ...s, meetingId, speakerLabel: s.source === "microphone" ? "Microphone" : "Speaker" }]),
    ...segments.map((s): [string, LiveSegment] => [s.id, s]),
  ]).values()].sort((a, b) => a.startMs - b.startMs);

  if (all.length === 0) {
    return <p className="py-6 text-muted-foreground">The transcript appears here as people speak.</p>;
  }
  return (
    <div
      className="selectable space-y-3 py-4"
      aria-live="polite"
      onScroll={(e) => {
        const el = e.currentTarget.parentElement;
        if (el) stick.current = el.scrollHeight - el.scrollTop - el.clientHeight < 80;
      }}
    >
      {all.map((s) => (
        <div key={s.id} className="grid grid-cols-[120px_1fr] gap-4">
          <div className="text-xs leading-5">
            <div className="truncate font-medium">{s.speakerLabel}</div>
            <div className="text-muted-foreground tabular-nums">{formatClock(s.startMs)}</div>
          </div>
          <p className="leading-5">{s.text}</p>
        </div>
      ))}
      <div ref={endRef} />
    </div>
  );
}
