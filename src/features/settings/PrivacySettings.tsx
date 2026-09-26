import { RadioGroup, RadioGroupItem } from "@/components/ui/radio-group";
import { Label } from "@/components/ui/label";
import { Switch } from "@/components/ui/switch";
import { Section } from "@/components/app/Page";
import { usePatchSettings, useSettings } from "@/lib/api/queries";
import type { AudioRetention } from "@/lib/types";
import { Field } from "./Field";

const options: [AudioRetention, string, string][] = [
  ["delete_after_processing", "Delete after the transcript is finalised", "Recommended. Audio is kept only until processing has safely completed."],
  ["keep_7_days", "Keep for 7 days", "Lets you re-run transcription for a week."],
  ["keep_forever", "Keep indefinitely", "Audio stays on this computer until you delete the meeting."],
];

export function PrivacySettings() {
  const { data } = useSettings();
  const patch = usePatchSettings();
  if (!data) return null;
  return (
    <>
      <Section title="Meeting audio" description="Transcripts are always kept. This controls the raw audio recordings.">
        <RadioGroup
          value={data.privacy.audioRetention}
          onValueChange={(v) => patch((s) => void (s.privacy.audioRetention = v as AudioRetention))}
          className="gap-3 py-3"
        >
          {options.map(([value, label, help]) => (
            <div key={value} className="flex items-start gap-3">
              <RadioGroupItem value={value} id={`ret-${value}`} className="mt-0.5" />
              <Label htmlFor={`ret-${value}`} className="block font-normal">
                <span className="font-medium">{label}</span>
                <span className="mt-0.5 block text-muted-foreground">{help}</span>
              </Label>
            </div>
          ))}
        </RadioGroup>
      </Section>
      <Section title="Network">
        <Field
          label="Check for app updates"
          help="The only automatic network request Minutes can make. No telemetry or analytics are ever sent."
          htmlFor="updates"
        >
          <Switch id="updates" checked={data.privacy.checkForUpdates} onCheckedChange={(v) => patch((s) => void (s.privacy.checkForUpdates = v))} />
        </Field>
      </Section>
    </>
  );
}
