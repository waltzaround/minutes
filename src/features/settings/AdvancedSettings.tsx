import { useQuery, useQueryClient } from "@tanstack/react-query";
import { RefreshCw } from "lucide-react";
import { useState } from "react";
import { toast } from "sonner";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { Select, SelectContent, SelectItem, SelectTrigger, SelectValue } from "@/components/ui/select";
import { KeyValue, Section } from "@/components/app/Page";
import { api, modelsApi } from "@/lib/api";
import { invoke } from "@/lib/api/invoke";
import { save } from "@tauri-apps/plugin-dialog";
import { keys, useAppInfo, useCapabilities, usePatchSettings, useSettings } from "@/lib/api/queries";
import type { AdvancedSettings as Adv, InferenceBackendPreference } from "@/lib/types";
import { formatBytes } from "@/lib/utils/format";
import { Field } from "./Field";
import { profileLabel } from "@/features/onboarding/labels";

function NumberField({ label, help, value, onCommit, step = 1, placeholder }: {
  label: string;
  help?: string;
  value: number | null;
  onCommit: (v: number | null) => void;
  step?: number;
  placeholder?: string;
}) {
  const [draft, setDraft] = useState(value?.toString() ?? "");
  const id = label.toLowerCase().replace(/\W+/g, "-");
  return (
    <Field label={label} help={help} htmlFor={id}>
      <Input
        id={id}
        className="w-28 text-right"
        inputMode="decimal"
        step={step}
        placeholder={placeholder ?? "Auto"}
        value={draft}
        onChange={(e) => setDraft(e.target.value)}
        onBlur={() => {
          const t = draft.trim();
          if (t === "") return onCommit(null);
          const n = Number(t);
          if (Number.isFinite(n)) onCommit(n);
          else setDraft(value?.toString() ?? "");
        }}
      />
    </Field>
  );
}

