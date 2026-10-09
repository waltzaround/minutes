import { useQuery, useQueryClient } from "@tanstack/react-query";
import { AlertTriangle, Check, Loader2, Plus, RefreshCw, Sparkles } from "lucide-react";
import { useState, type ReactNode } from "react";
import { toast } from "sonner";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { Checkbox } from "@/components/ui/checkbox";
import { Input } from "@/components/ui/input";
import { Select, SelectContent, SelectItem, SelectTrigger, SelectValue } from "@/components/ui/select";
import { analysisApi } from "@/lib/api";
import { EVENTS, useTauriEvent } from "@/lib/api/events";
import { usePeople } from "@/lib/api/queries";
import type { ActionItemView, AnalysisProgress, AssignmentType, EvidenceRef, MeetingDetail, Person } from "@/lib/types";
import { cn } from "@/lib/utils";
import { formatClock } from "@/lib/utils/format";

const assignmentLabel: Record<AssignmentType, string> = {
  explicit_acceptance: "Explicitly accepted",
  explicit_assignment: "Assigned",
  suggested: "Suggested",
  unclear: "Owner unclear",
};

function Evidence({ refs, onJump }: { refs: EvidenceRef[]; onJump: (segmentId: string) => void }) {
  const [open, setOpen] = useState(false);
  if (refs.length === 0) return null;
  return (
    <div className="mt-1">
      <div className="flex flex-wrap items-center gap-1.5 text-xs text-muted-foreground">
        <button className="hover:text-foreground" onClick={() => setOpen((o) => !o)} aria-expanded={open}>
          Evidence
        </button>
        {refs.map((r) => (
          <button
            key={r.segmentId}
            onClick={() => onJump(r.segmentId)}
            className="rounded px-1 font-medium text-foreground/80 tabular-nums underline-offset-2 hover:underline"
            title={`${r.speaker}: ${r.text}`}
          >
            {formatClock(r.startMs)}
          </button>
        ))}
      </div>
      {open && (
        <div className="mt-1.5 space-y-1.5 border-l-2 pl-3">
          {refs.map((r) => (
            <button key={r.segmentId} onClick={() => onJump(r.segmentId)} className="block text-left text-xs hover:text-foreground">
              <span className="font-medium">{r.speaker}</span>
              <span className="text-muted-foreground"> · {formatClock(r.startMs)}</span>
              <span className="block text-muted-foreground italic">“{r.text}”</span>
            </button>
          ))}
        </div>
      )}
    </div>
  );
}

