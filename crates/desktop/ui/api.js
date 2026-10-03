// コマンド境界の呼び出し口 (設計書 6.1)。
//
// 画面は失敗の分類 (kind) と文面だけを見る。状態コードや内部の型は解釈しない。

const invoke = window.__TAURI__.core.invoke;
export const listen = window.__TAURI__.event.listen;

/** 呼び出しの失敗。境界が返した分類をそのまま持つ。 */
export class CallFailure extends Error {
  constructor(payload) {
    const detail = payload && payload.message ? payload.message : String(payload);
    super(detail);
    this.kind = (payload && payload.kind) || "unknown";
    this.nextAction = (payload && payload.next_action) || "";
  }
}

async function call(command, args) {
  try {
    return await invoke(command, args);
  } catch (payload) {
    throw new CallFailure(payload);
  }
}

// 設定と接続 (FR-01、NFR-17)
export const loadSettings = () => call("load_settings");
export const saveSettings = (settings) => call("save_settings", { settings });
export const connectionState = () => call("connection_state");
export const connect = (settings, token, proxyUser, proxyPassword) =>
  call("connect", { settings, token, proxyUser, proxyPassword });
export const disconnect = () => call("disconnect");

// 繰り返し定義 (FR-03、FR-14、FR-21)
export const listSchedules = () => call("list_schedules");
export const createSchedule = (schedule) => call("create_schedule", { schedule });
export const updateSchedule = (schedule) => call("update_schedule", { schedule });
export const duplicateSchedule = (id) => call("duplicate_schedule", { id });
export const setScheduleEnabled = (id, enabled) =>
  call("set_schedule_enabled", { id, enabled });
export const deleteScheduleConfirmation = (id) => call("delete_schedule_confirmation", { id });
export const deleteSchedule = (id) => call("delete_schedule", { id });
export const patternChangeConfirmation = (schedule, start, end) =>
  call("pattern_change_confirmation", { schedule, start, end });

// 展開 (FR-07、EC-24)
export const resolvePreset = (preset) => call("resolve_preset", { preset });
export const expand = (start, end) => call("expand", { start, end });

// 登録 (FR-09、FR-10、FR-12、FR-20)
export const registerBatch = (rows) => call("register_batch", { rows });
export const retryFailed = (rows) => call("retry_failed", { rows });
export const registerSingle = (row) => call("register_single", { row });
export const cancelRegistration = () => call("cancel_registration");
export const isRegistering = () => call("is_registering");

// 登録先 (FR-02)
export const listProjects = () => call("list_projects");
export const addProject = (projectPath) => call("add_project", { projectPath });
export const projectDeleteConfirmation = (id) => call("project_delete_confirmation", { id });
export const deleteProject = (id) => call("delete_project", { id });

// 休日 (FR-13、FR-22)
export const listHolidays = () => call("list_holidays");
export const addHoliday = (date, name) => call("add_holiday", { date, name });
export const deleteHoliday = (date) => call("delete_holiday", { date });
export const importHolidays = (path) => call("import_holidays", { path });
export const exportHolidays = (path) => call("export_holidays", { path });

// 定義の受け渡し (FR-18)
export const exportSchedules = (path) => call("export_schedules", { path });
export const importSchedules = (path, overwrite) =>
  call("import_schedules", { path, overwrite });

// 履歴 (FR-19、FR-23)
export const listHistory = (filter) => call("list_history", filter);
export const historyDeleteConfirmation = (keys) => call("history_delete_confirmation", { keys });
export const deleteHistory = (keys) => call("delete_history", { keys });

// 候補 (FR-16)
export const candidates = (project, kind) => call("candidates", { project, kind });
export const refetchCandidates = (project, kind) => call("refetch_candidates", { project, kind });