function SystemCheck() {
  const qc = useQueryClient();
  const { data: caps, isFetching } = useCapabilities();
  const [running, setRunning] = useState(false);
  const recheck = async () => {
    setRunning(true);
    try {
      await api.system.capabilities(true);
      await api.system.runBenchmark("synthetic");
      const models = await modelsApi.list();
      if (models.some((m) => m.state === "installed" && m.manifest.purpose === "llm")) {
        await api.system.runBenchmark("llm");
      }
      qc.setQueryData(keys.capabilities, await api.system.capabilities(false));
      toast.success("System check complete");
    } catch (e) {
      toast.error((e as Error).message);
    } finally {
      setRunning(false);
    }
  };
  if (!caps) return null;
  const hw = caps.hardware;
  const a = caps.assessment;
  const b = a.budget;
  const syn = caps.benchmarks.synthetic;
  return (
    <>
      <Section
        title="Detected hardware"
        description={`Checked ${new Date(caps.detectedAt).toLocaleString()}`}
      >
        <div className="mb-4">
          <Button size="sm" variant="outline" onClick={recheck} disabled={running || isFetching}>
            <RefreshCw className={running ? "animate-spin" : undefined} /> Run system check again
          </Button>
        </div>
        <KeyValue
          rows={[
            ["Operating system", `${hw.osVersion} (${hw.architecture})`],
            ["Processor", `${hw.cpu.model ?? "Unknown"} · ${hw.cpu.physicalCores ?? "?"} cores / ${hw.cpu.logicalCores} threads${hw.cpu.avx2 != null ? ` · AVX2 ${hw.cpu.avx2 ? "yes" : "no"}` : ""}`],
            ["Memory", `${formatBytes(hw.memory.totalBytes)} total · ${formatBytes(hw.memory.availableBytes)} available · pressure ${hw.memory.pressure ?? "unknown"}`],
            ["Disk", `${formatBytes(hw.disk.availableBytes)} free of ${formatBytes(hw.disk.totalBytes)}`],
            ...hw.gpus.map((g, i): [string, string] => [
              `GPU ${hw.gpus.length > 1 ? i + 1 : ""}`.trim(),
              `${g.model ?? "Unknown"} · ${g.unifiedMemory ? `unified, ${formatBytes(g.vramBytes)} GPU working set` : g.vramBytes ? `${formatBytes(g.vramBytes)} VRAM` : "shared memory"} · ${[g.metal && "Metal", g.cuda && "CUDA", g.vulkan && "Vulkan"].filter(Boolean).join(", ") || "no compute API"}`,
            ]),
            ["Microphones", hw.audio.inputDevices.map((d) => d.name).join(", ") || "None found"],
            ["Output device", hw.audio.outputDevice?.name ?? "None found"],
            ["Microphone permission", hw.audio.microphonePermission],
            ["Meeting audio", hw.audio.systemAudioAvailable ? `available · permission ${hw.audio.systemAudioPermission}` : (hw.audio.systemAudioUnavailableReason ?? "unavailable")],
          ]}
        />
      </Section>
      <Section title="Capability">
        <KeyValue
          rows={[
            ["Tier", `${profileLabel[a.profile]} (${a.basis === "benchmarked" ? "benchmarked" : a.basis === "estimated" ? "estimated" : "hardware only"})`],
            ["Memory allows", a.memoryCeiling ? profileLabel[a.memoryCeiling] : "Nothing (insufficient memory)"],
            ["Acceleration allows", profileLabel[a.accelerationCap]],
            ["Support level", `${a.requirements.level}${a.requirements.meetsOfficialMinimum ? "" : " (below published minimum)"}`],
            ["LLM backend", a.inference.llmBackend],
            ["Recommended LLM", a.settings.llmModelId ?? "None"],
            ["Context", a.settings.contextTokens ? `${a.settings.contextTokens.toLocaleString()} tokens` : "—"],
            ["Model loading", a.settings.llmFit ? (a.settings.sequentialLoading ? "Sequential (speech models unloaded first)" : "Concurrent") : "—"],
            ["Live transcript", a.settings.realtimeTranscript ? "On" : "Off"],
            ...a.adjustments.map((adj): [string, string] => [`${profileLabel[adj.from]} → ${profileLabel[adj.to]}`, adj.reason]),
          ]}
        />
      </Section>
      <Section title="Memory budget" description="How much memory local AI may use. Reserves keep the computer responsive during calls.">
        <KeyValue
          rows={[
            ["Total", formatBytes(b.totalBytes)],
            ["Operating system reserve", formatBytes(b.osReserveBytes)],
            ["App overhead", formatBytes(b.appOverheadBytes)],
            ["Meeting app allowance", formatBytes(b.meetingAppAllowanceBytes)],
            ["Planning budget", formatBytes(b.planningBytes)],
            ["Available right now", formatBytes(b.liveBytes)],
            ["Usable GPU memory", b.unifiedMemory ? `${formatBytes(b.gpuWorkingSetBytes)} (unified)` : formatBytes(b.discreteVramUsableBytes)],
          ]}
        />
      </Section>
      <Section title="Benchmarks">
        <KeyValue
          rows={[
            ["Memory bandwidth", syn ? `${syn.memoryBandwidthGbps.toFixed(1)} GB/s` : "Not run"],
            ["CPU throughput", syn ? `${syn.cpuGflops.toFixed(0)} GFLOP/s (${syn.threads} threads)` : "Not run"],
            ["Transcription", caps.benchmarks.asr ? `RTF ${caps.benchmarks.asr.rtf.toFixed(3)} · load ${caps.benchmarks.asr.loadMs} ms` : "Runs after the speech model is installed"],
            ...caps.benchmarks.llm.map((l): [string, string] => [
              l.modelId,
              `${l.generationTokensPerSecond.toFixed(1)} tok/s generate · ${l.promptTokensPerSecond.toFixed(0)} tok/s prompt · ${l.structuredOutputValid ? "valid output" : "invalid output"}`,
            ]),
          ]}
        />
      </Section>
    </>
  );
}

