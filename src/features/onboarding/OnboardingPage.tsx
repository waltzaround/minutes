import { useQueryClient } from "@tanstack/react-query";
import { ArrowLeft, Check, Loader2, Lock, Mic, MonitorSpeaker } from "lucide-react";
import { useEffect, useRef, useState, type ReactNode } from "react";
import { useNavigate } from "react-router";
import { toast } from "sonner";
import { Button } from "@/components/ui/button";
import { Label } from "@/components/ui/label";
import { Select, SelectContent, SelectItem, SelectTrigger, SelectValue } from "@/components/ui/select";
import { Switch } from "@/components/ui/switch";
import { api, modelsApi, peopleApi } from "@/lib/api";
import { Input } from "@/components/ui/input";
import { VoiceEnrollmentDialog } from "@/features/people/VoiceEnrollmentDialog";
import { keys, useAudioDevices, useCapabilities, usePatchSettings, usePeople, useSettings } from "@/lib/api/queries";
import type { PermissionState, SystemCapabilities } from "@/lib/types";
import { cn } from "@/lib/utils";
import { formatBytes } from "@/lib/utils/format";
import { ModelList, useRequiredModelsState } from "@/features/models/ModelList";
import { CapabilityResult } from "./CapabilityResult";

const STEPS = ["welcome", "check", "result", "permissions", "microphone", "models", "you", "ready"] as const;
type Step = (typeof STEPS)[number];

function StepFrame({
  step,
  children,
  footer,
  onBack,
}: {
  step: Step;
  children: ReactNode;
  footer?: ReactNode;
  onBack?: () => void;
}) {
  const index = STEPS.indexOf(step);
  return (
    <div className="flex h-full flex-col">
      <div data-tauri-drag-region className="flex h-12 shrink-0 items-center justify-center">
        <ol className="flex gap-1.5" aria-label={`Step ${index + 1} of ${STEPS.length}`}>
          {STEPS.map((s, i) => (
            <li
              key={s}
              className={cn("h-1 w-6 rounded-full", i <= index ? "bg-foreground/70" : "bg-foreground/15")}
            />
          ))}
        </ol>
      </div>
      <div className="min-h-0 flex-1 overflow-y-auto">
        <div className="mx-auto max-w-xl px-8 pt-10 pb-8">{children}</div>
      </div>
      <div className="shrink-0 border-t">
        <div className="mx-auto flex max-w-xl items-center gap-2 px-8 py-4">
          {onBack && (
            <Button variant="ghost" onClick={onBack}>
              <ArrowLeft /> Back
            </Button>
          )}
          <div className="ml-auto flex gap-2">{footer}</div>
        </div>
      </div>
    </div>
  );
}

function Welcome({ next }: { next: () => void }) {
  return (
    <StepFrame step="welcome" footer={<Button onClick={next} autoFocus>Check my computer</Button>}>
      <h1 className="font-display text-[34px] leading-tight tracking-tight"><span className="mr-2 text-brand" aria-hidden>✻</span>Welcome to Minutes</h1>
      <p className="mt-3 text-[15px] leading-relaxed text-muted-foreground">
        Minutes records your meetings, writes the transcript, recognises who spoke and drafts notes and action items.
      </p>
      <ul className="mt-8 space-y-4">
        {[
          ["Everything stays on this computer", "Audio, transcripts and AI summaries are processed locally. Nothing is sent to an AI service."],
          ["No subscriptions", "The speech and language models run on your own hardware."],
          ["You stay in control", "Notion and Linear are optional, and nothing is sent there until you review it."],
        ].map(([title, body]) => (
          <li key={title} className="flex gap-3">
            <Lock className="mt-0.5 size-4 shrink-0 text-muted-foreground" aria-hidden />
            <div>
              <div className="font-medium">{title}</div>
              <div className="text-muted-foreground">{body}</div>
            </div>
          </li>
        ))}
      </ul>
      <p className="mt-8 text-muted-foreground">
        First, we’ll check what this computer can run. This takes a few seconds and downloads nothing.
      </p>
    </StepFrame>
  );
}

const CHECK_ITEMS = [
  "Processor and memory",
  "Graphics acceleration",
  "Disk space",
  "Microphone and meeting audio",
  "Quick performance test",
];

