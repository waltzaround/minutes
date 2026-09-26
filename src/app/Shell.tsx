import { PanelLeft } from "lucide-react";
import { useCallback, useEffect, useMemo, useState } from "react";
import { Outlet } from "react-router";
import { Button } from "@/components/ui/button";
import { SimulationBanner } from "@/components/app/SimulationBanner";
import { GlobalEffects } from "./GlobalEffects";
import { Sidebar } from "./Sidebar";
import { isMac, SidebarContext } from "./sidebar-context";

const KEY = "minutes.sidebar";

function readOpen(): boolean {
  try {
    return localStorage.getItem(KEY) !== "closed";
  } catch {
    return true;
  }
}

export function Shell() {
  const [open, setOpen] = useState(readOpen);
  const toggle = useCallback(() => {
    setOpen((o) => {
      try {
        localStorage.setItem(KEY, o ? "closed" : "open");
      } catch {
        /* per-viewer convenience only */
      }
      return !o;
    });
  }, []);

  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if ((isMac ? e.metaKey : e.ctrlKey) && e.key.toLowerCase() === "b") {
        e.preventDefault();
        toggle();
      }
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [toggle]);

  const ctx = useMemo(() => ({ open, toggle }), [open, toggle]);
  return (
    <SidebarContext.Provider value={ctx}>
      <div className="flex h-full">
        <GlobalEffects />
        {open && <Sidebar />}
        <main className="relative flex min-w-0 flex-1 flex-col">
          {!open && (
            <Button
              size="icon"
              variant="ghost"
              className={`absolute top-2.5 z-20 size-7 text-muted-foreground ${isMac ? "left-[84px]" : "left-2"}`}
              onClick={toggle}
              aria-label="Show sidebar"
            >
              <PanelLeft className="size-4" />
            </Button>
          )}
          <SimulationBanner />
          <Outlet />
        </main>
      </div>
    </SidebarContext.Provider>
  );
}
