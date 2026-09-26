import { useQuery, useQueryClient } from "@tanstack/react-query";
import { Copy, Search, Scissors } from "lucide-react";
import { useEffect, useMemo, useRef, useState } from "react";
import { useSearchParams } from "react-router";
import { toast } from "sonner";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { Textarea } from "@/components/ui/textarea";
import { speakersApi } from "@/lib/api";
import { usePeople } from "@/lib/api/queries";
import type { MeetingDetail, Person, SpeakerCluster, TranscriptSegment } from "@/lib/types";
import { cn } from "@/lib/utils";
import { formatClock } from "@/lib/utils/format";
import { SpeakersBar } from "./SpeakersBar";

export function speakerLabel(seg: TranscriptSegment, clusters: SpeakerCluster[], people: Person[]): { name: string; tentative: boolean } {
  const person = seg.personId ? people.find((p) => p.id === seg.personId) : undefined;
  if (person) return { name: person.displayName, tentative: false };
  const cluster = clusters.find((c) => c.id === seg.speakerClusterId);
  if (cluster) {
    if (cluster.identity.type === "possible") {
      const p = people.find((x) => x.id === (cluster.identity as { personId: string }).personId);
      return { name: p ? `${cluster.label} (probably ${p.displayName})` : cluster.label, tentative: true };
    }
    return { name: cluster.label, tentative: false };
  }
  return { name: seg.source === "microphone" ? "You" : "Meeting audio", tentative: false };
}

function SegmentRow({
  seg,
  label,
  highlighted,
  query,
  onChanged,
}: {
  seg: TranscriptSegment;
  label: { name: string; tentative: boolean };
  highlighted: boolean;
  query: string;
  onChanged: () => void;
}) {
  const [mode, setMode] = useState<"view" | "edit" | "split">("view");
  const [draft, setDraft] = useState(seg.text);

  const save = async () => {
    if (draft.trim() === seg.text) return setMode("view");
    try {
      await speakersApi.editText(seg.id, draft);
      setMode("view");
      onChanged();
    } catch (e) {
      toast.error((e as Error).message);
    }
  };
  const split = async (at: number) => {
    try {
      await speakersApi.split(seg.id, at, null);
      setMode("view");
      onChanged();
      toast.success("Split into a new speaker. Assign them in the speaker list above.");
    } catch (e) {
      toast.error((e as Error).message);
    }
  };

  const text = useMemo(() => {
    if (!query) return seg.text;
    const i = seg.text.toLowerCase().indexOf(query.toLowerCase());
    if (i < 0) return seg.text;
    return (
      <>
        {seg.text.slice(0, i)}
        <mark className="rounded-sm bg-warning/40 text-foreground">{seg.text.slice(i, i + query.length)}</mark>
        {seg.text.slice(i + query.length)}
      </>
    );
  }, [seg.text, query]);

  return (
    <div
      id={`seg-${seg.id}`}
      className={cn(
        "group grid scroll-mt-28 grid-cols-[150px_1fr] gap-4 rounded-md px-2 py-1.5 transition-colors",
        highlighted && "bg-warning/15 ring-1 ring-warning/40",
      )}
    >
      <div className="text-xs leading-5">
        <div className={cn("truncate font-medium", label.tentative && "text-muted-foreground italic")} title={label.name}>
          {label.name}
        </div>
        <a href={`#seg-${seg.id}`} className="text-muted-foreground tabular-nums hover:text-foreground" onClick={(e) => e.preventDefault()}>
          {formatClock(seg.startMs)}
        </a>
      </div>
      <div className="min-w-0">
        {mode === "view" && (
          <div className="flex items-start gap-2">
            <p
              className="flex-1 cursor-text leading-5"
              onDoubleClick={() => {
                setDraft(seg.text);
                setMode("edit");
              }}
            >
              {text}
              {seg.edited && <span className="ml-1.5 text-[11px] text-muted-foreground">(edited)</span>}
            </p>
            <div className="flex shrink-0 gap-0.5 opacity-0 transition-opacity group-focus-within:opacity-100 group-hover:opacity-100">
              <Button size="sm" variant="ghost" className="h-6 px-2 text-xs" onClick={() => { setDraft(seg.text); setMode("edit"); }}>
                Edit
              </Button>
              {seg.words.length > 1 && (
                <Button size="icon" variant="ghost" className="size-6" aria-label="Split segment" onClick={() => setMode("split")}>
                  <Scissors className="size-3.5" />
                </Button>
              )}
            </div>
          </div>
        )}
        {mode === "edit" && (
          <div className="space-y-2">
            <Textarea
              autoFocus
              value={draft}
              onChange={(e) => setDraft(e.target.value)}
              onKeyDown={(e) => {
                if (e.key === "Escape") setMode("view");
                if (e.key === "Enter" && (e.metaKey || e.ctrlKey)) void save();
              }}
              aria-label="Correct transcript text"
            />
            <div className="flex gap-2">
              <Button size="sm" onClick={save}>Save</Button>
              <Button size="sm" variant="ghost" onClick={() => setMode("view")}>Cancel</Button>
            </div>
          </div>
        )}
        {mode === "split" && (
          <div>
            <p className="mb-1.5 text-xs text-muted-foreground">Click the first word spoken by the other person.</p>
            <p className="leading-6">
              {seg.words.map((w, i) => (
                <button
                  key={i}
                  disabled={i === 0}
                  className="mr-1 rounded px-0.5 hover:bg-primary hover:text-primary-foreground disabled:hover:bg-transparent disabled:hover:text-inherit"
                  onClick={() => split(i)}
                >
                  {w.text}
                </button>
              ))}
            </p>
            <Button size="sm" variant="ghost" className="mt-1" onClick={() => setMode("view")}>Cancel</Button>
          </div>
        )}
      </div>
    </div>
  );
}

