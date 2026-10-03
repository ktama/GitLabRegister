// 画面の入口 (設計書 4.2 UI 層)。
//
// 描画と入力だけを担う。計算と入出力はコマンド境界へ委ねる。
// パーソナルアクセストークンは入力欄から受け取って境界へ渡したら、ただちに欄を空にする。
// 画面側の状態には残さない (NFR-05、設計書 8.3)。

import * as api from "./api.js";
import { PreviewGrid } from "./grid.js";
import {
  buildCatalog,
  candidateLabel,
  candidatePicker,
  orderCandidates,
  projectSelect,
} from "./pickers.js";
import { $, confirmAction, el, fill, notify, notifyFailure, text } from "./ui.js";

const state = {
  connection: null,
  schedules: [],
  projects: [],
  lastBatch: null,
};

/** 登録先の一覧を読み直す。選択欄の選択肢に使う (FR-02)。 */
async function refreshProjects() {
  try {
    state.projects = await api.listProjects();
  } catch (error) {
    notifyFailure(error);
    state.projects = [];
  }
  return state.projects;
}

// ---- 画面の切り替え (NFR-15: キーボードだけで到達できる) ----

/**
 * 画面を切り替える。
 *
 * 定義の編集に保存していない変更があり、利用者が破棄を選ばなかった場合は切り替えない。
 * @returns {Promise<boolean>} 切り替えたか
 */
async function showScreen(name) {
  if (name !== "schedules" && !(await closeEditor())) return false;
  for (const tab of document.querySelectorAll(".tabs button")) {
    tab.setAttribute("aria-selected", String(tab.dataset.screen === name));
  }
  for (const screen of document.querySelectorAll(".screen")) {
    screen.hidden = screen.id !== `screen-${name}`;
  }
  const loaders = {
    schedules: loadSchedules,
    master: loadMaster,
    history: () => searchHistory(),
    single: renderSingleForm,
    // 展開していない状態で一括登録を押せないようにする (FR-07)。
    preview: updatePreviewActions,
  };
  loaders[name]?.();
  return true;
}

const tabs = [...document.querySelectorAll(".tabs button")];

for (const tab of tabs) {
  tab.addEventListener("click", () => showScreen(tab.dataset.screen));
}

for (const link of document.querySelectorAll("[data-goto]")) {
  link.addEventListener("click", () => showScreen(link.dataset.goto));
}

/** 画面を切り替え、その画面のタブへ焦点を移す。 */
async function focusScreen(tab) {
  if (await showScreen(tab.dataset.screen)) tab.focus();
}

// 上下の矢印でタブを順に移る。
$(".tabs").addEventListener("keydown", (event) => {
  const index = tabs.indexOf(document.activeElement);
  if (index < 0) return;
  const step = { ArrowDown: 1, ArrowRight: 1, ArrowUp: -1, ArrowLeft: -1 }[event.key];
  if (!step) return;
  event.preventDefault();
  focusScreen(tabs[(index + step + tabs.length) % tabs.length]);
});

// Ctrl+1〜6 で、サイドナビの並び順の画面へ直接移る。
document.addEventListener("keydown", (event) => {
  if (!event.ctrlKey || event.altKey || event.shiftKey || event.metaKey) return;
  const number = Number.parseInt(event.key, 10);
  if (Number.isNaN(number) || number < 1 || number > tabs.length) return;
  event.preventDefault();
  focusScreen(tabs[number - 1]);
});

// ---- 接続 (FR-01、NFR-17) ----

function renderConnection(view) {
  state.connection = view;
  $("#conn-summary").textContent = view.description;
  $("#conn-status").dataset.conn = view.state;
  // 展開前の案内は接続の有無で文面が変わる。
  if (!grid.loaded) grid.render();
  // EC-15: ティアが許さない接続では、表にも EE 固有項目の列を出さない。
  grid.setEeAllowed(view.allows_ee_fields);
  renderColumnMenu();
  const detail = $("#conn-detail");
  const notes = [];
  if (!view.can_register) {
    notes.push("この状態では一括登録と単発登録を実行できません。定義の編集と展開は行えます。");
  }
  if (!view.allows_ee_fields) {
    notes.push("エピック・イテレーション・ウェイト・複数担当者は登録に使えません。");
  }
  fill(detail, notes.map((n) => el("p", {}, [text(n)])));
}

function settingsFromForm() {
  const mode = $("#proxy-mode").value;
  const port = Number.parseInt($("#proxy-port").value, 10);
  return {
    gitlab_url: $("#gitlab-url").value.trim() || null,
    proxy: {
      mode,
      host: mode === "Manual" ? $("#proxy-host").value.trim() || null : null,
      port: mode === "Manual" && !Number.isNaN(port) ? port : null,
      requires_credentials: mode === "Manual" && $("#proxy-auth").checked,
    },
  };
}

function applySettingsToForm(settings) {
  $("#gitlab-url").value = settings.gitlab_url ?? "";
  $("#proxy-mode").value = settings.proxy?.mode ?? "FollowSystem";
  $("#proxy-host").value = settings.proxy?.host ?? "";
  $("#proxy-port").value = settings.proxy?.port ?? "";
  $("#proxy-auth").checked = !!settings.proxy?.requires_credentials;
  updateProxyVisibility();
}

function updateProxyVisibility() {
  $("#proxy-manual").hidden = $("#proxy-mode").value !== "Manual";
  $("#proxy-creds").hidden = !$("#proxy-auth").checked;
}

$("#proxy-mode").addEventListener("change", updateProxyVisibility);
$("#proxy-auth").addEventListener("change", updateProxyVisibility);

$("#btn-connect").addEventListener("click", async () => {
  const tokenField = $("#token");
  const token = tokenField.value;
  const user = $("#proxy-user").value;
  const password = $("#proxy-password").value;
  try {
    const result = await api.connect(
      settingsFromForm(),
      token,
      user || null,
      password || null,
    );
    renderConnection(result.connection);
    for (const warning of result.warnings) notify(warning, "", false);
    if (result.warnings.length === 0) {
      notify("接続しました", "", true);
      // 注意事項がなければ、次の作業である展開と登録へ進める。
      if (result.connection.can_register && (await showScreen("preview"))) {
        $("#range-start").focus();
      }
    }
  } catch (error) {
    notifyFailure(error);
    renderConnection(await api.connectionState());
  } finally {
    // 入力欄に残さない (NFR-05)。
    tokenField.value = "";
    $("#proxy-password").value = "";
  }
});

$("#btn-disconnect").addEventListener("click", async () => {
  renderConnection(await api.disconnect());
  notify("切断しました。秘密情報を破棄しました", "", true);
});

