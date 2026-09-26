import { useQuery } from "@tanstack/react-query";
import { open } from "@tauri-apps/plugin-dialog";
import { toast } from "sonner";
import { Button } from "@/components/ui/button";
import { KeyValue, Section } from "@/components/app/Page";
import { invoke } from "@/lib/api/invoke";
import { usePatchSettings, useSettings } from "@/lib/api/queries";
import type { StorageUsage } from "@/lib/types";
import { formatBytes } from "@/lib/utils/format";
import { Field } from "./Field";

export function StorageSettings() {
  const { data: settings } = useSettings();
  const patch = usePatchSettings();
  const { data: usage, refetch } = useQuery({ queryKey: ["storage"], queryFn: () => invoke<StorageUsage>("storage_usage") });
  if (!settings) return null;
  const chooseFolder = async () => {
    const dir = await open({ directory: true, multiple: false });
    if (!dir || Array.isArray(dir)) return;
    patch((s) => void (s.advanced.modelsDir = dir));
    toast.success("Models folder changed. Download or move models into the new folder.");
    setTimeout(() => void refetch(), 200);
  };
  return (
    <Section description="Everything stays on this computer.">
      {usage && (
        <KeyValue
          rows={[
            ["AI models", formatBytes(usage.modelsBytes)],
            ["Meeting audio", formatBytes(usage.audioBytes)],
            ["Transcripts and notes", formatBytes(usage.databaseBytes)],
            ["Free space", formatBytes(usage.freeBytes)],
          ]}
        />
      )}
      <div className="mt-4">
        <Field label="Models folder" help={usage?.modelsDir}>
          <div className="flex gap-2">
            <Button size="sm" variant="outline" onClick={chooseFolder}>Change…</Button>
            {settings.advanced.modelsDir && (
              <Button size="sm" variant="ghost" onClick={() => { patch((s) => void (s.advanced.modelsDir = null)); setTimeout(() => void refetch(), 200); }}>
                Use default
              </Button>
            )}
          </div>
        </Field>
      </div>
    </Section>
  );
}
