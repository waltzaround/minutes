import { cn } from "@/lib/utils";

/** Five-bar meter. `level` is 0..1 (already log-scaled by the backend). */
export function LevelMeter({ level, disabled }: { level: number; disabled?: boolean }) {
  const bars = [0.12, 0.3, 0.5, 0.7, 0.88];
  return (
    <div className="flex h-5 items-end gap-[3px]" aria-hidden>
      {bars.map((threshold, i) => (
        <span
          key={i}
          className={cn(
            "w-[4px] rounded-full transition-[height,background-color] duration-75",
            disabled ? "bg-muted-foreground/25" : level >= threshold ? "bg-success" : "bg-foreground/15",
          )}
          style={{ height: `${30 + i * 17}%` }}
        />
      ))}
    </div>
  );
}