// ---- 繰り返し定義 (FR-03〜FR-06、FR-14、FR-21) ----

async function loadSchedules() {
  try {
    state.schedules = await api.listSchedules();
  } catch (error) {
    notifyFailure(error);
    return;
  }
  const host = $("#schedule-list");
  if (state.schedules.length === 0) {
    fill(host, [
      el("p", { class: "empty" }, [
        text("繰り返しタスク定義がありません。「新規作成」で追加してください。"),
      ]),
    ]);
    return;
  }
  const table = el("table", { class: "list" });
  table.append(
    el("thead", {}, [
      rowOf("th", ["状態", "名称", "登録先", "繰り返し", "営業日調整", "操作"]),
    ]),
  );
  const body = el("tbody");
  for (const schedule of state.schedules) {
    const tr = el("tr", schedule.enabled ? {} : { class: "row-off" });
    tr.append(
      el("td", {}, [
        el("span", { class: schedule.enabled ? "chip chip-on" : "chip chip-off" }, [
          text(schedule.enabled ? "有効" : "無効"),
        ]),
      ]),
    );
    tr.append(el("td", {}, [text(schedule.name)]));
    tr.append(el("td", {}, [text(schedule.project)]));
    tr.append(el("td", {}, [text(describePattern(schedule.pattern))]));
    tr.append(el("td", {}, [text(describeAdjustment(schedule.adjustment))]));
    const actions = el("td", { class: "row-actions" });
    actions.append(button("編集", () => openEditor(schedule)));
    actions.append(
      button(schedule.enabled ? "無効にする" : "有効にする", async () => {
        try {
          await api.setScheduleEnabled(schedule.id, !schedule.enabled);
          await loadSchedules();
        } catch (error) {
          notifyFailure(error);
        }
      }),
    );
    actions.append(
      button("複製", async () => {
        try {
          await api.duplicateSchedule(idOf(schedule));
          await loadSchedules();
          notify("複製しました", "", true);
        } catch (error) {
          notifyFailure(error);
        }
      }),
    );
    const del = button("削除", async () => {
      try {
        const message = await api.deleteScheduleConfirmation(idOf(schedule));
        const confirmed = await confirmAction({
          title: `定義「${schedule.name}」を削除します`,
          body: message,
          confirmLabel: "削除する",
          danger: true,
        });
        if (!confirmed) return;
        await api.deleteSchedule(idOf(schedule));
        await loadSchedules();
        notify("削除しました", "", true);
      } catch (error) {
        notifyFailure(error);
      }
    });
    del.classList.add("danger");
    actions.append(del);
    tr.append(actions);
    body.append(tr);
  }
  table.append(body);
  fill(host, [table]);
}

// ScheduleId は newtype であり、JSON では素の文字列として現れる。
function idOf(schedule) {
  return schedule.id;
}

function describePattern(pattern) {
  if (pattern === "Daily") return "毎日";
  if (pattern === "EveryBusinessDay") return "毎営業日";
  const [kind, value] = Object.entries(pattern)[0] ?? [];
  switch (kind) {
    case "EveryNDays":
      return `${value.interval} 日ごと (起算 ${value.anchor})`;
    case "Weekly":
      return `${value.interval} 週ごと ${value.weekdays.map(weekdayLabel).join("・")}${
        value.anchor ? ` (起算 ${value.anchor})` : ""
      }`;
    case "MonthlyByDay":
      return `毎月 ${value.day === "LastDay" ? "末日" : `${value.day.Day} 日`}`;
    case "MonthlyByWeekday":
      return `毎月 第${ordinalLabel(value.ordinal)} ${weekdayLabel(value.weekday)}`;
    case "Yearly":
      return `毎年 ${value.month} 月 ${value.day} 日`;
    default:
      return String(kind ?? "");
  }
}

const WEEKDAYS = { Mon: "月", Tue: "火", Wed: "水", Thu: "木", Fri: "金", Sat: "土", Sun: "日" };
const ORDINALS = { First: "1", Second: "2", Third: "3", Fourth: "4", Fifth: "5" };
const weekdayLabel = (key) => WEEKDAYS[key] ?? key;
const ordinalLabel = (key) => ORDINALS[key] ?? key;

function describeAdjustment(adjustment) {
  return (
    {
      None: "調整しない",
      PreviousBusinessDay: "直前の営業日",
      NextBusinessDay: "直後の営業日",
      SkipOccurrence: "その回を発生させない",
    }[adjustment] ?? adjustment
  );
}

function rowOf(tag, labels) {
  const tr = el("tr");
  for (const label of labels) tr.append(el(tag, { scope: "col" }, [text(label)]));
  return tr;
}

function button(label, handler) {
  const node = el("button", { type: "button" }, [text(label)]);
  node.addEventListener("click", handler);
  return node;
}

$("#btn-new-schedule").addEventListener("click", () => openEditor(null));

$("#btn-export-schedules").addEventListener("click", async () => {
  let path;
  try {
    path = await api.chooseFileToSave("定義の書き出し先", "schedules.json", api.FILE_FILTERS.json);
  } catch (error) {
    notifyFailure(error);
    return;
  }
  if (!path) return;
  try {
    const count = await api.exportSchedules(path);
    notify(`${count} 件を書き出しました`, path, true);
  } catch (error) {
    notifyFailure(error);
  }
});

$("#btn-import-schedules").addEventListener("click", async () => {
  let path;
  try {
    path = await api.chooseFileToOpen("取り込む定義のファイル", api.FILE_FILTERS.json);
  } catch (error) {
    notifyFailure(error);
    return;
  }
  if (!path) return;
  const overwrite = await confirmAction({
    title: "同じ名称の定義の扱い",
    body:
      "取り込むファイルに、既にある定義と同じ名称の定義が含まれている場合の扱いを選んでください。\n" +
      "「上書きしない」を選ぶと、同じ名称の定義だけを取り込まず、ほかの定義は取り込みます。",
    confirmLabel: "上書きする",
    cancelLabel: "上書きしない",
  });
  try {
    const outcome = await api.importSchedules(path, overwrite);
    const parts = [`追加 ${outcome.added} 件`, `上書き ${outcome.overwritten} 件`];
    if (outcome.skipped_names.length > 0) {
      parts.push(`取り込まなかった定義: ${outcome.skipped_names.join(", ")}`);
    }
    notify(parts.join(" / "), "", true);
    await loadSchedules();
  } catch (error) {
    notifyFailure(error);
  }
});

// 定義の編集に、保存していない変更があるか。
// 入力欄はすべて編集欄の中にあるため、編集欄で入力と変更を受けたら変更ありとする。
const editorState = { dirty: false };
for (const type of ["input", "change"]) {
  $("#schedule-editor").addEventListener(type, () => {
    editorState.dirty = true;
  });
}

