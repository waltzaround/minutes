import { useQuery, useQueryClient } from "@tanstack/react-query";
import { CheckCircle2 } from "lucide-react";
import { useState } from "react";
import { toast } from "sonner";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import { RadioGroup, RadioGroupItem } from "@/components/ui/radio-group";
import { Select, SelectContent, SelectItem, SelectTrigger, SelectValue } from "@/components/ui/select";
import { Section } from "@/components/app/Page";
import { integrationsApi } from "@/lib/api";
import { keys, useIntegrations, usePatchSettings, usePeople, useSettings } from "@/lib/api/queries";
import type { NotionUploadMode } from "@/lib/types";
import { Field } from "@/features/settings/Field";

export function NotionSettings() {
  const qc = useQueryClient();
  const { data: status } = useIntegrations();
  const { data: settings } = useSettings();
  const patch = usePatchSettings();
  const [token, setToken] = useState("");
  const [busy, setBusy] = useState(false);
  const connected = status?.notion.connected ?? false;
  const refresh = () => {
    void qc.invalidateQueries({ queryKey: ["integrations"] });
    void qc.invalidateQueries({ queryKey: keys.settings });
  };

  const connect = async () => {
    setBusy(true);
    try {
      const ws = await integrationsApi.notionConnect(token);
      toast.success(`Connected to ${ws}`);
      setToken("");
      refresh();
    } catch (e) {
      toast.error((e as Error).message);
    } finally {
      setBusy(false);
    }
  };

  if (!settings) return null;
  return (
    <>
      <Section
        title="Notion"
        description="Send meeting notes to a Notion database. Nothing is sent until you choose “Send to Notion” on a meeting."
      >
        {connected ? (
          <Field label={<span className="flex items-center gap-1.5"><CheckCircle2 className="size-4 text-success" aria-hidden /> Connected</span>}>
            <Button size="sm" variant="outline" onClick={() => integrationsApi.notionDisconnect().then(refresh, (e) => toast.error(e.message))}>
              Disconnect
            </Button>
          </Field>
        ) : (
          <div className="space-y-3 py-2">
            <ol className="list-decimal space-y-1 pl-5 text-muted-foreground">
              <li>In Notion, open Settings → Connections → Develop or manage integrations, and create an internal integration.</li>
              <li>Copy its “Internal integration secret”.</li>
              <li>Open your meetings database in Notion and add the integration under ••• → Connections.</li>
            </ol>
            <form className="flex gap-2" onSubmit={(e) => { e.preventDefault(); void connect(); }}>
              <Input type="password" placeholder="Integration secret" value={token} onChange={(e) => setToken(e.target.value)} aria-label="Notion integration secret" autoComplete="off" />
              <Button type="submit" disabled={!token.trim() || busy}>{busy ? "Checking…" : "Connect"}</Button>
            </form>
            <p className="text-xs text-muted-foreground">The secret is stored in your system’s credential store, not in Minutes’ files.</p>
          </div>
        )}
      </Section>
      {connected && (
        <>
          <DatabasePicker current={settings.notion.dataSourceName} onChanged={refresh} />
          <Section title="Upload">
            <RadioGroup
              value={settings.notion.uploadMode}
              onValueChange={(v) => patch((s) => void (s.notion.uploadMode = v as NotionUploadMode))}
              className="gap-3 py-3"
            >
              {([
                ["summaryAndActions", "Summary + actions only"],
                ["summaryAndTranscript", "Summary + transcript"],
                ["askEachTime", "Ask each time"],
              ] as const).map(([v, label]) => (
                <div key={v} className="flex items-center gap-3">
                  <RadioGroupItem value={v} id={`nu-${v}`} />
                  <Label htmlFor={`nu-${v}`} className="font-normal">{label}</Label>
                </div>
              ))}
            </RadioGroup>
          </Section>
          <NotionPeople />
        </>
      )}
    </>
  );
}

function DatabasePicker({ current, onChanged }: { current: string | null; onChanged: () => void }) {
  const [query, setQuery] = useState("");
  const { data, isFetching, error, refetch } = useQuery({
    queryKey: ["notionSearch", query],
    queryFn: () => integrationsApi.notionSearch(query),
    staleTime: 60_000,
  });
  return (
    <Section title="Meeting database" description={current ? `Pages are created in “${current}”.` : "Choose where meeting pages are created."}>
      <div className="flex gap-2 pt-3">
        <Input placeholder="Search databases shared with Minutes" value={query} onChange={(e) => setQuery(e.target.value)} aria-label="Search Notion databases" />
        <Button variant="outline" onClick={() => refetch()} disabled={isFetching}>Refresh</Button>
      </div>
      {error && <p className="mt-2 text-destructive">{error.message}</p>}
      <ul className="mt-2 divide-y">
        {data?.map((d) => (
          <li key={d.id} className="flex items-center justify-between py-2">
            <span>{d.name}</span>
            <Button
              size="sm"
              variant={current === d.name ? "secondary" : "outline"}
              onClick={() => integrationsApi.notionSelect(d.id, d.name).then(() => { toast.success(`Using “${d.name}”`); onChanged(); }, (e) => toast.error(e.message))}
            >
              {current === d.name ? "Selected" : "Use"}
            </Button>
          </li>
        ))}
        {data && data.length === 0 && <li className="py-2 text-muted-foreground">No databases found. Share a database with the integration in Notion.</li>}
      </ul>
    </Section>
  );
}

function NotionPeople() {
  const qc = useQueryClient();
  const { data: people = [] } = usePeople();
  const { data: users } = useQuery({ queryKey: ["notionUsers"], queryFn: integrationsApi.notionUsers, staleTime: 300_000 });
  const NONE = "__none";
  if (!users || people.length === 0) return null;
  return (
    <Section title="People" description="Link people to Notion users so action items can mention them.">
      {people.map((p) => (
        <Field key={p.id} label={p.displayName}>
          <Select
            value={p.notionUserId ?? NONE}
            onValueChange={(v) => {
              const u = users.find((x) => x.id === v);
              integrationsApi.mapPerson(p.id, "notion", v === NONE ? null : v, u?.name ?? null).then(
                () => void qc.invalidateQueries({ queryKey: ["people"] }),
                (e) => toast.error(e.message),
              );
            }}
          >
            <SelectTrigger className="w-56"><SelectValue /></SelectTrigger>
            <SelectContent>
              <SelectItem value={NONE}>Not linked</SelectItem>
              {users.map((u) => <SelectItem key={u.id} value={u.id}>{u.name}{u.email ? ` (${u.email})` : ""}</SelectItem>)}
            </SelectContent>
          </Select>
        </Field>
      ))}
    </Section>
  );
}
