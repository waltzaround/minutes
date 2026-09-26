import { useQuery, useQueryClient } from "@tanstack/react-query";
import { open } from "@tauri-apps/plugin-dialog";
import { toast } from "sonner";
import { Button } from "@/components/ui/button";
import { Select, SelectContent, SelectItem, SelectTrigger, SelectValue } from "@/components/ui/select";
import { Section } from "@/components/app/Page";
import { analysisApi } from "@/lib/api";
import { keys, useAppInfo, useCapabilities, usePatchSettings, useSettings } from "@/lib/api/queries";
import { ModelList } from "@/features/models/ModelList";
import { Field } from "./Field";

export function ModelsSettings() {
  const qc = useQueryClient();
  const { data: caps } = useCapabilities();
  const { data: info } = useAppInfo();
  const { data: settings } = useSettings();
  const patch = usePatchSettings();
  const { data: choices = [] } = useQuery({ queryKey: ["llmChoices"], queryFn: analysisApi.llmChoices });
  const recommended = caps?.assessment.requiredModels ?? [];
  const AUTO = "__auto";

  const importModel = async () => {
    const path = await open({ multiple: false, filters: [{ name: "GGUF model", extensions: ["gguf"] }] });
    if (!path || Array.isArray(path)) return;
    const t = toast.loading("Importing model…");
    try {
      const c = await analysisApi.importGguf(path);
      toast.success(`Imported ${c.name}`, { id: t });
      void qc.invalidateQueries({ queryKey: ["llmChoices"] });
    } catch (e) {
      toast.error((e as Error).message, { id: t });
    }
  };

  return (
    <>
      <Section description="AI models run on this computer. The models recommended for it are listed first.">
        <ModelList ids={recommended} removable />
      </Section>
      <Section title="Other models">
        <ModelList exclude={recommended} removable />
      </Section>
      {settings && (
        <Section title="Meeting summaries">
          <Field label="Summary model" help="Automatic picks the best model this computer can run comfortably.">
            <Select
              value={settings.advanced.llmModelId ?? AUTO}
              onValueChange={(v) => {
                patch((s) => void (s.advanced.llmModelId = v === AUTO ? null : v));
                void qc.invalidateQueries({ queryKey: keys.capabilities });
              }}
            >
              <SelectTrigger className="w-64">
                <SelectValue />
              </SelectTrigger>
              <SelectContent>
                <SelectItem value={AUTO}>Automatic</SelectItem>
                {choices.filter((c) => c.installed).map((c) => (
                  <SelectItem key={c.id} value={c.id}>
                    {c.name}{c.custom ? " (imported)" : ""}
                  </SelectItem>
                ))}
              </SelectContent>
            </Select>
          </Field>
          <Field label="Import a GGUF model" help="For advanced users. The file is copied into the models folder. Memory use is estimated conservatively.">
            <Button size="sm" variant="outline" onClick={importModel}>Import…</Button>
          </Field>
        </Section>
      )}
      {info && <p className="pb-6 text-xs text-muted-foreground selectable">Stored in {info.modelsDir}</p>}
    </>
  );
}