function CheckComputer({ onDone, onBack }: { onDone: (caps: SystemCapabilities) => void; onBack: () => void }) {
  const qc = useQueryClient();
  const [phase, setPhase] = useState(0);
  const [error, setError] = useState<string | null>(null);
  const onDoneRef = useRef(onDone);
  onDoneRef.current = onDone;

  useEffect(() => {
    let cancelled = false;
    (async () => {
      try {
        setPhase(0);
        await api.system.capabilities(true);
        if (cancelled) return;
        setPhase(4);
        await api.system.runBenchmark("synthetic");
        const caps = await api.system.capabilities(false);
        if (cancelled) return;
        qc.setQueryData(keys.capabilities, caps);
        setPhase(5);
        onDoneRef.current(caps);
      } catch (e) {
        if (!cancelled) setError((e as Error).message);
      }
    })();
    return () => {
      cancelled = true;
    };
  }, [qc]);

  return (
    <StepFrame step="check" onBack={onBack}>
      <h1 className="font-display text-[28px] leading-tight tracking-tight">Checking your computer</h1>
      <p className="mt-2 text-muted-foreground">This runs entirely on this computer.</p>
      <ul className="mt-8 space-y-3" aria-live="polite">
        {CHECK_ITEMS.map((label, i) => {
          // Phase 0: hardware detection (items 0-3); phase 4: benchmark; 5: done.
          const done = phase === 5 || (phase === 4 && i < 4);
          const active = !done && (phase === 0 ? i < 4 : i === 4);
          return (
            <li key={label} className="flex items-center gap-3">
              {done ? (
                <Check className="size-4 text-success" aria-hidden />
              ) : active ? (
                <Loader2 className="size-4 animate-spin text-muted-foreground" aria-hidden />
              ) : (
                <span className="size-4" />
              )}
              <span className={cn(!done && !active && "text-muted-foreground")}>{label}</span>
            </li>
          );
        })}
      </ul>
      {error && <p className="mt-6 text-destructive">{error}</p>}
    </StepFrame>
  );
}

function Result({ caps, next, back, recheck }: { caps: SystemCapabilities; next: () => void; back: () => void; recheck: () => void }) {
  const models = useRequiredModelsState(caps.assessment.requiredModels);
  return (
    <StepFrame
      step="result"
      onBack={back}
      footer={
        caps.assessment.canContinue ? (
          <Button onClick={next} autoFocus>
            Continue
          </Button>
        ) : (
          <Button variant="outline" onClick={recheck}>
            Check again
          </Button>
        )
      }
    >
      <CapabilityResult caps={caps} downloadBytes={models.total || undefined} />
    </StepFrame>
  );
}

function permissionText(p: PermissionState): string {
  switch (p) {
    case "granted":
      return "Allowed";
    case "denied":
      return "Blocked";
    case "restricted":
      return "Blocked by your organisation";
    case "notDetermined":
      return "Not asked yet";
    case "notRequired":
      return "No permission needed";
    case "unknown":
      return "Checked when you first record";
  }
}

