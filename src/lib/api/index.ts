import type {
  ActionItemPatch,
  AnalysisView,
  AppInfo,
  LlmChoice,
  AppSettings,
  AudioSource,
  InterruptedMeeting,
  CreateIssueResult,
  EnrollmentResult,
  IntegrationsStatus,
  IssueDraft,
  IssueProposal,
  LinearDirectory,
  NotionDataSource,
  NotionPage,
  NotionUser,
  IdentityUpdate,
  MeetingDetail,
  Person,
  PersonInput,
  SpeakerCluster,
  MeetingSummary,
  SearchHit,
  RecordingStatus,
  AudioDeviceList,
  BenchmarkSummary,
  CapabilityAssessment,
  HardwareFixture,
  ModelStatus,
  PermissionState,
  SystemCapabilities,
} from "@/lib/types";
import { invoke } from "./invoke";

export { CommandError, isTauri } from "./invoke";

export const api = {
  app: {
    info: () => invoke<AppInfo>("get_app_info"),
    settings: () => invoke<AppSettings>("get_settings"),
    updateSettings: (settings: AppSettings) => invoke<AppSettings>("update_settings", { settings }),
    completeOnboarding: () => invoke<AppSettings>("complete_onboarding"),
  },
  system: {
    capabilities: (refresh = false) => invoke<SystemCapabilities>("get_capabilities", { refresh }),
    recommendedProfile: () => invoke<CapabilityAssessment>("get_recommended_profile"),
    runBenchmark: (stage: "synthetic" | "asr" | "llm") => invoke<BenchmarkSummary>("run_benchmark", { stage }),
    audioDevices: () => invoke<AudioDeviceList>("list_audio_devices"),
    requestMicrophone: () => invoke<PermissionState>("request_microphone_permission"),
    openPrivacySettings: (kind: "microphone" | "systemAudio") => invoke<void>("open_privacy_settings", { kind }),
    fixtures: () => invoke<HardwareFixture[]>("list_hardware_fixtures"),
  },
};

export const modelsApi = {
  list: () => invoke<ModelStatus[]>("list_models"),
  download: (modelId: string) => invoke<void>("download_model", { modelId }),
  pause: (modelId: string) => invoke<void>("pause_download", { modelId }),
  cancel: (modelId: string) => invoke<void>("cancel_download", { modelId }),
  remove: (modelId: string) => invoke<void>("remove_model", { modelId }),
  requiredSpace: (modelIds: string[]) => invoke<number>("required_download_space", { modelIds }),
};

export const meetingsApi = {
  start: (title?: string) => invoke<RecordingStatus>("start_meeting", { title: title ?? null }),
  stop: () => invoke<string>("stop_meeting"),
  pause: () => invoke<string>("pause_meeting"),
  resume: (meetingId: string) => invoke<RecordingStatus>("resume_meeting", { meetingId }),
  finishPaused: (meetingId: string) => invoke<void>("finish_paused_meeting", { meetingId }),
  importMedia: (path: string) => invoke<string>("import_media", { path }),
  status: () => invoke<RecordingStatus | null>("get_recording_status"),
  reconnect: (source: AudioSource) => invoke<RecordingStatus>("reconnect_source", { source }),
  list: (limit?: number) => invoke<MeetingSummary[]>("list_meetings", { limit: limit ?? null }),
  rename: (meetingId: string, title: string) => invoke<void>("rename_meeting", { meetingId, title }),
  remove: (meetingId: string) => invoke<void>("delete_meeting", { meetingId }),
  interrupted: () => invoke<InterruptedMeeting[]>("list_interrupted_meetings"),
  recover: (meetingId: string) => invoke<void>("recover_meeting", { meetingId }),
};

export const meetingDetailApi = {
  get: (meetingId: string) => invoke<MeetingDetail>("get_meeting", { meetingId }),
  reprocess: (meetingId: string) => invoke<void>("reprocess_meeting", { meetingId }),
};

