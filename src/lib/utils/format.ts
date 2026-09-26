const GB = 1024 ** 3;
const MB = 1024 ** 2;

/** Human-readable size. Uses binary units but the familiar "GB" label. */
export function formatBytes(bytes: number | null | undefined, digits = 1): string {
  if (bytes == null || !Number.isFinite(bytes)) return "—";
  if (bytes >= GB) return `${(bytes / GB).toFixed(bytes >= 10 * GB ? 0 : digits)} GB`;
  if (bytes >= MB) return `${Math.round(bytes / MB)} MB`;
  return `${Math.max(1, Math.round(bytes / 1024))} KB`;
}

/** "1:02:03" / "4:05" for a millisecond duration. */
export function formatClock(ms: number): string {
  const total = Math.max(0, Math.floor(ms / 1000));
  const h = Math.floor(total / 3600);
  const m = Math.floor((total % 3600) / 60);
  const s = total % 60;
  const pad = (n: number) => n.toString().padStart(2, "0");
  return h > 0 ? `${h}:${pad(m)}:${pad(s)}` : `${m}:${pad(s)}`;
}

/** "47 min" / "1 h 5 min". */
export function formatDuration(ms: number | null | undefined): string {
  if (ms == null) return "—";
  const minutes = Math.max(1, Math.round(ms / 60000));
  if (minutes < 60) return `${minutes} min`;
  const h = Math.floor(minutes / 60);
  const m = minutes % 60;
  return m ? `${h} h ${m} min` : `${h} h`;
}

export function greeting(date = new Date()): string {
  const h = date.getHours();
  if (h < 12) return "Good morning";
  if (h < 18) return "Good afternoon";
  return "Good evening";
}

/** "Today" / "Yesterday" / "Mon 22 Sep". */
export function relativeDay(iso: string, now = new Date()): string {
  const d = new Date(iso);
  const startOf = (x: Date) => new Date(x.getFullYear(), x.getMonth(), x.getDate()).getTime();
  const diff = Math.round((startOf(now) - startOf(d)) / 86400000);
  if (diff === 0) return "Today";
  if (diff === 1) return "Yesterday";
  return d.toLocaleDateString(undefined, { weekday: "short", day: "numeric", month: "short" });
}

/** Group items by recency like a chat history: Today, Yesterday, Previous 7 days, Earlier. */
export function groupByRecency<T>(items: T[], date: (t: T) => string, now = new Date()): { label: string; items: T[] }[] {
  const startOf = (x: Date) => new Date(x.getFullYear(), x.getMonth(), x.getDate()).getTime();
  const today = startOf(now);
  const groups: Record<string, T[]> = { Today: [], Yesterday: [], "Previous 7 days": [], Earlier: [] };
  for (const it of items) {
    const days = Math.round((today - startOf(new Date(date(it)))) / 86400000);
    const key = days <= 0 ? "Today" : days === 1 ? "Yesterday" : days < 8 ? "Previous 7 days" : "Earlier";
    groups[key].push(it);
  }
  return Object.entries(groups)
    .filter(([, v]) => v.length > 0)
    .map(([label, v]) => ({ label, items: v }));
}
