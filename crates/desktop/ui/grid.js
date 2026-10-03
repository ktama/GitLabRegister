// プレビューの表 (FR-07、FR-08)。
//
// 行ごとに登録する・しないを切り替え、各項目をその回に限り編集できる。
// 編集は元の定義を変更しない。画面が持つのは展開結果の写しだけである。
//
// 最大 500 行 (NFR-04) をそのまま DOM へ出す。仮想化を入れていないのは、
// 552 行 × 15 列 (約 16,000 要素、うち入力欄 6,600) でも展開から描画まで 0.4 秒弱、
// 1 文字の入力の処理が 10 ms 未満にとどまり、計測で足りると確かめたためである
// (設計書 R-6、plan.md の実行記録 10 を参照)。列を隠すと要素はさらに減る。

import { el, text } from "./ui.js";

const COLUMNS = [
  { key: "select", label: "登録", kind: "check" },
  { key: "schedule_name", label: "定義名", kind: "readonly" },
  { key: "occurrence_date", label: "発生日", kind: "date-readonly" },
  { key: "project", label: "登録先", kind: "readonly" },
  { key: "title", label: "タイトル", kind: "text" },
  { key: "description", label: "説明", kind: "multiline" },
  { key: "labels", label: "ラベル", kind: "list", catalog: "labels" },
  { key: "milestone", label: "マイルストーン", kind: "text-opt", catalog: "milestones" },
  { key: "assignee_ids", label: "担当者", kind: "id-list", catalog: "members" },
  { key: "start_date", label: "開始日", kind: "date" },
  { key: "due_date", label: "期限", kind: "date" },
  { key: "epic_id", label: "エピック", kind: "id-opt", catalog: "epics" },
  { key: "iteration_id", label: "イテレーション", kind: "id-opt", catalog: "iterations" },
  { key: "weight", label: "ウェイト", kind: "number" },
  { key: "confidential", label: "機密", kind: "check" },
];

/** 表示を切り替えられる列。登録の判断に欠かせない列 (登録・定義名・発生日・登録先・タイトル) は常に出す。 */
const TOGGLEABLE_KEYS = new Set([
  "description",
  "labels",
  "milestone",
  "assignee_ids",
  "start_date",
  "due_date",
  "epic_id",
  "iteration_id",
  "weight",
  "confidential",
]);

/** EE 固有項目の列。ティアが許さない接続では出さない (EC-15)。 */
const EE_KEYS = new Set(["epic_id", "iteration_id", "weight"]);

/** 隠した列を覚えておく場所。この端末の画面の好みだけで、登録の内容には関わらない。 */
const HIDDEN_COLUMNS_KEY = "preview.hiddenColumns";

function loadHiddenColumns() {
  try {
    const saved = JSON.parse(localStorage.getItem(HIDDEN_COLUMNS_KEY) ?? "[]");
    return new Set(Array.isArray(saved) ? saved.filter((k) => TOGGLEABLE_KEYS.has(k)) : []);
  } catch {
    return new Set();
  }
}

function saveHiddenColumns(hidden) {
  try {
    localStorage.setItem(HIDDEN_COLUMNS_KEY, JSON.stringify([...hidden]));
  } catch {
    // 覚えられなくても表示は切り替わる。次回は全列から始まる。
  }
}

/** 識別子で指定する項目は、識別子を数値で送る。イテレーションだけ文字列のまま送る。 */
const NUMERIC_ID_KEYS = new Set(["assignee_ids", "epic_id"]);

/** タイトルの上限 (EC-02)。コードポイント数で数える。 */
const TITLE_MAX = 255;

function charCount(value) {
  return Array.from(value ?? "").length;
}

/** 候補一覧を引くための名前。プロジェクト名をそのまま使えないので符号化する。 */
function catalogId(project, kind) {
  const safe = Array.from(project)
    .map((c) => c.codePointAt(0).toString(16))
    .join("");
  return `cat-${kind}-${safe}`;
}