function Permissions({ caps, next, back }: { caps: SystemCapabilities; next: () => void; back: () => void }) {
  const qc = useQueryClient();
  const [mic, setMic] = useState<PermissionState>(caps.hardware.audio.microphonePermission);
  const isMac = caps.hardware.os === "macos";
  const sysAudio = caps.hardware.audio;

  const requestMic = async () => {
    try {
      const result = await api.system.requestMicrophone();
      setMic(result);
      void qc.invalidateQueries({ queryKey: keys.capabilities });
    } catch (e) {
      toast.error((e as Error).message);
    }
  };

  return (
    <StepFrame step="permissions" onBack={back} footer={<Button onClick={next}>Continue</Button>}>
      <h1 className="font-display text-[28px] leading-tight tracking-tight">Audio permissions</h1>
      <p className="mt-2 text-muted-foreground">
        Minutes records two things separately: your microphone, and the meeting audio coming out of your computer.
      </p>

      <div className="mt-8 space-y-6">
        <div className="flex gap-3">
          <Mic className="mt-0.5 size-4 shrink-0" aria-hidden />
          <div className="flex-1">
            <div className="flex items-center justify-between gap-4">
              <div className="font-medium">Microphone</div>
              <div className={cn("text-xs", mic === "granted" ? "text-success" : mic === "denied" || mic === "restricted" ? "text-destructive" : "text-muted-foreground")}>
                {permissionText(mic)}
              </div>
            </div>
            <p className="mt-1 text-muted-foreground">Used to record your own voice.</p>
            <div className="mt-3 flex gap-2">
              {mic === "notDetermined" && (
                <Button size="sm" onClick={requestMic}>
                  Allow microphone
                </Button>
              )}
              {(mic === "denied" || mic === "restricted" || (!isMac && mic === "unknown")) && (
                <Button size="sm" variant="outline" onClick={() => api.system.openPrivacySettings("microphone").catch((e) => toast.error(e.message))}>
                  Open {isMac ? "System Settings" : "privacy settings"}
                </Button>
              )}
            </div>
            {mic === "denied" && isMac && (
              <p className="mt-2 text-xs text-muted-foreground">
                Turn on Minutes under Privacy &amp; Security → Microphone, then return here.
              </p>
            )}
          </div>
        </div>

        <div className="flex gap-3">
          <MonitorSpeaker className="mt-0.5 size-4 shrink-0" aria-hidden />
          <div className="flex-1">
            <div className="flex items-center justify-between gap-4">
              <div className="font-medium">Meeting audio</div>
              <div className="text-xs text-muted-foreground">
                {sysAudio.systemAudioAvailable ? permissionText(sysAudio.systemAudioPermission) : "Not available"}
              </div>
            </div>
            {!sysAudio.systemAudioAvailable ? (
              <p className="mt-1 text-warning">{sysAudio.systemAudioUnavailableReason}</p>
            ) : isMac ? (
              <>
                <p className="mt-1 text-muted-foreground">
                  Captures other people in Zoom, Teams or Meet. macOS asks for “System Audio Recording” permission the
                  first time you record, and Minutes checks that audio is really arriving.
                </p>
                <Button size="sm" variant="outline" className="mt-3" onClick={() => api.system.openPrivacySettings("systemAudio").catch((e) => toast.error(e.message))}>
                  Open System Settings
                </Button>
              </>
            ) : (
              <p className="mt-1 text-muted-foreground">
                Captures other people in Zoom, Teams or Meet from your speakers or headset. Windows needs no extra
                permission and no virtual audio cable.
              </p>
            )}
          </div>
        </div>
      </div>
    </StepFrame>
  );
}

function Microphone({ next, back }: { next: () => void; back: () => void }) {
  const { data: devices, refetch } = useAudioDevices();
  const { data: settings } = useSettings();
  const patch = usePatchSettings();
  if (!settings) return null;
  const inputs = devices?.inputs ?? [];
  const outputs = devices?.outputs ?? [];
  const DEFAULT = "__default";
  return (
    <StepFrame step="microphone" onBack={back} footer={<Button onClick={next}>Continue</Button>}>
      <h1 className="font-display text-[28px] leading-tight tracking-tight">Choose your audio devices</h1>
      <p className="mt-2 text-muted-foreground">You can change these any time in Settings → Audio.</p>
      <div className="mt-8 space-y-6">
        <div className="space-y-2">
          <Label htmlFor="mic">Microphone</Label>
          <Select
            value={settings.audio.microphoneDeviceId ?? DEFAULT}
            onValueChange={(v) => patch((s) => void (s.audio.microphoneDeviceId = v === DEFAULT ? null : v))}
            onOpenChange={(o) => o && void refetch()}
          >
            <SelectTrigger id="mic" className="w-full">
              <SelectValue />
            </SelectTrigger>
            <SelectContent>
              <SelectItem value={DEFAULT}>System default{inputs.find((d) => d.isDefault) ? ` (${inputs.find((d) => d.isDefault)!.name})` : ""}</SelectItem>
              {inputs.map((d) => (
                <SelectItem key={d.id} value={d.id}>
                  {d.name}
                </SelectItem>
              ))}
            </SelectContent>
          </Select>
          {inputs.length === 0 && <p className="text-warning">No microphone was found. Connect one and it will appear here.</p>}
        </div>

        <div className="flex items-start justify-between gap-6">
          <div>
            <Label htmlFor="capture-system">Record meeting audio</Label>
            <p className="mt-1 text-muted-foreground">Capture other participants from your speakers or headset.</p>
          </div>
          <Switch
            id="capture-system"
            checked={settings.audio.captureSystemAudio}
            onCheckedChange={(v) => patch((s) => void (s.audio.captureSystemAudio = v))}
          />
        </div>

        {settings.audio.captureSystemAudio && (
          <div className="space-y-2">
            <Label htmlFor="out">Meeting audio from</Label>
            <Select
              value={settings.audio.outputDeviceId ?? DEFAULT}
              onValueChange={(v) => patch((s) => void (s.audio.outputDeviceId = v === DEFAULT ? null : v))}
              onOpenChange={(o) => o && void refetch()}
            >
              <SelectTrigger id="out" className="w-full">
                <SelectValue />
              </SelectTrigger>
              <SelectContent>
                <SelectItem value={DEFAULT}>Current output{outputs.find((d) => d.isDefault) ? ` (${outputs.find((d) => d.isDefault)!.name})` : ""}</SelectItem>
                {outputs.map((d) => (
                  <SelectItem key={d.id} value={d.id}>
                    {d.name}
                  </SelectItem>
                ))}
              </SelectContent>
            </Select>
          </div>
        )}
      </div>
    </StepFrame>
  );
}

