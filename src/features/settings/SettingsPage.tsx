import { NavLink, useParams } from "react-router";
import { PageBody, PageHeader } from "@/components/app/Page";
import { cn } from "@/lib/utils";
import { GeneralSettings } from "./GeneralSettings";
import { AudioSettings } from "./AudioSettings";
import { ModelsSettings } from "./ModelsSettings";
import { PeopleSettings } from "./PeopleSettings";
import { NotionSettings } from "@/features/integrations/NotionSettings";
import { LinearSettings } from "@/features/integrations/LinearSettings";
import { StorageSettings } from "./StorageSettings";
import { PrivacySettings } from "./PrivacySettings";
import { AdvancedSettings } from "./AdvancedSettings";

const SECTIONS = [
  { id: "general", label: "General", el: <GeneralSettings /> },
  { id: "audio", label: "Audio", el: <AudioSettings /> },
  { id: "models", label: "Models", el: <ModelsSettings /> },
  { id: "people", label: "People & Voices", el: <PeopleSettings /> },
  { id: "notion", label: "Notion", el: <NotionSettings /> },
  { id: "linear", label: "Linear", el: <LinearSettings /> },
  { id: "storage", label: "Storage", el: <StorageSettings /> },
  { id: "privacy", label: "Privacy", el: <PrivacySettings /> },
  { id: "advanced", label: "Advanced", el: <AdvancedSettings /> },
];

export function SettingsPage() {
  const { section = "general" } = useParams();
  const current = SECTIONS.find((s) => s.id === section) ?? SECTIONS[0];
  return (
    <>
      <PageHeader title="Settings" />
      <div className="flex min-h-0 flex-1">
        <nav className="w-52 shrink-0 space-y-0.5 px-3 py-2" aria-label="Settings sections">
          {SECTIONS.map((s) => (
            <NavLink
              key={s.id}
              to={`/settings/${s.id}`}
              className={({ isActive }) =>
                cn(
                  "flex h-8 items-center rounded-lg px-2.5 text-[13px] text-muted-foreground outline-none hover:bg-accent/70 hover:text-foreground focus-visible:ring-2 focus-visible:ring-ring",
                  isActive && "bg-accent text-foreground",
                )
              }
            >
              {s.label}
            </NavLink>
          ))}
        </nav>
        <PageBody>
          <div className="max-w-2xl px-8 pb-12">
            <h1 className="pt-2 pb-2 font-display text-[26px] tracking-tight">{current.label}</h1>
            {current.el}
          </div>
        </PageBody>
      </div>
    </>
  );
}
