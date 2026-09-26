import { StrictMode } from "react";
import { createRoot } from "react-dom/client";
import { App } from "./App";
import "@fontsource-variable/kode-mono/wght.css";
import "./index.css";

async function boot() {
  const mock = new URLSearchParams(location.search).get("mock");
  if (import.meta.env.DEV && mock !== null) {
    const { installMockBackend } = await import("./dev/mockBackend");
    installMockBackend(mock || "mac-m1-16gb");
  }
  createRoot(document.getElementById("root")!).render(
    <StrictMode>
      <App />
    </StrictMode>,
  );
}

void boot();