/**
 * 定義の編集を閉じる。保存していない変更があれば、破棄してよいか確かめる。
 * @returns {Promise<boolean>} 閉じたか (編集していない場合も true)
 */
async function closeEditor({ discard = false } = {}) {
  const host = $("#schedule-editor");
  if (host.hidden) return true;
  if (
    !discard &&
    editorState.dirty &&
    !(await confirmAction({
      title: "保存していない変更があります",
      body: "定義の編集内容を保存していません。閉じると変更は失われます。",
      confirmLabel: "変更を破棄して閉じる",
      cancelLabel: "編集に戻る",
      danger: true,
    }))
  ) {
    return false;
  }
  host.hidden = true;
  editorState.dirty = false;
  return true;
}

// 定義の編集。項目はコア層の検証に任せ、画面は入力の受け渡しに徹する。
async function openEditor(schedule) {
  await refreshProjects();
  const host = $("#schedule-editor");
  host.hidden = false;
  const draft = schedule
    ? structuredClone(schedule)
    : {
        id: "",
        name: "",
        project: "",
        enabled: true,
        active_period: { start: null, end: null },
        pattern: { MonthlyByDay: { day: { Day: 1 }, on_missing: "SkipMonth" } },
        adjustment: "None",
        issue: {
          title: "",
          description: "",
          labels: [],
          assignees: [],
          milestone: null,
          start_offset_days: null,
          due_offset_days: null,
          confidential: false,
          epic: null,
          iteration: null,
          iteration_by_occurrence: false,
          weight: null,
        },
      };

  const form = el("div", { class: "form" });
  form.append(el("h3", {}, [text(schedule ? "定義の編集" : "定義の新規作成")]));
  const nameField = inputField("名称", draft.name, (v) => (draft.name = v));
  form.append(nameField);

  // FR-02: 登録先は登録済みの一覧から選ぶ。自由入力にしない。
  const pickers = [];
  form.append(
    projectSelect("登録先プロジェクト", state.projects, draft.project, (v) => {
      draft.project = v;
      // 登録先が変わると候補も変わる (FR-16)。
      for (const picker of pickers) picker.reloadFor(v);
    }),
  );

  form.append(inputField("Issue のタイトル", draft.issue.title, (v) => (draft.issue.title = v)));
  form.append(
    textareaField("説明", draft.issue.description, (v) => (draft.issue.description = v)),
  );

  // FR-16: ラベル・担当者・マイルストーン・エピック・イテレーションは候補から選ぶ。
  const labelPicker = candidatePicker({
    label: "ラベル",
    project: draft.project,
    kind: "label",
    multiple: true,
    // GitLab-EE はラベルを名前で受け取る。EC-32 の突合も名前で行う。
    useName: true,
    value: draft.issue.labels,
    onChange: (ids) => (draft.issue.labels = ids),
  });
  const assigneePicker = candidatePicker({
    label: "担当者",
    project: draft.project,
    kind: "member",
    multiple: true,
    value: draft.issue.assignees,
    onChange: (ids) => (draft.issue.assignees = ids),
  });
  const milestonePicker = candidatePicker({
    label: "マイルストーン",
    project: draft.project,
    kind: "milestone",
    multiple: false,
    // GitLab-EE はマイルストーンを名称で受け取る。
    useName: true,
    value: draft.issue.milestone ? [draft.issue.milestone] : [],
    onChange: (ids) => (draft.issue.milestone = ids[0] ?? null),
  });
  const epicPicker = candidatePicker({
    label: "エピック (EE 固有項目)",
    project: draft.project,
    kind: "epic",
    multiple: false,
    value: draft.issue.epic ? [draft.issue.epic] : [],
    onChange: (ids) => (draft.issue.epic = ids[0] ?? null),
  });
  // 固定で選んだ値は、自動へ切り替えて戻したときに復元できるよう別に持つ。
  let fixedIteration = draft.issue.iteration;
  const iterationPicker = candidatePicker({
    label: "イテレーション",
    project: draft.project,
    kind: "iteration",
    multiple: false,
    value: draft.issue.iteration ? [draft.issue.iteration] : [],
    onChange: (ids) => {
      fixedIteration = ids[0] ?? null;
      if (!draft.issue.iteration_by_occurrence) draft.issue.iteration = fixedIteration;
    },
  });
  const iterationStatus = iterationCandidateStatus(draft.project);
  pickers.push(
    labelPicker,
    assigneePicker,
    milestonePicker,
    epicPicker,
    iterationPicker,
    iterationStatus,
  );
  // 計画 9 (D-28): 繰り返しの各回を、その発生日を含むイテレーションへ入れられるようにする。
  const iterationRule = selectField(
    "イテレーションの決め方",
    { fixed: "固定で選ぶ", occurrence: "発生日を含むイテレーションを自動で選ぶ" },
    draft.issue.iteration_by_occurrence ? "occurrence" : "fixed",
    (v) => {
      draft.issue.iteration_by_occurrence = v === "occurrence";
      draft.issue.iteration = draft.issue.iteration_by_occurrence ? null : fixedIteration;
      showIterationRule();
    },
  );
  function showIterationRule() {
    iterationPicker.hidden = draft.issue.iteration_by_occurrence;
    iterationStatus.hidden = !draft.issue.iteration_by_occurrence;
  }
  showIterationRule();
  form.append(labelPicker, assigneePicker, milestonePicker);

  form.append(
    numberField("開始日オフセット (日)", draft.issue.start_offset_days, (v) => {
      draft.issue.start_offset_days = v;
    }),
  );
  form.append(
    numberField("期限オフセット (日)", draft.issue.due_offset_days, (v) => {
      draft.issue.due_offset_days = v;
    }),
  );
  form.append(
    checkField("機密 Issue にする", draft.issue.confidential, (v) => {
      draft.issue.confidential = v;
    }),
  );

  // EC-15: ライセンスティアが許さない場合は EE 固有項目を入力できないようにする。
  const eeAllowed = state.connection ? state.connection.allows_ee_fields : true;
  const eeBox = el("fieldset");
  eeBox.append(el("legend", {}, [text("EE 固有項目")]));
  if (eeAllowed) {
    eeBox.append(epicPicker, iterationRule, iterationPicker, iterationStatus);
    eeBox.append(
      numberField("ウェイト", draft.issue.weight, (v) => (draft.issue.weight = v)),
    );
  } else {
    eeBox.append(
      el("p", { class: "hint" }, [
        text(
          "ライセンスティアがエピック・イテレーション・ウェイト・複数担当者を許可していないため入力できません。",
        ),
      ]),
    );
  }
  form.append(eeBox);

  form.append(patternEditor(draft));
  form.append(
    selectField(
      "営業日調整",
      {
        None: "調整しない",
        PreviousBusinessDay: "直前の営業日に前倒し",
        NextBusinessDay: "直後の営業日に後ろ倒し",
        SkipOccurrence: "その回を発生させない",
      },
      draft.adjustment,
      (v) => (draft.adjustment = v),
    ),
  );
  form.append(
    dateField("有効期間の開始日", draft.active_period.start, (v) => (draft.active_period.start = v)),
  );
  form.append(
    dateField("有効期間の終了日", draft.active_period.end, (v) => (draft.active_period.end = v)),
  );

  const actions = el("div", { class: "actions" });
  const save = button(schedule ? "保存する" : "作成する", async () => {
    try {
      if (schedule) {
        await api.updateSchedule(draft);
      } else {
        draft.id = `s${Date.now().toString(16)}`;
        await api.createSchedule(draft);
      }
      await closeEditor({ discard: true });
      await loadSchedules();
      notify("保存しました", "", true);
    } catch (error) {
      notifyFailure(error);
    }
  });
  save.classList.add("primary");
  actions.append(save);
  actions.append(
    button("閉じる", () => closeEditor()),
  );
  form.append(actions);
  fill(host, [form]);
  editorState.dirty = false;
  // 編集中は一覧を隠す (styles.css)。転がした位置を先頭へ戻し、入力欄から始める。
  $("main").scrollTop = 0;
  nameField.querySelector("input").focus();
}

