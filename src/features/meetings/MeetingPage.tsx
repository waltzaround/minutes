import { useQuery, useQueryClient } from "@tanstack/react-query";
import { AlertTriangle, Loader2, MoreHorizontal } from "lucide-react";
import { useState } from "react";
import { useNavigate, useParams, useSearchParams } from "react-router";
import { toast } from "sonner";
import { Button } from "@/components/ui/button";
import { DropdownMenu, DropdownMenuContent, DropdownMenuItem, DropdownMenuSeparator, DropdownMenuTrigger } from "@/components/ui/dropdown-menu";
import { Progress } from "@/components/ui/progress";
import { PageBody, useHeaderInset } from "@/components/app/Page";
import { cn } from "@/lib/utils";
import { meetingDetailApi, meetingsApi } from "@/lib/api";
import { EVENTS, useTauriEvent } from "@/lib/api/events";
import type { ProcessingProgress } from "@/lib/types";
import { formatDuration, relativeDay } from "@/lib/utils/format";
import { TranscriptView } from "@/features/transcript/TranscriptView";
import { NotesView } from "./NotesView";
import { MeetingIntegrations } from "@/features/integrations/MeetingIntegrations";

const stageLabel: Record<string, string> = {
  queued: "Waiting to process…",
  transcribing: "Transcribing…",
  speakers: "Identifying speakers…",
  finishing: "Finishing up…",
};

