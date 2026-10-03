// 小さな DOM の道具。枠組みを入れないのは、画面の規模に対して釣り合わないためである。

/** 要素を作る。属性と子をまとめて渡せる。 */
export function el(tag, attrs = {}, children = []) {
  const node = document.createElement(tag);
  for (const [name, value] of Object.entries(attrs)) {
    if (value === null || value === undefined) continue;
    node.setAttribute(name, String(value));
  }
  for (const child of children) node.append(child);
  return node;
}

export function text(value) {
  return document.createTextNode(String(value ?? ""));
}

export function $(selector) {
  const node = document.querySelector(selector);
  if (!node) throw new Error(`要素が見つかりません: ${selector}`);
  return node;
}

/** 一覧や段落をまとめて置き換える。 */
export function fill(host, children) {
  host.replaceChildren(...children);
}

/** 失敗と成功を知らせる (NFR-16)。分類ごとに色は変えず、文面と次の操作を必ず出す。 */
export function notify(message, nextAction = "", ok = false) {
  const toast = $("#toast");
  toast.replaceChildren(text(message));
  if (nextAction) {
    toast.append(el("span", { class: "next" }, [text(nextAction)]));
  }
  toast.classList.toggle("ok", ok);
  toast.hidden = false;
  clearTimeout(notify.timer);
  notify.timer = setTimeout(() => {
    toast.hidden = true;
  }, ok ? 4000 : 9000);
}

export function notifyFailure(error) {
  notify(error.message ?? String(error), error.nextAction ?? "", false);
}

/**
 * 確認を取る。取り消せない操作の前に必ず通す。
 *
 * 画面内の確認画面 (index.html の #confirm-dialog) を出し、選ばれた方を返す。
 * Esc と「やめる」は実行しない側に倒す。削除など元に戻せない操作では
 * 実行のボタンを赤くし、焦点を実行しない側に置いて、Enter の押し間違いで実行させない。
 *
 * @param {object} options
 * @param {string} options.title 見出し。何をするのかを書く
 * @param {string | Node[]} options.body 本文。文字列は改行で段落に分ける
 * @param {string} options.confirmLabel 実行するボタンの文言。動詞で書く
 * @param {string} [options.cancelLabel] 実行しないボタンの文言
 * @param {boolean} [options.danger] 元に戻せない操作か
 * @returns {Promise<boolean>} 実行を選んだか
 */
export function confirmAction({ title, body, confirmLabel, cancelLabel = "やめる", danger = false }) {
  const dialog = $("#confirm-dialog");
  const nodes =
    typeof body === "string"
      ? body
          .split("\n")
          .filter((line) => line.trim() !== "")
          .map((line) => el("p", {}, [text(line)]))
      : body;
  const cancel = el("button", { type: "button" }, [text(cancelLabel)]);
  const ok = el("button", { type: "button", class: danger ? "danger-solid" : "primary" }, [
    text(confirmLabel),
  ]);
  fill(dialog, [
    el("h2", { id: "confirm-title" }, [text(title)]),
    el("div", { class: "dialog-body" }, nodes),
    el("div", { class: "dialog-actions" }, [cancel, ok]),
  ]);

  return new Promise((resolve) => {
    // 確認画面は使い回すため、閉じるたびに受け口を外す。
    const listeners = new AbortController();
    const finish = (result) => {
      listeners.abort();
      dialog.close();
      resolve(result);
    };
    cancel.addEventListener("click", () => finish(false), { signal: listeners.signal });
    ok.addEventListener("click", () => finish(true), { signal: listeners.signal });
    dialog.addEventListener(
      "cancel",
      (event) => {
        event.preventDefault();
        finish(false);
      },
      { signal: listeners.signal },
    );
    dialog.showModal();
    (danger ? cancel : ok).focus();
  });
}

export function formatDate(value) {
  return value ?? "";
}
