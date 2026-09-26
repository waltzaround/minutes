import { createHashRouter, Navigate } from "react-router";
import { Shell } from "./Shell";
import { OnboardingGate } from "./OnboardingGate";
import { OnboardingPage } from "@/features/onboarding/OnboardingPage";
import { HomePage } from "@/features/meetings/HomePage";
import { SettingsPage } from "@/features/settings/SettingsPage";
import { MeetingPage } from "@/features/meetings/MeetingPage";
import { RecordingPage } from "@/features/recording/RecordingPage";
import { PeoplePage } from "@/features/people/PeoplePage";

export const router = createHashRouter([
  { path: "/welcome", element: <OnboardingPage /> },
  {
    element: (
      <OnboardingGate>
        <Shell />
      </OnboardingGate>
    ),
    children: [
      { index: true, element: <HomePage /> },
      { path: "recording", element: <RecordingPage /> },
      { path: "meetings/:id", element: <MeetingPage /> },
      { path: "people", element: <PeoplePage /> },
      { path: "settings", element: <Navigate to="/settings/general" replace /> },
      { path: "settings/:section", element: <SettingsPage /> },
      { path: "*", element: <Navigate to="/" replace /> },
    ],
  },
]);