export function MeetingPage() {
  const { id = "" } = useParams();
  const navigate = useNavigate();
  const qc = useQueryClient();
  const key = ["meeting", id];
  const { data: m, error } = useQuery({ queryKey: key, queryFn: () => meetingDetailApi.get(id) });
  const [progress, setProgress] = useState<ProcessingProgress | null>(null);
  const [sessionBusy, setSessionBusy] = useState(false);
  const [editingTitle, setEditingTitle] = useState(false);
  const inset = useHeaderInset();
  const [params, setParams] = useSearchParams();
  const tab = params.get("tab") ?? "notes";
  const jump = (segmentId: string) => setParams({ tab: "transcript", segment: segmentId });

  useTauriEvent<ProcessingProgress>(EVENTS.processingProgress, (p) => {
    if (p.meetingId !== id) return;
    setProgress(p);
    if (p.stage === "done" || p.stage === "failed") {
      void qc.invalidateQueries({ queryKey: key });
      void qc.invalidateQueries({ queryKey: ["meetings"] });
    }
  });

  if (error) return <div className="p-8 text-destructive">{error.message}</div>;
  if (!m) return null;
  const s = m.summary;

  const rename = async (title: string) => {
    setEditingTitle(false);
    if (title.trim() && title !== s.title) {
      await meetingsApi.rename(id, title).catch((e) => toast.error(e.message));
      void qc.invalidateQueries({ queryKey: key });
      void qc.invalidateQueries({ queryKey: ["meetings"] });
    }
  };
  const reprocess = async () => {
    try {
      await meetingDetailApi.reprocess(id);
      setProgress({ meetingId: id, stage: "queued", fraction: null, message: null });
      void qc.invalidateQueries({ queryKey: key });
    } catch (e) {
      toast.error((e as Error).message);
    }
  };
  const remove = async () => {
    if (!window.confirm(`Delete “${s.title}”? The transcript, notes and audio are removed from this computer.`)) return;
    try {
      await meetingsApi.remove(id);
      void qc.invalidateQueries({ queryKey: ["meetings"] });
      navigate("/", { replace: true });
    } catch (e) {
      toast.error((e as Error).message);
    }
  };

  const processing = s.status === "processing";
  const paused = s.status === "paused";
  const sessionAction = async (resume: boolean) => {
    setSessionBusy(true);
    try {
      if (resume) {
        const status = await meetingsApi.resume(id);
        qc.setQueryData(["recording"], status);
        navigate("/recording");
      } else { await meetingsApi.finishPaused(id); }
      void qc.invalidateQueries({ queryKey: key });
      void qc.invalidateQueries({ queryKey: ["meetings"] });
    } catch (e) { toast.error((e as Error).message); }
    finally { setSessionBusy(false); }
  };
  const meta = [relativeDay(s.startedAt), formatDuration(s.durationMs)];
  if (s.speakerCount > 0) meta.push(`${s.speakerCount} speakers`);

  const tabs = [
    { id: "notes", label: "Notes" },
    { id: "transcript", label: "Transcript" },
  ];

  return (
    <>
      <header data-tauri-drag-region className={cn("flex h-12 shrink-0 items-center gap-2 px-4", inset)}>
        <div className="min-w-0 flex-1 truncate text-[13px] text-muted-foreground" data-tauri-drag-region>
          {s.title}
        </div>
        <div role="tablist" aria-label="Meeting view" className="flex rounded-lg bg-accent/60 p-0.5">
          {tabs.map((t) => (
            <button
              key={t.id}
              role="tab"
              aria-selected={tab === t.id}
              onClick={() => setParams({ tab: t.id })}
              className={cn(
                "h-6.5 rounded-md px-3 text-xs font-medium text-muted-foreground outline-none transition-colors focus-visible:ring-2 focus-visible:ring-ring",
                tab === t.id && "bg-background text-foreground shadow-sm",
              )}
            >
              {t.label}
            </button>
          ))}
        </div>
        <div className="flex flex-1 justify-end">
          <DropdownMenu>
            <DropdownMenuTrigger asChild>
              <Button size="icon" variant="ghost" className="size-7 text-muted-foreground" aria-label="Meeting actions">
                <MoreHorizontal />
              </Button>
            </DropdownMenuTrigger>
            <DropdownMenuContent align="end">
              <DropdownMenuItem onSelect={() => setEditingTitle(true)}>Rename</DropdownMenuItem>
              <DropdownMenuItem disabled={!m.audioAvailable || processing || paused} onSelect={reprocess}>
                Transcribe again
              </DropdownMenuItem>
              <DropdownMenuSeparator />
              <DropdownMenuItem variant="destructive" onSelect={remove}>
                Delete meeting
              </DropdownMenuItem>
            </DropdownMenuContent>
          </DropdownMenu>
        </div>
      </header>

      <PageBody>
        <div className="mx-auto max-w-3xl px-8 pt-6 pb-4">
          {paused && (
            <div className="mb-5 rounded-xl border bg-card p-4">
              <p className="font-medium">Session paused</p>
              <p className="mt-1 text-xs text-muted-foreground">Saved on this computer. You can close Minutes and resume later. Break time is excluded; speakers are identified across the full session when you finish.</p>
              <div className="mt-3 flex gap-2">
                <Button size="sm" disabled={sessionBusy} onClick={() => sessionAction(true)}>Resume recording</Button>
                <Button size="sm" variant="outline" disabled={sessionBusy} onClick={() => sessionAction(false)}>Finish and process</Button>
              </div>
            </div>
          )}
          {editingTitle ? (
            <input
              autoFocus
              defaultValue={s.title}
              aria-label="Meeting title"
              className="selectable w-full rounded-md bg-transparent font-display text-[28px] tracking-tight outline-none ring-1 ring-ring"
              onBlur={(e) => rename(e.currentTarget.value)}
              onKeyDown={(e) => {
                if (e.key === "Enter") e.currentTarget.blur();
                if (e.key === "Escape") setEditingTitle(false);
              }}
            />
          ) : (
            <h1
              className="cursor-text font-display text-[28px] leading-tight tracking-tight"
              onDoubleClick={() => setEditingTitle(true)}
              title="Double-click to rename"
            >
              {s.title}
            </h1>
          )}
          <div className="mt-2 flex flex-wrap items-center gap-1.5 text-xs text-muted-foreground">
            {meta.map((x) => (
              <span key={x} className="rounded-full border px-2 py-0.5">{x}</span>
            ))}
          </div>

          {processing && (
            <div className="mt-5 rounded-xl border bg-card/60 px-4 py-3" role="status">
              <div className="flex items-center gap-2 text-[13px]">
                <Loader2 className="size-3.5 animate-spin" aria-hidden />
                {stageLabel[progress?.stage ?? "queued"] ?? "Processing…"}
              </div>
              {progress?.fraction != null && <Progress value={progress.fraction * 100} className="mt-2.5 h-1" aria-label="Processing progress" />}
            </div>
          )}
          {s.status === "failed" && (
            <div className="mt-5 flex items-start gap-3 rounded-xl border border-destructive/30 bg-destructive/5 px-4 py-3" role="alert">
              <AlertTriangle className="mt-0.5 size-4 shrink-0 text-destructive" aria-hidden />
              <div className="flex-1">
                <div className="font-medium">This meeting couldn’t be processed.</div>
                <p className="mt-0.5 text-muted-foreground">{m.processingError ?? "Something went wrong."} Your recording is safe.</p>
              </div>
              {m.audioAvailable && (
                <Button size="sm" variant="outline" className="h-7" onClick={reprocess}>
                  Transcribe again
                </Button>
              )}
            </div>
          )}
        </div>
        <div className="mx-auto max-w-3xl px-8 pb-10">
          {tab === "transcript" ? <TranscriptView meeting={m} /> : <NotesView meeting={m} onJump={jump} actions={<MeetingIntegrations meeting={m} />} />}
        </div>
      </PageBody>
    </>
  );
}
