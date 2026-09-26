import { useQueryClient } from "@tanstack/react-query";
import { openUrl } from "@tauri-apps/plugin-opener";
import { ExternalLink } from "lucide-react";
import { useEffect, useState } from "react";
import { toast } from "sonner";
import { Button } from "@/components/ui/button";
import { Dialog, DialogContent, DialogDescription, DialogFooter, DialogHeader, DialogTitle } from "@/components/ui/dialog";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import { Select, SelectContent, SelectItem, SelectTrigger, SelectValue } from "@/components/ui/select";
import { Sheet, SheetContent, SheetDescription, SheetFooter, SheetHeader, SheetTitle } from "@/components/ui/sheet";
import { Textarea } from "@/components/ui/textarea";
import { integrationsApi } from "@/lib/api";
import { useIntegrations, useSettings } from "@/lib/api/queries";
import type { CreateIssueResult, IssueDraft, IssueProposal, LinearDirectory, MeetingDetail } from "@/lib/types";
import { useLinearDirectory } from "./LinearSettings";

const PRIORITIES: [number, string][] = [
  [0, "No priority"],
  [1, "Urgent"],
  [2, "High"],
  [3, "Medium"],
  [4, "Low"],
];

export function MeetingIntegrations({ meeting }: { meeting: MeetingDetail }) {
  const { data: status } = useIntegrations();
  const { data: settings } = useSettings();
  const qc = useQueryClient();
  const [notionBusy, setNotionBusy] = useState(false);
  const [askNotion, setAskNotion] = useState(false);
  const [linearOpen, setLinearOpen] = useState(false);
  if (!status || !settings) return null;

  const sendNotion = async (includeTranscript: boolean) => {
    setAskNotion(false);
    setNotionBusy(true);
    const t = toast.loading("Sending to Notion…");
    try {
      const page = await integrationsApi.syncToNotion(meeting.summary.id, includeTranscript);
      toast.success("Sent to Notion", { id: t, action: { label: "Open", onClick: () => void openUrl(page.url) } });
      void qc.invalidateQueries({ queryKey: ["meeting", meeting.summary.id] });
    } catch (e) {
      toast.error(`${(e as Error).message} Your notes are saved here; you can try again.`, { id: t });
    } finally {
      setNotionBusy(false);
    }
  };
  const onNotion = () => {
    const mode = settings.notion.uploadMode;
    if (mode === "askEachTime") setAskNotion(true);
    else void sendNotion(mode === "summaryAndTranscript");
  };

  const notionReady = status.notion.connected && !!settings.notion.dataSourceId;
  return (
    <>
      {status.notion.connected && (
        <Button size="sm" variant="outline" className="h-7 rounded-full text-xs" onClick={onNotion} disabled={notionBusy || !notionReady} title={notionReady ? undefined : "Choose a Notion database in Settings → Notion"}>
          {meeting.notionPageUrl ? "Update in Notion" : "Send to Notion"}
        </Button>
      )}
      {meeting.notionPageUrl && (
        <Button size="sm" variant="ghost" className="h-7 rounded-full text-xs" onClick={() => void openUrl(meeting.notionPageUrl!)}>
          <ExternalLink /> Open in Notion
        </Button>
      )}
      {status.linear.connected && (
        <Button size="sm" variant="outline" className="h-7 rounded-full text-xs" onClick={() => setLinearOpen(true)}>
          Create selected Linear issues
        </Button>
      )}
      <Dialog open={askNotion} onOpenChange={setAskNotion}>
        <DialogContent className="sm:max-w-sm">
          <DialogHeader>
            <DialogTitle>Send to Notion</DialogTitle>
            <DialogDescription>Include the full transcript on the Notion page?</DialogDescription>
          </DialogHeader>
          <DialogFooter>
            <Button variant="outline" onClick={() => sendNotion(false)}>Summary + actions only</Button>
            <Button onClick={() => sendNotion(true)}>Include transcript</Button>
          </DialogFooter>
        </DialogContent>
      </Dialog>
      {linearOpen && <LinearReviewSheet meetingId={meeting.summary.id} onClose={() => setLinearOpen(false)} />}
    </>
  );
}

