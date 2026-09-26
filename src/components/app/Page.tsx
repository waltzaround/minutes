import type { ReactNode } from "react";
import { cn } from "@/lib/utils";
import { isMac, useSidebar } from "@/app/sidebar-context";

/** Leaves room for the traffic lights and the "show sidebar" button when the sidebar is hidden. */
export function useHeaderInset(): string {
  const { open } = useSidebar();
  if (open) return "";
  return isMac ? "pl-[120px]" : "pl-12";
}

export function PageHeader({ title, actions, subtitle, leading }: { title?: ReactNode; subtitle?: ReactNode; actions?: ReactNode; leading?: ReactNode }) {
  const inset = useHeaderInset();
  return (
    <header data-tauri-drag-region className={cn("flex h-12 shrink-0 items-center gap-3 px-5", inset)}>
      {leading}
      <div className="flex min-w-0 flex-1 items-baseline gap-3" data-tauri-drag-region>
        {title && <div className="truncate text-[13px] font-semibold">{title}</div>}
        {subtitle && <div className="truncate text-xs text-muted-foreground">{subtitle}</div>}
      </div>
      {actions && <div className="flex shrink-0 items-center gap-1.5">{actions}</div>}
    </header>
  );
}

export function PageBody({ children, className }: { children: ReactNode; className?: string }) {
  return <div className={cn("scrollbar-thin min-h-0 flex-1 overflow-y-auto", className)}>{children}</div>;
}

export function Section({ title, description, children, className, plain }: {
  title?: ReactNode;
  description?: ReactNode;
  children: ReactNode;
  className?: string;
  /** Render children without the card container. */
  plain?: boolean;
}) {
  return (
    <section className={cn("py-4", className)}>
      {title && <h2 className="px-1 text-[13px] font-semibold">{title}</h2>}
      {description && <p className="mt-1 px-1 text-muted-foreground">{description}</p>}
      <div className={cn(title || description ? "mt-2.5" : undefined, !plain && "rounded-xl border bg-card/60 px-4 py-1")}>{children}</div>
    </section>
  );
}

/** A bordered card group, like grouped settings rows. */
export function Panel({ children, className }: { children: ReactNode; className?: string }) {
  return <div className={cn("rounded-xl border bg-card", className)}>{children}</div>;
}

export function KeyValue({ rows }: { rows: [ReactNode, ReactNode][] }) {
  return (
    <dl className="grid grid-cols-[minmax(140px,max-content)_1fr] gap-x-6 gap-y-1.5 py-3">
      {rows.map(([k, v], i) => (
        <div key={i} className="contents">
          <dt className="text-muted-foreground">{k}</dt>
          <dd className="selectable min-w-0 break-words">{v}</dd>
        </div>
      ))}
    </dl>
  );
}
