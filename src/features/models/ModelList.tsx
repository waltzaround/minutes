import { Download, Loader2, Pause, Play, Trash2, X } from "lucide-react";
import { useState } from "react";
import { toast } from "sonner";
import { Button } from "@/components/ui/button";
import { Progress } from "@/components/ui/progress";
import { modelsApi } from "@/lib/api";
import { useModels } from "@/lib/api/queries";
import type { ModelStatus } from "@/lib/types";
import { formatBytes } from "@/lib/utils/format";

const purposeLabel: Record<string, string> = {
  asr: "Transcription",
  vad: "Speech detection",
  diarization: "Speaker separation",
  embedding: "Voice recognition",
  llm: "Meeting summaries",
};

function run(p: Promise<unknown>) {
  p.catch((e: Error) => toast.error(e.message));
}

function ModelRow({ m, removable }: { m: ModelStatus; removable: boolean }) {
  const id = m.manifest.id;
  // Immediate feedback between clicking and the first progress event.
  const [startedFrom, setStartedFrom] = useState<string | null>(null);
  const starting = startedFrom !== null && startedFrom === m.state;
  const start = () => {
    setStartedFrom(m.state);
    modelsApi.download(id).catch((e: Error) => {
      setStartedFrom(null);
      toast.error(e.message);
    });
  };
  const pct = m.manifest.byteSize ? (m.bytesDownloaded / m.manifest.byteSize) * 100 : 0;
  const busy = m.state === "downloading" || m.state === "verifying";
  return (
    <li className="flex items-center gap-4 py-3">
      <div className="min-w-0 flex-1">
        <div className="flex items-baseline gap-2">
          <span className="font-medium">{m.manifest.name}</span>
          <span className="text-xs text-muted-foreground">{purposeLabel[m.manifest.purpose]}</span>
        </div>
        <div className="mt-0.5 text-xs text-muted-foreground">
          {m.state === "installed" && `Installed · ${formatBytes(m.manifest.byteSize)}`}
          {m.state === "notInstalled" && formatBytes(m.manifest.byteSize)}
          {m.state === "downloading" &&
            `${formatBytes(m.bytesDownloaded)} of ${formatBytes(m.manifest.byteSize)}`}
          {m.state === "verifying" && `Checking downloaded data · ${formatBytes(m.bytesDownloaded)} of ${formatBytes(m.manifest.byteSize)}`}
          {m.state === "paused" && (starting ? "Resuming…" : `Paused · ${formatBytes(m.bytesDownloaded)} of ${formatBytes(m.manifest.byteSize)}`)}
          {m.state === "failed" && <span className="text-destructive">{m.error ?? "Download failed"}</span>}
        </div>
        {(busy || m.state === "paused") && <Progress value={pct} className="mt-2 h-1" aria-label={`${m.manifest.name} download`} />}
      </div>
      <div className="flex shrink-0 gap-1">
        {(m.state === "notInstalled" || m.state === "failed") && (
          <Button size="sm" variant="outline" onClick={start} disabled={starting}>
            {starting ? <Loader2 className="animate-spin" /> : <Download />} {m.state === "failed" ? "Retry" : "Download"}
          </Button>
        )}
        {m.state === "paused" && (
          <Button size="sm" variant="outline" onClick={start} disabled={starting}>
            {starting ? <Loader2 className="animate-spin" /> : <Play />} Resume
          </Button>
        )}
        {m.state === "downloading" && (
          <Button size="icon" variant="ghost" aria-label="Pause download" onClick={() => run(modelsApi.pause(id))}>
            <Pause />
          </Button>
        )}
        {(busy || m.state === "paused" || m.state === "failed") && (
          <Button size="icon" variant="ghost" aria-label="Cancel download" onClick={() => run(modelsApi.cancel(id))}>
            <X />
          </Button>
        )}
        {m.state === "installed" && removable && (
          <Button size="icon" variant="ghost" aria-label={`Remove ${m.manifest.name}`} onClick={() => run(modelsApi.remove(id))}>
            <Trash2 />
          </Button>
        )}
      </div>
    </li>
  );
}

/** Model list with explicit per-model actions. Nothing downloads silently. */
export function ModelList({ ids, exclude, removable = false }: { ids?: string[]; exclude?: string[]; removable?: boolean }) {
  const { data, error } = useModels();
  if (error) return <p className="text-destructive">{error.message}</p>;
  if (!data) return null;
  const rows = (ids ? data.filter((m) => ids.includes(m.manifest.id)) : data).filter(
    (m) => !exclude?.includes(m.manifest.id),
  );
  return (
    <ul className="divide-y">
      {rows.map((m) => (
        <ModelRow key={m.manifest.id} m={m} removable={removable} />
      ))}
    </ul>
  );
}

export function useRequiredModelsState(ids: string[]) {
  const { data } = useModels();
  const rows = data?.filter((m) => ids.includes(m.manifest.id)) ?? [];
  const total = rows.reduce((s, m) => s + m.manifest.byteSize, 0);
  const remaining = rows.filter((m) => m.state !== "installed").reduce((s, m) => s + m.manifest.byteSize - m.bytesDownloaded, 0);
  return {
    rows,
    total,
    remaining,
    allInstalled: rows.length === ids.length && rows.every((m) => m.state === "installed"),
    anyActive: rows.some((m) => m.state === "downloading" || m.state === "verifying"),
  };
}