export const peopleApi = {
  list: () => invoke<Person[]>("list_people"),
  create: (input: PersonInput) => invoke<Person>("create_person", { input }),
  update: (personId: string, input: PersonInput) => invoke<Person>("update_person", { personId, input }),
  remove: (personId: string) => invoke<void>("delete_person", { personId }),
  exportTo: (path: string) => invoke<void>("export_people", { path }),
  startEnrollment: (personId: string) => invoke<void>("start_voice_enrollment", { personId }),
  cancelEnrollment: () => invoke<void>("cancel_voice_enrollment"),
  finishEnrollment: (keepRecording: boolean) => invoke<EnrollmentResult>("finish_voice_enrollment", { keepRecording }),
  deleteVoice: (personId: string) => invoke<void>("delete_voice_profile", { personId }),
  clearAllVoices: () => invoke<void>("clear_all_voice_data"),
};

export const speakersApi = {
  list: (meetingId: string) => invoke<SpeakerCluster[]>("list_speakers", { meetingId }),
  rename: (clusterId: string, label: string) => invoke<void>("rename_speaker", { clusterId, label }),
  setPerson: (clusterId: string, personId: string | null, improveProfile: boolean) =>
    invoke<IdentityUpdate>("update_speaker_identity", { clusterId, personId, improveProfile }),
  merge: (fromClusterId: string, intoClusterId: string) => invoke<void>("merge_speakers", { fromClusterId, intoClusterId }),
  split: (segmentId: string, atWord: number, clusterId: string | null) => invoke<void>("split_segment", { segmentId, atWord, clusterId }),
  editText: (segmentId: string, text: string) => invoke<void>("edit_segment", { segmentId, text }),
};

export const analysisApi = {
  get: (meetingId: string) => invoke<AnalysisView | null>("get_analysis", { meetingId }),
  regenerate: (meetingId: string) => invoke<void>("analyse_meeting", { meetingId }),
  updateAction: (actionItemId: string, patch: Partial<ActionItemPatch>) => invoke<void>("update_action_item", { actionItemId, patch }),
  addAction: (meetingId: string, title: string) => invoke<string>("add_action_item", { meetingId, title }),
  llmChoices: () => invoke<LlmChoice[]>("list_llm_choices"),
  importGguf: (path: string) => invoke<LlmChoice>("import_gguf", { path }),
};

export const integrationsApi = {
  status: () => invoke<IntegrationsStatus>("integrations_status"),
  notionConnect: (token: string) => invoke<string>("notion_connect", { token }),
  notionDisconnect: () => invoke<void>("notion_disconnect"),
  notionSearch: (query: string) => invoke<NotionDataSource[]>("notion_search_databases", { query }),
  notionSelect: (dataSourceId: string, name: string) => invoke<void>("notion_select_database", { dataSourceId, name }),
  notionUsers: () => invoke<NotionUser[]>("notion_users"),
  linearConnect: (apiKey: string) => invoke<string>("linear_connect", { apiKey }),
  linearDisconnect: () => invoke<void>("linear_disconnect"),
  linearDirectory: () => invoke<LinearDirectory>("linear_directory"),
  mapPerson: (personId: string, provider: "notion" | "linear", remoteId: string | null, remoteName: string | null) =>
    invoke<void>("set_person_mapping", { personId, provider, remoteId, remoteName }),
  setLinearDefaults: (teamId: string | null, projectId: string | null) => invoke<void>("set_linear_defaults", { teamId, projectId }),
  syncToNotion: (meetingId: string, includeTranscript: boolean) => invoke<NotionPage>("sync_to_notion", { meetingId, includeTranscript }),
  prepareLinear: (meetingId: string) => invoke<IssueProposal[]>("prepare_linear_issues", { meetingId }),
  createLinearIssue: (actionItemId: string, draft: IssueDraft) => invoke<CreateIssueResult>("create_linear_issue", { actionItemId, draft }),
};

export const searchApi = {
  meetings: (query: string) => invoke<SearchHit[]>("search_meetings", { query }),
};
