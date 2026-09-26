import type { ReactNode } from "react";

/** A labelled settings row: label + help on the left, control on the right. */
export function Field({ label, help, htmlFor, children }: { label: ReactNode; help?: ReactNode; htmlFor?: string; children: ReactNode }) {
  return (
    <div className="flex items-start justify-between gap-8 border-b py-3.5 last:border-0">
      <div className="min-w-0">
        <label htmlFor={htmlFor} className="font-medium">
          {label}
        </label>
        {help && <p className="mt-0.5 text-muted-foreground">{help}</p>}
      </div>
      <div className="shrink-0">{children}</div>
    </div>
  );
}
