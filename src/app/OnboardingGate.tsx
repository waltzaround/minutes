import type { ReactNode } from "react";
import { Navigate } from "react-router";
import { useSettings } from "@/lib/api/queries";
import { isTauri } from "@/lib/api";
import { FullPageMessage } from "@/components/app/FullPageMessage";

/** Sends first-run users to onboarding before anything else. */
export function OnboardingGate({ children }: { children: ReactNode }) {
  const { data, isLoading, error } = useSettings();
  if (!isTauri()) {
    return (
      <FullPageMessage title="Open Minutes from the app">
        This page is the Minutes desktop interface and needs the Minutes app to run.
      </FullPageMessage>
    );
  }
  if (isLoading) return null;
  if (error) {
    return <FullPageMessage title="Minutes could not start">{error.message}</FullPageMessage>;
  }
  if (!data?.onboarding.completed) return <Navigate to="/welcome" replace />;
  return <>{children}</>;
}