const SPEECH_IDS = ["silero-vad", "parakeet-tdt-0.6b-v3-int8"];

function Models({ caps, next, back }: { caps: SystemCapabilities; next: () => void; back: () => void }) {
  const qc = useQueryClient();
  const ids = caps.assessment.requiredModels;
  const state = useRequiredModelsState(ids);
  const speech = useRequiredModelsState(SPEECH_IDS);
  const free = caps.hardware.disk.availableBytes;
  const llmId = caps.assessment.settings.llmModelId;
  const llmInstalled = useRequiredModelsState(llmId ? [llmId] : []).allInstalled && !!llmId;
  const [benchmark, setBenchmark] = useState<"idle" | "asr" | "llm" | "done" | "failed">("idle");
  const needAsr = speech.allInstalled && !caps.benchmarks.asr;
  const needLlm = llmInstalled && !caps.benchmarks.llm.some((b) => b.modelId === llmId);

  // Once models are in place, measure real performance on this computer
  // (local only, nothing is downloaded) and refresh the recommendation.
  useEffect(() => {
    if (benchmark === "asr" || benchmark === "llm") return;
    const stage = needAsr ? "asr" : needLlm ? "llm" : null;
    if (!stage) return;
    setBenchmark(stage);
    api.system
      .runBenchmark(stage)
      .then(() => api.system.capabilities(false))
      .then((c) => {
        qc.setQueryData(keys.capabilities, c);
        setBenchmark("done");
      })
      .catch(() => setBenchmark("failed"));
  }, [needAsr, needLlm, benchmark, qc]);

  const downloadAll = async () => {
    for (const m of state.rows) {
      if (m.state === "installed" || m.state === "downloading" || m.state === "verifying") continue;
      try {
        await modelsApi.download(m.manifest.id);
      } catch (e) {
        toast.error((e as Error).message);
        return;
      }
    }
  };

  return (
    <StepFrame
      step="models"
      onBack={back}
      footer={
        <>
          {!state.allInstalled && (
            <Button variant="ghost" onClick={next}>
              {state.anyActive ? "Continue while downloading" : "Skip for now"}
            </Button>
          )}
          {state.allInstalled ? (
            <Button onClick={next}>Continue</Button>
          ) : (
            <Button onClick={downloadAll} disabled={state.anyActive && state.rows.every((m) => m.state !== "notInstalled" && m.state !== "failed" && m.state !== "paused")}>
              Download models
            </Button>
          )}
        </>
      }
    >
      <h1 className="font-display text-[28px] leading-tight tracking-tight">Download the AI models</h1>
      <p className="mt-2 text-muted-foreground">
        These run on your computer. They are downloaded once, checked for integrity, and never updated without asking.
      </p>
      <div className="mt-4 flex gap-6 text-xs text-muted-foreground">
        <span>Total {formatBytes(state.total)}</span>
        <span>{formatBytes(free)} free on this disk</span>
      </div>
      <div className="mt-4">
        <ModelList ids={ids} />
      </div>
      {(benchmark === "asr" || benchmark === "llm") && (
        <p className="mt-4 flex items-center gap-2 text-muted-foreground" role="status">
          <Loader2 className="size-3.5 animate-spin" aria-hidden />
          {benchmark === "asr" ? "Testing transcription speed on this computer…" : "Testing meeting summaries on this computer…"}
        </p>
      )}
      {caps.assessment.adjustments.map((adj) => (
        <p key={adj.reason} className="mt-2 text-muted-foreground">{adj.reason}</p>
      ))}
      {caps.benchmarks.asr && (
        <p className="mt-4 text-muted-foreground">
          Transcription runs {Math.max(1, Math.round(1 / Math.max(caps.benchmarks.asr.rtf, 0.001)))}× faster than real time on this computer.
        </p>
      )}
      {!state.allInstalled && (
        <p className="mt-4 text-xs text-muted-foreground">
          Until the models are installed, Minutes can’t transcribe or summarise. You can download them later in
          Settings → Models.
        </p>
      )}
    </StepFrame>
  );
}

