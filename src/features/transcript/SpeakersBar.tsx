import { useState } from "react";
import { toast } from "sonner";
import { Button } from "@/components/ui/button";
import { Dialog, DialogContent, DialogDescription, DialogFooter, DialogHeader, DialogTitle } from "@/components/ui/dialog";
import { Input } from "@/components/ui/input";
import { Popover, PopoverContent, PopoverTrigger } from "@/components/ui/popover";
import { Separator } from "@/components/ui/separator";
import { peopleApi, speakersApi } from "@/lib/api";
import type { MeetingDetail, Person, SpeakerCluster } from "@/lib/types";
import { cn } from "@/lib/utils";
import { formatDuration } from "@/lib/utils/format";

function identityText(c: SpeakerCluster, people: Person[]): string | null {
  const name = (id: string) => people.find((p) => p.id === id)?.displayName ?? "someone";
  if (c.isLocalUser) return "This computer’s microphone";
  if (c.confirmed && c.personId) return name(c.personId);
  if (c.identity.type === "known") return `Recognised as ${name(c.identity.personId)}`;
  if (c.identity.type === "possible") return `Probably ${name(c.identity.personId)}`;
  return null;
}

export function SpeakersBar({
  meeting,
  clusters,
  people,
  onChanged,
}: {
  meeting: MeetingDetail;
  clusters: SpeakerCluster[];
  people: Person[];
  onChanged: () => void;
}) {
  const [improve, setImprove] = useState<{ cluster: SpeakerCluster; person: Person } | null>(null);
  const visible = clusters.filter((c) => c.segmentCount > 0);
  if (visible.length === 0) return null;

  const assign = async (cluster: SpeakerCluster, personId: string | null) => {
    try {
      await speakersApi.setPerson(cluster.id, personId, false);
      onChanged();
      const person = people.find((p) => p.id === personId);
      if (person && meeting.audioAvailable) setImprove({ cluster, person });
    } catch (e) {
      toast.error((e as Error).message);
    }
  };

  const addPerson = async (cluster: SpeakerCluster, name: string) => {
    try {
      const p = await peopleApi.create({ displayName: name, email: null, isSelf: null });
      await speakersApi.setPerson(cluster.id, p.id, false);
      onChanged();
    } catch (e) {
      toast.error((e as Error).message);
    }
  };

  return (
    <section aria-label="Speakers" className="mb-4">
      <h2 className="mb-2 text-xs font-medium tracking-wide text-muted-foreground uppercase">Speakers</h2>
      <div className="flex flex-wrap gap-2">
        {visible.map((c) => {
          const person = c.personId ? people.find((p) => p.id === c.personId) : undefined;
          const title = c.confirmed || c.isLocalUser ? (person?.displayName ?? c.label) : c.label;
          const sub = identityText(c, people);
          const possible = c.identity.type === "possible" && !c.confirmed;
          return (
            <SpeakerChip
              key={c.id}
              cluster={c}
              title={title}
              subtitle={sub}
              highlight={possible}
              people={people}
              others={visible.filter((o) => o.id !== c.id)}
              onAssign={(pid) => assign(c, pid)}
              onAddPerson={(name) => addPerson(c, name)}
              onRename={(label) => speakersApi.rename(c.id, label).then(onChanged, (e) => toast.error(e.message))}
              onMerge={(into) => speakersApi.merge(c.id, into).then(onChanged, (e) => toast.error(e.message))}
            />
          );
        })}
      </div>
      {visible.some((c) => c.identity.type === "possible" && !c.confirmed) && (
        <p className="mt-2 text-xs text-muted-foreground">Suggested names are guesses from voice profiles. Confirm them to use them in notes and actions.</p>
      )}

      <Dialog open={!!improve} onOpenChange={(o) => !o && setImprove(null)}>
        <DialogContent className="sm:max-w-sm">
          <DialogHeader>
            <DialogTitle>Improve recognition?</DialogTitle>
            <DialogDescription>
              Use confirmed speech from this meeting to improve recognition of {improve?.person.displayName} in future meetings?
            </DialogDescription>
          </DialogHeader>
          <DialogFooter>
            <Button variant="ghost" onClick={() => setImprove(null)}>Not now</Button>
            <Button
              onClick={async () => {
                if (!improve) return;
                try {
                  const r = await speakersApi.setPerson(improve.cluster.id, improve.person.id, true);
                  toast.success(r.profileSamplesAdded > 0 ? `Voice profile updated (${r.profileSamplesAdded} samples)` : "Not enough clear speech to add to the profile");
                } catch (e) {
                  toast.error((e as Error).message);
                }
                setImprove(null);
                onChanged();
              }}
            >
              Improve profile
            </Button>
          </DialogFooter>
        </DialogContent>
      </Dialog>
    </section>
  );
}