/**
 * 自動で選ぶ場合に出す、取得済みのイテレーション候補の状態 (計画 9)。
 *
 * 割り当ては展開時に保持済みの候補から行い、展開の中では取得しない (FR-07)。
 * そのため、ここで候補の件数・取得日時・直近のイテレーションを見せ、再取得できるようにする。
 */
function iterationCandidateStatus(initialProject) {
  let project = initialProject;
  const summary = el("span", { class: "hint" });
  const refetch = el("button", { type: "button" }, [text("再取得")]);
  const wrap = el("div", { class: "iteration-status" }, [
    el("p", { class: "hint" }, [
      text(
        "展開するとき、各回の発生日を含むイテレーションを入れます。取得済みの候補から選ぶため、新しいイテレーションができたら再取得してください。",
      ),
    ]),
    el("div", { class: "picker-foot" }, [summary, refetch]),
  ]);

  async function load(refresh) {
    if (!project) {
      summary.textContent = "登録先プロジェクトを先に選んでください";
      refetch.disabled = true;
      return;
    }
    refetch.disabled = false;
    try {
      const list = refresh
        ? await api.refetchCandidates(project, "iteration")
        : await api.candidates(project, "iteration");
      const today = new Date().toISOString().slice(0, 10);
      const dated = list.items.filter((i) => i.start_date && i.due_date);
      const next = orderCandidates("iteration", dated).find((i) => i.due_date >= today);
      summary.textContent =
        dated.length === 0
          ? `期間を持つイテレーションがありません。${list.description}`
          : `${dated.length} 件 (${list.description})${next ? `。直近: ${candidateLabel(next)}` : ""}`;
    } catch (error) {
      notifyFailure(error);
      summary.textContent = error.message;
    }
  }
  refetch.addEventListener("click", () => load(true));
  load(false);
  wrap.reloadFor = (next) => {
    project = next;
    load(false);
  };
  return wrap;
}

function inputField(label, value, onInput) {
  const field = el("input", { type: "text", autocomplete: "off" });
  field.value = value ?? "";
  field.addEventListener("input", () => onInput(field.value));
  return el("label", {}, [text(label), field]);
}

function textareaField(label, value, onInput) {
  const field = el("textarea");
  field.value = value ?? "";
  field.addEventListener("input", () => onInput(field.value));
  return el("label", {}, [text(label), field]);
}

function numberField(label, value, onInput) {
  const field = el("input", { type: "number", min: "0" });
  field.value = value ?? "";
  field.addEventListener("input", () => {
    const parsed = Number.parseInt(field.value, 10);
    onInput(Number.isNaN(parsed) ? null : parsed);
  });
  return el("label", {}, [text(label), field]);
}

function checkField(label, value, onChange) {
  const field = el("input", { type: "checkbox" });
  field.checked = !!value;
  field.addEventListener("change", () => onChange(field.checked));
  return el("label", { class: "check" }, [field, text(label)]);
}

function dateField(label, value, onChange) {
  const field = el("input", { type: "date" });
  field.value = value ?? "";
  field.addEventListener("change", () => onChange(field.value || null));
  return el("label", {}, [text(label), field]);
}

function selectField(label, options, value, onChange) {
  const field = el("select");
  for (const [key, caption] of Object.entries(options)) {
    field.append(el("option", { value: key }, [text(caption)]));
  }
  field.value = value;
  field.addEventListener("change", () => onChange(field.value));
  return el("label", {}, [text(label), field]);
}