function You({ next, back }: { next: () => void; back: () => void }) {
  const qc = useQueryClient();
  const { data: people = [] } = usePeople();
  const me = people.find((p) => p.isSelf);
  const [name, setName] = useState("");
  const [enrolling, setEnrolling] = useState(false);
  const save = async () => {
    try {
      await peopleApi.create({ displayName: name, email: null, isSelf: true });
      void qc.invalidateQueries({ queryKey: ["people"] });
    } catch (e) {
      toast.error((e as Error).message);
    }
  };
  return (
    <StepFrame step="you" onBack={back} footer={<Button onClick={next}>{me ? "Continue" : "Skip"}</Button>}>
      <h1 className="font-display text-[28px] leading-tight tracking-tight">A few optional extras</h1>
      <div className="mt-8 space-y-8">
        <div>
          <div className="font-medium">Your name</div>
          <p className="mt-1 text-muted-foreground">Your microphone is labelled with your name in transcripts.</p>
          {me ? (
            <p className="mt-3 flex items-center gap-2"><Check className="size-4 text-success" aria-hidden /> {me.displayName}</p>
          ) : (
            <form className="mt-3 flex gap-2" onSubmit={(e) => { e.preventDefault(); if (name.trim()) void save(); }}>
              <Input placeholder="Your name" value={name} onChange={(e) => setName(e.target.value)} aria-label="Your name" />
              <Button type="submit" variant="outline" disabled={!name.trim()}>Save</Button>
            </form>
          )}
        </div>
        <div>
          <div className="font-medium">Voice profile</div>
          <p className="mt-1 text-muted-foreground">
            Optional. Helps Minutes recognise you when you’re on a shared room microphone. Colleagues can set theirs up later
            under People.
          </p>
          <Button className="mt-3" size="sm" variant="outline" disabled={!me} onClick={() => setEnrolling(true)}>
            {me?.voiceProfile ? "Re-record voice" : "Set up my voice"}
          </Button>
        </div>
        <div>
          <div className="font-medium">Notion and Linear</div>
          <p className="mt-1 text-muted-foreground">
            Optional. Connect them any time in Settings to send notes to Notion or turn reviewed actions into Linear issues.
            Everything works offline without them.
          </p>
        </div>
      </div>
      {me && enrolling && <VoiceEnrollmentDialog person={me} open onOpenChange={setEnrolling} />}
    </StepFrame>
  );
}

function Ready({ back }: { back: () => void }) {
  const navigate = useNavigate();
  const qc = useQueryClient();
  const finish = async () => {
    try {
      const s = await api.app.completeOnboarding();
      qc.setQueryData(keys.settings, s);
      navigate("/", { replace: true });
    } catch (e) {
      toast.error((e as Error).message);
    }
  };
  return (
    <StepFrame step="ready" onBack={back} footer={<Button onClick={finish} autoFocus>Start using Minutes</Button>}>
      <h1 className="font-display text-[28px] leading-tight tracking-tight">You’re set up</h1>
      <p className="mt-2 text-muted-foreground">
        Start a meeting from the home screen. A clear recording indicator is shown whenever Minutes is listening.
      </p>
      <p className="mt-4 text-muted-foreground">
        You can change anything later in Settings.
      </p>
    </StepFrame>
  );
}

export function OnboardingPage() {
  const [step, setStep] = useState<Step>("welcome");
  const [caps, setCaps] = useState<SystemCapabilities | null>(null);
  const cached = useCapabilities();
  const go = (s: Step) => setStep(s);
  const current = caps ?? cached.data ?? null;

  switch (step) {
    case "welcome":
      return <Welcome next={() => go("check")} />;
    case "check":
      return (
        <CheckComputer
          onBack={() => go("welcome")}
          onDone={(c) => {
            setCaps(c);
            go("result");
          }}
        />
      );
    case "result":
      return current ? <Result caps={current} back={() => go("welcome")} next={() => go("permissions")} recheck={() => go("check")} /> : null;
    case "permissions":
      return current ? <Permissions caps={current} back={() => go("result")} next={() => go("microphone")} /> : null;
    case "microphone":
      return <Microphone back={() => go("permissions")} next={() => go("models")} />;
    case "models":
      return current ? <Models caps={cached.data ?? current} back={() => go("microphone")} next={() => go("you")} /> : null;
    case "you":
      return <You back={() => go("models")} next={() => go("ready")} />;
    case "ready":
      return <Ready back={() => go("you")} />;
  }
}
