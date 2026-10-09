import { useQueryClient } from "@tanstack/react-query";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { AlertTriangle, Mic, MonitorSpeaker, Pause, Square } from "lucide-react";
import { useEffect, useState } from "react";
import { Navigate, useNavigate } from "react-router";
import { toast } from "sonner";
import { Button } from "@/components/ui/button";
import { PageBody, useHeaderInset } from "@/components/app/Page";
import { isTauri, meetingsApi } from "@/lib/api";
import { EVENTS, useTauriEvent } from "@/lib/api/events";
import { useRecordingStatus } from "@/lib/api/queries";
import type { AudioLevel, AudioSource, MeetingWarning, RecordingStatus, SourceStatus } from "@/lib/types";
import { cn } from "@/lib/utils";
import { formatClock } from "@/lib/utils/format";
import { useRecordingElapsed } from "./useRecordingElapsed";
import { LevelMeter } from "./LevelMeter";
import { LiveTranscript } from "./LiveTranscript";


function SourceCard({ label, icon: Icon, status, level, onReconnect }: {
  label: string;
  icon: typeof Mic;
  status: SourceStatus;
  level: number;
  onReconnect: () => void;
}) {
  const bad = status.state === "disconnected" || status.state === "failed";
  return (
    <div className={cn("flex items-center gap-3 rounded-xl border bg-card/60 px-3.5 py-3", bad && "border-destructive/40 bg-destructive/5")}>
      <span className={cn("flex size-8 shrink-0 items-center justify-center rounded-lg bg-accent", bad && "bg-destructive/15")}>
        <Icon className={cn("size-4", bad ? "text-destructive" : "text-muted-foreground")} aria-hidden />
      </span>
      <div className="min-w-0 flex-1">
        <div className="text-[11px] text-muted-foreground">{label}</div>
        <div className="truncate text-[13px] font-medium">
          {status.state === "off" ? "Not recording" : (status.deviceName ?? "—")}
        </div>
      </div>
      {bad ? (
        <Button size="sm" variant="outline" className="h-7" onClick={onReconnect}>
          Reconnect
        </Button>
      ) : (
        <LevelMeter level={level} disabled={status.state !== "recording"} />
      )}
    </div>
  );
}

