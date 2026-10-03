// 候補からの選択 (FR-02、FR-16)。
//
// 候補を取得できている項目は一覧から選ぶ。取得していない、または到達できない場合は
// 直接入力へ倒す。どちらの場合も取得日時と鮮度を併記する (EC-19)。

import * as api from "./api.js";
import { el, fill, notifyFailure, text } from "./ui.js";

/**
 * 候補を画面に出すときの名前 (計画 9)。
 *
 * イテレーションは名前より期間で見分けるため、期間を持つものは期間を併記する。
 * 題名がなく期間を名前にしている候補には、同じ期間を重ねて付けない。
 */
export function candidateLabel(item) {
  if (!item.start_date || !item.due_date) return item.name;
  const period = `${item.start_date}〜${item.due_date}`;
  return item.name === period ? period : `${item.name} (${period})`;
}

/**
 * 候補を画面に並べる順 (計画 9)。
 *
 * イテレーションは、進行中と今後のものを開始日の早い順に先に並べ、終わったものを新しい順に後ろへ回す。
 * 期間を持たない候補は最後に置く。ほかの種別は取得した順のままにする。
 */
export function orderCandidates(kind, items, today = new Date().toISOString().slice(0, 10)) {
  if (kind !== "iteration") return items;
  const rank = (item) => {
    if (!item.start_date || !item.due_date) return 2;
    return item.due_date >= today ? 0 : 1;
  };
  return [...items].sort((a, b) => {
    const byRank = rank(a) - rank(b);
    if (byRank !== 0) return byRank;
    if (rank(a) === 0) return a.start_date.localeCompare(b.start_date);
    if (rank(a) === 1) return b.start_date.localeCompare(a.start_date);
    return 0;
  });
}

/** 登録先プロジェクトの選択 (FR-02)。登録済みの一覧から選ぶ。 */
export function projectSelect(labelText, projects, current, onChange) {
  const select = el("select");
  select.append(el("option", { value: "" }, [text("(選択してください)")]));
  for (const project of projects) {
    select.append(el("option", { value: project.display_name }, [text(project.display_name)]));
  }
  // 登録済みにない値が入っている場合も失わない。
  if (current && !projects.some((p) => p.display_name === current)) {
    select.append(el("option", { value: current }, [text(`${current} (一覧にありません)`)]));
  }
  select.value = current ?? "";
  select.addEventListener("change", () => onChange(select.value));

  const wrap = el("label", {}, [text(labelText), select]);
  if (projects.length === 0) {
    wrap.append(
      el("span", { class: "hint" }, [
        text("登録先がありません。「登録先と休日」で追加してください。"),
      ]),
    );
  }
  return wrap;
}

/**
 * 候補から選ぶ入力を作る (FR-16)。
 *
 * @param {object} options
 * @param {string} options.label 画面に出す名前
 * @param {string} options.project 登録先プロジェクト
 * @param {string} options.kind 候補の種別
 * @param {boolean} options.multiple 複数選べるか
 * @param {boolean} options.useName 値に名前を使う。GitLab-EE が名前で受け取る項目に使う
 * @param {string[]} options.value 選択済みの値
 * @param {(ids: string[]) => void} options.onChange
 */
