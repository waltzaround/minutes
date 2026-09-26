import { useQueryClient } from "@tanstack/react-query";
import { save } from "@tauri-apps/plugin-dialog";
import { Link } from "react-router";
import { toast } from "sonner";
import { Button } from "@/components/ui/button";
import { Section } from "@/components/app/Page";
import { peopleApi } from "@/lib/api";
import { usePeople } from "@/lib/api/queries";
import { Field } from "./Field";

export function PeopleSettings() {
  const qc = useQueryClient();
  const { data: people = [] } = usePeople();
  const withVoice = people.filter((p) => p.voiceProfile).length;
  return (
    <>
      <Section description="Voice profiles are encrypted with a key kept in your system’s credential store and never leave this computer.">
        <Field label="People" help={`${people.length} people, ${withVoice} with voice profiles.`}>
          <Button size="sm" variant="outline" asChild>
            <Link to="/people">Manage people</Link>
          </Button>
        </Field>
        <Field label="Export people" help="Names, emails and integration links as JSON. Voice data is never exported.">
          <Button
            size="sm"
            variant="outline"
            onClick={async () => {
              const path = await save({ defaultPath: "minutes-people.json", filters: [{ name: "JSON", extensions: ["json"] }] });
              if (!path) return;
              peopleApi.exportTo(path).then(() => toast.success("People exported"), (e) => toast.error(e.message));
            }}
          >
            Export…
          </Button>
        </Field>
        <Field label="Clear all voice data" help="Deletes every voice profile and the encryption key. People are kept.">
          <Button
            size="sm"
            variant="outline"
            className="text-destructive"
            onClick={() => {
              if (!window.confirm("Delete all voice profiles? Speakers won’t be recognised until people set up their voices again.")) return;
              peopleApi.clearAllVoices().then(() => {
                toast.success("All voice data deleted");
                void qc.invalidateQueries({ queryKey: ["people"] });
              }, (e) => toast.error(e.message));
            }}
          >
            Clear voice data
          </Button>
        </Field>
      </Section>
    </>
  );
}
