// プレビューの表 (FR-07、FR-08)。
//
// 行ごとに登録する・しないを切り替え、各項目をその回に限り編集できる。
// 編集は元の定義を変更しない。画面が持つのは展開結果の写しだけである。
//
// 最大 500 行 (NFR-04) をそのまま DOM へ出す。仮想化を入れていないのは、
// 500 行 × 11 列で約 6,000 要素にとどまり、計測で足りると確かめたためである
// (設計書 R-6、plan.md の実行記録を参照)。

import { el, text } from "./ui.js";

const COLUMNS = [
  { key: "select", label: "登録", kind: "check" },
  { key: "schedule_name", label: "定義名", kind: "readonly" },
  { key: "occurrence_date", label: "発生日", kind: "date-readonly" },
  { key: "project", label: "登録先", kind: "readonly" },
  { key: "title", label: "タイトル", kind: "text" },
  { key: "description", label: "説明", kind: "text" },
  { key: "labels", label: "ラベル", kind: "list" },
  { key: "milestone", label: "マイルストーン", kind: "text-opt" },
  { key: "start_date", label: "開始日", kind: "date" },
  { key: "due_date", label: "期限", kind: "date" },
  { key: "weight", label: "ウェイト", kind: "number" },
  { key: "confidential", label: "機密", kind: "check" },
];

/** タイトルの上限 (EC-02)。コードポイント数で数える。 */
const TITLE_MAX = 255;

function charCount(value) {
  return Array.from(value ?? "").length;
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
  }

  /** 展開結果を取り込む。 */
  load(preview) {
    this.entries = preview.candidates.map((row) => ({
      row: structuredClone(row),
      selected: true,
      excluded: false,
    }));
    this.excluded = preview.excluded ?? [];
    this.render();
  }

  /** 結果一覧から失敗した行だけを残す (FR-20)。 */
  keepOnly(predicate) {
    this.entries = this.entries.filter((e) => predicate(e.row));
    this.entries.forEach((e) => {
      e.selected = true;
    });
    this.render();
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
    if (this.entries.length === 0 && (!this.showExcluded || this.excluded?.length === 0)) {
      this.host.append(el("p", { class: "empty" }, [text("登録候補はありません。")]));
      return;
    }

    const table = el("table", { class: "grid" });
    const head = el("tr");
    for (const column of COLUMNS) {
      head.append(el("th", { scope: "col" }, [text(column.label)]));
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

  renderRow(entry, index) {
    const length = charCount(entry.row.title);
    const invalid = entry.selected && (length === 0 || length > TITLE_MAX);
    const tr = el("tr", invalid ? { class: "invalid" } : {});

    for (const column of COLUMNS) {
      tr.append(this.renderCell(column, entry, index));
    }
    return tr;
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
            this.render();
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
        const field = el("input", {
          type: "text",
          "aria-label": `${column.label} (${row.schedule_name} ${row.occurrence_date})`,
        });
        field.value = row[column.key] ?? "";
        field.addEventListener("input", () => {
          row[column.key] = column.kind === "text-opt" && field.value === "" ? null : field.value;
          if (column.key === "title") {
            this.render();
          }
          notify();
        });
        td.append(field);
        if (column.key === "title") {
          const length = charCount(row.title);
          if (length === 0) {
            td.append(el("span", { class: "cellnote" }, [text("タイトルを入力してください")]));
          } else if (length > TITLE_MAX) {
            td.append(
              el("span", { class: "cellnote" }, [
                text(`${TITLE_MAX} 文字以内にしてください (現在 ${length} 文字)`),
              ]),
            );
          }
        }
        break;
      }
      case "list": {
        const field = el("input", {
          type: "text",
          "aria-label": `${column.label} (読点で区切る)`,
        });
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
    return td;
  }

  renderExcludedRow(excluded) {
    const tr = el("tr", { class: "excluded" });
    tr.append(el("td", {}, [text("登録済み")]));
    tr.append(el("td", {}, [text(excluded.schedule_id)]));
    tr.append(el("td", {}, [text(excluded.occurrence_date)]));
    for (let i = 3; i < COLUMNS.length; i += 1) {
      tr.append(el("td"));
    }
    return tr;
  }
}
