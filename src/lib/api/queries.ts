import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import type { AppSettings, DownloadProgress, ModelStatus } from "@/lib/types";
import { api, integrationsApi, meetingsApi, modelsApi, peopleApi } from ".";
import { EVENTS, useTauriEvent } from "./events";

export const keys = {
  settings: ["settings"] as const,
  appInfo: ["appInfo"] as const,
  capabilities: ["capabilities"] as const,
  audioDevices: ["audioDevices"] as const,
};

export function useSettings() {
  return useQuery({ queryKey: keys.settings, queryFn: api.app.settings, staleTime: Infinity });
}

export function useUpdateSettings() {
  const qc = useQueryClient();
  return useMutation({
    mutationFn: (s: AppSettings) => api.app.updateSettings(s),
    onSuccess: (s) => qc.setQueryData(keys.settings, s),
  });
}

/** Mutate a copy of the current settings and persist it. */
export function usePatchSettings() {
  const { data } = useSettings();
  const update = useUpdateSettings();
  return (patch: (draft: AppSettings) => void) => {
    if (!data) return;
    const next = structuredClone(data);
    patch(next);
    update.mutate(next);
  };
}

export function useAppInfo() {
  return useQuery({ queryKey: keys.appInfo, queryFn: api.app.info, staleTime: Infinity });
}

export function useCapabilities() {
  return useQuery({ queryKey: keys.capabilities, queryFn: () => api.system.capabilities(false), staleTime: 60_000 });
}

export function useAudioDevices() {
  return useQuery({ queryKey: keys.audioDevices, queryFn: api.system.audioDevices, staleTime: 10_000 });
}

export function useModels() {
  const qc = useQueryClient();
  const query = useQuery({ queryKey: ["models"], queryFn: modelsApi.list, staleTime: 5_000 });
  useTauriEvent<DownloadProgress>(EVENTS.downloadProgress, (p) => {
    qc.setQueryData<ModelStatus[]>(["models"], (old) =>
      old?.map((m) =>
        m.manifest.id === p.modelId
          ? { ...m, bytesDownloaded: p.bytesDownloaded, state: p.state, error: p.error ?? m.error }
          : m,
      ),
    );
    if (p.state !== "downloading" && p.state !== "verifying") {
      void qc.invalidateQueries({ queryKey: ["models"] });
      void qc.invalidateQueries({ queryKey: keys.capabilities });
    }
  });
  return query;
}

export function useMeetings() {
  return useQuery({ queryKey: ["meetings"], queryFn: () => meetingsApi.list(50), staleTime: 5_000 });
}

export function useRecordingStatus() {
  return useQuery({ queryKey: ["recording"], queryFn: meetingsApi.status, refetchInterval: 5_000 });
}

export function usePeople() {
  return useQuery({ queryKey: ["people"], queryFn: peopleApi.list, staleTime: 30_000 });
}

export function useIntegrations() {
  return useQuery({ queryKey: ["integrations"], queryFn: integrationsApi.status, staleTime: 30_000 });
}
