import { useQuery, useQueryClient } from "@tanstack/react-query";
import { AlertTriangle, Download, Loader2, Mic, MonitorSpeaker, ShieldCheck, Tag } from "lucide-react";
import { useState, type ReactNode } from "react";
import { Link, Navigate } from "react-router";
import { toast } from "sonner";
import { Button } from "@/components/ui/button";
import { Select, SelectContent, SelectItem, SelectSeparator, SelectTrigger, SelectValue } from "@/components/ui/select";
import { PageBody, PageHeader } from "@/components/app/Page";
import { meetingsApi } from "@/lib/api";
import { useAudioDevices, useCapabilities, useModels, usePatchSettings, useRecordingStatus, useSettings } from "@/lib/api/queries";
import { cn } from "@/lib/utils";
import { useStartMeeting } from "@/app/Sidebar";
import { isMac } from "@/app/sidebar-context";

const DEFAULT = "__default";
const OFF = "__off";

function Row({ icon: Icon, label, hint, htmlFor, children }: { icon: typeof Mic; label: string; hint?: string; htmlFor?: string; children: ReactNode }) {
  return (
    <div className="flex items-center gap-4 border-b px-4 py-3 last:border-0">
      <Icon className="size-4 shrink-0 text-muted-foreground" aria-hidden />
      <div className="w-36 shrink-0">
        <label htmlFor={htmlFor} className="text-[13px] font-medium">{label}</label>
        {hint && <div className="text-[11px] text-muted-foreground">{hint}</div>}
      </div>
      <div className="min-w-0 flex-1">{children}</div>
    </div>
  );
}

function NewMeetingForm() {
  const start = useStartMeeting();
  const { data: settings } = useSettings();
  const { data: devices, refetch } = useAudioDevices();
  const patch = usePatchSettings();
  const [title, setTitle] = useState("");
  const [starting, setStarting] = useState(false);
  if (!settings) return null;

  const inputs = devices?.inputs ?? [];
  const outputs = devices?.outputs ?? [];
  const defaultMic = inputs.find((d) => d.isDefault)?.name;
  const defaultOut = outputs.find((d) => d.isDefault)?.name;
  const systemOn = settings.audio.captureSystemAudio;

  const go = async () => {
    setStarting(true);
    await start(title.trim() || undefined);
    setStarting(false);
  };

  return (
    <form
      onSubmit={(e) => {
        e.preventDefault();
        void go();
      }}
    >
      <div className="rounded-xl border bg-card/60">
        <Row icon={Tag} label="Name" hint="Optional" htmlFor="meeting-title">
          <input
            id="meeting-title"
            value={title}
            onChange={(e) => setTitle(e.target.value)}
            placeholder="Named automatically from the notes"
            autoComplete="off"
            className="selectable h-8 w-full rounded-md border border-input bg-transparent px-2.5 text-[13px] outline-none placeholder:text-muted-foreground/60 focus-visible:ring-2 focus-visible:ring-ring"
          />
        </Row>
        <Row icon={Mic} label="Microphone" hint="Your voice" htmlFor="mic-select">
          <Select
            value={settings.audio.microphoneDeviceId ?? DEFAULT}
            onOpenChange={(o) => o && void refetch()}
            onValueChange={(v) => patch((s) => void (s.audio.microphoneDeviceId = v === DEFAULT ? null : v))}
          >
            <SelectTrigger id="mic-select" className="h-8 w-full">
              <SelectValue />
            </SelectTrigger>
            <SelectContent>
              <SelectItem value={DEFAULT}>System default{defaultMic ? ` (${defaultMic})` : ""}</SelectItem>
              {inputs.map((d) => (
                <SelectItem key={d.id} value={d.id}>{d.name}</SelectItem>
              ))}
            </SelectContent>
          </Select>
        </Row>
        <Row icon={MonitorSpeaker} label="Meeting audio" hint="Other participants" htmlFor="out-select">
          <Select
            value={systemOn ? (settings.audio.outputDeviceId ?? DEFAULT) : OFF}
            onOpenChange={(o) => o && void refetch()}
            onValueChange={(v) =>
              patch((s) => {
                s.audio.captureSystemAudio = v !== OFF;
                if (v !== OFF) s.audio.outputDeviceId = v === DEFAULT ? null : v;
              })
            }
          >
            <SelectTrigger id="out-select" className="h-8 w-full">
              <SelectValue />
            </SelectTrigger>
            <SelectContent>
              <SelectItem value={DEFAULT}>Current output{defaultOut ? ` (${defaultOut})` : ""}</SelectItem>
              {outputs.map((d) => (
                <SelectItem key={d.id} value={d.id}>{d.name}</SelectItem>
              ))}
              <SelectSeparator />
              <SelectItem value={OFF}>Off — in-person meeting, microphone only</SelectItem>
            </SelectContent>
          </Select>
        </Row>
      </div>

      <div className="mt-4 flex items-center gap-3">
        <div className="flex min-w-0 flex-1 flex-col gap-0.5 text-xs text-muted-foreground">
          <span className="flex items-center gap-1.5">
            <ShieldCheck className="size-3.5 shrink-0" aria-hidden /> Recorded and processed on this computer
          </span>
          {settings.general.consentReminder && <span className="pl-5">Let everyone know the meeting is being recorded.</span>}
        </div>
        <Button type="submit" disabled={starting} className="h-9 gap-2 bg-recording px-4 text-white hover:bg-recording/90">
          {starting ? <Loader2 className="size-3.5 animate-spin" /> : <span className="size-2 rounded-full bg-white" aria-hidden />}
          Start recording
          <kbd className="ml-1 font-sans text-[11px] opacity-70">{isMac ? "⇧⌘R" : "Ctrl+Shift+R"}</kbd>
        </Button>
      </div>
    </form>
  );
}

