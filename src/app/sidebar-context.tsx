import { createContext, useContext } from "react";

export const isMac = typeof navigator !== "undefined" && /Mac/i.test(navigator.platform);

export interface SidebarState {
  open: boolean;
  toggle: () => void;
}

export const SidebarContext = createContext<SidebarState>({ open: true, toggle: () => {} });

export function useSidebar() {
  return useContext(SidebarContext);
}