export function TranscriptView({ meeting }: { meeting: MeetingDetail }) {
  const qc = useQueryClient();
  const id = meeting.summary.id;
  const { data: people = [] } = usePeople();
  const { data: clusters = [] } = useQuery({ queryKey: ["speakers", id], queryFn: () => speakersApi.list(id) });
  const [query, setQuery] = useState("");
  const [params] = useSearchParams();
  const focus = params.get("segment");
  const [highlight, setHighlight] = useState<string | null>(focus);
  const matchIndex = useRef(0);

  useEffect(() => {
    if (!focus) return;
    setHighlight(focus);
    requestAnimationFrame(() => document.getElementById(`seg-${focus}`)?.scrollIntoView({ block: "center", behavior: "smooth" }));
    const t = setTimeout(() => setHighlight(null), 2500);
    return () => clearTimeout(t);
  }, [focus, meeting.segments.length]);

  const refresh = () => {
    void qc.invalidateQueries({ queryKey: ["meeting", id] });
    void qc.invalidateQueries({ queryKey: ["speakers", id] });
    void qc.invalidateQueries({ queryKey: ["meetings"] });
  };

  const segs = meeting.segments;
  const matches = query ? segs.filter((s) => s.text.toLowerCase().includes(query.toLowerCase())) : [];

  const copy = async () => {
    const text = segs
      .map((s) => `${speakerLabel(s, clusters, people).name} · ${formatClock(s.startMs)}\n${s.text}`)
      .join("\n\n");
    await navigator.clipboard.writeText(`${meeting.summary.title}\n\n${text}\n`);
    toast.success("Transcript copied");
  };

  const jumpNext = () => {
    if (!matches.length) return;
    const m = matches[matchIndex.current % matches.length];
    matchIndex.current += 1;
    setHighlight(m.id);
    document.getElementById(`seg-${m.id}`)?.scrollIntoView({ block: "center", behavior: "smooth" });
  };

  if (segs.length === 0) {
    return (
      <p className="text-muted-foreground">
        {meeting.summary.status === "processing" ? "The transcript will appear here shortly." : "No speech was transcribed in this meeting."}
      </p>
    );
  }

  return (
    <div>
      <SpeakersBar meeting={meeting} clusters={clusters} people={people} onChanged={refresh} />
      <div className="sticky top-0 z-10 -mx-2 mb-3 flex items-center gap-2 bg-background/95 px-2 py-2 backdrop-blur">
        <div className="relative flex-1">
          <Search className="absolute top-1/2 left-2.5 size-3.5 -translate-y-1/2 text-muted-foreground" aria-hidden />
          <Input
            value={query}
            onChange={(e) => {
              setQuery(e.target.value);
              matchIndex.current = 0;
            }}
            onKeyDown={(e) => e.key === "Enter" && jumpNext()}
            placeholder="Search transcript"
            aria-label="Search transcript"
            className="h-8 pl-8"
          />
        </div>
        {query && <span className="text-xs text-muted-foreground tabular-nums">{matches.length} found</span>}
        <Button size="sm" variant="ghost" onClick={copy}>
          <Copy /> Copy
        </Button>
      </div>
      <div className="selectable space-y-1">
        {segs.map((s) => (
          <SegmentRow
            key={`${s.id}-${s.text}`}
            seg={s}
            label={speakerLabel(s, clusters, people)}
            highlighted={highlight === s.id}
            query={query}
            onChanged={refresh}
          />
        ))}
      </div>
    </div>
  );
}