// 繰り返しパターンの編集 (FR-04)。7 種類をそのまま選べるようにする。
function patternEditor(draft) {
  const wrap = el("fieldset");
  wrap.append(el("legend", {}, [text("繰り返しパターン")]));
  const kinds = {
    Daily: "毎日",
    EveryBusinessDay: "毎営業日",
    EveryNDays: "N 日ごと",
    Weekly: "毎週・N 週ごと",
    MonthlyByDay: "毎月 日付指定",
    MonthlyByWeekday: "毎月 第 N 曜日",
    Yearly: "毎年",
  };
  const detail = el("div", { class: "form" });
  const current = () =>
    typeof draft.pattern === "string" ? draft.pattern : Object.keys(draft.pattern)[0];

  const select = el("select");
  for (const [key, caption] of Object.entries(kinds)) {
    select.append(el("option", { value: key }, [text(caption)]));
  }
  select.value = current();
  select.addEventListener("change", () => {
    draft.pattern = defaultPattern(select.value);
    renderDetail();
  });
  wrap.append(el("label", {}, [text("種別"), select]), detail);

  function renderDetail() {
    const kind = current();
    const value = typeof draft.pattern === "string" ? null : draft.pattern[kind];
    const parts = [];
    if (kind === "EveryNDays") {
      parts.push(
        numberField("間隔 (日、1〜365)", value.interval, (v) => (value.interval = v ?? 1)),
      );
      parts.push(dateField("起算日", value.anchor, (v) => (value.anchor = v)));
    } else if (kind === "Weekly") {
      parts.push(
        numberField("間隔 (週、1〜52。2 が隔週)", value.interval, (v) => (value.interval = v ?? 1)),
      );
      const days = el("div", { class: "form inline" });
      for (const [key, caption] of Object.entries(WEEKDAYS)) {
        const box = el("input", { type: "checkbox" });
        box.checked = value.weekdays.includes(key);
        box.addEventListener("change", () => {
          const set = new Set(value.weekdays);
          if (box.checked) set.add(key);
          else set.delete(key);
          value.weekdays = Object.keys(WEEKDAYS).filter((k) => set.has(k));
        });
        days.append(el("label", { class: "check" }, [box, text(caption)]));
      }
      parts.push(el("label", {}, [text("曜日"), days]));
      parts.push(
        dateField("起算日 (2 週ごと以上では必須)", value.anchor, (v) => (value.anchor = v)),
      );
    } else if (kind === "MonthlyByDay") {
      const isLast = value.day === "LastDay";
      const lastBox = el("input", { type: "checkbox" });
      lastBox.checked = isLast;
      lastBox.addEventListener("change", () => {
        value.day = lastBox.checked ? "LastDay" : { Day: 1 };
        renderDetail();
      });
      parts.push(el("label", { class: "check" }, [lastBox, text("末日にする")]));
      if (!isLast) {
        parts.push(
          numberField("日付 (1〜31)", value.day.Day, (v) => (value.day = { Day: v ?? 1 })),
        );
      }
      parts.push(
        selectField(
          "その月に存在しない日付のとき",
          { ClampToLastDay: "末日に繰り上げる", SkipMonth: "その月は発生させない" },
          value.on_missing,
          (v) => (value.on_missing = v),
        ),
      );
    } else if (kind === "MonthlyByWeekday") {
      parts.push(
        selectField(
          "第",
          { First: "1", Second: "2", Third: "3", Fourth: "4", Fifth: "5" },
          value.ordinal,
          (v) => (value.ordinal = v),
        ),
      );
      parts.push(selectField("曜日", WEEKDAYS, value.weekday, (v) => (value.weekday = v)));
    } else if (kind === "Yearly") {
      parts.push(numberField("月 (1〜12)", value.month, (v) => (value.month = v ?? 1)));
      parts.push(numberField("日 (1〜31)", value.day, (v) => (value.day = v ?? 1)));
    }
    fill(detail, parts);
  }
  renderDetail();
  return wrap;
}

function defaultPattern(kind) {
  switch (kind) {
    case "Daily":
      return "Daily";
    case "EveryBusinessDay":
      return "EveryBusinessDay";
    case "EveryNDays":
      return { EveryNDays: { interval: 1, anchor: new Date().toISOString().slice(0, 10) } };
    case "Weekly":
      return { Weekly: { interval: 1, weekdays: ["Mon"], anchor: null } };
    case "MonthlyByDay":
      return { MonthlyByDay: { day: { Day: 1 }, on_missing: "SkipMonth" } };
    case "MonthlyByWeekday":
      return { MonthlyByWeekday: { ordinal: "First", weekday: "Mon" } };
    case "Yearly":
      return { Yearly: { month: 4, day: 1 } };
    default:
      return "Daily";
  }
}

// ---- 展開と登録 (FR-07〜FR-10、FR-20) ----

const grid = new PreviewGrid($("#preview-grid"), updatePreviewActions);

// 展開前は、空の表の代わりに次の操作を案内する。
grid.placeholder = () => {
  const box = el("div", { class: "empty-state" }, [
    el("p", { class: "empty-title" }, [text("期間を選んで「展開する」を押してください")]),
    el("p", { class: "empty" }, [
      text("有効な繰り返し定義から登録候補を作り、ここに一覧にします。登録の前に各行を編集できます。"),
    ]),
  ]);
  if (!state.connection?.can_register) {
    box.append(
      el("p", { class: "empty-note" }, [
        text("GitLab-EE に接続していません。展開はできますが、登録には接続が必要です。"),
      ]),
      button("接続画面へ", () => showScreen("connection")),
    );
  }
  return [box];
};
grid.render();

/** 列の表示切り替えの選択肢を作る。接続によって EE 固有項目の有無が変わるため、そのたびに作り直す。 */
function renderColumnMenu() {
  fill(
    $("#column-menu"),
    grid.columnOptions().map((option) => {
      const box = el("input", { type: "checkbox" });
      box.checked = option.shown;
      box.addEventListener("change", () => grid.setColumnShown(option.key, box.checked));
      return el("label", { class: "check" }, [box, text(option.label)]);
    }),
  );
}
renderColumnMenu();

for (const preset of document.querySelectorAll("[data-preset]")) {
  preset.addEventListener("click", async () => {
    try {
      const [start, end] = await api.resolvePreset(preset.dataset.preset);
      $("#range-start").value = start;
      $("#range-end").value = end;
    } catch (error) {
      notifyFailure(error);
    }
  });
}

$("#btn-expand").addEventListener("click", async () => {
  const start = $("#range-start").value;
  const end = $("#range-end").value;
  if (!start || !end) {
    notify("期間を指定してください", "開始日と終了日を入力してください");
    return;
  }
  try {
    const preview = await api.expand(start, end);
    // FR-08 / FR-16: 行の編集で候補から選べるようにする。
    // 候補はプロジェクトごとに 1 つ作り、全ての行から参照する (行数に比例させない)。
    const projects = [...new Set(preview.candidates.map((c) => c.project))];
    const catalog = await buildCatalog(projects);
    grid.load(preview, catalog);
    $("#result-panel").hidden = true;
    const warnings = $("#preview-warnings");
    if (preview.no_enabled_schedules) {
      preview.warnings.push(
        "有効な繰り返しタスク定義がありません。「繰り返し定義」で作成するか有効にしてください。",
      );
    }
    fill(
      warnings,
      preview.warnings.length === 0
        ? []
        : [el("ul", {}, preview.warnings.map((w) => el("li", {}, [text(w)])))],
    );
    warnings.classList.toggle("warnings", preview.warnings.length > 0);
    updatePreviewActions();
  } catch (error) {
    notifyFailure(error);
  }
});

$("#btn-select-all").addEventListener("click", () => grid.selectAll(true));
$("#btn-select-none").addEventListener("click", () => grid.selectAll(false));
$("#show-excluded").addEventListener("change", (event) =>
  grid.setShowExcluded(event.target.checked),
);

