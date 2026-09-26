import type { ReactNode } from "react";

export function FullPageMessage({ title, children }: { title: string; children: ReactNode }) {
  return (
    <div data-tauri-drag-region className="flex h-full items-center justify-center p-8">
      <div className="max-w-sm text-center">
        <h1 className="text-base font-semibold">{title}</h1>
        <p className="mt-2 text-muted-foreground">{children}</p>
      </div>
    </div>
  );
}