function FixtureSimulator() {
  const { data: info } = useAppInfo();
  const { data: settings } = useSettings();
  const patch = usePatchSettings();
  const qc = useQueryClient();
  const fixtures = useQuery({ queryKey: ["fixtures"], queryFn: api.system.fixtures, enabled: !!info?.debugBuild });
  if (!info?.debugBuild || !settings) return null;
  const NONE = "__none";
  return (
    <Section title="Developer: simulate hardware" description="Debug builds only. Evaluates a hardware fixture instead of this computer.">
      <Select
        value={settings.advanced.simulateHardwareFixture ?? NONE}
        onValueChange={(v) => {
          patch((s) => void (s.advanced.simulateHardwareFixture = v === NONE ? null : v));
          setTimeout(() => void qc.invalidateQueries({ queryKey: keys.capabilities }), 100);
        }}
      >
        <SelectTrigger className="w-80">
          <SelectValue />
        </SelectTrigger>
        <SelectContent>
          <SelectItem value={NONE}>This computer</SelectItem>
          {fixtures.data?.map((f) => (
            <SelectItem key={f.id} value={f.id}>
              {f.description}
            </SelectItem>
          ))}
        </SelectContent>
      </Select>
    </Section>
  );
}

export function AdvancedSettings() {
  const { data } = useSettings();
  const { data: info } = useAppInfo();
  const patch = usePatchSettings();
  if (!data) return null;
  const set = <K extends keyof Adv>(k: K) => (v: Adv[K]) => patch((s) => void (s.advanced[k] = v));
  return (
    <>
      <Section description="Most people never need to change these.">
        <Field label="Inference backend" htmlFor="backend">
          <Select value={data.advanced.inferenceBackend} onValueChange={(v) => set("inferenceBackend")(v as InferenceBackendPreference)}>
            <SelectTrigger id="backend" className="w-36">
              <SelectValue />
            </SelectTrigger>
            <SelectContent>
              <SelectItem value="auto">Automatic</SelectItem>
              <SelectItem value="metal">Metal</SelectItem>
              <SelectItem value="cuda">CUDA</SelectItem>
              <SelectItem value="vulkan">Vulkan</SelectItem>
              <SelectItem value="cpu">CPU only</SelectItem>
            </SelectContent>
          </Select>
        </Field>
        <NumberField label="Context size" help="Tokens. Automatic uses the capability profile." value={data.advanced.contextTokens} onCommit={set("contextTokens")} />
        <NumberField label="GPU layers" help="Layers offloaded to the GPU. Automatic fits them to memory." value={data.advanced.gpuLayers} onCommit={set("gpuLayers")} />
        <NumberField label="Transcription threads" value={data.advanced.asrThreads} onCommit={set("asrThreads")} />
        <NumberField label="Transcription chunk (s)" value={data.advanced.asrChunkSeconds} onCommit={(v) => set("asrChunkSeconds")(v ?? 20)} placeholder="20" />
        <NumberField label="Known speaker similarity" step={0.05} value={data.advanced.speakerKnownThreshold} onCommit={(v) => set("speakerKnownThreshold")(v ?? 0.6)} placeholder="0.6" />
        <NumberField label="Possible speaker similarity" step={0.05} value={data.advanced.speakerPossibleThreshold} onCommit={(v) => set("speakerPossibleThreshold")(v ?? 0.45)} placeholder="0.45" />
      </Section>
      <SystemCheck />
      <FixtureSimulator />
      {info && (
        <Section title="Diagnostics" description="A report for troubleshooting. It never includes transcripts, audio, voice profiles or passwords.">
          <KeyValue rows={[["Log folder", info.logDir], ["Data folder", info.dataDir], ["Version", info.version]]} />
          <Button
            className="mt-3"
            size="sm"
            variant="outline"
            onClick={async () => {
              const path = await save({ defaultPath: "minutes-diagnostics.json", filters: [{ name: "JSON", extensions: ["json"] }] });
              if (!path) return;
              invoke<void>("export_diagnostics", { path }).then(() => toast.success("Diagnostics saved"), (e) => toast.error(e.message));
            }}
          >
            Export diagnostics…
          </Button>
        </Section>
      )}
    </>
  );
}