function updatePreviewActions() {
  const selected = grid.selectedCount();
  const invalid = grid.invalidReasons();
  $("#preview-count").textContent = `選択 ${selected} 件 / 全 ${grid.entries.length} 件`;
  const canRegister =
    selected > 0 && invalid.length === 0 && !!state.connection?.can_register;
  const reason = canRegister
    ? ""
    : invalid.length > 0
      ? invalid.join(" / ")
      : selected === 0
        ? "登録する行を選んでください"
        : "GitLab-EE に接続してください";
  $("#btn-register").disabled = !canRegister;
  $("#btn-register").title = reason;
  // 押せない理由はボタンの横にも出す。マウスを乗せなくても分かるようにする。
  $("#register-reason").textContent =
    invalid.length > 1 ? `登録できない行が ${invalid.length} 件あります。${invalid[0]} ほか` : reason;
}

/**
 * 一括登録の確認に出す要約。件数・発生日の範囲・登録先ごとの件数・機密 Issue の件数を並べる。
 * 登録先や期間の取り違えに、登録する前に気づけるようにする。
 */
function registrationSummary(rows) {
  const dates = rows.map((r) => r.occurrence_date).sort();
  const first = dates[0];
  const last = dates[dates.length - 1];
  const byProject = new Map();
  for (const row of rows) byProject.set(row.project, (byProject.get(row.project) ?? 0) + 1);
  const confidential = rows.filter((r) => r.confidential).length;

  const list = el("dl", { class: "summary" });
  const item = (term, ...values) => list.append(el("dt", {}, [text(term)]), el("dd", {}, values));
  item("件数", text(`${rows.length} 件`));
  item("発生日", el("span", { class: "mono" }, [text(first === last ? first : `${first} 〜 ${last}`)]));
  item(
    "登録先",
    el(
      "ul",
      { class: "summary-list" },
      [...byProject]
        .sort((a, b) => b[1] - a[1])
        .map(([project, count]) =>
          el("li", {}, [el("span", {}, [text(project)]), el("span", { class: "mono" }, [text(`${count} 件`)])]),
        ),
    ),
  );
  if (confidential > 0) {
    item("機密 Issue", el("strong", { class: "attention" }, [text(`${confidential} 件`)]));
  }
  return [
    el("p", {}, [
      text("GitLab-EE に Issue を作成します。作成した Issue は、このアプリから取り消せません。"),
    ]),
    list,
  ];
}

$("#btn-register").addEventListener("click", async () => {
  const rows = grid.selectedRows();
  if (rows.length === 0) return;
  const confirmed = await confirmAction({
    title: `${rows.length} 件を一括登録します`,
    body: registrationSummary(rows),
    confirmLabel: `${rows.length} 件を登録する`,
    cancelLabel: "戻って確認する",
  });
  if (!confirmed) return;
  await runRegistration(() => api.registerBatch(rows), rows.length);
});

$("#btn-cancel-register").addEventListener("click", () => api.cancelRegistration());

async function runRegistration(action, total) {
  const progress = $("#progress");
  progress.hidden = false;
  fill(progress, [
    el("progress", { max: String(total), value: "0" }),
    el("span", { id: "progress-text" }, [text(`0 / ${total}`)]),
  ]);
  $("#btn-register").disabled = true;
  $("#btn-cancel-register").hidden = false;
  try {
    state.lastBatch = await action();
    renderResults(state.lastBatch);
    // 成功した行はプレビューから外し、二重に登録できないようにする。
    // 失敗と未処理の行は残し、直してから登録し直せるようにする。
    const done = new Set(
      state.lastBatch.rows
        .filter((r) => r.status === "succeeded")
        .map((r) => `${r.schedule_name}|${r.occurrence_date}`),
    );
    if (done.size > 0) {
      grid.removeRegistered((row) => done.has(`${row.schedule_name}|${row.occurrence_date}`));
    }
  } catch (error) {
    notifyFailure(error);
  } finally {
    $("#btn-cancel-register").hidden = true;
    progress.hidden = true;
    updatePreviewActions();
    renderConnection(await api.connectionState());
  }
}

api.listen("registration-progress", (event) => {
  const { completed, total } = event.payload;
  const bar = document.querySelector("#progress progress");
  const label = document.querySelector("#progress-text");
  if (bar) {
    bar.max = String(total);
    bar.value = String(completed);
  }
  if (label) label.textContent = `${completed} / ${total}`;
});

const STATUS_LABELS = { failed: "失敗", not_attempted: "未処理", succeeded: "成功" };
// 対処が要る行ほど上に置く (NFR-16)。
const STATUS_ORDER = { failed: 0, not_attempted: 1, succeeded: 2 };

function renderResults(batch) {
  const host = $("#result-panel");
  host.hidden = false;
  const parts = [];

  const counts = el(
    "div",
    { class: "result-counts" },
    [
      ["failed", batch.failed],
      ["not_attempted", batch.not_attempted],
      ["succeeded", batch.succeeded],
    ].map(([status, count]) =>
      el("span", { class: `count-chip count-${status}${count === 0 ? " zero" : ""}` }, [
        el("strong", {}, [text(String(count))]),
        text(` 件${STATUS_LABELS[status]}`),
      ]),
    ),
  );
  const head = el("div", { class: "result-head" }, [counts]);
  const tools = el("div", { class: "toolbar" });
  const problems = batch.failed + batch.not_attempted;

  const table = el("table", { class: "list" });
  if (problems > 0 && batch.succeeded > 0) {
    const onlyProblems = el("input", { type: "checkbox" });
    onlyProblems.addEventListener("change", () => {
      table.classList.toggle("only-problems", onlyProblems.checked);
    });
    tools.append(el("label", { class: "check" }, [onlyProblems, text("失敗と未処理だけを表示する")]));
  }
  if (batch.can_retry) {
    const retry = button("失敗した行のみ再実行する", async () => {
      const failed = new Set(
        batch.rows
          .filter((r) => r.status === "failed")
          .map((r) => `${r.schedule_name}|${r.occurrence_date}`),
      );
      const confirmed = await confirmAction({
        title: `失敗した ${failed.size} 件を再実行します`,
        body: "失敗した行だけを、プレビューの内容のまま GitLab-EE に登録し直します。成功した行は登録しません。",
        confirmLabel: `${failed.size} 件を再実行する`,
        cancelLabel: "やめる",
      });
      if (!confirmed) return;
      grid.keepOnly((row) => failed.has(`${row.schedule_name}|${row.occurrence_date}`));
      const rows = grid.selectedRows();
      await runRegistration(() => api.retryFailed(rows), rows.length);
    });
    retry.classList.add("primary");
    tools.append(retry);
  }
  head.append(tools);
  parts.push(head);
  if (batch.cancelled) parts.push(el("p", { class: "result-note" }, [text("利用者の操作で中止しました。")]));
  if (batch.aborted) parts.push(el("p", { class: "result-note" }, [text(batch.aborted)]));

  table.append(
    el("thead", {}, [rowOf("th", ["結果", "定義名", "発生日", "登録先", "Issue", "備考"])]),
  );
  const body = el("tbody");
  const rows = [...batch.rows].sort((a, b) => STATUS_ORDER[a.status] - STATUS_ORDER[b.status]);
  for (const row of rows) {
    const tr = el("tr", { class: `result-${row.status}` });
    tr.append(
      el("td", { class: `status-${row.status}` }, [
        text(STATUS_LABELS[row.status]),
      ]),
    );
    tr.append(el("td", {}, [text(row.schedule_name)]));
    tr.append(el("td", {}, [text(row.occurrence_date)]));
    tr.append(el("td", {}, [text(row.project)]));
    const issue = el("td", { class: "issue-cell" });
    if (row.issue_reference) {
      issue.append(
        el("span", { class: "mono" }, [text(row.issue_reference)]),
        openIssueButton(row.project, row.issue_reference),
      );
      if (row.web_url) {
        issue.append(el("span", { class: "cellnote" }, [text(row.web_url)]));
      }
    }
    tr.append(issue);
    const note = el("td");
    if (row.note) note.append(text(row.note));
    if (row.next_action) note.append(el("span", { class: "cellnote" }, [text(row.next_action)]));
    for (const dropped of row.dropped_fields ?? []) {
      note.append(el("span", { class: "cellnote" }, [text(`登録から外した項目: ${dropped}`)]));
    }
    tr.append(note);
    body.append(tr);
  }
  table.append(body);
  parts.push(table);
  fill(host, parts);
}

