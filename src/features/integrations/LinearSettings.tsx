import { useQuery, useQueryClient } from "@tanstack/react-query";
import { CheckCircle2 } from "lucide-react";
import { useState } from "react";
import { toast } from "sonner";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { Select, SelectContent, SelectItem, SelectTrigger, SelectValue } from "@/components/ui/select";
import { Section } from "@/components/app/Page";
import { integrationsApi } from "@/lib/api";
import { keys, useIntegrations, usePeople, useSettings } from "@/lib/api/queries";
import { Field } from "@/features/settings/Field";

export function useLinearDirectory(enabled: boolean) {
  return useQuery({ queryKey: ["linearDirectory"], queryFn: integrationsApi.linearDirectory, enabled, staleTime: 300_000 });
}

export function LinearSettings() {
  const qc = useQueryClient();
  const { data: status } = useIntegrations();
  const { data: settings } = useSettings();
  const { data: people = [] } = usePeople();
  const [key, setKey] = useState("");
  const [busy, setBusy] = useState(false);
  const connected = status?.linear.connected ?? false;
  const dir = useLinearDirectory(connected);
  const NONE = "__none";
  const refresh = () => {
    void qc.invalidateQueries({ queryKey: ["integrations"] });
    void qc.invalidateQueries({ queryKey: keys.settings });
    void qc.invalidateQueries({ queryKey: ["linearDirectory"] });
  };

  const connect = async () => {
    setBusy(true);
    try {
      const ws = await integrationsApi.linearConnect(key);
      toast.success(`Connected to ${ws}`);
      setKey("");
      refresh();
    } catch (e) {
      toast.error((e as Error).message);
    } finally {
      setBusy(false);
    }
  };

  if (!settings) return null;
  const setDefaults = (team: string | null, project: string | null) =>
    integrationsApi.setLinearDefaults(team, project).then(refresh, (e) => toast.error(e.message));

  return (
    <>
      <Section description="Create issues from reviewed action items. Nothing is created without your review.">
        {connected ? (
          <Field label={<span className="flex items-center gap-1.5"><CheckCircle2 className="size-4 text-success" aria-hidden /> Connected{status?.linear.account ? ` to ${status.linear.account}` : ""}</span>}>
            <Button size="sm" variant="outline" onClick={() => integrationsApi.linearDisconnect().then(refresh, (e) => toast.error(e.message))}>Disconnect</Button>
          </Field>
        ) : (
          <div className="space-y-3 py-2">
            <p className="text-muted-foreground">In Linear, open Settings → Security &amp; access → Personal API keys, create a key and paste it here.</p>
            <form className="flex gap-2" onSubmit={(e) => { e.preventDefault(); void connect(); }}>
              <Input type="password" placeholder="lin_api_…" value={key} onChange={(e) => setKey(e.target.value)} aria-label="Linear API key" autoComplete="off" />
              <Button type="submit" disabled={!key.trim() || busy}>{busy ? "Checking…" : "Connect"}</Button>
            </form>
            <p className="text-xs text-muted-foreground">The key is stored in your system’s credential store, not in Minutes’ files.</p>
          </div>
        )}
      </Section>
      {connected && dir.error && <p className="text-destructive">{dir.error.message}</p>}
      {connected && dir.data && (
        <>
          <Section title="Defaults">
            <Field label="Team">
              <Select value={settings.linear.defaultTeamId ?? NONE} onValueChange={(v) => setDefaults(v === NONE ? null : v, settings.linear.defaultProjectId)}>
                <SelectTrigger className="w-56"><SelectValue /></SelectTrigger>
                <SelectContent>
                  <SelectItem value={NONE}>Choose each time</SelectItem>
                  {dir.data.teams.map((t) => <SelectItem key={t.id} value={t.id}>{t.name}</SelectItem>)}
                </SelectContent>
              </Select>
            </Field>
            <Field label="Project">
              <Select value={settings.linear.defaultProjectId ?? NONE} onValueChange={(v) => setDefaults(settings.linear.defaultTeamId, v === NONE ? null : v)}>
                <SelectTrigger className="w-56"><SelectValue /></SelectTrigger>
                <SelectContent>
                  <SelectItem value={NONE}>No project</SelectItem>
                  {dir.data.projects.map((p) => <SelectItem key={p.id} value={p.id}>{p.name}</SelectItem>)}
                </SelectContent>
              </Select>
            </Field>
          </Section>
          <Section title="People" description="Link people to Linear users so issues are assigned to the right person.">
            {people.length === 0 && <p className="text-muted-foreground">Add people first.</p>}
            {people.map((p) => (
              <Field key={p.id} label={p.displayName}>
                <Select
                  value={p.linearUserId ?? NONE}
                  onValueChange={(v) => {
                    const u = dir.data!.users.find((x) => x.id === v);
                    integrationsApi.mapPerson(p.id, "linear", v === NONE ? null : v, u?.name ?? null).then(
                      () => void qc.invalidateQueries({ queryKey: ["people"] }),
                      (e) => toast.error(e.message),
                    );
                  }}
                >
                  <SelectTrigger className="w-56"><SelectValue /></SelectTrigger>
                  <SelectContent>
                    <SelectItem value={NONE}>Not linked</SelectItem>
                    {dir.data!.users.map((u) => <SelectItem key={u.id} value={u.id}>{u.name}{u.detail ? ` (${u.detail})` : ""}</SelectItem>)}
                  </SelectContent>
                </Select>
              </Field>
            ))}
          </Section>
        </>
      )}
    </>
  );
}
