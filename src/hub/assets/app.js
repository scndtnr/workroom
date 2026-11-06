"use strict";
(() => {
  const $ = (id) => document.getElementById(id);
  const content = $("content");
  let config;
  let generation = 0;
  let appliedFilters = null;
  let controlFilterState = null;
  let theme;
  try { theme = localStorage.getItem("workroom-hub-theme"); } catch { /* 保存できなくても表示する */ }
  if (theme === "dark" || theme === "light") document.documentElement.className = theme;
  const currentTheme = () => theme || (matchMedia("(prefers-color-scheme: dark)").matches ? "dark" : "light");
  const state = () => new URLSearchParams(location.hash.slice(1));
  const isMarkdown = (path) => /\.md(?:\.txt)?$/i.test(path);
  function element(tag, text, className) {
    const node = document.createElement(tag);
    if (text !== undefined) node.textContent = text;
    if (className) node.className = className;
    return node;
  }
  function button(text, action, selected = false) {
    const node = element("button", text);
    node.type = "button";
    node.setAttribute("aria-pressed", String(selected));
    node.addEventListener("click", action);
    return node;
  }
  function change(values) {
    const params = state();
    for (const [key, value] of Object.entries(values)) params.set(key, String(value));
    const hash = params.toString();
    if (hash === location.hash.slice(1)) refresh();
    else location.hash = hash;
  }
  async function api(path) {
    const response = await fetch(path, { cache: "no-store" });
    if (!response.ok) throw new Error(`読み取れません（HTTP ${response.status}）`);
    return response.json();
  }
  function scope(params) {
    return new URLSearchParams({ q: params.get("q") || "", mode: params.get("mode") || "recent", days: params.get("days") || "0", sort: params.get("sort") || "date", unanswered: params.get("unanswered") || "0" });
  }
  function filterValues() {
    return { q: $("query").value, mode: $("mode").value, days: $("days").value || "0", sort: $("sort").value, unanswered: $("unanswered").checked ? "1" : "0" };
  }
  function updateFilterButton() {
    const values = filterValues();
    const dirty = appliedFilters !== null && Object.keys(appliedFilters).some((key) => values[key] !== appliedFilters[key]);
    const submit = $("show-filters");
    submit.textContent = dirty ? "● この条件で表示" : "この条件で表示";
    submit.classList.toggle("pending-filters", dirty);
    submit.title = dirty ? "表示に反映していない条件があります" : "この条件で部屋の一覧を表示します";
  }
  function syncFilters(params) {
    const values = Object.fromEntries(scope(params));
    const key = new URLSearchParams(values).toString();
    // 再取得やタブ移動だけでは、まだ決定していない入力を消さない。
    if (key !== controlFilterState) {
      $("mode").value = values.mode; $("days").value = values.days; $("query").value = values.q;
      $("sort").value = values.sort; $("unanswered").checked = values.unanswered === "1";
      controlFilterState = key;
    }
    updateFilterButton();
  }
  function fileUrl(room, path, markdownTheme = true) {
    const url = new URL("/f/" + room.split("/").concat(path.split("/")).map(encodeURIComponent).join("/"), location.origin);
    if (markdownTheme && isMarkdown(path)) url.searchParams.set("theme", currentTheme());
    return url;
  }
  function fileSelection(room, path) {
    const tab = state().get("tab");
    return { page: "rooms", room, tab: tab === "files" || tab === "other" ? tab : path.toLowerCase() === "readme.md" ? "overview" : "files", file: path };
  }
  function hubUrl(room, path) {
    const url = new URL("/", location.origin);
    const params = state();
    for (const [key, value] of Object.entries(fileSelection(room, path))) params.set(key, value);
    url.hash = params.toString();
    return url;
  }
  function separateTab(url, text = "別タブで開く") {
    const link = element("a", text);
    link.href = url;
    link.target = "_blank";
    link.rel = "noopener noreferrer";
    return link;
  }
  function warnings(parent, data) {
    if (data.error) parent.append(element("p", data.error, "warning"));
    for (const warning of data.warnings || []) parent.append(element("p", `${warning.file} ${warning.line}行目: ${warning.reason}`, "warning"));
  }
  const ask = (question) => question.ask || question.answers[0]?.question || {};
  function archiveSearchText(question, room) {
    const info = ask(question);
    return [room, question.id, info.question, info.background, ...(info.choices || []),
      ...question.answers.map((answer) => answer.text), ...question.done.map((event) => event.to),
      question.withdrawals.at(-1)?.reason].filter((value) => typeof value === "string").join("\n").toLocaleLowerCase();
  }
  const statusClass = (status) => status.startsWith("待ち") || status.endsWith("待ち") ? "pending" : status.startsWith("進行中") || status.startsWith("完了可") ? "unanswered" : "";
  const labels = { unanswered: "未回答", pending: "回答済み・未反映", resolved: "解決済み", withdrawn: "取り下げ" };
  function questionCard(question, room) {
    const card = element("section", undefined, "question");
    const info = ask(question);
    if (room) card.append(button(room, () => change({ page: "rooms", room, tab: "questions", all: "1", file: "" })));
    card.append(element("h3", `${question.id} · ${labels[question.state] || question.state}`));
    card.append(element("p", info.question || "質問なし", "question-text"));
    if (!question.ask) card.append(element("p", "質問なし（写しから表示）", "warning"));
    const at = question.ask?.at || info.asked_at;
    if (info.by || at) card.append(element("p", `起票: ${info.by || ""} ${at || ""}`, "muted"));
    if (info.origin) card.append(element("p", `元の質問: ${info.origin}`, "muted"));
    if (info.background) card.append(element("pre", info.background));
    if (info.choices?.length) {
      const choices = element("ol");
      info.choices.forEach((choice, index) => choices.append(element("li", choice + (info.recommend === index + 1 ? "（推奨）" : ""))));
      card.append(choices);
    }
    for (const answer of question.answers) {
      const reflected = answer.n <= question.reflected_upto;
      const block = element("div", undefined, "answer" + (reflected ? " reflected" : ""));
      block.append(element("p", `回答 ${answer.n} · ${answer.at} · ${answer.kind === "not_needed" ? "回答不要" : "回答"} · ${reflected ? "反映済み" : "未反映"}`, "muted"));
      block.append(element("pre", answer.text || "（理由なし）"));
      card.append(block);
    }
    for (const event of question.done) {
      card.append(element("p", `反映先: ${event.to}（${event.at}、回答 ${event.upto} まで）`));
      if (event.chat) card.append(element("pre", "会話での回答: " + event.chat));
    }
    const withdrawal = question.withdrawals.at(-1);
    if (withdrawal) {
      card.append(element("pre", "取り下げの理由: " + withdrawal.reason));
      if (withdrawal.successor) card.append(element("p", "後継: " + withdrawal.successor));
      if (question.answers.length) card.append(element("p", "取り下げた項目への回答を記録として表示しています。", "muted"));
    }
    return card;
  }
  async function textPreview(response, parent, ticket) {
    const reader = response.body.getReader();
    const decoder = new TextDecoder();
    let text = "";
    let size = 0;
    const limit = 1024 * 1024;
    try {
      while (size < limit) {
        const { value, done } = await reader.read();
        if (done) break;
        if (ticket !== generation) return;
        const chunk = value.subarray(0, limit - size);
        size += chunk.length;
        text += decoder.decode(chunk, { stream: true });
      }
      text += decoder.decode();
    } finally { await reader.cancel(); }
    if (ticket !== generation) return;
    parent.append(element("pre", text));
    if (size >= limit) parent.append(element("p", "先頭 1 MiB までを表示しています。", "muted"));
  }
  async function preview(parent, room, path, ticket) {
    if (parent === content) content.classList.add("previewing");
    const panel = element("section", undefined, "preview");
    panel.append(element("h3", path));
    const url = fileUrl(room, path);
    const toolbar = element("div", undefined, "toolbar");
    toolbar.append(separateTab(hubUrl(room, path)), separateTab(url, "ファイルだけを開く"));
    const markdown = isMarkdown(path);
    const textMode = state().get("view") === "text";
    if (markdown) {
      toolbar.append(
        button("プレビュー", () => change({ ...fileSelection(room, path), view: "preview" }), !textMode),
        button("テキスト", () => change({ ...fileSelection(room, path), view: "text" }), textMode)
      );
    }
    panel.append(toolbar);
    const body = element("div", undefined, "preview-body");
    panel.append(body);
    parent.append(panel);
    const extension = path.split(".").at(-1).toLowerCase();
    if ((markdown && !textMode) || ["html", "htm", "pdf"].includes(extension)) {
      body.classList.add("framed");
      const frame = element("iframe");
      frame.title = path;
      if (extension !== "pdf") frame.setAttribute("sandbox", markdown ? "allow-popups allow-popups-to-escape-sandbox" : "allow-scripts allow-forms allow-modals allow-popups allow-popups-to-escape-sandbox");
      frame.src = url;
      body.append(frame);
    } else if (["png", "jpg", "jpeg", "gif", "webp", "avif", "bmp", "ico", "svg"].includes(extension)) {
      const image = element("img");
      image.alt = path;
      image.src = url;
      body.append(image);
    } else {
      url.searchParams.set("raw", "1");
      const response = await fetch(url, { cache: "no-store" });
      if (!response.ok) throw new Error(`読み取れません（HTTP ${response.status}）`);
      if (response.headers.get("Content-Type")?.startsWith("text/plain")) await textPreview(response, body, ticket);
      else {
        await response.body.cancel();
        if (ticket !== generation) return;
        const download = element("a", "ダウンロード");
        download.href = url;
        download.download = "";
        body.append(download);
      }
    }
  }
  function fileList(parent, group, room, params) {
    if (!group.items.length) parent.append(element("p", "ファイルはありません。", "muted"));
    const list = element("ul", undefined, "files");
    for (const item of group.items) {
      const line = element("li");
      if (item.kind === "directory") line.append(element("span", item.path + "/", "directory"));
      else {
        const file = button(item.path, () => change({ file: item.path }), params.get("file") === item.path);
        file.classList.add("file");
        line.append(file);
        if (item.size !== null) line.append(element("span", ` · ${item.size.toLocaleString()} B`, "muted"));
      }
      list.append(line);
    }
    parent.append(list);
    if (group.omitted) parent.append(element("p", `… 他 ${group.omitted} 件`, "muted"));
  }
  async function showRoom(room, params, ticket) {
    const label = room.room;
    content.append(element("h1", label));
    content.append(element("p", `${room.state} · 未回答 ${room.unanswered} · 未反映 ${room.pending}`, "muted"));
    const tabs = element("div", undefined, "tabs");
    const tab = params.get("tab") || "overview";
    for (const [key, text] of [["overview", "概要"], ["files", "成果物"], ["questions", `確認事項 ${room.unanswered + room.pending}`], ["other", "その他のファイル"]]) {
      tabs.append(button(text, () => change({ tab: key, file: "" }), tab === key));
    }
    content.append(tabs);
    warnings(content, room);
    if (tab === "overview" && room.readme) await preview(content, label, "README.md", ticket);
    else if (tab === "overview") {
      content.append(element("p", "README.md がないため、成果物の一覧を表示します。", "muted"));
      const files = await api("/api/files?" + new URLSearchParams({ room: label }));
      if (ticket !== generation) return;
      fileList(content, files.main, label, params);
      if (params.get("file")) await preview(content, label, params.get("file"), ticket);
    }
    else if (tab === "files" || tab === "other") {
      const files = await api("/api/files?" + new URLSearchParams({ room: label }));
      if (ticket !== generation) return;
      fileList(content, files[tab === "files" ? "main" : "other"], label, params);
      if (params.get("file")) await preview(content, label, params.get("file"), ticket);
    } else {
      const data = await api("/api/questions?" + new URLSearchParams({ room: label }));
      if (ticket !== generation) return;
      content.append(button(params.get("all") === "1" ? "未回答・未反映だけ" : "すべて", () => change({ all: params.get("all") === "1" ? "0" : "1" })));
      warnings(content, data);
      const questions = data.questions.filter((q) => params.get("all") === "1" || ["unanswered", "pending"].includes(q.state));
      if (!questions.length) content.append(element("p", "確認事項はありません。", "muted"));
      for (const question of questions) content.append(questionCard(question));
      for (const legacy of data.legacy) {
        const details = element("details");
        details.append(element("summary", "旧形式: " + legacy));
        content.append(details);
        details.addEventListener("toggle", () => {
          if (details.open && details.childElementCount === 1) preview(details, label, legacy, ticket).catch(showError);
        });
      }
    }
  }
  function archiveFilters(groups, params) {
    const controls = element("div", undefined, "archive-filters");
    const rooms = element("select");
    rooms.setAttribute("aria-label", "アーカイブの部屋");
    const all = element("option", "全部屋"); all.value = ""; rooms.append(all);
    for (const group of groups) { const option = element("option", group.room); option.value = group.room; rooms.append(option); }
    rooms.value = params.get("archiveRoom") || "";
    rooms.addEventListener("change", () => change({ archiveRoom: rooms.value }));
    const status = element("select"); status.setAttribute("aria-label", "確認事項の状態");
    for (const [key, text] of [["", "すべての状態"], ...Object.entries(labels)]) { const option = element("option", text); option.value = key; status.append(option); }
    status.value = params.get("status") || "";
    status.addEventListener("change", () => change({ status: status.value }));
    const search = element("input"); search.type = "search"; search.placeholder = "質問・回答を検索"; search.setAttribute("aria-label", "質問・回答を検索"); search.value = params.get("search") || "";
    search.addEventListener("change", () => change({ search: search.value }));
    controls.append(rooms, status, search);
    content.append(controls);
  }
  async function showQuestions(params, ticket) {
    const archive = params.get("page") === "archive";
    content.append(element("h1", archive ? "アーカイブ" : "確認事項（全部屋）"));
    const data = await api("/api/questions/all?" + scope(params));
    if (ticket !== generation) return;
    if (archive) {
      archiveFilters(data.rooms, params);
      const search = (params.get("search") || "").toLocaleLowerCase();
      const items = data.rooms.flatMap((group) => {
        warnings(content, group);
        return group.questions.map((question) => ({ room: group.room, question }));
      }).filter(({ room, question }) => (!params.get("archiveRoom") || room === params.get("archiveRoom")) && (!params.get("status") || question.state === params.get("status")) && archiveSearchText(question, room).includes(search));
      items.sort((a, b) => b.question.id.slice(2, 10).localeCompare(a.question.id.slice(2, 10)) || Number(b.question.id.slice(11)) - Number(a.question.id.slice(11)) || a.room.localeCompare(b.room));
      content.append(element("p", `${items.length} 件`, "muted"));
      for (const item of items) content.append(questionCard(item.question, item.room));
    } else {
      let count = 0;
      for (const group of data.rooms) {
        const unanswered = group.questions.filter((q) => q.state === "unanswered");
        const pending = group.questions.filter((q) => q.state === "pending");
        if (!unanswered.length && !pending.length && !group.error && !group.warnings.length) continue;
        count += unanswered.length + pending.length;
        content.append(element("h2", group.room));
        warnings(content, group);
        for (const question of unanswered) content.append(questionCard(question));
        if (pending.length) {
          const details = element("details");
          details.append(element("summary", `回答済み・未反映 ${pending.length} 件`));
          for (const question of pending) details.append(questionCard(question));
          content.append(details);
        }
      }
      if (!count) content.append(element("p", "未回答・未反映の確認事項はありません。", "muted"));
    }
  }
  function showError(error) { $("notice").textContent = error.message || String(error); }
  async function refresh() {
    if (!config) return;
    const ticket = ++generation;
    const params = state();
    $("notice").textContent = "読み込み中…";
    content.classList.remove("previewing");
    content.replaceChildren();
    syncFilters(params);
    const page = params.get("page") || "rooms";
    document.querySelectorAll("[data-page]").forEach((node) => node.setAttribute("aria-pressed", String(node.dataset.page === page)));
    $("rooms").hidden = page !== "rooms"; $("layout").classList.toggle("wide", page !== "rooms");
    try {
      const data = await api("/api/rooms?" + scope(params));
      if (ticket !== generation) return;
      appliedFilters = Object.fromEntries(scope(params));
      updateFilterButton();
      $("notice").textContent = `${data.label} · ${data.rooms.length} 部屋`;
      $("rooms").replaceChildren();
      let selected = data.rooms.find((room) => room.room === params.get("room")) || data.rooms[0];
      if (selected && params.get("room") !== selected.room) {
        params.set("room", selected.room);
        history.replaceState(null, "", "#" + params.toString());
      }
      for (const room of data.rooms) {
        const row = button("", () => change({ page: "rooms", room: room.room, file: "" }), selected?.room === room.room);
        row.classList.add("room");
        row.append(element("span", `${room.date}  ${room.title}`, "room-title"));
        if (room.description) row.append(element("span", room.description, "room-desc"));
        const status = element("span", undefined, "room-state");
        status.append(element("span", room.state, statusClass(room.state)));
        if (room.unanswered) status.append(element("span", `● ${room.unanswered}`, "badge unanswered"));
        if (room.pending) status.append(element("span", `◆ ${room.pending}`, "badge pending"));
        row.append(status);
        if (room.status) row.append(element("span", "status: " + room.status, "room-desc"));
        if (room.modified) row.append(element("span", "最終更新 " + new Date(room.modified * 1000).toLocaleString(), "room-desc"));
        $("rooms").append(row);
      }
      if (page === "rooms") {
        if (selected) await showRoom(selected, params, ticket);
        else content.append(element("p", "該当する部屋はありません。表示条件を変えてください。", "muted"));
      } else await showQuestions(params, ticket);
    } catch (error) { if (ticket === generation) showError(error); }
  }
  $("filters").addEventListener("submit", (event) => {
    event.preventDefault();
    change(filterValues());
  });
  $("filters").addEventListener("input", updateFilterButton);
  $("filters").addEventListener("change", updateFilterButton);
  document.querySelectorAll("[data-page]").forEach((node) => node.addEventListener("click", () => change({ page: node.dataset.page })));
  $("reload").addEventListener("click", refresh);
  $("theme").addEventListener("click", () => {
    theme = currentTheme() === "dark" ? "light" : "dark";
    document.documentElement.className = theme;
    try { localStorage.setItem("workroom-hub-theme", theme); } catch { /* 保存は任意 */ }
    refresh();
  });
  matchMedia("(prefers-color-scheme: dark)").addEventListener("change", () => { if (!theme) refresh(); });
  document.addEventListener("visibilitychange", () => { if (document.visibilityState === "visible") refresh(); });
  window.addEventListener("hashchange", refresh);
  let printDetails = [];
  window.addEventListener("beforeprint", () => {
    printDetails = [...document.querySelectorAll("details:not([open])")];
    printDetails.forEach((details) => { details.open = true; });
  });
  window.addEventListener("afterprint", () => printDetails.forEach((details) => { details.open = false; }));
  api("/api/config").then((data) => {
    config = data;
    $("root").textContent = data.root;
    const params = state();
    for (const [key, value] of Object.entries({ q: data.filter.query || "", mode: data.filter.mode, days: data.filter.days, page: "rooms", sort: "date" })) if (!params.has(key)) params.set(key, String(value));
    history.replaceState(null, "", "#" + params.toString());
    refresh();
  }).catch(showError);
})();