// ---- 単発登録 (FR-12) ----

async function renderSingleForm() {
  await refreshProjects();
  const today = new Date().toISOString().slice(0, 10);
  const draft = {
    schedule_id: "single",
    schedule_name: "単発登録",
    project: "",
    occurrence_date: today,
    original_date: today,
    title: "",
    description: "",
    labels: [],
    assignee_ids: [],
    milestone: null,
    start_date: null,
    due_date: null,
    confidential: false,
    epic_id: null,
    iteration_id: null,
    weight: null,
  };
  const host = $("#single-form");
  const pickers = [];

  // FR-02: 登録先は一覧から選ぶ。
  const projectField = projectSelect("登録先プロジェクト", state.projects, "", (v) => {
    draft.project = v;
    for (const picker of pickers) picker.reloadFor(v);
  });

  // FR-16: ラベル・担当者・マイルストーンは候補から選ぶ。
  const labelPicker = candidatePicker({
    label: "ラベル",
    project: "",
    kind: "label",
    multiple: true,
    useName: true,
    value: [],
    onChange: (names) => (draft.labels = names),
  });
  const assigneePicker = candidatePicker({
    label: "担当者",
    project: "",
    kind: "member",
    multiple: true,
    value: [],
    onChange: (ids) => {
      draft.assignee_ids = ids
        .map((id) => Number.parseInt(id, 10))
        .filter((n) => !Number.isNaN(n));
    },
  });
  const milestonePicker = candidatePicker({
    label: "マイルストーン",
    project: "",
    kind: "milestone",
    multiple: false,
    useName: true,
    value: [],
    onChange: (ids) => (draft.milestone = ids[0] ?? null),
  });
  const epicPicker = candidatePicker({
    label: "エピック (EE 固有項目)",
    project: "",
    kind: "epic",
    multiple: false,
    value: [],
    onChange: (ids) => {
      const parsed = Number.parseInt(ids[0] ?? "", 10);
      draft.epic_id = Number.isNaN(parsed) ? null : parsed;
    },
  });
  const iterationPicker = candidatePicker({
    label: "イテレーション (EE 固有項目)",
    project: "",
    kind: "iteration",
    multiple: false,
    value: [],
    // 識別子のまま渡す。GraphQL が受け取る形への変換はコマンド境界で行う (ADR-05)。
    onChange: (ids) => (draft.iteration_id = ids[0] ?? null),
  });
  pickers.push(labelPicker, assigneePicker, milestonePicker, epicPicker, iterationPicker);

  const parts = [
    projectField,
    inputField("タイトル", draft.title, (v) => (draft.title = v)),
    textareaField("説明", draft.description, (v) => (draft.description = v)),
    labelPicker,
    assigneePicker,
    milestonePicker,
    dateField("開始日", draft.start_date, (v) => (draft.start_date = v)),
    dateField("期限", draft.due_date, (v) => (draft.due_date = v)),
    checkField("機密 Issue にする", draft.confidential, (v) => (draft.confidential = v)),
  ];
  // FR-12: 入力できる項目は FR-06 と同一にする。
  // EC-15: ライセンスティアが許さない場合は EE 固有項目を出さない。
  if (state.connection ? state.connection.allows_ee_fields : true) {
    const eeBox = el("fieldset");
    eeBox.append(el("legend", {}, [text("EE 固有項目")]));
    eeBox.append(epicPicker, iterationPicker);
    eeBox.append(numberField("ウェイト", draft.weight, (v) => (draft.weight = v)));
    parts.push(eeBox);
  }
  const register = button("登録する", async () => {
    try {
      const row = await api.registerSingle(draft);
      notify(`登録しました ${row.issue_reference ?? ""}`, row.web_url ?? "", true);
    } catch (error) {
      notifyFailure(error);
    }
  });
  register.classList.add("primary");
  parts.push(el("div", { class: "actions" }, [register]));
  fill(host, parts);
}

// ---- 登録先と休日 (FR-02、FR-13、FR-22) ----

async function loadMaster() {
  try {
    const projects = await api.listProjects();
    const host = $("#project-list");
    if (projects.length === 0) {
      fill(host, [el("p", { class: "empty" }, [text("登録先がありません。")])]);
    } else {
      const table = el("table", { class: "list" });
      table.append(el("thead", {}, [rowOf("th", ["識別子", "表示名", "操作"])]));
      const body = el("tbody");
      for (const project of projects) {
        const tr = el("tr");
        tr.append(el("td", {}, [text(project.id)]));
        tr.append(el("td", {}, [text(project.display_name)]));
        const del = button("削除", async () => {
          try {
            const message = await api.projectDeleteConfirmation(project.id);
            const confirmed =
              !message ||
              (await confirmAction({
                title: `登録先「${project.display_name}」を削除します`,
                body: message,
                confirmLabel: "削除する",
                danger: true,
              }));
            if (!confirmed) return;
            await api.deleteProject(project.id);
            await loadMaster();
          } catch (error) {
            notifyFailure(error);
          }
        });
        del.classList.add("danger");
        tr.append(el("td", { class: "row-actions" }, [del]));
        body.append(tr);
      }
      table.append(body);
      fill(host, [table]);
    }
  } catch (error) {
    notifyFailure(error);
  }

  try {
    const holidays = await api.listHolidays();
    const host = $("#holiday-list");
    if (holidays.length === 0) {
      fill(host, [
        el("p", { class: "empty" }, [
          text("休日が登録されていません。営業日調整を使う場合は登録してください。"),
        ]),
      ]);
      return;
    }
    const table = el("table", { class: "list" });
    table.append(el("thead", {}, [rowOf("th", ["日付", "名称", "操作"])]));
    const body = el("tbody");
    for (const holiday of holidays) {
      const tr = el("tr");
      tr.append(el("td", {}, [text(holiday.date)]));
      tr.append(el("td", {}, [text(holiday.name)]));
      const del = button("削除", async () => {
        try {
          await api.deleteHoliday(holiday.date);
          await loadMaster();
        } catch (error) {
          notifyFailure(error);
        }
      });
      del.classList.add("danger");
      tr.append(el("td", { class: "row-actions" }, [del]));
      body.append(tr);
    }
    table.append(body);
    fill(host, [table]);
  } catch (error) {
    notifyFailure(error);
  }
}

