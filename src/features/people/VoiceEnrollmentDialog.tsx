import { useQueryClient } from "@tanstack/react-query";
import { CheckCircle2, Mic } from "lucide-react";
import { useEffect, useState } from "react";
import { toast } from "sonner";
import { Button } from "@/components/ui/button";
import { Checkbox } from "@/components/ui/checkbox";
import { Dialog, DialogContent, DialogDescription, DialogFooter, DialogHeader, DialogTitle } from "@/components/ui/dialog";
import { Label } from "@/components/ui/label";
import { peopleApi } from "@/lib/api";
import { EVENTS, useTauriEvent } from "@/lib/api/events";
import type { AudioLevel, EnrollmentResult, Person } from "@/lib/types";
import { formatClock } from "@/lib/utils/format";
import { LevelMeter } from "@/features/recording/LevelMeter";

const PROMPTS = [
  "Describe what you worked on this week.",
  "Explain how you'd get from the office to your favourite lunch spot.",
  "Talk about a project you're looking forward to.",
  "Describe your morning routine.",
];

type Phase = "intro" | "recording" | "analysing" | "done";

export function VoiceEnrollmentDialog({ person, open, onOpenChange }: { person: Person; open: boolean; onOpenChange: (o: boolean) => void }) {
  const qc = useQueryClient();
  const [phase, setPhase] = useState<Phase>("intro");
  const [level, setLevel] = useState(0);
  const [started, setStarted] = useState(0);
  const [now, setNow] = useState(0);
  const [keep, setKeep] = useState(false);
  const [result, setResult] = useState<EnrollmentResult | null>(null);

  useTauriEvent<AudioLevel>(EVENTS.audioLevel, (l) => phase === "recording" && setLevel(l.level));
  useEffect(() => {
    if (phase !== "recording") return;
    const t = setInterval(() => setNow(Date.now()), 250);
    return () => clearInterval(t);
  }, [phase]);
  useEffect(() => {
    if (!open) {
      if (phase === "recording") void peopleApi.cancelEnrollment();
      setPhase("intro");
      setResult(null);
    }
  }, [open]); // eslint-disable-line react-hooks/exhaustive-deps

  const elapsed = phase === "recording" ? now - started : 0;
  const start = async () => {
    try {
      await peopleApi.startEnrollment(person.id);
      setStarted(Date.now());
      setNow(Date.now());
      setPhase("recording");
    } catch (e) {
      toast.error((e as Error).message);
    }
  };
  const finish = async () => {
    setPhase("analysing");
    try {
      const r = await peopleApi.finishEnrollment(keep);
      setResult(r);
      setPhase("done");
      void qc.invalidateQueries({ queryKey: ["people"] });
    } catch (e) {
      toast.error((e as Error).message);
      setPhase("intro");
    }
  };

  const prompt = PROMPTS[Math.floor(elapsed / 15000) % PROMPTS.length];

  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogContent className="sm:max-w-md">
        <DialogHeader>
          <DialogTitle>Set up voice recognition for {person.displayName}</DialogTitle>
          <DialogDescription>
            Record about 30–60 seconds of natural speech. Minutes stores a voiceprint on this computer, encrypted. This
            doesn’t train a model, and the recording is deleted afterwards.
          </DialogDescription>
        </DialogHeader>

        {phase === "intro" && (
          <div className="space-y-4">
            <ul className="list-disc space-y-1 pl-5 text-muted-foreground">
              <li>Use the microphone you normally use in meetings.</li>
              <li>Speak naturally. Only {person.displayName} should talk.</li>
              <li>Several short answers work better than one long sentence.</li>
            </ul>
            <div className="flex items-center gap-2">
              <Checkbox id="keep" checked={keep} onCheckedChange={(v) => setKeep(v === true)} />
              <Label htmlFor="keep" className="font-normal text-muted-foreground">Keep the enrollment recording on this computer</Label>
            </div>
          </div>
        )}

        {phase === "recording" && (
          <div className="space-y-4 py-2">
            <div className="flex items-center gap-3">
              <Mic className="size-4 text-recording" aria-hidden />
              <span className="font-mono tabular-nums">{formatClock(elapsed)}</span>
              <div className="ml-auto">
                <LevelMeter level={level} />
              </div>
            </div>
            <div className="rounded-md bg-muted p-3">
              <div className="text-xs text-muted-foreground">Try talking about:</div>
              <div className="mt-1 text-[15px]">{prompt}</div>
            </div>
            <p className="text-xs text-muted-foreground" aria-live="polite">
              {elapsed < 30000 ? `Keep going, about ${Math.ceil((30000 - elapsed) / 1000)} more seconds.` : "That’s enough. You can finish now, or keep going up to a minute."}
            </p>
          </div>
        )}

        {phase === "analysing" && <p className="py-4 text-muted-foreground">Creating the voiceprint…</p>}

        {phase === "done" && result && (
          <div className="py-2">
            {result.accepted ? (
              <div className="flex gap-3">
                <CheckCircle2 className="mt-0.5 size-5 text-success" aria-hidden />
                <div>
                  <div className="font-medium">Voice profile saved</div>
                  <p className="text-muted-foreground">
                    {Math.round(result.speechSeconds)} seconds of speech, {result.samples} samples. {person.displayName} will
                    be suggested in future meetings when their voice is recognised.
                  </p>
                </div>
              </div>
            ) : (
              <p className="text-warning">{result.message}</p>
            )}
          </div>
        )}

        <DialogFooter>
          {phase === "intro" && <Button onClick={start}>Start recording</Button>}
          {phase === "recording" && (
            <>
              <Button variant="ghost" onClick={() => onOpenChange(false)}>Cancel</Button>
              <Button onClick={finish} disabled={elapsed < 20000}>Finish</Button>
            </>
          )}
          {phase === "done" && (
            <>
              {!result?.accepted && <Button variant="outline" onClick={() => setPhase("intro")}>Try again</Button>}
              <Button onClick={() => onOpenChange(false)}>Done</Button>
            </>
          )}
        </DialogFooter>
      </DialogContent>
    </Dialog>
  );
}