export function candidatePicker(options) {
  const host = el("div", { class: "picker" });
  const wrap = el("label", {}, [text(options.label), host]);
  let selected = [...(options.value ?? [])];
  let list = { items: [], freshness: "never_fetched", description: "" };

  async function load(refetch) {
    if (!options.project) {
      list = {
        items: [],
        freshness: "never_fetched",
        description: "登録先プロジェクトを先に選んでください",
      };
      render();
      return;
    }
    try {
      list = refetch
        ? await api.refetchCandidates(options.project, options.kind)
        : await api.candidates(options.project, options.kind);
    } catch (error) {
      notifyFailure(error);
      list = { items: [], freshness: "never_fetched", description: error.message };
    }
    render();
  }

  function commit() {
    options.onChange([...selected]);
  }

  function render() {
    const parts = [];
    if (list.items.length === 0) {
      // FR-16: 候補がない場合は直接入力できるようにする。
      const field = el("input", {
        type: "text",
        autocomplete: "off",
        placeholder: options.multiple ? "読点で区切って入力" : "識別子を入力",
      });
      field.value = selected.join(", ");
      field.addEventListener("input", () => {
        selected = field.value
          .split(",")
          .map((s) => s.trim())
          .filter((s) => s.length > 0);
        commit();
      });
      parts.push(field);
    } else if (options.multiple) {
      parts.push(multiSelect());
    } else {
      parts.push(singleSelect());
    }

    const note = el("span", { class: "hint" }, [text(list.description)]);
    const refetch = el("button", { type: "button" }, [text("再取得")]);
    refetch.addEventListener("click", () => load(true));
    parts.push(el("div", { class: "picker-foot" }, [note, refetch]));
    fill(host, parts);
  }

  /**
   * 候補から保存する値を取り出す。
   *
   * GitLab-EE はラベルとマイルストーンを名称で受け取り、担当者とエピックを識別子で受け取る。
   * 組み込みの Object.prototype と重なる名前は使わない。重なると、定義を書き忘れても
   * 参照エラーにならず、呼び出した時点で初めて壊れる。
   */
  const candidateValue = (item) => (options.useName ? item.name : item.id);

  function singleSelect() {
    const select = el("select");
    select.append(el("option", { value: "" }, [text("(指定しない)")]));
    for (const item of orderCandidates(options.kind, list.items)) {
      select.append(el("option", { value: candidateValue(item) }, [text(candidateLabel(item))]));
    }
    const current = selected[0] ?? "";
    if (current && !list.items.some((i) => candidateValue(i) === current)) {
      select.append(el("option", { value: current }, [text(`${current} (候補にありません)`)]));
    }
    select.value = current;
    select.addEventListener("change", () => {
      selected = select.value ? [select.value] : [];
      commit();
    });
    return select;
  }

  function multiSelect() {
    const box = el("div", { class: "picker-list" });
    const filter = el("input", {
      type: "search",
      autocomplete: "off",
      placeholder: "絞り込む",
      "aria-label": `${options.label}を絞り込む`,
    });

    const renderItems = () => {
      const needle = filter.value.trim().toLowerCase();
      const shown = needle
        ? list.items.filter((i) => i.name.toLowerCase().includes(needle))
        : list.items;
      const rows = shown.map((item) => {
        const value = candidateValue(item);
        const check = el("input", { type: "checkbox" });
        check.checked = selected.includes(value);
        check.addEventListener("change", () => {
          if (check.checked) {
            if (!selected.includes(value)) selected.push(value);
          } else {
            selected = selected.filter((v) => v !== value);
          }
          commit();
        });
        return el("label", { class: "check" }, [check, text(item.name)]);
      });
      if (rows.length === 0) {
        rows.push(el("p", { class: "empty" }, [text("該当する候補がありません。")]));
      }
      fill(box, rows);
    };
    filter.addEventListener("input", renderItems);
    renderItems();
    return el("div", {}, [filter, box]);
  }

  load(false);

  /** 登録先が変わったときに候補を取り直す。 */
  wrap.reloadFor = (project) => {
    options.project = project;
    selected = [];
    commit();
    load(false);
  };
  return wrap;
}

/**
 * 表の中で使う候補一覧 (FR-08、FR-16)。
 *
 * 500 行それぞれに選択欄を置くと要素数が跳ね上がるため、
 * プロジェクトごとに 1 つの候補一覧を作り、各行の入力欄から参照する。
 * これにより候補の要素は行数に比例しない。
 */
export async function buildCatalog(projects) {
  // 名称で指定する項目は名前の一覧、識別子で指定する項目は識別子と名前の対で持つ。
  // 表では名前で見せて編集し、登録の直前に識別子へ戻す (設計書 6.2)。
  const byName = [
    ["label", "labels"],
    ["milestone", "milestones"],
  ];
  const byId = [
    ["member", "members"],
    ["epic", "epics"],
    ["iteration", "iterations"],
  ];
  const catalog = {};
  for (const project of projects) {
    catalog[project] = {
      labels: [],
      milestones: [],
      members: [],
      epics: [],
      iterations: [],
    };
    for (const [kind, key] of byName) {
      try {
        const list = await api.candidates(project, kind);
        catalog[project][key] = list.items.map((i) => i.name);
      } catch {
        // 取得できない場合は空のままにする。入力は自由入力として続けられる。
      }
    }
    for (const [kind, key] of byId) {
      try {
        const list = await api.candidates(project, kind);
        catalog[project][key] = orderCandidates(kind, list.items).map((i) => ({
          id: i.id,
          name: i.name,
          label: candidateLabel(i),
        }));
      } catch {
        // 同上。
      }
    }
  }
  return catalog;
}
