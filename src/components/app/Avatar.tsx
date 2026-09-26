import { cn } from "@/lib/utils";

function hue(name: string): number {
  let h = 0;
  for (const c of name) h = (h * 31 + c.charCodeAt(0)) % 360;
  return h;
}

export function initials(name: string): string {
  return name
    .split(/\s+/)
    .filter(Boolean)
    .map((w) => w[0])
    .slice(0, 2)
    .join("")
    .toUpperCase();
}

/** Initials avatar with a stable, muted colour derived from the name. */
export function Avatar({ name, size = "md", className }: { name: string; size?: "sm" | "md" | "lg"; className?: string }) {
  const h = hue(name);
  return (
    <span
      aria-hidden
      className={cn(
        "inline-flex shrink-0 items-center justify-center rounded-full font-medium select-none",
        size === "sm" && "size-6 text-[10px]",
        size === "md" && "size-8 text-[11px]",
        size === "lg" && "size-10 text-[13px]",
        "bg-[oklch(0.6_0.09_var(--h)/0.18)] text-[oklch(0.45_0.11_var(--h))] dark:bg-[oklch(0.55_0.09_var(--h)/0.22)] dark:text-[oklch(0.8_0.1_var(--h))]",
        className,
      )}
      style={{ "--h": h } as React.CSSProperties}
    >
      {initials(name) || "?"}
    </span>
  );
}