function IssueCard({
  proposal,
  dir,
  result,
  busy,
  onChange,
  onCreate,
}: {
  proposal: IssueProposal;
  dir: LinearDirectory;
  result?: CreateIssueResult;
  busy: boolean;
  onChange: (d: IssueDraft) => void;
  onCreate: () => void;
}) {
  const d = proposal.draft;
  const NONE = "__none";
  const created = result?.state === "created";
  const projects = dir.projects.filter((p) => p.teamIds.length === 0 || p.teamIds.includes(d.teamId));
  return (
    <div className="space-y-3 rounded-lg border p-4">
      <div className="space-y-1">
        <Label htmlFor={`t-${proposal.actionItemId}`}>Title</Label>
        <Input id={`t-${proposal.actionItemId}`} value={d.title} disabled={created} onChange={(e) => onChange({ ...d, title: e.target.value })} />
      </div>
      <div className="grid grid-cols-2 gap-3">
        <div className="space-y-1">
          <Label>Assignee</Label>
          <Select value={d.assigneeId ?? NONE} disabled={created} onValueChange={(v) => onChange({ ...d, assigneeId: v === NONE ? null : v })}>
            <SelectTrigger className="w-full"><SelectValue /></SelectTrigger>
            <SelectContent>
              <SelectItem value={NONE}>Unassigned</SelectItem>
              {dir.users.map((u) => <SelectItem key={u.id} value={u.id}>{u.name}</SelectItem>)}
            </SelectContent>
          </Select>
        </div>
        <div className="space-y-1">
          <Label>Team</Label>
          <Select value={d.teamId || NONE} disabled={created} onValueChange={(v) => onChange({ ...d, teamId: v === NONE ? "" : v, projectId: null })}>
            <SelectTrigger className="w-full"><SelectValue placeholder="Choose a team" /></SelectTrigger>
            <SelectContent>
              <SelectItem value={NONE}>Choose a team</SelectItem>
              {dir.teams.map((t) => <SelectItem key={t.id} value={t.id}>{t.name}</SelectItem>)}
            </SelectContent>
          </Select>
        </div>
        <div className="space-y-1">
          <Label>Project</Label>
          <Select value={d.projectId ?? NONE} disabled={created} onValueChange={(v) => onChange({ ...d, projectId: v === NONE ? null : v })}>
            <SelectTrigger className="w-full"><SelectValue /></SelectTrigger>
            <SelectContent>
              <SelectItem value={NONE}>No project</SelectItem>
              {projects.map((p) => <SelectItem key={p.id} value={p.id}>{p.name}</SelectItem>)}
            </SelectContent>
          </Select>
        </div>
        <div className="space-y-1">
          <Label>Priority</Label>
          <Select value={String(d.priority ?? 0)} disabled={created} onValueChange={(v) => onChange({ ...d, priority: Number(v) })}>
            <SelectTrigger className="w-full"><SelectValue /></SelectTrigger>
            <SelectContent>
              {PRIORITIES.map(([v, l]) => <SelectItem key={v} value={String(v)}>{l}</SelectItem>)}
            </SelectContent>
          </Select>
        </div>
        <div className="space-y-1">
          <Label htmlFor={`due-${proposal.actionItemId}`}>Due</Label>
          <Input id={`due-${proposal.actionItemId}`} type="date" disabled={created} value={d.dueDate ?? ""} onChange={(e) => onChange({ ...d, dueDate: e.target.value || null })} />
        </div>
      </div>
      <details>
        <summary className="cursor-pointer text-xs text-muted-foreground">Description</summary>
        <Textarea className="mt-2 font-mono text-xs" rows={8} disabled={created} value={d.description} onChange={(e) => onChange({ ...d, description: e.target.value })} />
      </details>
      {proposal.notes.map((n) => <p key={n} className="text-xs text-muted-foreground">{n}</p>)}
      <div className="flex items-center gap-3">
        {created && result?.issue ? (
          <Button size="sm" variant="ghost" onClick={() => void openUrl(result.issue!.url)}>
            <ExternalLink /> {result.issue.identifier} created
          </Button>
        ) : (
          <Button size="sm" onClick={onCreate} disabled={busy || !d.teamId || !d.title.trim()}>
            {result ? "Try again" : "Create issue"}
          </Button>
        )}
        {result && !created && (
          <span className={result.state === "unknown" ? "text-xs text-warning" : "text-xs text-destructive"}>
            {result.state === "unknown" ? "We couldn't confirm whether it was created. Trying again is safe and won't create a duplicate." : result.error}
          </span>
        )}
      </div>
    </div>
  );
}

function LinearReviewSheet({ meetingId, onClose }: { meetingId: string; onClose: () => void }) {
  const qc = useQueryClient();
  const dir = useLinearDirectory(true);
  const [proposals, setProposals] = useState<IssueProposal[] | null>(null);
  const [results, setResults] = useState<Record<string, CreateIssueResult>>({});
  const [busy, setBusy] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    integrationsApi.prepareLinear(meetingId).then(setProposals, (e) => setError(e.message));
  }, [meetingId]);

  const create = async (p: IssueProposal) => {
    setBusy(p.actionItemId);
    try {
      const r = await integrationsApi.createLinearIssue(p.actionItemId, p.draft);
      setResults((prev) => ({ ...prev, [p.actionItemId]: r }));
    } catch (e) {
      setResults((prev) => ({ ...prev, [p.actionItemId]: { state: "failed", issue: null, error: (e as Error).message } }));
    } finally {
      setBusy(null);
      void qc.invalidateQueries({ queryKey: ["analysis", meetingId] });
    }
  };
  const createAll = async () => {
    for (const p of proposals ?? []) {
      if (results[p.actionItemId]?.state === "created" || !p.draft.teamId) continue;
      await create(p);
    }
  };

  const pending = (proposals ?? []).filter((p) => results[p.actionItemId]?.state !== "created");
  return (
    <Sheet open onOpenChange={(o) => !o && onClose()}>
      <SheetContent className="w-full overflow-y-auto sm:max-w-xl">
        <SheetHeader>
          <SheetTitle>Review Linear issues</SheetTitle>
          <SheetDescription>Check each issue before it’s created. Only the selected action items are listed.</SheetDescription>
        </SheetHeader>
        <div className="space-y-4 px-4">
          {error && <p className="text-destructive">{error}</p>}
          {dir.error && <p className="text-destructive">{dir.error.message}</p>}
          {proposals && proposals.length === 0 && <p className="text-muted-foreground">No selected action items. Tick the actions you want to send first.</p>}
          {proposals && dir.data &&
            proposals.map((p, i) => (
              <IssueCard
                key={p.actionItemId}
                proposal={p}
                dir={dir.data!}
                result={results[p.actionItemId]}
                busy={busy !== null}
                onChange={(d) => setProposals((prev) => prev!.map((x, j) => (j === i ? { ...x, draft: d } : x)))}
                onCreate={() => create(p)}
              />
            ))}
        </div>
        <SheetFooter>
          {pending.length > 1 && (
            <Button onClick={createAll} disabled={busy !== null || pending.some((p) => !p.draft.teamId)}>
              Create {pending.length} issues
            </Button>
          )}
          <Button variant="ghost" onClick={onClose}>Done</Button>
        </SheetFooter>
      </SheetContent>
    </Sheet>
  );
}