export class PreviewGrid {
  /**
   * @param {HTMLElement} host 表を置く場所
   * @param {() => void} onChange 選択や内容が変わったときの通知
   */
  constructor(host, onChange) {
    this.host = host;
    this.onChange = onChange;
    /** @type {Array<{row: object, selected: boolean, excluded: boolean}>} */
    this.entries = [];
    this.showExcluded = false;
    /** プロジェクトごとの候補 (FR-16)。 */
    this.catalog = {};
    /** 展開を 1 度でも行ったか。行う前は placeholder の案内を出す。 */
    this.loaded = false;
    /** 展開前に出す案内。画面側が接続の状態に合わせて組み立てる。 */
    this.placeholder = () => [];
    /** この展開から登録を終えて外した行の数。0 件になった理由の表示に使う。 */
    this.registeredCount = 0;
    /** 利用者が隠した列 (計画 9)。 */
    this.hiddenColumns = loadHiddenColumns();
    /** EE 固有項目を使える接続か (EC-15)。接続前は使える前提で出す。 */
    this.eeAllowed = true;
  }

  /** いま表に出す列。 */
  visibleColumns() {
    return COLUMNS.filter(
      (c) => !(EE_KEYS.has(c.key) && !this.eeAllowed) && !this.hiddenColumns.has(c.key),
    );
  }

  /** 列の表示切り替えの選択肢。ティアが許さない EE 固有項目は選択肢にも出さない。 */
  columnOptions() {
    return COLUMNS.filter((c) => TOGGLEABLE_KEYS.has(c.key))
      .filter((c) => !(EE_KEYS.has(c.key) && !this.eeAllowed))
      .map((c) => ({ key: c.key, label: c.label, shown: !this.hiddenColumns.has(c.key) }));
  }

  setColumnShown(key, shown) {
    if (!TOGGLEABLE_KEYS.has(key)) return;
    if (shown) this.hiddenColumns.delete(key);
    else this.hiddenColumns.add(key);
    saveHiddenColumns(this.hiddenColumns);
    if (this.loaded) this.render();
  }

  setEeAllowed(allowed) {
    if (this.eeAllowed === allowed) return;
    this.eeAllowed = allowed;
    if (this.loaded) this.render();
  }

  /** 展開結果と候補を取り込む。 */
  load(preview, catalog = {}) {
    this.entries = preview.candidates.map((row) => ({
      row: structuredClone(row),
      selected: true,
      excluded: false,
    }));
    this.excluded = preview.excluded ?? [];
    this.catalog = catalog;
    this.loaded = true;
    this.registeredCount = 0;
    this.render();
  }

  /**
   * 候補一覧を 1 組だけ作る (FR-16)。
   *
   * 各行に選択欄を置くと要素数が行数に比例して増える。
   * 共有の一覧を参照させることで、候補の要素数は行数に依存しなくなる。
   */
  renderCatalog() {
    const lists = [];
    for (const [project, sets] of Object.entries(this.catalog)) {
      for (const [key, values] of Object.entries(sets)) {
        const id = catalogId(project, key);
        const node = el("datalist", { id });
        for (const value of values) {
          // 名称で指定する項目は文字列、識別子で指定する項目は対で持つ。
          // どちらも画面には名前を出す。
          const shown = typeof value === "string" ? value : value?.name;
          if (!shown) continue;
          // イテレーションは期間を併記した表示名を持つ。値は名前のまま、期間を補足として見せる。
          const label = typeof value === "object" && value.label !== shown ? value.label : null;
          node.append(el("option", label ? { value: shown, label } : { value: shown }));
        }
        lists.push(node);
      }
    }
    return lists;
  }

  /** 結果一覧から失敗した行だけを残す (FR-20)。 */
  keepOnly(predicate) {
    this.entries = this.entries.filter((e) => predicate(e.row));
    this.entries.forEach((e) => {
      e.selected = true;
    });
    this.render();
  }

  /**
   * 登録を終えた行を外す。
   *
   * 残すと同じ行をもう一度一括登録でき、同じ Issue が二重に作られる。
   * 履歴には登録済みとして残るため、次の展開でも候補に戻らない (FR-11)。
   */
  removeRegistered(predicate) {
    const before = this.entries.length;
    this.entries = this.entries.filter((e) => !predicate(e.row));
    this.registeredCount += before - this.entries.length;
    this.render();
    this.onChange();
  }

