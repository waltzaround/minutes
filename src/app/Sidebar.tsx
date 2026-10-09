import { useQueryClient } from "@tanstack/react-query";
import { AlertCircle, FileText, Loader2, PanelLeft, Search, Settings, SquarePen, Users } from "lucide-react";
import { useRecordingElapsed } from "@/features/recording/useRecordingElapsed";
import { SearchDialog } from "@/features/search/SearchDialog";
import { useEffect, useState } from "react";
import { NavLink, useNavigate } from "react-router";
import { toast } from "sonner";
import { Button } from "@/components/ui/button";
import { Tooltip, TooltipContent, TooltipTrigger } from "@/components/ui/tooltip";
import { meetingsApi } from "@/lib/api";
import { useMeetings, useRecordingStatus } from "@/lib/api/queries";
import { cn } from "@/lib/utils";
import { formatClock, formatDuration, groupByRecency } from "@/lib/utils/format";
import { isMac, useSidebar } from "./sidebar-context";

const itemClass = (active: boolean) =>
  cn(
    "flex h-8 items-center gap-2.5 rounded-lg px-2.5 text-[13px] outline-none transition-colors",
    "text-muted-foreground hover:bg-accent/70 hover:text-foreground focus-visible:ring-2 focus-visible:ring-ring",
    active && "bg-accent text-foreground",
  );

function RecordingRow() {
  const { data: rec } = useRecordingStatus();
  const elapsed = useRecordingElapsed(rec?.startedAt, rec?.elapsedMs ?? 0);
  if (!rec) return null;
  return (
    <NavLink
      to="/recording"
      className={({ isActive }) =>
        cn(
          "flex h-9 items-center gap-2.5 rounded-lg px-2.5 text-[13px] font-medium text-recording outline-none",
          "bg-recording/10 hover:bg-recording/15 focus-visible:ring-2 focus-visible:ring-ring",
          isActive && "bg-recording/15",
        )
      }
    >
      <span className="relative flex size-2">
        <span className="absolute inline-flex size-full animate-ping rounded-full bg-recording opacity-60" />
        <span className="relative inline-flex size-2 rounded-full bg-recording" />
      </span>
      <span className="flex-1 truncate">Recording</span>
      <span className="font-mono text-xs tabular-nums">{formatClock(elapsed)}</span>
    </NavLink>
  );
}