$("#btn-add-project").addEventListener("click", async () => {
  const path = $("#project-path").value.trim();
  if (!path) {
    notify("プロジェクトの経路または識別子を入力してください", "");
    return;
  }
  try {
    const project = await api.addProject(path);
    notify(`${project.display_name} を追加しました`, "", true);
    $("#project-path").value = "";
    await loadMaster();
  } catch (error) {
    notifyFailure(error);
  }
});

$("#btn-add-holiday").addEventListener("click", async () => {
  const date = $("#holiday-date").value;
  if (!date) {
    notify("日付を指定してください", "");
    return;
  }
  try {
    await api.addHoliday(date, $("#holiday-name").value.trim());
    $("#holiday-name").value = "";
    await loadMaster();
  } catch (error) {
    notifyFailure(error);
  }
});

$("#btn-import-holidays").addEventListener("click", async () => {
  let path;
  try {
    path = await api.chooseFileToOpen(
      "取り込む休日のファイル (1 行 1 件、YYYY-MM-DD,名称)",
      api.FILE_FILTERS.text,
    );
  } catch (error) {
    notifyFailure(error);
    return;
  }
  if (!path) return;
  try {
    const outcome = await api.importHolidays(path);
    const parts = [`${outcome.added} 件を取り込みました`];
    if (outcome.malformed_lines.length > 0) {
      parts.push(`解釈できなかった行: ${outcome.malformed_lines.join(", ")}`);
    }
    if (outcome.duplicate_lines.length > 0) {
      parts.push(`重複した行: ${outcome.duplicate_lines.join(", ")}`);
    }
    notify(parts.join(" / "), "", outcome.malformed_lines.length === 0);
    await loadMaster();
  } catch (error) {
    notifyFailure(error);
  }
});

$("#btn-export-holidays").addEventListener("click", async () => {
  let path;
  try {
    path = await api.chooseFileToSave("休日の書き出し先", "holidays.txt", api.FILE_FILTERS.text);
  } catch (error) {
    notifyFailure(error);
    return;
  }
  if (!path) return;
  try {
    const count = await api.exportHolidays(path);
    notify(`${count} 件を書き出しました`, path, true);
  } catch (error) {
    notifyFailure(error);
  }
});

// ---- 登録履歴 (FR-19、FR-23) ----

const historySelection = new Set();

async function searchHistory() {
  historySelection.clear();
  try {
    const entries = await api.listHistory({
      scheduleName: $("#hist-name").value.trim() || null,
      from: $("#hist-from").value || null,
      to: $("#hist-to").value || null,
      project: $("#hist-project").value.trim() || null,
    });
    const host = $("#history-list");
    if (entries.length === 0) {
      fill(host, [el("p", { class: "empty" }, [text("登録履歴は 0 件です。")])]);
      return;
    }
    const table = el("table", { class: "list" });
    table.append(
      el("thead", {}, [
        rowOf("th", ["選択", "登録日時", "定義名", "発生日", "登録先", "Issue", "操作"]),
      ]),
    );
    const body = el("tbody");
    for (const entry of entries) {
      const key = `${entry.schedule_id}|${entry.occurrence_date}`;
      const tr = el("tr");
      const box = el("input", { type: "checkbox", "aria-label": "この履歴を選択する" });
      box.addEventListener("change", () => {
        if (box.checked) historySelection.add(key);
        else historySelection.delete(key);
      });
      tr.append(el("td", {}, [box]));
      tr.append(
        el("td", { class: "mono" }, [text(String(entry.registered_at).replace("T", " "))]),
      );
      tr.append(el("td", {}, [text(entry.schedule_name)]));
      tr.append(el("td", { class: "mono" }, [text(entry.occurrence_date)]));
      tr.append(el("td", {}, [text(entry.project)]));
      tr.append(el("td", { class: "mono" }, [text(entry.issue_reference)]));
      tr.append(
        el("td", { class: "row-actions" }, [openIssueButton(entry.project, entry.issue_reference)]),
      );
      body.append(tr);
    }
    table.append(body);
    fill(host, [table]);
  } catch (error) {
    notifyFailure(error);
  }
}

/** 作成された Issue を既定のブラウザで開くボタン (FR-19)。URL は境界側で組み立てる。 */
function openIssueButton(project, issueReference) {
  const node = button("開く", async () => {
    try {
      await api.openIssue(project, issueReference);
    } catch (error) {
      notifyFailure(error);
    }
  });
  node.setAttribute("aria-label", `${project} ${issueReference} をブラウザで開く`);
  return node;
}

$("#btn-search-history").addEventListener("click", () => searchHistory());

$("#btn-delete-history").addEventListener("click", async () => {
  if (historySelection.size === 0) {
    notify("削除する履歴を選んでください", "一覧の左端で選択してください");
    return;
  }
  const keys = [...historySelection].map((k) => {
    const [id, date] = k.split("|");
    return [id, date];
  });
  try {
    const message = await api.historyDeleteConfirmation(keys);
    const confirmed = await confirmAction({
      title: `登録履歴 ${keys.length} 件を削除します`,
      body: message,
      confirmLabel: "削除する",
      danger: true,
    });
    if (!confirmed) return;
    const removed = await api.deleteHistory(keys);
    notify(`${removed} 件を削除しました`, "", true);
    await searchHistory();
  } catch (error) {
    notifyFailure(error);
  }
});

// ---- 起動 ----

(async function start() {
  try {
    applySettingsToForm(await api.loadSettings());
    renderConnection(await api.connectionState());
  } catch (error) {
    notifyFailure(error);
  }
  updatePreviewActions();
  showScreen("connection");
  $("#gitlab-url").focus();
})();
