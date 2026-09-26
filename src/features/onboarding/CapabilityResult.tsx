import { AlertTriangle, CheckCircle2, Info } from "lucide-react";
import type { SystemCapabilities } from "@/lib/types";
import { formatBytes } from "@/lib/utils/format";
import { cn } from "@/lib/utils";
import { headlineTitle, modelNames, ratingLabel, ratingTone } from "./labels";

export function headlineBody(caps: SystemCapabilities): string {
  const a = caps.assessment;
  const memGb = Math.round(caps.hardware.memory.totalBytes / 1024 ** 3);
  switch (a.headline) {
    case "ready":
      return "Everything runs on this computer, including high-quality meeting summaries.";
    case "compatible":
      return "Recording, transcription, speaker recognition and summaries all run on this computer.";
    case "limited":
      if (memGb < 15) {
        return `Your computer has ${memGb} GB of memory. Recording and transcription may work, but local AI summaries can be slower and other applications may affect reliability. 16 GB or more is recommended.`;
      }
      return "Your computer can record and transcribe meetings, but larger local AI models are not recommended. Meeting summaries will use the lightweight model.";
    case "notSupported":
      return "Transcription could not keep up on this computer, so meetings can't be processed reliably.";
  }
}

export function CapabilityResult({ caps, downloadBytes }: { caps: SystemCapabilities; downloadBytes?: number }) {
  const a = caps.assessment;
  const r = a.ratings;
  const rows: [string, typeof r.transcription][] = [
    ["Transcription", r.transcription],
    ["Speaker recognition", r.speakerRecognition],
    ["Meeting summaries", r.meetingSummaries],
  ];
  const memoryInHeadline = a.headline === "limited" && caps.hardware.memory.totalBytes < 15 * 1024 ** 3;
  // The memory warning is already the headline body on low-memory machines.
  const warnings = a.requirements.checks.filter((c) => c.status !== "pass" && !(memoryInHeadline && c.id === "memory"));
  const Icon = a.headline === "ready" || a.headline === "compatible" ? CheckCircle2 : AlertTriangle;
  return (
    <div className="space-y-6">
      <div className="flex items-start gap-3">
        <Icon
          aria-hidden
          className={cn("mt-0.5 size-5 shrink-0", a.headline === "ready" || a.headline === "compatible" ? "text-success" : "text-warning")}
        />
        <div>
          <h2 className="font-display text-[26px] leading-tight tracking-tight">{headlineTitle[a.headline]}</h2>
          <p className="mt-1 max-w-prose text-muted-foreground">{headlineBody(caps)}</p>
        </div>
      </div>

      <table className="w-full max-w-md overflow-hidden rounded-xl border bg-card/60">
        <tbody>
          {rows.map(([label, rating]) => (
            <tr key={label} className="border-b last:border-0 [&>td]:px-4">
              <td className="py-2 text-muted-foreground">{label}</td>
              <td className={cn("py-2 text-right font-medium", ratingTone[rating])}>{ratingLabel[rating]}</td>
            </tr>
          ))}
        </tbody>
      </table>
      {r.estimated && (
        <p className="flex items-center gap-1.5 text-xs text-muted-foreground">
          <Info className="size-3.5" aria-hidden />
          Estimated from your hardware. Ratings are confirmed after the models are installed.
        </p>
      )}

      <div className="grid max-w-md grid-cols-2 gap-4">
        <div>
          <div className="text-xs text-muted-foreground">Recommended model</div>
          <div className="mt-0.5 font-medium">
            {a.settings.llmModelId ? (modelNames[a.settings.llmModelId] ?? a.settings.llmModelId) : "None (transcripts only)"}
          </div>
        </div>
        {downloadBytes != null && (
          <div>
            <div className="text-xs text-muted-foreground">Model download</div>
            <div className="mt-0.5 font-medium">~{formatBytes(downloadBytes)}</div>
          </div>
        )}
      </div>

      {(warnings.length > 0 || a.notes.length > 0) && (
        <ul className="max-w-prose space-y-2">
          {warnings.map((c) => (
            <li key={c.id} className="flex gap-2">
              <AlertTriangle
                aria-hidden
                className={cn("mt-0.5 size-4 shrink-0", c.status === "fail" ? "text-destructive" : "text-warning")}
              />
              <span>
                <span className="font-medium">{c.label}.</span> <span className="text-muted-foreground">{c.detail}</span>
              </span>
            </li>
          ))}
          {a.notes.map((n) => (
            <li key={n} className="flex gap-2">
              <Info aria-hidden className="mt-0.5 size-4 shrink-0 text-muted-foreground" />
              <span className="text-muted-foreground">{n}</span>
            </li>
          ))}
        </ul>
      )}
    </div>
  );
}