export function Sidebar() {
  const { toggle } = useSidebar();
  const { data: meetings = [] } = useMeetings();
  const { data: recording } = useRecordingStatus();
  const visible = meetings.filter((m) => m.status !== "recording" && m.status !== "interrupted");
  const groups = groupByRecency(visible, (m) => m.startedAt);

  const [searchOpen, setSearchOpen] = useState(false);
  const newMeeting = () => {
    // Focus the name field on the new-meeting page.
    requestAnimationFrame(() => document.getElementById("meeting-title")?.focus());
  };

  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if ((isMac ? e.metaKey : e.ctrlKey) && e.key.toLowerCase() === "k") {
        e.preventDefault();
        setSearchOpen((o) => !o);
      }
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, []);

  return (
    <aside className="flex w-64 shrink-0 flex-col border-r border-sidebar-border bg-sidebar" aria-label="Sidebar">
      <div data-tauri-drag-region className={cn("flex h-12 shrink-0 items-center justify-end px-2", !isMac && "justify-between")}>
        {!isMac && <span className="pl-2 text-[13px] font-semibold">Minutes</span>}
        <Tooltip>
          <TooltipTrigger asChild>
            <Button size="icon" variant="ghost" className="size-7 text-muted-foreground" onClick={toggle} aria-label="Hide sidebar">
              <PanelLeft className="size-4" />
            </Button>
          </TooltipTrigger>
          <TooltipContent side="right">Hide sidebar · {isMac ? "⌘" : "Ctrl+"}B</TooltipContent>
        </Tooltip>
      </div>

      <div className="flex flex-col gap-0.5 px-2">
        <RecordingRow />
        {!recording && (
          <NavLink to="/" end onClick={newMeeting} className={({ isActive }) => cn(itemClass(isActive), !isActive && "text-foreground")}>
            <SquarePen className="size-4" aria-hidden />
            <span className="flex-1 text-left">New meeting</span>
            <kbd className="font-sans text-[11px] text-muted-foreground/70">{isMac ? "⇧⌘R" : "Ctrl+Shift+R"}</kbd>
          </NavLink>
        )}
        <button onClick={() => setSearchOpen(true)} className={itemClass(false)}>
          <Search className="size-4" aria-hidden />
          <span className="flex-1 text-left">Search</span>
          <kbd className="font-sans text-[11px] text-muted-foreground/70">{isMac ? "⌘K" : "Ctrl+K"}</kbd>
        </button>
        <NavLink to="/people" className={({ isActive }) => itemClass(isActive)}>
          <Users className="size-4" aria-hidden />
          People
        </NavLink>
        <NavLink to="/settings" className={({ isActive }) => itemClass(isActive)}>
          <Settings className="size-4" aria-hidden />
          Settings
        </NavLink>
      </div>

      <nav className="scrollbar-thin mt-4 min-h-0 flex-1 overflow-y-auto px-2 pb-2" aria-label="Meetings">
        <div className="flex items-center justify-between px-2.5 pb-1.5">
          <span className="text-[11px] font-semibold tracking-wide text-muted-foreground uppercase">Meetings</span>
          {visible.length > 0 && <span className="text-[11px] text-muted-foreground/70 tabular-nums">{visible.length}</span>}
        </div>
        {groups.length === 0 && (
          <p className="px-2.5 py-2 text-xs text-muted-foreground">Your meetings will appear here.</p>
        )}
        {groups.map((g) => (
          <section key={g.label} className="mb-2" aria-label={g.label}>
            <div className="px-2.5 pt-1.5 pb-1 text-[11px] text-muted-foreground/70">{g.label}</div>
            <ul className="space-y-px">
              {g.items.map((m) => {
                const time = new Date(m.startedAt).toLocaleTimeString(undefined, { hour: "numeric", minute: "2-digit" });
                const meta = [time, m.durationMs != null ? formatDuration(m.durationMs) : null, m.actionCount > 0 ? `${m.actionCount} action${m.actionCount === 1 ? "" : "s"}` : null]
                  .filter(Boolean)
                  .join(" · ");
                return (
                  <li key={m.id}>
                    <NavLink
                      to={`/meetings/${m.id}`}
                      title={m.title}
                      className={({ isActive }) =>
                        cn(
                          "group flex items-start gap-2.5 rounded-lg px-2.5 py-1.5 outline-none transition-colors",
                          "hover:bg-accent/70 focus-visible:ring-2 focus-visible:ring-ring",
                          isActive && "bg-accent",
                        )
                      }
                    >
                      <FileText className="mt-0.5 size-3.5 shrink-0 text-muted-foreground" aria-hidden />
                      <span className="min-w-0 flex-1">
                        <span className="block truncate text-[13px] text-foreground/90">{m.title}</span>
                        <span className="block truncate text-[11px] text-muted-foreground">{meta}</span>
                      </span>
                      {m.status === "paused" && <span className="text-[11px] text-muted-foreground">Paused</span>}
                      {m.status === "processing" && <Loader2 className="mt-0.5 size-3 shrink-0 animate-spin text-muted-foreground" aria-label="Processing" />}
                      {m.status === "failed" && <AlertCircle className="mt-0.5 size-3.5 shrink-0 text-destructive" aria-label="Needs attention" />}
                    </NavLink>
                  </li>
                );
              })}
            </ul>
          </section>
        ))}
      </nav>
      <SearchDialog open={searchOpen} onOpenChange={setSearchOpen} />
    </aside>
  );
}

export function useStartMeeting() {
  const qc = useQueryClient();
  const navigate = useNavigate();
  return async (title?: string) => {
    try {
      const s = await meetingsApi.start(title);
      qc.setQueryData(["recording"], s);
      navigate("/recording");
    } catch (e) {
      toast.error((e as Error).message);
    }
  };
}