function SpeakerChip({
  cluster,
  title,
  subtitle,
  highlight,
  people,
  others,
  onAssign,
  onAddPerson,
  onRename,
  onMerge,
}: {
  cluster: SpeakerCluster;
  title: string;
  subtitle: string | null;
  highlight: boolean;
  people: Person[];
  others: SpeakerCluster[];
  onAssign: (personId: string | null) => void;
  onAddPerson: (name: string) => void;
  onRename: (label: string) => void;
  onMerge: (into: string) => void;
}) {
  const [open, setOpen] = useState(false);
  const [label, setLabel] = useState(cluster.label);
  const [newName, setNewName] = useState("");
  const suggested = cluster.identity.type !== "unknown" ? (cluster.identity as { personId: string }).personId : null;
  const ordered = [...people].sort((a, b) => (a.id === suggested ? -1 : b.id === suggested ? 1 : 0));
  return (
    <Popover open={open} onOpenChange={setOpen}>
      <PopoverTrigger asChild>
        <button
          className={cn(
            "rounded-md border px-3 py-1.5 text-left outline-none hover:bg-accent focus-visible:ring-2 focus-visible:ring-ring",
            highlight && "border-warning/50 bg-warning/5",
          )}
        >
          <div className="text-[13px] font-medium">{title}</div>
          <div className="text-[11px] text-muted-foreground">
            {subtitle ? `${subtitle} · ` : ""}
            {formatDuration(cluster.speakingMs)}
          </div>
        </button>
      </PopoverTrigger>
      <PopoverContent className="w-72 p-0" align="start">
        <div className="p-3">
          <div className="text-xs font-medium text-muted-foreground">{cluster.label}</div>
          {subtitle && <div className="mt-0.5 font-medium">{subtitle}</div>}
          {!cluster.isLocalUser && (
            <div className="mt-2 flex flex-wrap gap-1.5">
              {ordered.slice(0, 8).map((p) => (
                <Button
                  key={p.id}
                  size="sm"
                  variant={p.id === cluster.personId && cluster.confirmed ? "default" : p.id === suggested ? "secondary" : "outline"}
                  onClick={() => {
                    onAssign(p.id);
                    setOpen(false);
                  }}
                >
                  {p.displayName}
                </Button>
              ))}
              <Button size="sm" variant="ghost" onClick={() => { onAssign(null); setOpen(false); }}>
                Unknown
              </Button>
            </div>
          )}
          {!cluster.isLocalUser && (
            <form
              className="mt-2 flex gap-1.5"
              onSubmit={(e) => {
                e.preventDefault();
                if (newName.trim()) {
                  onAddPerson(newName.trim());
                  setNewName("");
                  setOpen(false);
                }
              }}
            >
              <Input className="h-8" placeholder="Add a new person…" value={newName} onChange={(e) => setNewName(e.target.value)} aria-label="New person name" />
              <Button size="sm" type="submit" variant="outline" disabled={!newName.trim()}>Add</Button>
            </form>
          )}
        </div>
        <Separator />
        <form
          className="flex gap-1.5 p-3"
          onSubmit={(e) => {
            e.preventDefault();
            onRename(label);
            setOpen(false);
          }}
        >
          <Input className="h-8" value={label} onChange={(e) => setLabel(e.target.value)} aria-label="Rename speaker" />
          <Button size="sm" type="submit" variant="outline">Rename</Button>
        </form>
        {others.length > 0 && (
          <>
            <Separator />
            <div className="p-3">
              <div className="mb-1.5 text-xs text-muted-foreground">Same person as…</div>
              <div className="flex flex-wrap gap-1.5">
                {others.map((o) => (
                  <Button key={o.id} size="sm" variant="outline" onClick={() => { onMerge(o.id); setOpen(false); }}>
                    {o.label}
                  </Button>
                ))}
              </div>
            </div>
          </>
        )}
      </PopoverContent>
    </Popover>
  );
}
