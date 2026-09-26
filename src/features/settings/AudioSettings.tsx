import { Select, SelectContent, SelectItem, SelectTrigger, SelectValue } from "@/components/ui/select";
import { Switch } from "@/components/ui/switch";
import { Section } from "@/components/app/Page";
import { useAudioDevices, usePatchSettings, useSettings } from "@/lib/api/queries";
import { Field } from "./Field";

const DEFAULT = "__default";

export function AudioSettings() {
  const { data } = useSettings();
  const { data: devices, refetch } = useAudioDevices();
  const patch = usePatchSettings();
  if (!data) return null;
  const inputs = devices?.inputs ?? [];
  const outputs = devices?.outputs ?? [];
  return (
    <Section description="Microphone and meeting audio are recorded as separate streams.">
      <Field label="Microphone" htmlFor="mic">
        <Select
          value={data.audio.microphoneDeviceId ?? DEFAULT}
          onOpenChange={(o) => o && void refetch()}
          onValueChange={(v) => patch((s) => void (s.audio.microphoneDeviceId = v === DEFAULT ? null : v))}
        >
          <SelectTrigger id="mic" className="w-64">
            <SelectValue />
          </SelectTrigger>
          <SelectContent>
            <SelectItem value={DEFAULT}>System default</SelectItem>
            {inputs.map((d) => (
              <SelectItem key={d.id} value={d.id}>
                {d.name}
              </SelectItem>
            ))}
          </SelectContent>
        </Select>
      </Field>
      <Field label="Record meeting audio" help="Capture other participants from your speakers or headset." htmlFor="sys">
        <Switch id="sys" checked={data.audio.captureSystemAudio} onCheckedChange={(v) => patch((s) => void (s.audio.captureSystemAudio = v))} />
      </Field>
      {data.audio.captureSystemAudio && (
        <Field label="Meeting audio from" htmlFor="out">
          <Select
            value={data.audio.outputDeviceId ?? DEFAULT}
            onOpenChange={(o) => o && void refetch()}
            onValueChange={(v) => patch((s) => void (s.audio.outputDeviceId = v === DEFAULT ? null : v))}
          >
            <SelectTrigger id="out" className="w-64">
              <SelectValue />
            </SelectTrigger>
            <SelectContent>
              <SelectItem value={DEFAULT}>Current output</SelectItem>
              {outputs.map((d) => (
                <SelectItem key={d.id} value={d.id}>
                  {d.name}
                </SelectItem>
              ))}
            </SelectContent>
          </Select>
        </Field>
      )}
    </Section>
  );
}
