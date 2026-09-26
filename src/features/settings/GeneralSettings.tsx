import { Select, SelectContent, SelectItem, SelectTrigger, SelectValue } from "@/components/ui/select";
import { Switch } from "@/components/ui/switch";
import { Section } from "@/components/app/Page";
import { usePatchSettings, useSettings } from "@/lib/api/queries";
import type { ThemePreference } from "@/lib/types";
import { Field } from "./Field";

export function GeneralSettings() {
  const { data } = useSettings();
  const patch = usePatchSettings();
  if (!data) return null;
  return (
    <Section>
      <Field label="Appearance" htmlFor="theme">
        <Select value={data.general.theme} onValueChange={(v) => patch((s) => void (s.general.theme = v as ThemePreference))}>
          <SelectTrigger id="theme" className="w-36">
            <SelectValue />
          </SelectTrigger>
          <SelectContent>
            <SelectItem value="system">Match system</SelectItem>
            <SelectItem value="light">Light</SelectItem>
            <SelectItem value="dark">Dark</SelectItem>
          </SelectContent>
        </Select>
      </Field>
      <Field
        label="Recording consent reminder"
        help="Remind me to tell participants that the meeting is being recorded."
        htmlFor="consent"
      >
        <Switch id="consent" checked={data.general.consentReminder} onCheckedChange={(v) => patch((s) => void (s.general.consentReminder = v))} />
      </Field>
    </Section>
  );
}
