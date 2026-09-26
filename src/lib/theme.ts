import { useEffect, useSyncExternalStore } from "react";
import type { ThemePreference } from "@/lib/types";

const query = () => window.matchMedia("(prefers-color-scheme: dark)");

function subscribe(cb: () => void) {
  const mq = query();
  mq.addEventListener("change", cb);
  return () => mq.removeEventListener("change", cb);
}

export function useSystemDark(): boolean {
  return useSyncExternalStore(subscribe, () => query().matches, () => false);
}

export function useResolvedTheme(pref: ThemePreference | undefined): "light" | "dark" {
  const systemDark = useSystemDark();
  if (pref === "light") return "light";
  if (pref === "dark") return "dark";
  return systemDark ? "dark" : "light";
}

/** Apply the theme class to <html>. */
export function useApplyTheme(pref: ThemePreference | undefined) {
  const resolved = useResolvedTheme(pref);
  useEffect(() => {
    document.documentElement.classList.toggle("dark", resolved === "dark");
    document.documentElement.style.colorScheme = resolved;
  }, [resolved]);
  return resolved;
}
