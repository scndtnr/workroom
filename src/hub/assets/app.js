"use strict";
(() => {
  const $ = (id) => document.getElementById(id);
  const content = $("content");
  let config;
  let generation = 0;
  let controlFilterState = null;
  let filterTimer = null;
  let requestController = null;
  const compactViewport = matchMedia("(max-width: 850px)");
  let sidebarPreference = null;
  try {
    const saved = localStorage.getItem("workroom-hub-sidebar");
    if (saved === "collapsed" || saved === "expanded") sidebarPreference = saved;
  } catch { /* 保存できなくても表示する */ }
  let sidebarCollapsed = sidebarPreference === null ? compactViewport.matches : sidebarPreference === "collapsed";
  let filesCollapsed = false;
  try { filesCollapsed = localStorage.getItem("workroom-hub-files") === "collapsed"; } catch { /* 保存は任意 */ }
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
    else {
      invalidateRequest();
      location.hash = hash;
    }
  }
  async function api(path) {
    const response = await fetch(path, { cache: "no-store", signal: requestController?.signal });
    if (!response.ok) throw new Error(`読み取れません（HTTP ${response.status}）`);
    return response.json();
  }
  function scope(params) {
    const defaults = initialFilters();
    return new URLSearchParams(Object.entries(defaults).map(([key, value]) => [key, params.get(key) ?? value]));
  }
  function initialFilters() {
    return { q: config.filter.query || "", status: config.filter.status, period: config.filter.period ? "1" : "0", days: String(config.filter.days), range: config.filter.range, scope: config.filter.scope, sort: "date", unanswered: "0" };
  }
  function setSegment(id, value) {
    const group = $(id);
    group.dataset.value = value;
    group.querySelectorAll("button").forEach((node) => node.setAttribute("aria-pressed", String(node.value === value)));
  }
  function updateFilterControls() {
    const allRooms = $("scope-filter").dataset.value === "all";
    const period = $("period").checked;
    $("status-filter").classList.toggle("disabled-filter", allRooms);
    $("status-filter").querySelectorAll("button").forEach((node) => { node.disabled = allRooms; });
    $("period-filter").classList.toggle("disabled-filter", allRooms);
    $("period-options").classList.toggle("disabled-filter", !allRooms && !period);
    $("period").disabled = allRooms;
    $("days").disabled = allRooms || !period;
    $("range-filter").querySelectorAll("button").forEach((node) => { node.disabled = allRooms || !period; });
    $("status-filter").title = allRooms ? "全部屋検索中は状態の条件を使いません" : "完了は .done のある部屋です";
    $("period-filter").title = allRooms ? "全部屋検索中は期間の条件を使いません" : "部屋名の日付で絞ります。チェックを外すと期間を制限しません";
  }
  function filterValues() {
    return { q: $("query").value, status: $("status-filter").dataset.value, period: $("period").checked ? "1" : "0", days: $("days").value || "0", range: $("range-filter").dataset.value, scope: $("scope-filter").dataset.value, sort: $("sort").value, unanswered: $("unanswered").checked ? "1" : "0" };
  }
  function invalidateRequest() {
    generation++;
    requestController?.abort();
  }
  function applyFilters() {
    clearTimeout(filterTimer);
    filterTimer = null;
    if (!$("filters").checkValidity()) return;
    const values = filterValues();
    // hashchange を待つ間に次の文字が入力されても、古い値で入力欄を戻さない。
    controlFilterState = new URLSearchParams(values).toString();
    change(values);
  }
  function scheduleFilters(delay) {
    clearTimeout(filterTimer);
    invalidateRequest();
    notice();
    if (delay === 0) applyFilters();
    else filterTimer = setTimeout(applyFilters, delay);
  }
  function syncFilters(params) {
    const values = Object.fromEntries(scope(params));
    const key = new URLSearchParams(values).toString();
    // 入力待ちの間に再取得が走っても、入力中の文字を消さない。
    if (key !== controlFilterState) {
      clearTimeout(filterTimer);
      filterTimer = null;
      setSegment("status-filter", values.status); setSegment("range-filter", values.range); setSegment("scope-filter", values.scope);
      $("period").checked = values.period === "1"; $("days").value = values.days; $("query").value = values.q;
      $("sort").value = values.sort; $("unanswered").checked = values.unanswered === "1";
      controlFilterState = key;
    }
    updateFilterControls();
  }
  function updateSidebar() {
    const roomPage = (state().get("page") || "rooms") === "rooms";
    const visible = roomPage && !sidebarCollapsed;
    $("rooms").hidden = !visible;
    $("layout").classList.toggle("wide", !visible);
    const toggle = $("sidebar-toggle");
    toggle.disabled = !roomPage;
    toggle.textContent = sidebarCollapsed ? "☰" : "◀";
    toggle.title = visible ? "部屋の一覧を畳む" : "部屋の一覧を表示";
    toggle.setAttribute("aria-label", toggle.title);
    toggle.setAttribute("aria-expanded", String(visible));
  }
  function fileUrl(room, path, markdownTheme = true) {
    const url = new URL("/f/" + room.split("/").concat(path.split("/")).map(encodeURIComponent).join("/"), location.origin);
    if (markdownTheme && isMarkdown(path)) url.searchParams.set("theme", currentTheme());
    return url;
  }
  function fileSelection(room, path) {
    const tab = state().get("tab");
    return { page: "rooms", room, tab: tab === "files" || tab === "other" ? "files" : path.toLowerCase() === "readme.md" ? "overview" : "files", file: path };
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
  async function preview(parent, room, path, ticket, fileToggle) {
    if (ticket !== generation) return;
    const panel = element("section", undefined, "preview");
    const heading = element("div", undefined, "preview-header");
    if (fileToggle) heading.append(fileToggle);
    const title = element("h3", path);
    title.title = path;
    heading.append(title);
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
    heading.append(toolbar);
    panel.append(heading);
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
      const response = await fetch(url, { cache: "no-store", signal: requestController?.signal });
      if (ticket !== generation) { await response.body?.cancel(); return; }
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
        if (item.size !== null) line.append(element("span", ` · ${item.size.toLocaleString()} B`, "file-size"));
      }
      list.append(line);
    }
    parent.append(list);
    if (group.omitted) parent.append(element("p", `… 他 ${group.omitted} 件`, "muted"));
  }
  async function showRoom(room, params, ticket) {
    const label = room.room;
    const header = element("div", undefined, "room-header");
    const heading = element("div", undefined, "room-heading");
    const name = element("h1", label);
    name.title = label;
    heading.append(name, element("span", room.state, "badge " + statusClass(room.state)));
    if (room.unanswered) heading.append(element("span", `未回答 ${room.unanswered}`, "badge unanswered"));
    if (room.pending) heading.append(element("span", `未反映 ${room.pending}`, "badge pending"));
    if (room.status && room.status !== room.state) {
      const status = element("span", "status: " + room.status, "room-status");
      status.title = status.textContent;
      heading.append(status);
    }
    header.append(heading);
    const tabs = element("div", undefined, "tabs");
    const tab = params.get("tab") === "other" ? "files" : params.get("tab") || "overview";
    for (const [key, text] of [["overview", "概要"], ["files", "成果物"], ["questions", `確認事項 ${room.unanswered + room.pending}`]]) {
      tabs.append(button(text, () => change({ tab: key, file: "" }), tab === key));
    }
    header.append(tabs);
    content.append(header);
    if (room.error || room.warnings?.length) {
      const alerts = element("details");
      alerts.append(element("summary", `読み取りの警告 ${room.warnings?.length || 0} 件${room.error ? "・エラーあり" : ""}`));
      warnings(alerts, room);
      header.append(alerts);
    }
    if (tab === "overview" && room.readme) {
      const pane = element("div", undefined, "view-pane");
      content.append(pane);
      await preview(pane, label, "README.md", ticket);
    } else if (tab === "overview") {
      const files = await api("/api/files?" + new URLSearchParams({ room: label }));
      if (ticket !== generation) return;
      await showFiles(files, label, params, ticket, "README.md がないため、成果物を表示します。");
    } else if (tab === "files") {
      const files = await api("/api/files?" + new URLSearchParams({ room: label }));
      if (ticket !== generation) return;
      await showFiles(files, label, params, ticket);
    } else {
      tabs.append(button(params.get("all") === "1" ? "未回答・未反映だけ" : "すべて", () => change({ all: params.get("all") === "1" ? "0" : "1" })));
      const body = element("div", undefined, "content-body");
      content.append(body);
      const data = await api("/api/questions?" + new URLSearchParams({ room: label }));
      if (ticket !== generation) return;
      warnings(body, data);
      const questions = data.questions.filter((q) => params.get("all") === "1" || ["unanswered", "pending"].includes(q.state));
      if (!questions.length) body.append(element("p", "確認事項はありません。", "muted"));
      for (const question of questions) body.append(questionCard(question));
      for (const legacy of data.legacy) {
        const details = element("details");
        details.append(element("summary", "旧形式: " + legacy));
        body.append(details);
        details.addEventListener("toggle", () => {
          if (details.open && details.childElementCount === 1) preview(details, label, legacy, ticket).catch((error) => { if (ticket === generation) showError(error); });
        });
      }
    }
  }
  async function showFiles(files, room, params, ticket, message) {
    if (ticket !== generation) return;
    const workspace = element("div", undefined, "file-workspace");
    const browser = element("aside", undefined, "file-browser");
    browser.id = "file-browser";
    browser.setAttribute("aria-label", "ファイルの一覧");
    browser.append(element("h2", "主なファイル"));
    if (message) browser.append(element("p", message, "muted"));
    fileList(browser, files.main, room, params);
    const other = element("details");
    other.append(element("summary", `その他のファイル（${files.other.items.length} 件）`));
    other.open = files.other.items.some((item) => item.path === params.get("file"));
    fileList(other, files.other, room, params);
    browser.append(other);
    const pane = element("div", undefined, "file-preview");
    workspace.append(browser, pane);
    content.append(workspace);
    const update = () => {
      workspace.classList.toggle("files-collapsed", filesCollapsed);
      browser.hidden = filesCollapsed;
      toggle.textContent = filesCollapsed ? "一覧 ▸" : "◂ 一覧";
      toggle.title = filesCollapsed ? "ファイルの一覧を表示" : "ファイルの一覧を畳む";
      toggle.setAttribute("aria-label", toggle.title);
      toggle.setAttribute("aria-expanded", String(!filesCollapsed));
    };
    const toggle = button("", () => {
      filesCollapsed = !filesCollapsed;
      try { localStorage.setItem("workroom-hub-files", filesCollapsed ? "collapsed" : "expanded"); } catch { /* 保存は任意 */ }
      update();
    });
    toggle.setAttribute("aria-controls", "file-browser");
    update();
    if (params.get("file")) await preview(pane, room, params.get("file"), ticket, toggle);
    else {
      const toolbar = element("div", undefined, "toolbar");
      toolbar.append(toggle);
      pane.append(toolbar, element("p", "ファイルを選択してください。", "muted"));
    }
  }
  function archiveFilters(groups, params, parent) {
    const controls = element("div", undefined, "archive-filters");
    const rooms = element("select");
    rooms.setAttribute("aria-label", "アーカイブの部屋");
    const all = element("option", "全部屋"); all.value = ""; rooms.append(all);
    for (const group of groups) { const option = element("option", group.room); option.value = group.room; rooms.append(option); }
    rooms.value = params.get("archiveRoom") || "";
    rooms.addEventListener("change", () => change({ archiveRoom: rooms.value }));
    const status = element("select"); status.setAttribute("aria-label", "確認事項の状態");
    for (const [key, text] of [["", "すべての状態"], ...Object.entries(labels)]) { const option = element("option", text); option.value = key; status.append(option); }
    status.value = params.get("question_status") || "";
    status.addEventListener("change", () => change({ question_status: status.value }));
    const search = element("input"); search.type = "search"; search.placeholder = "質問・回答を検索"; search.setAttribute("aria-label", "質問・回答を検索"); search.value = params.get("search") || "";
    search.addEventListener("change", () => change({ search: search.value }));
    controls.append(rooms, status, search);
    parent.append(controls);
  }
  async function showQuestions(params, ticket) {
    const archive = params.get("page") === "archive";
    const header = element("div", undefined, "page-header");
    header.append(element("h1", archive ? "アーカイブ" : "確認事項（全部屋）"));
    const body = element("div", undefined, "content-body");
    content.append(header, body);
    const data = await api("/api/questions/all?" + scope(params));
    if (ticket !== generation) return;
    if (archive) {
      archiveFilters(data.rooms, params, header);
      const search = (params.get("search") || "").toLocaleLowerCase();
      const items = data.rooms.flatMap((group) => {
        warnings(body, group);
        return group.questions.map((question) => ({ room: group.room, question }));
      }).filter(({ room, question }) => (!params.get("archiveRoom") || room === params.get("archiveRoom")) && (!params.get("question_status") || question.state === params.get("question_status")) && archiveSearchText(question, room).includes(search));
      items.sort((a, b) => b.question.id.slice(2, 10).localeCompare(a.question.id.slice(2, 10)) || Number(b.question.id.slice(11)) - Number(a.question.id.slice(11)) || a.room.localeCompare(b.room));
      header.append(element("span", `${items.length} 件`, "muted"));
      for (const item of items) body.append(questionCard(item.question, item.room));
    } else {
      let count = 0;
      for (const group of data.rooms) {
        const unanswered = group.questions.filter((q) => q.state === "unanswered");
        const pending = group.questions.filter((q) => q.state === "pending");
        if (!unanswered.length && !pending.length && !group.error && !group.warnings.length) continue;
        count += unanswered.length + pending.length;
        body.append(element("h2", group.room));
        warnings(body, group);
        for (const question of unanswered) body.append(questionCard(question));
        if (pending.length) {
          const details = element("details");
          details.append(element("summary", `回答済み・未反映 ${pending.length} 件`));
          for (const question of pending) details.append(questionCard(question));
          body.append(details);
        }
      }
      if (!count) body.append(element("p", "未回答・未反映の確認事項はありません。", "muted"));
    }
  }
  function notice(text = "", error = false) {
    const node = $("notice");
    node.textContent = text;
    node.hidden = !text;
    node.classList.toggle("error", error);
  }
  function showError(error) { notice(error.message || String(error), true); }
  async function refresh() {
    if (!config) return;
    const ticket = ++generation;
    requestController?.abort();
    requestController = new AbortController();
    const params = state();
    notice("読み込み中…");
    $("filter-summary").textContent = "…";
    content.replaceChildren();
    syncFilters(params);
    const page = params.get("page") || "rooms";
    document.querySelectorAll("[data-page]").forEach((node) => node.setAttribute("aria-pressed", String(node.dataset.page === page)));
    updateSidebar();
    try {
      const data = await api("/api/rooms?" + scope(params));
      if (ticket !== generation) return;
      const summary = `${data.rooms.length} 件 · ${data.label}`;
      $("filter-summary").textContent = summary;
      $("filter-summary").title = summary;
      $("rooms").setAttribute("aria-label", `${data.label} · ${data.rooms.length} 部屋`);
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
      if (ticket === generation) notice();
    } catch (error) { if (ticket === generation) showError(error); }
  }
  $("filters").addEventListener("submit", (event) => {
    event.preventDefault();
    applyFilters();
  });
  $("filters").addEventListener("input", (event) => {
    if (event.target.id === "query" || event.target.id === "days") scheduleFilters(300);
  });
  $("filters").addEventListener("change", (event) => {
    if (["period", "sort", "unanswered"].includes(event.target.id)) { updateFilterControls(); scheduleFilters(0); }
  });
  for (const id of ["status-filter", "range-filter", "scope-filter"]) {
    $(id).querySelectorAll("button").forEach((node) => node.addEventListener("click", () => {
      setSegment(id, node.value);
      updateFilterControls();
      scheduleFilters(0);
    }));
  }
  $("clear-filters").addEventListener("click", () => {
    clearTimeout(filterTimer);
    filterTimer = null;
    controlFilterState = null;
    change(initialFilters());
  });
  $("filters").addEventListener("keydown", (event) => {
    if (event.key === "Enter" && !event.isComposing && ["query", "days"].includes(event.target.id)) {
      event.preventDefault();
      applyFilters();
    }
  });
  $("sidebar-toggle").addEventListener("click", () => {
    sidebarCollapsed = !sidebarCollapsed;
    sidebarPreference = sidebarCollapsed ? "collapsed" : "expanded";
    try { localStorage.setItem("workroom-hub-sidebar", sidebarPreference); } catch { /* 保存は任意 */ }
    updateSidebar();
  });
  compactViewport.addEventListener("change", (event) => {
    if (sidebarPreference === null) sidebarCollapsed = event.matches;
    updateSidebar();
  });
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
    $("root").title = "作業ルート: " + data.root;
    const params = state();
    for (const [key, value] of Object.entries({ ...initialFilters(), page: "rooms" })) if (!params.has(key)) params.set(key, String(value));
    history.replaceState(null, "", "#" + params.toString());
    refresh();
  }).catch(showError);
})();