function Readiness() {
  const { data: caps } = useCapabilities();
  const { data: models } = useModels();
  if (!caps || !models) return null;
  const speech = ["silero-vad", "parakeet-tdt-0.6b-v3-int8"];
  const missingSpeech = models.filter((m) => speech.includes(m.manifest.id) && m.state !== "installed");
  const llm = caps.assessment.settings.llmModelId;
  const missingLlm = llm && models.find((m) => m.manifest.id === llm && m.state !== "installed");
  const mic = caps.hardware.audio.microphonePermission;
  const items: { icon: typeof Mic; tone: string; text: string; to: string; cta: string }[] = [];
  if (missingSpeech.length) items.push({ icon: Download, tone: "text-warning", text: "Transcription models aren’t installed yet, so meetings can be recorded but not transcribed.", to: "/settings/models", cta: "Download" });
  else if (missingLlm) items.push({ icon: Download, tone: "text-muted-foreground", text: "The meeting-notes model isn’t installed. Transcripts work; summaries need it.", to: "/settings/models", cta: "Download" });
  if (mic === "denied" || mic === "restricted") items.push({ icon: AlertTriangle, tone: "text-destructive", text: "Microphone access is blocked for Minutes.", to: "/settings/audio", cta: "Fix" });
  if (!items.length) return null;
  return (
    <div className="space-y-2">
      {items.map((i) => (
        <div key={i.text} className="flex items-center gap-3 rounded-xl border bg-card/60 px-3.5 py-2.5 text-[13px]">
          <i.icon className={cn("size-4 shrink-0", i.tone)} aria-hidden />
          <span className="flex-1 text-muted-foreground">{i.text}</span>
          <Button size="sm" variant="outline" className="h-7" asChild>
            <Link to={i.to}>{i.cta}</Link>
          </Button>
        </div>
      ))}
    </div>
  );
}

function RecoveryPrompt() {
  const qc = useQueryClient();
  const { data } = useQuery({ queryKey: ["interrupted"], queryFn: meetingsApi.interrupted });
  if (!data?.length) return null;
  const refresh = () => {
    void qc.invalidateQueries({ queryKey: ["interrupted"] });
    void qc.invalidateQueries({ queryKey: ["meetings"] });
  };
  return (
    <div className="space-y-2">
      {data.map((m) => {
        const mins = Math.max(1, Math.round(m.recoveredMs / 60000));
        return (
          <div key={m.meeting.id} role="alert" className="flex items-start gap-3 rounded-xl border border-warning/30 bg-warning/5 px-3.5 py-3 text-[13px]">
            <AlertTriangle className="mt-0.5 size-4 shrink-0 text-warning" aria-hidden />
            <div className="flex-1">
              <div className="font-medium">We found an unfinished meeting.</div>
              <p className="mt-0.5 text-muted-foreground">
                {m.recoveredMs > 0
                  ? `${mins} minute${mins === 1 ? "" : "s"} of audio were recovered from “${m.meeting.title}”.`
                  : `No audio could be recovered from “${m.meeting.title}”.`}
              </p>
            </div>
            <div className="flex shrink-0 gap-1.5">
              {m.recoveredMs > 0 && (
                <Button size="sm" className="h-7" onClick={() => meetingsApi.recover(m.meeting.id).then(refresh, (e) => toast.error(e.message))}>
                  Recover meeting
                </Button>
              )}
              <Button size="sm" variant="ghost" className="h-7" onClick={() => meetingsApi.remove(m.meeting.id).then(refresh, (e) => toast.error(e.message))}>
                Delete
              </Button>
            </div>
          </div>
        );
      })}
    </div>
  );
}

export function HomePage() {
  const { data: recording } = useRecordingStatus();
  if (recording) return <Navigate to="/recording" replace />;

  return (
    <>
      <PageHeader />
      <PageBody>
        <div className="mx-auto max-w-2xl px-6 pt-6 pb-12">
          <h1 className="font-display text-[26px] tracking-tight">New meeting</h1>
          <p className="mt-1.5 text-muted-foreground">
            Record a call or an in-person meeting. Minutes transcribes it, works out who spoke and drafts the notes.
          </p>
          <div className="mt-5 space-y-2 empty:hidden">
            <RecoveryPrompt />
            <Readiness />
          </div>
          <div className="mt-6">
            <NewMeetingForm />
          </div>
        </div>
      </PageBody>
    </>
  );
}