  setShowExcluded(show) {
    this.showExcluded = show;
    this.render();
  }

  selectAll(selected) {
    this.entries.forEach((e) => {
      e.selected = selected;
    });
    this.render();
  }

  /** 登録する行。タイトルが空の行は含めない (FR-08)。 */
  selectedRows() {
    return this.entries
      .filter((e) => e.selected && charCount(e.row.title) > 0)
      .map((e) => e.row);
  }

  selectedCount() {
    return this.entries.filter((e) => e.selected).length;
  }

  /** 登録できない行の理由。タイトル未入力と文字数超過 (EC-01、EC-02)。 */
  invalidReasons() {
    const reasons = [];
    for (const entry of this.entries) {
      if (!entry.selected) continue;
      const length = charCount(entry.row.title);
      if (length === 0) {
        reasons.push(`${entry.row.schedule_name} ${entry.row.occurrence_date}: タイトルが未入力です`);
      } else if (length > TITLE_MAX) {
        reasons.push(
          `${entry.row.schedule_name} ${entry.row.occurrence_date}: タイトルが ${TITLE_MAX} 文字を超えています (現在 ${length} 文字)`,
        );
      }
    }
    return reasons;
  }

  render() {
    this.host.replaceChildren();
    if (!this.loaded) {
      this.host.append(...this.placeholder());
      return;
    }
    if (this.entries.length === 0 && (!this.showExcluded || this.excluded?.length === 0)) {
      if (this.registeredCount > 0) {
        this.host.append(
          el("div", { class: "empty-state" }, [
            el("p", { class: "empty-title" }, [text("選択した行はすべて登録しました")]),
            el("p", { class: "empty" }, [
              text(`${this.registeredCount} 件を登録しました。結果は下の一覧で確認できます。`),
            ]),
          ]),
        );
        return;
      }
      const state = el("div", { class: "empty-state" }, [
        el("p", { class: "empty-title" }, [text("この期間の登録候補はありません")]),
      ]);
      // 履歴で除外された発生日があるなら、0 件の理由として示す (FR-19)。
      if (this.excluded?.length > 0) {
        state.append(
          el("p", { class: "empty" }, [
            text(
              `登録済みの発生日が ${this.excluded.length} 件あります。「除外された発生日も表示する」で確認できます。`,
            ),
          ]),
        );
      }
      this.host.append(state);
      return;
    }

    for (const list of this.renderCatalog()) {
      this.host.append(list);
    }
    const table = el("table", { class: "grid" });
    const head = el("tr");
    for (const column of this.visibleColumns()) {
      head.append(el("th", { scope: "col", "data-col": column.key }, [text(column.label)]));
    }
    table.append(el("thead", {}, [head]));

    const body = el("tbody");
    // 500 行までは一度に組み立てる。断片へまとめて追加し、再描画を 1 回に抑える。
    const fragment = document.createDocumentFragment();
    this.entries.forEach((entry, index) => {
      fragment.append(this.renderRow(entry, index));
    });
    if (this.showExcluded) {
      for (const excluded of this.excluded ?? []) {
        fragment.append(this.renderExcludedRow(excluded));
      }
    }
    body.append(fragment);
    table.append(body);
    this.host.append(table);
  }

  /** 候補の対 ({id, name}) を返す。無ければ空。 */
  pairs(project, kind) {
    const values = this.catalog?.[project]?.[kind];
    return Array.isArray(values) ? values.filter((v) => v && typeof v === "object") : [];
  }

  /** 識別子を表示名に直す。候補にない識別子はそのまま見せる。 */
  nameOf(project, kind, id) {
    if (id === null || id === undefined || id === "") return "";
    const hit = this.pairs(project, kind).find((p) => String(p.id) === String(id));
    return hit ? hit.name : String(id);
  }

  /** 表示名を識別子に直す。候補にない名前は、数値であればそのまま識別子として扱う。 */
  idOf(project, kind, name, numeric) {
    const trimmed = (name ?? "").trim();
    if (trimmed === "") return null;
    const hit = this.pairs(project, kind).find((p) => p.name === trimmed);
    const raw = hit ? hit.id : trimmed;
    if (!numeric) return String(raw);
    const parsed = Number.parseInt(raw, 10);
    return Number.isNaN(parsed) ? null : parsed;
  }

