import { useQueryClient } from "@tanstack/react-query";
import { AudioLines, MoreHorizontal, Plus } from "lucide-react";
import { useState } from "react";
import { toast } from "sonner";
import { Button } from "@/components/ui/button";
import { Checkbox } from "@/components/ui/checkbox";
import { Dialog, DialogContent, DialogFooter, DialogHeader, DialogTitle } from "@/components/ui/dialog";
import { DropdownMenu, DropdownMenuContent, DropdownMenuItem, DropdownMenuSeparator, DropdownMenuTrigger } from "@/components/ui/dropdown-menu";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import { PageBody, PageHeader, Panel } from "@/components/app/Page";
import { Avatar } from "@/components/app/Avatar";
import { peopleApi } from "@/lib/api";
import { usePeople } from "@/lib/api/queries";
import type { Person } from "@/lib/types";
import { VoiceEnrollmentDialog } from "./VoiceEnrollmentDialog";

function PersonDialog({ person, open, onOpenChange }: { person?: Person; open: boolean; onOpenChange: (o: boolean) => void }) {
  const qc = useQueryClient();
  const [name, setName] = useState(person?.displayName ?? "");
  const [email, setEmail] = useState(person?.email ?? "");
  const [isSelf, setIsSelf] = useState(person?.isSelf ?? false);
  const save = async () => {
    try {
      const input = { displayName: name, email: email || null, isSelf };
      if (person) await peopleApi.update(person.id, input);
      else await peopleApi.create(input);
      void qc.invalidateQueries({ queryKey: ["people"] });
      onOpenChange(false);
    } catch (e) {
      toast.error((e as Error).message);
    }
  };
  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogContent className="sm:max-w-sm">
        <DialogHeader>
          <DialogTitle>{person ? "Edit person" : "Add person"}</DialogTitle>
        </DialogHeader>
        <form className="space-y-4" onSubmit={(e) => { e.preventDefault(); void save(); }}>
          <div className="space-y-1.5">
            <Label htmlFor="pname">Name</Label>
            <Input id="pname" value={name} onChange={(e) => setName(e.target.value)} autoFocus />
          </div>
          <div className="space-y-1.5">
            <Label htmlFor="pemail">Email (optional)</Label>
            <Input id="pemail" type="email" value={email} onChange={(e) => setEmail(e.target.value)} />
          </div>
          <div className="flex items-center gap-2">
            <Checkbox id="pself" checked={isSelf} onCheckedChange={(v) => setIsSelf(v === true)} />
            <Label htmlFor="pself" className="font-normal">This is me (my microphone is labelled with this name)</Label>
          </div>
          <DialogFooter>
            <Button type="submit" disabled={!name.trim()}>Save</Button>
          </DialogFooter>
        </form>
      </DialogContent>
    </Dialog>
  );
}

export function PeoplePage() {
  const qc = useQueryClient();
  const { data: people } = usePeople();
  const [editing, setEditing] = useState<Person | "new" | null>(null);
  const [enrolling, setEnrolling] = useState<Person | null>(null);
  const refresh = () => void qc.invalidateQueries({ queryKey: ["people"] });

  return (
    <>
      <PageHeader
        actions={
          <Button size="sm" variant="outline" className="h-7" onClick={() => setEditing("new")}>
            <Plus /> Add person
          </Button>
        }
      />
      <PageBody>
        <div className="mx-auto max-w-2xl px-6 pt-6 pb-10">
          <h1 className="font-display text-[26px] tracking-tight">People</h1>
          <p className="mt-1.5 max-w-prose text-muted-foreground">
            People you meet with regularly. Voice profiles help Minutes suggest who is speaking. They’re optional,
            encrypted on this computer, and never created automatically for guests.
          </p>
          {people && people.length === 0 && (
            <div className="mt-8 rounded-xl border border-dashed p-8 text-center">
              <p className="text-muted-foreground">No one yet. Start by adding yourself.</p>
              <Button size="sm" className="mt-3" onClick={() => setEditing("new")}>
                <Plus /> Add person
              </Button>
            </div>
          )}
          {people && people.length > 0 && (
            <Panel className="mt-6 divide-y">
              {people.map((p) => (
                <div key={p.id} className="group flex items-center gap-3 px-4 py-3">
                  <Avatar name={p.displayName} />
                  <div className="min-w-0 flex-1">
                    <div className="flex items-center gap-2">
                      <span className="truncate font-medium">{p.displayName}</span>
                      {p.isSelf && <span className="rounded-full bg-accent px-1.5 py-px text-[10px] font-medium text-muted-foreground">You</span>}
                    </div>
                    <div className="mt-0.5 flex items-center gap-1.5 text-xs text-muted-foreground">
                      <span className={p.voiceProfile ? "size-1.5 rounded-full bg-success" : "size-1.5 rounded-full bg-muted-foreground/40"} aria-hidden />
                      {p.voiceProfile
                        ? `Voice profile · ${p.voiceProfile.embeddingCount} sample${p.voiceProfile.embeddingCount === 1 ? "" : "s"}`
                        : "No voice profile"}
                      {p.email ? <span className="truncate">· {p.email}</span> : null}
                    </div>
                  </div>
                  <Button size="sm" variant={p.voiceProfile ? "ghost" : "outline"} className="h-7" onClick={() => setEnrolling(p)}>
                    <AudioLines /> {p.voiceProfile ? "Re-record" : "Set up voice"}
                  </Button>
                  <DropdownMenu>
                    <DropdownMenuTrigger asChild>
                      <Button size="icon" variant="ghost" className="size-7 text-muted-foreground" aria-label={`Actions for ${p.displayName}`}>
                        <MoreHorizontal />
                      </Button>
                    </DropdownMenuTrigger>
                    <DropdownMenuContent align="end">
                      <DropdownMenuItem onSelect={() => setEditing(p)}>Edit</DropdownMenuItem>
                      {p.voiceProfile && (
                        <DropdownMenuItem onSelect={() => peopleApi.deleteVoice(p.id).then(refresh, (e) => toast.error(e.message))}>
                          Delete voice profile
                        </DropdownMenuItem>
                      )}
                      <DropdownMenuSeparator />
                      <DropdownMenuItem
                        variant="destructive"
                        onSelect={() => {
                          if (window.confirm(`Remove ${p.displayName}? Their voice profile is deleted. Transcripts keep the text but lose the name.`)) {
                            peopleApi.remove(p.id).then(refresh, (e) => toast.error(e.message));
                          }
                        }}
                      >
                        Remove person
                      </DropdownMenuItem>
                    </DropdownMenuContent>
                  </DropdownMenu>
                </div>
              ))}
            </Panel>
          )}
        </div>
      </PageBody>
      {editing && (
        <PersonDialog
          key={editing === "new" ? "new" : editing.id}
          person={editing === "new" ? undefined : editing}
          open
          onOpenChange={(o) => !o && setEditing(null)}
        />
      )}
      {enrolling && <VoiceEnrollmentDialog person={enrolling} open onOpenChange={(o) => !o && setEnrolling(null)} />}
    </>
  );
}
