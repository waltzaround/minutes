/**
 * Development-only mock backend for previewing the UI in a plain browser
 * (`pnpm dev`, then open http://localhost:1420/?mock=<fixture-id>).
 *
 * This is an explicit development fixture: it is only loaded when
 * `import.meta.env.DEV` is true AND the `mock` query parameter is present,
 * and it serves capability data computed by the real Rust assessment for
 * the hardware fixtures (see `export_capability_samples`).
 */
import { mockIPC, mockWindows } from "@tauri-apps/api/mocks";
import samples from "./generated/samples.json";
import type { AppSettings, MeetingDetail, ModelManifest, ModelStatus, SystemCapabilities } from "@/lib/types";

export function installMockBackend(fixtureId: string) {
  const all = samples.capabilities as unknown as Record<string, SystemCapabilities>;
  const caps = all[fixtureId] ?? all["mac-m1-16gb"];
  const manifests = samples.manifests as unknown as ModelManifest[];
  const onboarded = new URLSearchParams(location.search).has("onboarded");
  let settings: AppSettings = {
    general: { theme: "system", consentReminder: true, selfPersonId: null },
    audio: { microphoneDeviceId: null, outputDeviceId: null, captureSystemAudio: true },
    privacy: { audioRetention: "delete_after_processing", checkForUpdates: false },
    advanced: {
      modelsDir: null, llmModelId: null, inferenceBackend: "auto", contextTokens: null, gpuLayers: null,
      asrThreads: null, asrChunkSeconds: 20, speakerKnownThreshold: 0.6,
      speakerPossibleThreshold: 0.45, simulateHardwareFixture: fixtureId,
    },
    notion: { connected: true, dataSourceId: null, dataSourceName: null, uploadMode: "summaryAndActions" },
    linear: { connected: true, defaultTeamId: null, defaultProjectId: null, workspaceName: null },
    onboarding: { completed: onboarded, completedAt: null },
  };
  // `&installed` shows the recommended models as installed (for screenshots).
  const installed = new URLSearchParams(location.search).has("installed");
  const recommended = new Set(caps.assessment.requiredModels);
  const models: ModelStatus[] = manifests.map((m) => {
    const done = installed && (recommended.size === 0 || recommended.has(m.id));
    return { manifest: m, state: done ? "installed" : "notInstalled", bytesDownloaded: done ? m.byteSize : 0, path: null, error: null };
  });

  const seg = (id: string, startMs: number, source: "microphone" | "system", text: string) => ({
    id, startMs, endMs: startMs + 4000, source, speakerClusterId: null, personId: null, speakerConfidence: null,
    text, transcriptionConfidence: null, words: [], isProvisional: false, edited: false,
  });
  const meetings: MeetingDetail[] = [
    {
      summary: { id: "m1", title: "Design Weekly", status: "ready", startedAt: new Date().toISOString(), endedAt: null, durationMs: 47 * 60_000, speakerCount: 4, actionCount: 3, segmentCount: 10 },
      processingError: null, audioAvailable: false, audioRetention: "delete_after_processing", transcriptRevision: 0, notionPageUrl: null, tracks: [],
      segments: [
        { ...seg("s0a", 1352_000, "system", "The onboarding flow tested well, but people still stall on the permissions step."), speakerClusterId: "c-2" },
        { ...seg("s0b", 1358_000, "microphone", "Is that the copy, or the fact that the dialog comes up before they've seen any value?"), speakerClusterId: "c-me", personId: "p-w" },
        { ...seg("s0c", 1365_000, "system", "Mostly timing. If we ask after the first recording, the drop-off roughly halves in the prototype."), speakerClusterId: "c-2" },
        { ...seg("s0d", 1372_000, "system", "That needs a small API change so we can defer the permission check."), speakerClusterId: "c-1", personId: "p-t" },
        { ...seg("s0e", 1378_000, "microphone", "Okay. Can we get that into this release, or is it too late?"), speakerClusterId: "c-me", personId: "p-w" },
        { ...seg("s1", 1384_000, "microphone", "Let's target Friday."), speakerClusterId: "c-me", personId: "p-w" },
        { ...seg("s2", 1389_000, "system", "Yep, I'll take the API changes."), speakerClusterId: "c-1", personId: "p-t" },
        { ...seg("s3", 1393_000, "system", "I'll have the screens ready Thursday."), speakerClusterId: "c-2" },
        { ...seg("s4", 1399_000, "microphone", "Great. The analytics numbers for last week still look off to me."), speakerClusterId: "c-me", personId: "p-w" },
        { ...seg("s5", 1405_000, "system", "Maybe Sarah can look at it."), speakerClusterId: "c-2" },
      ],
    },
    {
      summary: { id: "m2", title: "Engineering Standup", status: "failed", startedAt: new Date(Date.now() - 86400000).toISOString(), endedAt: null, durationMs: 18 * 60_000, speakerCount: 0, actionCount: 0, segmentCount: 0 },
      processingError: "The transcription models are not installed. Download them in Settings → Models, then choose “Transcribe again”.",
      audioAvailable: true, audioRetention: "delete_after_processing", transcriptRevision: 0, notionPageUrl: null, tracks: [], segments: [],
    },
  ];
  let recording: unknown = null;
  mockWindows("main");
  mockIPC(
    async (cmd, args) => {
      const a = (args ?? {}) as Record<string, unknown>;
      switch (cmd) {
        case "get_app_info":
          return { version: "0.1.0-mock", dataDir: "/mock/data", logDir: "/mock/logs", modelsDir: "/mock/models", debugBuild: true, platform: "mock" };
        case "get_settings":
          return settings;
        case "update_settings":
          settings = a.settings as AppSettings;
          return settings;
        case "complete_onboarding":
          settings = { ...settings, onboarding: { completed: true, completedAt: new Date().toISOString() } };
          return settings;
        case "get_capabilities":
          await new Promise((r) => setTimeout(r, 600));
          return caps;
        case "get_recommended_profile":
          return caps.assessment;
        case "run_benchmark":
          await new Promise((r) => setTimeout(r, 800));
          return caps.benchmarks;
        case "list_audio_devices":
          return { host: "Mock", inputs: caps.hardware.audio.inputDevices, outputs: caps.hardware.audio.outputDevice ? [caps.hardware.audio.outputDevice] : [] };
        case "request_microphone_permission":
          return "granted";
        case "list_hardware_fixtures":
          return [];
        case "list_models":
          return models;
        case "required_download_space":
          return 0;
        case "list_meetings":
          return meetings.map((m) => m.summary);
        case "get_meeting":
          return meetings.find((m) => m.summary.id === a.meetingId) ?? meetings[0];
        case "list_interrupted_meetings":
          return new URLSearchParams(location.search).has("crash")
            ? [{ meeting: { ...meetings[0].summary, id: "crashed", title: "Engineering Standup", status: "interrupted" }, recoveredMs: 42 * 60_000, tracks: [] }]
            : [];
        case "list_people":
          return [
            { id: "p-w", displayName: "Walter", email: null, avatarPath: null, isSelf: true, voiceProfile: null, notionUserId: null, linearUserId: null },
            { id: "p-t", displayName: "Tom Hutchison", email: "tom@example.com", avatarPath: null, isSelf: false, voiceProfile: { embeddingCount: 5, enrolledCount: 5, fromMeetingsCount: 0, createdAt: "", updatedAt: "" }, notionUserId: null, linearUserId: null },
            { id: "p-s", displayName: "Sarah", email: null, avatarPath: null, isSelf: false, voiceProfile: null, notionUserId: null, linearUserId: null },
          ];
        case "list_speakers":
          return [
            { id: "c-me", label: "Walter", source: "microphone", personId: "p-w", identity: { type: "known", personId: "p-w", confidence: 1 }, confirmed: false, isLocalUser: true, segmentCount: 1, speakingMs: 600000 },
            { id: "c-1", label: "Speaker 1", source: "system", personId: "p-t", identity: { type: "known", personId: "p-t", confidence: 0.82 }, confirmed: false, isLocalUser: false, segmentCount: 1, speakingMs: 900000 },
            { id: "c-2", label: "Speaker 2", source: "system", personId: "p-s", identity: { type: "possible", personId: "p-s", confidence: 0.5 }, confirmed: false, isLocalUser: false, segmentCount: 1, speakingMs: 420000 },
          ];
        case "get_analysis":
          return {
            status: "ready", error: null, modelId: "nemotron-3-nano-4b-q4km", strategy: "single_pass", updatedAt: new Date().toISOString(), stale: false,
            title: "Design Weekly", summary: ["Agreed to target Friday for the onboarding release.", "Tom takes the API changes; Sarah prepares the screens."],
            decisions: [{ id: "d1", text: "Target Friday for the onboarding release", evidence: [{ segmentId: "s1", startMs: 1384000, speaker: "Walter", text: "Let's target Friday." }] }],
            actionItems: [
              { id: "a1", title: "Update onboarding API", description: null, ownerPersonId: "p-t", ownerLabel: "Tom Hutchison", dueDate: "2026-10-02", dueText: "Friday", assignmentType: "explicit_acceptance", confidence: 0.9, selected: true, dismissed: false, userEdited: false,
                evidence: [{ segmentId: "s2", startMs: 1389000, speaker: "Tom Hutchison", text: "Yep, I'll take the API changes." }],
                linear: { state: "none", issueId: null, identifier: null, url: null, error: null, teamId: null, projectId: null, assigneeId: null, priority: null } },
              { id: "a2", title: "Prepare onboarding screens", description: null, ownerPersonId: null, ownerLabel: "Speaker 2", dueDate: "2026-10-01", dueText: "Thursday", assignmentType: "explicit_acceptance", confidence: 0.8, selected: true, dismissed: false, userEdited: false,
                evidence: [{ segmentId: "s3", startMs: 1393000, speaker: "Speaker 2", text: "I'll have the screens ready Thursday." }],
                linear: { state: "none", issueId: null, identifier: null, url: null, error: null, teamId: null, projectId: null, assigneeId: null, priority: null } },
              { id: "a3", title: "Investigate analytics discrepancy", description: null, ownerPersonId: null, ownerLabel: "Sarah", dueDate: null, dueText: null, assignmentType: "suggested", confidence: 0.35, selected: false, dismissed: false, userEdited: false,
                evidence: [{ segmentId: "s5", startMs: 1405000, speaker: "Speaker 2", text: "Maybe Sarah can look at it." }],
                linear: { state: "none", issueId: null, identifier: null, url: null, error: null, teamId: null, projectId: null, assigneeId: null, priority: null } },
            ],
            unresolvedQuestions: [],
          };
        case "integrations_status":
          return { notion: { connected: true, account: "Product Meetings" }, linear: { connected: true, account: "Acme" } };
        case "linear_directory":
          return {
            workspace: "Acme",
            teams: [{ id: "t-eng", name: "Engineering", detail: "ENG", teamIds: [] }, { id: "t-des", name: "Design", detail: "DES", teamIds: [] }],
            users: [{ id: "u-tom", name: "Tom Hutchison", detail: "tom@example.com", teamIds: [] }, { id: "u-sarah", name: "Sarah Lee", detail: null, teamIds: [] }],
            projects: [{ id: "pr-onb", name: "Onboarding", detail: null, teamIds: ["t-eng"] }],
          };
        case "prepare_linear_issues":
          return [
            { actionItemId: "a1", notes: [], draft: { title: "Update onboarding API", description: "Created from Design Weekly — 26 Sep 2026\n\n## Evidence\n\nTom Hutchison · 23:09\n\n> Yep, I'll take the API changes.\n", teamId: "t-eng", assigneeId: "u-tom", projectId: "pr-onb", priority: 3, dueDate: "2026-10-02" } },
            { actionItemId: "a2", notes: ["Speaker 2 isn't linked to a person yet."], draft: { title: "Prepare onboarding screens", description: "…", teamId: "", assigneeId: null, projectId: null, priority: 0, dueDate: "2026-10-01" } },
          ];
        case "create_linear_issue":
          await new Promise((r) => setTimeout(r, 400));
          return { state: "created", issue: { id: "i1", identifier: "ENG-42", url: "https://linear.app" }, error: null };
        case "search_meetings": {
          const q = String(a.query ?? "").toLowerCase();
          const hits: unknown[] = [];
          for (const m of meetings) {
            if (m.summary.title.toLowerCase().includes(q)) hits.push({ meetingId: m.summary.id, meetingTitle: m.summary.title, startedAt: m.summary.startedAt, kind: "title", text: m.summary.title, segmentId: null, startMs: null });
            for (const sg of m.segments) if (sg.text.toLowerCase().includes(q)) hits.push({ meetingId: m.summary.id, meetingTitle: m.summary.title, startedAt: m.summary.startedAt, kind: "transcript", text: sg.text, segmentId: sg.id, startMs: sg.startMs });
          }
          return hits;
        }
        case "get_recording_status":
          return recording;
        case "start_meeting":
          recording = {
            meetingId: "rec", title: "Meeting 26 Sep, 2:30 pm", startedAt: new Date().toISOString(), elapsedMs: 0,
            microphone: { state: "recording", deviceName: "MacBook Pro Microphone", message: null },
            system: { state: "disconnected", deviceName: "MacBook Pro Speakers", message: "device removed" },
            warnings: [{ meetingId: "rec", kind: "sourceDisconnected", source: "system", message: "Meeting audio disconnected at 14:32. Microphone recording is still active.", atMs: 872000 }],
          };
          return recording;
        case "stop_meeting":
          recording = null;
          return meetings[0].summary.id;
        case "plugin:event|listen":
          return 0;
        case "plugin:event|unlisten":
          return null;
        default:
          throw { code: "unavailable", message: `Mock backend does not implement ${cmd}` };
      }
    },
  );
}