function ActionRow({ a, people, onJump, onChange }: { a: ActionItemView; people: Person[]; onJump: (id: string) => void; onChange: () => void }) {
  const [editing, setEditing] = useState(false);
  const [title, setTitle] = useState(a.title);
  const strong = a.assignmentType === "explicit_acceptance" || a.assignmentType === "explicit_assignment";
  const owner = a.ownerPersonId ? people.find((p) => p.id === a.ownerPersonId) : undefined;
  const linked = a.linear.state === "created";
  const patch = (p: Parameters<typeof analysisApi.updateAction>[1]) =>
    analysisApi.updateAction(a.id, p).then(onChange, (e) => toast.error(e.message));
  const NONE = "__none";
  return (
    <li className={cn("flex gap-3 py-3", a.dismissed && "opacity-50")}>
      <Checkbox
        className="mt-0.5"
        checked={a.selected}
        disabled={linked}
        onCheckedChange={(v) => patch({ selected: v === true })}
        aria-label={`Include “${a.title}”`}
      />
      <div className="min-w-0 flex-1">
        {editing ? (
          <form
            onSubmit={(e) => {
              e.preventDefault();
              setEditing(false);
              if (title.trim() && title !== a.title) void patch({ title });
            }}
          >
            <Input autoFocus value={title} onChange={(e) => setTitle(e.target.value)} onBlur={(e) => e.currentTarget.form?.requestSubmit()} aria-label="Action title" className="h-8" />
          </form>
        ) : (
          <button className="text-left font-medium hover:underline" onClick={() => !linked && setEditing(true)}>
            {a.title}
          </button>
        )}
        {a.description && <p className="mt-0.5 text-muted-foreground">{a.description}</p>}
        <div className="mt-1.5 flex flex-wrap items-center gap-2 text-xs">
          <Select
            value={a.ownerPersonId ?? NONE}
            onValueChange={(v) => patch({ ownerPersonId: v === NONE ? null : v })}
            disabled={linked}
          >
            <SelectTrigger size="sm" className="h-6.5 w-auto min-w-24 rounded-full border-transparent bg-accent/70 px-2.5 text-xs shadow-none" aria-label="Owner">
              <SelectValue>
                {owner ? owner.displayName : a.ownerLabel ? (strong ? a.ownerLabel : `Possible owner: ${a.ownerLabel}`) : "No owner"}
              </SelectValue>
            </SelectTrigger>
            <SelectContent>
              <SelectItem value={NONE}>No owner</SelectItem>
              {people.map((p) => (
                <SelectItem key={p.id} value={p.id}>{p.displayName}</SelectItem>
              ))}
            </SelectContent>
          </Select>
          <Input
            type="date"
            className="h-6.5 w-34 rounded-full border-transparent bg-accent/70 px-2.5 text-xs shadow-none"
            value={a.dueDate ?? ""}
            onChange={(e) => patch({ dueDate: e.target.value || null })}
            disabled={linked}
            aria-label="Due date"
            title={a.dueText ? `Said: “${a.dueText}”` : undefined}
          />
          {!a.dueDate && a.dueText && <span className="text-muted-foreground">Said “{a.dueText}”</span>}
          {a.userEdited ? (
            <Badge variant="outline">Edited</Badge>
          ) : (
            <Badge variant={strong ? "secondary" : "outline"} className={cn(!strong && "text-muted-foreground")}>
              {assignmentLabel[a.assignmentType]}
              {!strong && a.confidence < 0.5 ? " · low confidence" : ""}
            </Badge>
          )}
          {linked && a.linear.url && (
            <a href={a.linear.url} target="_blank" rel="noreferrer" className="font-medium text-foreground underline underline-offset-2">
              {a.linear.identifier ?? "Linear issue"}
            </a>
          )}
          {a.linear.state === "failed" && <span className="text-destructive">Linear: {a.linear.error}</span>}
          {a.linear.state === "unknown" && <span className="text-warning">Linear result unknown — check before retrying</span>}
        </div>
        <Evidence refs={a.evidence} onJump={onJump} />
      </div>
      {!linked && (
        <Button size="sm" variant="ghost" className="h-7 self-start text-xs text-muted-foreground" onClick={() => patch({ dismissed: !a.dismissed, selected: false })}>
          {a.dismissed ? "Restore" : "Dismiss"}
        </Button>
      )}
    </li>
  );
}

function Block({ title, children }: { title: string; children: ReactNode }) {
  return (
    <section className="py-4">
      <h2 className="mb-2.5 text-[13px] font-semibold">{title}</h2>
      {children}
    </section>
  );
}

