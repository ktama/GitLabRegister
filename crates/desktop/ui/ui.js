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

/** 確認を取る。取り消せない操作の前に必ず通す。 */
export function confirmAction(message) {
  return window.confirm(message);
}

/** 文字列を受け取る。ファイルの経路の入力に使う。 */
export function promptText(message, initial = "") {
  const value = window.prompt(message, initial);
  return value === null ? null : value.trim();
}

export function formatDate(value) {
  return value ?? "";
}
