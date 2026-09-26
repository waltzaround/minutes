import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { RouterProvider } from "react-router";
import { TooltipProvider } from "@/components/ui/tooltip";
import { Toaster } from "@/components/ui/sonner";
import { router } from "@/app/router";
import { useSettings } from "@/lib/api/queries";
import { useApplyTheme } from "@/lib/theme";

const queryClient = new QueryClient({
  defaultOptions: { queries: { retry: 1, refetchOnWindowFocus: false } },
});

function Themed() {
  const { data } = useSettings();
  const theme = useApplyTheme(data?.general.theme);
  return (
    <>
      <RouterProvider router={router} />
      <Toaster theme={theme} position="bottom-right" />
    </>
  );
}

export function App() {
  return (
    <QueryClientProvider client={queryClient}>
      <TooltipProvider delayDuration={400}>
        <Themed />
      </TooltipProvider>
    </QueryClientProvider>
  );
}