export function NotesView({ meeting, onJump, actions }: { meeting: MeetingDetail; onJump: (segmentId: string) => void; actions?: ReactNode }) {
  const qc = useQueryClient();
  const id = meeting.summary.id;
  const key = ["analysis", id];
  const { data: analysis } = useQuery({ queryKey: key, queryFn: () => analysisApi.get(id) });
  const { data: people = [] } = usePeople();
  const [progress, setProgress] = useState<AnalysisProgress | null>(null);
  const [newAction, setNewAction] = useState("");

  useTauriEvent<AnalysisProgress>(EVENTS.analysisProgress, (p) => {
    if (p.meetingId !== id) return;
    setProgress(p);
    if (p.status !== "running" && p.status !== "pending") {
      void qc.invalidateQueries({ queryKey: key });
      void qc.invalidateQueries({ queryKey: ["meeting", id] });
      void qc.invalidateQueries({ queryKey: ["meetings"] });
    }
  });
  const refresh = () => void qc.invalidateQueries({ queryKey: key });
  const regenerate = () =>
    analysisApi.regenerate(id).then(
      () => setProgress({ meetingId: id, status: "pending", message: "Queued…", fraction: null }),
      (e) => toast.error(e.message),
    );

  const running = progress ? progress.status === "running" || progress.status === "pending" : analysis?.status === "running" || analysis?.status === "pending";

  if (meeting.summary.status === "processing" || meeting.summary.status === "paused") {
    return <p className="text-muted-foreground">Notes are written after the transcript is ready.</p>;
  }
  if (running) {
    return (
      <div className="flex items-center gap-2 py-6 text-muted-foreground" role="status">
        <Loader2 className="size-4 animate-spin" aria-hidden /> {progress?.message ?? "Writing meeting notes on this computer…"}
      </div>
    );
  }
  if (!analysis) {
    return (
      <div className="py-6">
        <p className="text-muted-foreground">No notes yet.</p>
        {meeting.segments.length > 0 && (
          <Button className="mt-3" size="sm" onClick={regenerate}>
            <Sparkles /> Write notes
          </Button>
        )}
      </div>
    );
  }
  if (analysis.status === "unavailable" || analysis.status === "failed") {
    return (
      <div className="flex items-start gap-3 rounded-lg border p-4" role="alert">
        <AlertTriangle className="mt-0.5 size-4 shrink-0 text-warning" aria-hidden />
        <div className="flex-1">
          <div className="font-medium">Summary unavailable</div>
          <p className="mt-0.5 text-muted-foreground">{analysis.error ?? "The local AI could not summarise this meeting."} The transcript is complete and saved.</p>
          <Button className="mt-3" size="sm" variant="outline" onClick={regenerate}>
            <RefreshCw /> Try summary again
          </Button>
        </div>
      </div>
    );
  }

  const visible = analysis.actionItems;
  return (
    <div>
      {analysis.stale && (
        <div className="mb-2 flex items-center gap-3 rounded-md bg-muted px-3 py-2 text-[13px]">
          <span className="flex-1">The transcript was edited after these notes were written.</span>
          <Button size="sm" variant="outline" onClick={regenerate}>
            <RefreshCw /> Regenerate
          </Button>
        </div>
      )}
      {actions && <div className="flex flex-wrap gap-1.5 pb-1">{actions}</div>}
      <Block title="Summary">
        <ul className="list-disc space-y-1 pl-5 selectable">
          {analysis.summary.map((s, i) => (
            <li key={i}>{s}</li>
          ))}
        </ul>
      </Block>
      {analysis.decisions.length > 0 && (
        <Block title="Decisions">
          <ul className="space-y-2">
            {analysis.decisions.map((d) => (
              <li key={d.id} className="flex gap-2">
                <Check className="mt-0.5 size-4 shrink-0 text-success" aria-hidden />
                <div>
                  <div className="selectable">{d.text}</div>
                  <Evidence refs={d.evidence} onJump={onJump} />
                </div>
              </li>
            ))}
          </ul>
        </Block>
      )}
      <Block title="Action items">
        {visible.length === 0 && <p className="text-muted-foreground">No action items were found.</p>}
        {visible.length > 0 && (
          <ul className="divide-y rounded-xl border bg-card/60 px-4">
            {visible.map((a) => (
              <ActionRow key={a.id} a={a} people={people} onJump={onJump} onChange={refresh} />
            ))}
          </ul>
        )}
        <form
          className="mt-2 flex gap-2"
          onSubmit={(e) => {
            e.preventDefault();
            if (!newAction.trim()) return;
            analysisApi.addAction(id, newAction).then(() => {
              setNewAction("");
              refresh();
            }, (err) => toast.error(err.message));
          }}
        >
          <Input className="h-8 rounded-lg" placeholder="Add an action item…" value={newAction} onChange={(e) => setNewAction(e.target.value)} aria-label="New action item" />
          <Button size="sm" variant="outline" type="submit" disabled={!newAction.trim()}>
            <Plus /> Add
          </Button>
        </form>
      </Block>
      {analysis.unresolvedQuestions.length > 0 && (
        <Block title="Open questions">
          <ul className="space-y-2">
            {analysis.unresolvedQuestions.map((q) => (
              <li key={q.id}>
                <div className="selectable">{q.text}</div>
                <Evidence refs={q.evidence} onJump={onJump} />
              </li>
            ))}
          </ul>
        </Block>
      )}
      <p className="pt-2 pb-6 text-xs text-muted-foreground">
        Written on this computer{analysis.modelId ? ` by ${analysis.modelId}` : ""}. Check names and dates before sharing.
      </p>
    </div>
  );
}
