import { useQuery } from "@tanstack/react-query";
import { CheckSquare, FileText, Gavel, ListTree, MessageSquareText, Search } from "lucide-react";
import { useEffect, useMemo, useRef, useState } from "react";
import { useNavigate } from "react-router";
import { Dialog, DialogContent, DialogTitle } from "@/components/ui/dialog";
import { searchApi } from "@/lib/api";
import { useMeetings } from "@/lib/api/queries";
import type { SearchHit } from "@/lib/types";
import { cn } from "@/lib/utils";
import { formatClock, relativeDay } from "@/lib/utils/format";

const kindIcon: Record<string, typeof FileText> = {
  title: FileText,
  summary: ListTree,
  decision: Gavel,
  action: CheckSquare,
  transcript: MessageSquareText,
};

const kindLabel: Record<string, string> = {
  title: "Meeting",
  summary: "Summary",
  decision: "Decision",
  action: "Action item",
  transcript: "Transcript",
};

function useDebounced<T>(value: T, ms: number): T {
  const [v, setV] = useState(value);
  useEffect(() => {
    const t = setTimeout(() => setV(value), ms);
    return () => clearTimeout(t);
  }, [value, ms]);
  return v;
}

function Highlight({ text, query }: { text: string; query: string }) {
  const i = text.toLowerCase().indexOf(query.toLowerCase());
  if (!query || i < 0) return <>{text}</>;
  // Trim long transcript lines around the match.
  const start = Math.max(0, i - 60);
  const prefix = start > 0 ? "…" : "";
  return (
    <>
      {prefix}
      {text.slice(start, i)}
      <mark className="rounded-sm bg-brand/25 text-foreground">{text.slice(i, i + query.length)}</mark>
      {text.slice(i + query.length)}
    </>
  );
}

export function SearchDialog({ open, onOpenChange }: { open: boolean; onOpenChange: (o: boolean) => void }) {
  const navigate = useNavigate();
  const [query, setQuery] = useState("");
  const [active, setActive] = useState(0);
  const q = useDebounced(query.trim(), 150);
  const { data: meetings = [] } = useMeetings();
  const { data: hits = [], isFetching } = useQuery({
    queryKey: ["search", q],
    queryFn: () => searchApi.meetings(q),
    enabled: open && q.length > 0,
    placeholderData: (prev) => prev,
  });
  const listRef = useRef<HTMLDivElement>(null);

  // Without a query, show recent meetings.
  const results: SearchHit[] = useMemo(() => {
    if (q) return hits;
    return meetings
      .filter((m) => m.status !== "recording" && m.status !== "interrupted")
      .slice(0, 8)
      .map((m) => ({ meetingId: m.id, meetingTitle: m.title, startedAt: m.startedAt, kind: "title", text: m.title, segmentId: null, startMs: null }));
  }, [q, hits, meetings]);

  useEffect(() => setActive(0), [q]);
  useEffect(() => {
    if (!open) setQuery("");
  }, [open]);
  useEffect(() => {
    listRef.current?.querySelector(`[data-index="${active}"]`)?.scrollIntoView({ block: "nearest" });
  }, [active]);

  const openHit = (h: SearchHit) => {
    onOpenChange(false);
    if (h.kind === "transcript" && h.segmentId) navigate(`/meetings/${h.meetingId}?tab=transcript&segment=${h.segmentId}`);
    else navigate(`/meetings/${h.meetingId}`);
  };

  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogContent className="top-[18%] translate-y-0 gap-0 overflow-hidden p-0 sm:max-w-xl" showCloseButton={false}>
        <DialogTitle className="sr-only">Search meetings</DialogTitle>
        <div className="flex items-center gap-2.5 border-b px-4">
          <Search className="size-4 shrink-0 text-muted-foreground" aria-hidden />
          <input
            autoFocus
            value={query}
            onChange={(e) => setQuery(e.target.value)}
            onKeyDown={(e) => {
              if (e.key === "ArrowDown") {
                e.preventDefault();
                setActive((a) => Math.min(a + 1, results.length - 1));
              } else if (e.key === "ArrowUp") {
                e.preventDefault();
                setActive((a) => Math.max(a - 1, 0));
              } else if (e.key === "Enter" && results[active]) {
                e.preventDefault();
                openHit(results[active]);
              }
            }}
            placeholder="Search meetings, notes and transcripts"
            aria-label="Search"
            role="combobox"
            aria-expanded
            aria-controls="search-results"
            className="selectable h-12 flex-1 bg-transparent text-[14px] outline-none placeholder:text-muted-foreground/70"
          />
          {isFetching && <span className="text-[11px] text-muted-foreground">Searching…</span>}
        </div>
        <div ref={listRef} id="search-results" role="listbox" className="scrollbar-thin max-h-[50vh] overflow-y-auto p-1.5">
          {!q && results.length > 0 && <div className="px-2.5 pt-1 pb-1.5 text-[11px] text-muted-foreground">Recent meetings</div>}
          {q && !isFetching && results.length === 0 && (
            <div className="px-3 py-8 text-center text-muted-foreground">No results for “{q}”.</div>
          )}
          {results.map((h, i) => {
            const Icon = kindIcon[h.kind] ?? FileText;
            return (
              <button
                key={`${h.kind}-${h.meetingId}-${h.segmentId ?? i}`}
                data-index={i}
                role="option"
                aria-selected={i === active}
                onMouseMove={() => setActive(i)}
                onClick={() => openHit(h)}
                className={cn("flex w-full items-start gap-3 rounded-lg px-2.5 py-2 text-left outline-none", i === active && "bg-accent")}
              >
                <Icon className="mt-0.5 size-4 shrink-0 text-muted-foreground" aria-hidden />
                <span className="min-w-0 flex-1">
                  <span className="block truncate text-[13px]">
                    {h.kind === "title" ? <Highlight text={h.text} query={q} /> : h.meetingTitle}
                  </span>
                  {h.kind !== "title" && (
                    <span className="line-clamp-2 text-xs text-muted-foreground">
                      <Highlight text={h.text} query={q} />
                    </span>
                  )}
                </span>
                <span className="shrink-0 text-right text-[11px] text-muted-foreground">
                  <span className="block">{kindLabel[h.kind]}</span>
                  <span className="block tabular-nums">
                    {h.startMs != null ? formatClock(h.startMs) : relativeDay(h.startedAt)}
                  </span>
                </span>
              </button>
            );
          })}
        </div>
      </DialogContent>
    </Dialog>
  );
}