  hasCatalog(project, kind) {
    const values = this.catalog?.[project]?.[kind];
    return Array.isArray(values) && values.length > 0;
  }


  renderRow(entry, index) {
    const tr = el("tr");
    for (const column of this.visibleColumns()) {
      tr.append(this.renderCell(column, entry, index));
    }
    this.updateRowState(tr, entry);
    return tr;
  }

  /**
   * 行の見た目だけを更新する。
   *
   * 表全体を作り直すと、入力中の欄が置き換わって焦点が外れる。
   * タイトルの入力と登録の切り替えでは、その行のクラスと注記だけを書き換える。
   */
  updateRowState(tr, entry) {
    const length = charCount(entry.row.title);
    const invalid = entry.selected && (length === 0 || length > TITLE_MAX);
    // 登録しない行は "off"、登録できない行は "invalid" として見た目を分ける。
    tr.className = invalid ? "invalid" : entry.selected ? "" : "off";
    const note = tr.querySelector('[data-note="title"]');
    if (!note) return;
    note.textContent =
      length === 0
        ? "タイトルを入力してください"
        : length > TITLE_MAX
          ? `${TITLE_MAX} 文字以内にしてください (現在 ${length} 文字)`
          : "";
    note.hidden = note.textContent === "";
  }

  renderCell(column, entry, index) {
    const row = entry.row;
    const td = el("td");
    const notify = () => {
      this.onChange();
    };

    switch (column.kind) {
      case "check": {
        if (column.key === "select") {
          const box = el("input", {
            type: "checkbox",
            "aria-label": `${row.schedule_name} ${row.occurrence_date} を登録する`,
          });
          box.checked = entry.selected;
          box.addEventListener("change", () => {
            entry.selected = box.checked;
            this.updateRowState(box.closest("tr"), entry);
            notify();
          });
          td.append(box);
        } else {
          const box = el("input", { type: "checkbox", "aria-label": "機密 Issue にする" });
          box.checked = !!row.confidential;
          box.addEventListener("change", () => {
            row.confidential = box.checked;
            notify();
          });
          td.append(box);
        }
        break;
      }
      case "readonly":
        td.append(text(row[column.key] ?? ""));
        break;
      case "date-readonly": {
        td.append(text(row.occurrence_date));
        // 営業日調整が効いた行は、調整前の日付を併記する (FR-05)。
        if (row.original_date && row.original_date !== row.occurrence_date) {
          td.append(el("span", { class: "cellnote" }, [text(`調整前 ${row.original_date}`)]));
        }
        break;
      }
      case "text":
      case "text-opt": {
        const attrs = {
          type: "text",
          "aria-label": `${column.label} (${row.schedule_name} ${row.occurrence_date})`,
        };
        // FR-16: マイルストーンは候補から選べるようにする。自由入力も残す。
        if (column.catalog && this.hasCatalog(row.project, column.catalog)) {
          attrs.list = catalogId(row.project, column.catalog);
        }
        const field = el("input", attrs);
        field.value = row[column.key] ?? "";
        field.addEventListener("input", () => {
          row[column.key] = column.kind === "text-opt" && field.value === "" ? null : field.value;
          if (column.key === "title") {
            this.updateRowState(field.closest("tr"), entry);
          }
          notify();
        });
        td.append(field);
        if (column.key === "title") {
          // 文面は updateRowState が書き込む。
          td.append(el("span", { class: "cellnote", "data-note": "title" }));
        }
        break;
      }
      case "multiline": {
        // 説明は複数行になり得る。1 行の入力欄では改行を入れられないため、文章欄にする。
        // 高さは内容に合わせて伸び、上限を超えた分は欄の中で転がす (styles.css)。
        const field = el("textarea", {
          rows: "1",
          "aria-label": `${column.label} (${row.schedule_name} ${row.occurrence_date})`,
        });
        field.value = row[column.key] ?? "";
        field.addEventListener("input", () => {
          row[column.key] = field.value;
          notify();
        });
        td.append(field);
        break;
      }
      case "list": {
        const attrs = {
          type: "text",
          "aria-label": `${column.label} (読点で区切る)`,
        };
        // FR-16: ラベルは候補から選べるようにする。複数指定のため読点で区切る。
        if (column.catalog && this.hasCatalog(row.project, column.catalog)) {
          attrs.list = catalogId(row.project, column.catalog);
        }
        const field = el("input", attrs);
        field.value = (row[column.key] ?? []).join(", ");
        field.addEventListener("input", () => {
          row[column.key] = field.value
            .split(",")
            .map((s) => s.trim())
            .filter((s) => s.length > 0);
          notify();
        });
        td.append(field);
        break;
      }
      // FR-08 / FR-16: 識別子で指定する項目。画面には名前を出し、保存の直前に識別子へ戻す。
      case "id-list": {
        const attrs = {
          type: "text",
          "aria-label": `${column.label} (読点で区切る)`,
        };
        if (column.catalog && this.hasCatalog(row.project, column.catalog)) {
          attrs.list = catalogId(row.project, column.catalog);
        }
        const field = el("input", attrs);
        field.value = (row[column.key] ?? [])
          .map((id) => this.nameOf(row.project, column.catalog, id))
          .join(", ");
        field.addEventListener("input", () => {
          row[column.key] = field.value
            .split(",")
            .map((name) => this.idOf(row.project, column.catalog, name, true))
            .filter((id) => id !== null);
          notify();
        });
        td.append(field);
        break;
      }
      case "id-opt": {
        const attrs = {
          type: "text",
          "aria-label": `${column.label} (${row.schedule_name} ${row.occurrence_date})`,
        };
        if (column.catalog && this.hasCatalog(row.project, column.catalog)) {
          attrs.list = catalogId(row.project, column.catalog);
        }
        const field = el("input", attrs);
        field.value = this.nameOf(row.project, column.catalog, row[column.key]);
        field.addEventListener("input", () => {
          row[column.key] = this.idOf(
            row.project,
            column.catalog,
            field.value,
            NUMERIC_ID_KEYS.has(column.key),
          );
          // 利用者が選び直したら、自動で割り当てられなかった印を外す。
          if (column.key === "iteration_id" && row.iteration_unresolved) {
            row.iteration_unresolved = false;
            td.querySelector(".cellnote")?.remove();
          }
          notify();
        });
        td.append(field);
        // 計画 9: 発生日を含むイテレーションが見つからなかった行に印を付ける。
        if (column.key === "iteration_id" && row.iteration_unresolved) {
          td.append(
            el("span", { class: "cellnote" }, [text("発生日を含むイテレーションがありません")]),
          );
        }
        break;
      }
      case "date": {
        const field = el("input", {
          type: "date",
          "aria-label": `${column.label} (${row.schedule_name} ${row.occurrence_date})`,
        });
        field.value = row[column.key] ?? "";
        field.addEventListener("change", () => {
          row[column.key] = field.value === "" ? null : field.value;
          notify();
        });
        td.append(field);
        break;
      }
      case "number": {
        const field = el("input", { type: "number", min: "0", "aria-label": column.label });
        field.value = row[column.key] ?? "";
        field.addEventListener("input", () => {
          const parsed = Number.parseInt(field.value, 10);
          row[column.key] = Number.isNaN(parsed) || parsed < 0 ? null : parsed;
          notify();
        });
        td.append(field);
        break;
      }
      default:
        td.append(text(String(row[column.key] ?? "")));
    }
    td.dataset.row = String(index);
    // 列ごとの幅と書体を styles.css から当てるための目印。
    td.dataset.col = column.key;
    return td;
  }

  renderExcludedRow(excluded) {
    const tr = el("tr", { class: "excluded" });
    tr.append(el("td", {}, [text("登録済み")]));
    tr.append(el("td", {}, [text(excluded.schedule_name ?? excluded.schedule_id)]));
    tr.append(el("td", {}, [text(excluded.occurrence_date)]));
    for (let i = 3; i < this.visibleColumns().length; i += 1) {
      tr.append(el("td"));
    }
    return tr;
  }
}