export function RecordingPage() {
  const qc = useQueryClient();
  const navigate = useNavigate();
  const { data: status, isLoading } = useRecordingStatus();
  const [levels, setLevels] = useState<Record<AudioSource, number>>({ microphone: 0, system: 0 });
  const [warnings, setWarnings] = useState<MeetingWarning[]>([]);
  const [stopping, setStopping] = useState(false);
  const elapsed = useRecordingElapsed(status?.startedAt, status?.elapsedMs ?? 0);
  const inset = useHeaderInset();

  useEffect(() => {
    if (status) setWarnings(status.warnings);
  }, [status]);

  useEffect(() => {
    if (!isTauri() || !status) return;
    const w = getCurrentWindow();
    void w.setTitle("● Recording — Minutes").catch(() => {});
    return () => void w.setTitle("Minutes").catch(() => {});
  }, [status]);

  useTauriEvent<AudioLevel>(EVENTS.audioLevel, (l) => setLevels((prev) => ({ ...prev, [l.source]: l.level })));
  useTauriEvent<MeetingWarning>(EVENTS.meetingWarning, (w) => {
    setWarnings((prev) => [...prev, w]);
    void qc.invalidateQueries({ queryKey: ["recording"] });
  });

  if (isLoading) return null;
  if (!status) return <Navigate to="/" replace />;

  const stop = async (pause = false) => {
    setStopping(true);
    try {
      const id = await (pause ? meetingsApi.pause() : meetingsApi.stop());
      qc.setQueryData(["recording"], null);
      void qc.invalidateQueries({ queryKey: ["meetings"] });
      navigate(`/meetings/${id}`, { replace: true });
    } catch (e) {
      toast.error((e as Error).message);
      setStopping(false);
    }
  };

  const reconnect = async (source: AudioSource) => {
    try {
      const s: RecordingStatus = await meetingsApi.reconnect(source);
      qc.setQueryData(["recording"], s);
      const st = source === "microphone" ? s.microphone : s.system;
      if (st.state === "recording") toast.success(`${source === "microphone" ? "Microphone" : "Meeting audio"} reconnected`);
    } catch (e) {
      toast.error((e as Error).message);
    }
  };

  // Only show warnings that still apply (a reconnected source clears its own).
  const visible = warnings.filter((w) => {
    if (!w.source) return true;
    const st = w.source === "microphone" ? status.microphone : status.system;
    if (w.kind === "sourceDisconnected" || w.kind === "sourceFailed") return st.state !== "recording";
    return true;
  });
  const latest = visible.slice(-3);

  return (
    <div className="flex h-full flex-col">
      <header data-tauri-drag-region className={cn("flex h-12 shrink-0 items-center gap-2.5 px-5", inset)}>
        <span className="relative flex size-2.5" aria-hidden>
          <span className="absolute inline-flex size-full animate-ping rounded-full bg-recording opacity-60" />
          <span className="relative inline-flex size-2.5 rounded-full bg-recording" />
        </span>
        <span className="text-[13px] font-semibold text-recording" role="status">Recording</span>
        <span className="truncate text-[13px] text-muted-foreground" data-tauri-drag-region>{status.title}</span>
      </header>

      <PageBody>
        <div className="mx-auto max-w-2xl px-6 pb-32">
          <div className="pt-4 pb-6 text-center">
            <div className="font-display text-[44px] leading-none tabular-nums tracking-tight" aria-label="Elapsed time">{formatClock(elapsed)}</div>
            <div className="mt-2 text-xs text-muted-foreground">Everything is recorded and processed on this computer</div>
          </div>

          {latest.length > 0 && (
            <div className="mb-3 space-y-2">
              {latest.map((w, i) => (
                <div key={`${w.kind}-${w.atMs}-${i}`} role="alert" className="flex items-start gap-2.5 rounded-xl border border-warning/30 bg-warning/5 px-3.5 py-2.5 text-[13px]">
                  <AlertTriangle className="mt-0.5 size-4 shrink-0 text-warning" aria-hidden />
                  <span>{w.message}</span>
                </div>
              ))}
            </div>
          )}

          <div className="grid gap-2 sm:grid-cols-2">
            <SourceCard label="Microphone" icon={Mic} status={status.microphone} level={levels.microphone} onReconnect={() => reconnect("microphone")} />
            <SourceCard label="Meeting audio" icon={MonitorSpeaker} status={status.system} level={levels.system} onReconnect={() => reconnect("system")} />
          </div>

          <div className="mt-8">
            <h2 className="mb-1 text-[13px] font-semibold">Live transcript</h2>
            <p className="text-xs text-muted-foreground">Speaker names are refined after the meeting.</p>
            <LiveTranscript meetingId={status.meetingId} />
          </div>
        </div>
      </PageBody>

      <div className="pointer-events-none absolute inset-x-0 bottom-6 flex justify-center gap-3">
        <Button size="lg" variant="outline" className="pointer-events-auto rounded-full" disabled={stopping} onClick={() => stop(true)}>
          <Pause className="size-4" /> Pause session
        </Button>
        <Button
          size="lg"
          className="pointer-events-auto h-11 gap-2.5 rounded-full bg-recording px-5 text-white shadow-lg shadow-black/30 hover:bg-recording/90"
          onClick={() => stop()}
          disabled={stopping}
        >
          <Square className="size-3.5 fill-current" aria-hidden />
          {stopping ? "Saving…" : "Stop meeting"}
          <span className="font-mono text-xs tabular-nums opacity-80">{formatClock(elapsed)}</span>
        </Button>
      </div>
    </div>
  );
}
