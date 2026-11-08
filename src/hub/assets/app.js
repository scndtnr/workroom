"use strict";
(() => {
  const $ = (id) => document.getElementById(id);
  const content = $("room-detail");
  let config;
  let generation = 0;
  let controlFilterState = null;
  let filterTimer = null;
  let questionFilterTimer = null;
  let controlQuestionFilterState = null;
  let requestController = null;
  let rootHash;
  const drafts = new Map();
  const answerErrors = new Map();
  const additionalAnswersOpen = new Set();
  let draftWarningShown = false;
  let questionGroups = [];
  let visibleQuestions = [];
  let renderedQuestionParams = null;
  let composingAnswer = false;
  let deferredQuestionRender = null;
  let confirmation = null;
  let submitting = false;
  const compactViewport = matchMedia("(max-width: 850px)");
  let sidebarPreference = null;
  try {
    const saved = localStorage.getItem("workroom-hub-sidebar");
    if (saved === "collapsed" || saved === "expanded") sidebarPreference = saved;
  } catch { /* 保存できなくても表示する */ }
  let sidebarCollapsed = sidebarPreference === null ? compactViewport.matches : sidebarPreference === "collapsed";
  let filesCollapsed = false;
  try { filesCollapsed = localStorage.getItem("workroom-hub-files") === "collapsed"; } catch { /* 保存は任意 */ }
  let collapsedPane = "";
  try {
    const saved = localStorage.getItem("workroom-hub-collapsed-pane");
    if (saved === "questions" || saved === "files") collapsedPane = saved;
  } catch { /* 読めないときは両方開く */ }
  let theme;
  try { theme = localStorage.getItem("workroom-hub-theme"); } catch { /* 保存できなくても表示する */ }
  if (theme === "dark" || theme === "light") document.documentElement.className = theme;
  const currentTheme = () => theme || (matchMedia("(prefers-color-scheme: dark)").matches ? "dark" : "light");
  const state = () => new URLSearchParams(location.hash.slice(1));
  function normalizeHash() {
    const params = state();
    if (params.get("page") === "archive") {
      params.set("page", "questions");
      params.set("qstate", "all");
      params.set("qsort", "new");
      if (params.has("search") && !params.has("qq")) params.set("qq", params.get("search"));
    }
    for (const key of ["archiveRoom", "question_status", "search"]) params.delete(key);
    if (params.get("all") === "1") params.set("qstate", "all");
    params.delete("all");
    if (params.get("qstate") === "waiting") params.set("qstate", "unanswered");
    if (params.get("tab") === "questions") {
      params.delete("tab");
      if (collapsedPane === "questions") setCollapsedPane("");
    }
    if (params.get("tab") === "other") params.set("tab", "files");
    // 旧 q は部屋の検索語。確認事項 ID と分け、API には従来どおり q で渡す。
    if (params.has("q") && !/^Q-\d{8}-\d{2,}$/.test(params.get("q"))) {
      if (!params.has("query")) params.set("query", params.get("q"));
      params.delete("q");
    }
    if (params.toString() !== location.hash.slice(1)) history.replaceState(null, "", "#" + params.toString());
    return params;
  }
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
    for (const [key, value] of Object.entries(values)) {
      if (value === null) params.delete(key);
      else params.set(key, String(value));
    }
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
    const values = roomFilterState(params);
    const { query, ...filters } = values;
    return new URLSearchParams({ ...filters, q: query });
  }
  function roomFilterState(params) {
    return Object.fromEntries(Object.entries(initialFilters()).map(([key, value]) => [key, params.get(key) ?? value]));
  }
  function initialFilters() {
    return { query: config.filter.query || "", status: config.filter.status, period: config.filter.period ? "1" : "0", days: String(config.filter.days), range: config.filter.range, scope: config.filter.scope, sort: "date", unanswered: "0" };
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
    return { query: $("query").value, status: $("status-filter").dataset.value, period: $("period").checked ? "1" : "0", days: $("days").value || "0", range: $("range-filter").dataset.value, scope: $("scope-filter").dataset.value, sort: $("sort").value, unanswered: $("unanswered").checked ? "1" : "0" };
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
    const values = roomFilterState(params);
    const key = new URLSearchParams(values).toString();
    // 入力待ちの間に再取得が走っても、入力中の文字を消さない。
    if (key !== controlFilterState) {
      clearTimeout(filterTimer);
      filterTimer = null;
      setSegment("status-filter", values.status); setSegment("range-filter", values.range); setSegment("scope-filter", values.scope);
      $("period").checked = values.period === "1"; $("days").value = values.days; $("query").value = values.query;
      $("sort").value = values.sort; $("unanswered").checked = values.unanswered === "1";
      controlFilterState = key;
    }
    updateFilterControls();
  }
  const initialQuestionFilters = () => ({ qstate: "unanswered", qq: "", qsort: "room" });
  function questionFilterState(params) {
    return Object.fromEntries(Object.entries(initialQuestionFilters()).map(([key, value]) => [key, params.get(key) ?? value]));
  }
  function applyQuestionFilters() {
    clearTimeout(questionFilterTimer);
    questionFilterTimer = null;
    const values = { qstate: $("qstate-filter").dataset.value, qq: $("question-query").value, qsort: $("qsort-filter").dataset.value };
    controlQuestionFilterState = new URLSearchParams(values).toString();
    change(values);
  }
  function scheduleQuestionFilters(delay) {
    clearTimeout(questionFilterTimer);
    invalidateRequest();
    notice();
    if (delay === 0) applyQuestionFilters();
    else questionFilterTimer = setTimeout(applyQuestionFilters, delay);
  }
  function syncQuestionFilters(params) {
    const values = questionFilterState(params);
    const key = new URLSearchParams(values).toString();
    if (key !== controlQuestionFilterState) {
      clearTimeout(questionFilterTimer);
      questionFilterTimer = null;
      setSegment("qstate-filter", values.qstate);
      setSegment("qsort-filter", values.qsort);
      $("question-query").value = values.qq;
      controlQuestionFilterState = key;
    }
    const roomPage = (params.get("page") || "rooms") === "rooms";
    $("qsort-filter").hidden = roomPage;
    $("all-answer-actions").hidden = roomPage;
    $("questions-heading").textContent = roomPage ? "確認事項（この部屋）" : "確認事項（条件内の部屋）";
  }
  function updatePanes() {
    for (const name of ["questions", "files"]) {
      const collapsed = collapsedPane === name;
      $(name + "-expanded").hidden = collapsed;
      $(name + "-restore").hidden = !collapsed;
      $(name + "-collapse").setAttribute("aria-expanded", String(!collapsed));
      $("workspace").classList.toggle(name + "-collapsed", collapsed);
    }
  }
  function setCollapsedPane(name) {
    collapsedPane = name;
    try { localStorage.setItem("workroom-hub-collapsed-pane", name); } catch { /* 保存は任意 */ }
    updatePanes();
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
    const params = state();
    const tab = params.get("tab");
    const page = params.get("page") === "questions" && params.get("room") === room && params.get("q") ? "questions" : "rooms";
    return { page, room, tab: tab === "files" ? "files" : path.toLowerCase() === "readme.md" ? "overview" : "files", file: path };
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
  function questionSearchText(question, room) {
    const info = ask(question);
    return [room, question.id, info.question, info.background, ...(info.choices || []),
      ...question.answers.map((answer) => answer.text), ...question.done.map((event) => event.to),
      question.withdrawals.at(-1)?.reason].filter((value) => typeof value === "string").join("\n").toLocaleLowerCase();
  }
  // 表示名を変えるときは、サーバの部屋の状態（src/hub/listing.rs の summary）もそろえる。
  const labels = { unanswered: "回答待ち", pending: "反映待ち", resolved: "解決済み", withdrawn: "取り下げ" };
  // 部屋の状態の「回答待ち」「反映待ち」は、確認事項の状態の印と同じ色にする。
  const statusClass = (status) => status === labels.unanswered ? "unanswered" : status === labels.pending ? "pending"
    : status.startsWith("待ち") || status.endsWith("待ち") ? "pending" : status.startsWith("進行中") || status.startsWith("完了可") ? "unanswered" : "";
  for (const status of ["unanswered", "pending"]) {
    $("qstate-filter").querySelector(`button[value="${status}"]`).textContent = labels[status];
  }
  $("unanswered-label").textContent = labels.unanswered + "あり";
  $("sort").querySelector('option[value="unanswered"]').textContent = labels.unanswered + "順";
  $("clear-question-filters").title = labels.unanswered + "・検索なし・部屋ごとに戻します";
  function questionUrl(room, id, roomPage = false) {
    const params = state();
    params.set("room", room);
    params.set("q", id);
    params.set("qstate", "all");
    params.delete("qq");
    if (roomPage) params.set("page", "rooms");
    if (params.get("room") !== state().get("room")) { params.delete("file"); params.delete("tab"); }
    return "#" + params.toString();
  }
  function questionLink(room, id, text = id, roomPage = false) {
    const link = element("a", text);
    link.href = questionUrl(room, id, roomPage);
    return link;
  }
  const draftPrefix = () => `workroom-hub-draft:${rootHash}:`;
  const draftKey = (room, id) => draftPrefix() + encodeURIComponent(room) + ":" + encodeURIComponent(id);
  const hasDraft = (draft) => Boolean(draft && (draft.kind === "not_needed" || draft.text.length));
  const sendable = (draft) => Boolean(draft && (draft.kind === "not_needed" || draft.text.trim()));
  function draftWarning() {
    if (draftWarningShown) return;
    draftWarningShown = true;
    $("draft-warning").textContent = "下書きを保存できません";
    $("draft-warning").hidden = false;
  }
  function loadDrafts() {
    try {
      for (let index = 0; index < localStorage.length; index++) {
        const key = localStorage.key(index);
        if (!key?.startsWith(draftPrefix())) continue;
        try {
          const draft = JSON.parse(localStorage.getItem(key));
          if (["answer", "not_needed"].includes(draft?.kind) && typeof draft.text === "string" && hasDraft(draft)) drafts.set(key, draft);
        } catch { draftWarning(); }
      }
    } catch { draftWarning(); }
  }
  function saveDraft(room, id, kind, text) {
    const key = draftKey(room, id);
    const draft = { kind, text, updated: new Date().toISOString() };
    if (hasDraft(draft)) drafts.set(key, draft);
    else drafts.delete(key);
    try {
      if (hasDraft(draft)) localStorage.setItem(key, JSON.stringify(draft));
      else localStorage.removeItem(key);
    } catch { draftWarning(); }
    answerErrors.delete(key);
    if (!submitting) { confirmation = null; renderConfirmation(); }
    updateDraftUI();
  }
  function removeDraft(room, id) {
    drafts.delete(draftKey(room, id));
    try { localStorage.removeItem(draftKey(room, id)); } catch { draftWarning(); }
  }
  function draftCount(room) {
    const prefix = draftPrefix() + encodeURIComponent(room) + ":";
    return [...drafts.keys()].filter((key) => key.startsWith(prefix)).length;
  }
  function draftCountBadge(room) {
    const badge = element("span", undefined, "badge pending");
    badge.dataset.draftCount = room;
    return badge;
  }
  function roomAnswers(room) {
    return (questionGroups.find((group) => group.room === room)?.questions || [])
      .filter((question) => question.state !== "withdrawn")
      .flatMap((question) => {
        const draft = drafts.get(draftKey(room, question.id));
        return sendable(draft) ? [{ id: question.id, kind: draft.kind, text: draft.text }] : [];
      });
  }
  function visibleAnswers() {
    return questionGroups.map(({ room }) => ({
      room,
      answers: roomAnswers(room).filter((answer) => visibleQuestions.some((item) => item.room === room && item.question.id === answer.id))
    })).filter((group) => group.answers.length);
  }
  function updateDraftUI() {
    document.querySelectorAll("[data-draft-count]").forEach((node) => {
      const count = draftCount(node.dataset.draftCount);
      node.textContent = `未送信 ${count}`;
      node.hidden = !count;
    });
    document.querySelectorAll(".question").forEach((card) => {
      const key = draftKey(card.dataset.room, card.dataset.questionId);
      card.querySelector(".draft-badge").hidden = !hasDraft(drafts.get(key));
      const error = card.querySelector(".answer-error");
      error.textContent = answerErrors.get(key) || "";
      error.hidden = !error.textContent;
    });
    document.querySelectorAll(".answer-form button, .answer-form textarea, .withdrawn-draft button").forEach((node) => { node.disabled = submitting; });
    document.querySelectorAll(".answer-choices button").forEach((node) => {
      node.disabled = submitting || node.closest(".answer-form").classList.contains("not-needed");
    });
    document.querySelectorAll("[data-answer-send]").forEach((node) => {
      node.disabled = submitting || !sendable(drafts.get(draftKey(node.dataset.room, node.dataset.answerSend)));
    });
    document.querySelectorAll("[data-room-send]").forEach((node) => {
      const count = roomAnswers(node.dataset.roomSend).length;
      node.textContent = `この部屋の回答を送信（${count} 件）`;
      node.disabled = submitting || !count;
    });
    const count = visibleAnswers().reduce((total, group) => total + group.answers.length, 0);
    $("all-answer-send").textContent = `記入済みの回答をすべて送信（${count} 件）`;
    $("all-answer-send").disabled = submitting || !count;
  }
  function roomSendButton(room) {
    const node = button("", () => confirmAnswers([{ room, answers: roomAnswers(room) }]));
    node.dataset.roomSend = room;
    return node;
  }
  function confirmAnswers(groups) {
    if (submitting || !groups.some((group) => group.answers.length)) return;
    confirmation = {
      groups: groups.filter((group) => group.answers.length).map(({ room, answers }) => ({
        room, answers, status: "ready", error: "",
        questionTexts: answers.map((answer) => {
          const question = questionGroups.find((group) => group.room === room)?.questions.find((question) => question.id === answer.id);
          return question ? ask(question).question || "質問なし" : "質問なし";
        })
      })),
      fromQuestions: state().get("page") === "questions", complete: false, tokenInvalid: false
    };
    renderConfirmation();
    $("answer-confirmation").scrollIntoView({ block: "nearest" });
  }
  function includesWithdrawn(group) {
    return group.answers.some((answer) => questionGroups.find((current) => current.room === group.room)?.questions.some((question) => question.id === answer.id && question.state === "withdrawn"));
  }
  function renderConfirmation() {
    const panel = $("answer-confirmation");
    panel.replaceChildren();
    panel.hidden = !confirmation;
    if (!confirmation) return;
    panel.append(element("h2", "回答の送信確認"));
    const list = element("div", undefined, "answer-confirmation-list");
    for (const group of confirmation.groups) {
      const section = element("section");
      section.append(element("h3", `${group.room}（${group.answers.length} 件）`));
      group.answers.forEach((answer, index) => {
        const entry = element("div", undefined, "answer-confirmation-entry");
        entry.append(element("p", answer.id), element("p", group.questionTexts[index], "confirmation-text"),
          element("p", answer.kind === "not_needed" ? "回答不要" : "回答", "muted"),
          element("p", answer.kind === "not_needed" && !answer.text ? "理由なし" : answer.text, "confirmation-text"));
        section.append(entry);
      });
      if (group.status === "sent") section.append(element("p", "送信しました"));
      if (group.status === "skipped") section.append(element("p", "送信を中止しました。下書きは残しています。", "warning"));
      if (group.error) section.append(element("p", group.error, "warning"));
      list.append(section);
    }
    const send = button(submitting ? "送信中…" : "送信する", sendAnswers);
    const cancel = button(confirmation.complete ? "閉じる" : "やめる", () => { confirmation = null; renderConfirmation(); });
    send.disabled = cancel.disabled = submitting;
    const withdrawn = !confirmation.complete && confirmation.groups.some(includesWithdrawn);
    if (withdrawn) send.disabled = true;
    const actions = element("div", undefined, "answer-actions");
    if (!confirmation.complete) actions.append(send);
    actions.append(cancel);
    panel.append(list);
    if (confirmation.complete) {
      const sent = confirmation.groups.filter((group) => group.status === "sent").length;
      panel.append(element("p", `${confirmation.groups.length} 部屋中 ${sent} 部屋を送信しました。`));
    }
    if (confirmation.tokenInvalid) panel.append(element("p", "トークンが無効なため、残りの送信を中止しました。ページを再読み込みしてください。", "warning"));
    if (withdrawn) panel.append(element("p", "取り下げた項目が含まれています。「やめる」で戻り、送信する回答を選び直してください。", "warning"));
    panel.append(actions);
  }
  function redrawQuestions() {
    if (renderedQuestionParams) renderQuestions(questionGroups, renderedQuestionParams, generation, renderedQuestionParams.get("page") !== "questions");
  }
  function replaceQuestionGroup(room, group) {
    questionGroups = questionGroups.map((current) => current.room === room ? group : current);
    redrawQuestions();
  }
  async function sendAnswers() {
    if (submitting || !confirmation || confirmation.complete || confirmation.groups.some(includesWithdrawn)) return;
    const batch = confirmation;
    submitting = true;
    for (const group of batch.groups) group.answers.forEach((answer) => answerErrors.delete(draftKey(group.room, answer.id)));
    renderConfirmation();
    updateDraftUI();
    try {
      for (const group of batch.groups) {
        if (batch.tokenInvalid) group.status = "skipped";
        else await sendAnswerGroup(group, batch);
        renderConfirmation();
        updateDraftUI();
      }
      // 一連の送信後に部屋の状態・件数・「回答待ちあり」の条件を取り直す。
      if (batch.groups.some((group) => group.status === "sent" || group.withdrawn)) await refresh();
    } finally {
      batch.complete = true;
      submitting = false;
      renderConfirmation();
      updateDraftUI();
    }
  }
  async function sendAnswerGroup(submission, batch) {
    const snapshots = submission.answers.map((answer) => drafts.get(draftKey(submission.room, answer.id)));
    submission.status = "failed";
    if (includesWithdrawn(submission)) {
      submission.error = "取り下げた項目が含まれているため、この部屋は送信しませんでした。下書きは残しています。";
      return;
    }
    try {
      const response = await fetch("/api/answers", {
        method: "POST",
        headers: { "Content-Type": "application/json", "X-Workroom-Token": document.querySelector('meta[name="workroom-token"]').content },
        body: JSON.stringify({ room: submission.room, answers: submission.answers })
      });
      // JSON を読めないエラー応答でも、403 は必ず残りの送信を止める。
      const data = await response.json().catch(() => null);
      if (response.status === 200) {
        if (!Array.isArray(data?.sent) || !data.questions) throw new Error();
        for (const id of data.sent) {
          const index = submission.answers.findIndex((answer) => answer.id === id);
          // 待っている間に新しい下書きができていたら、その入力は残す。
          if (index >= 0 && drafts.get(draftKey(submission.room, id)) === snapshots[index]) removeDraft(submission.room, id);
          answerErrors.delete(draftKey(submission.room, id));
          // 再回答を送り終えたカードは畳み直す（新しい下書きが残っていれば、下書きのために開いたままになる）。
          additionalAnswersOpen.delete(draftKey(submission.room, id));
        }
        submission.status = "sent";
        addAnswerGuide(submission.room, data.sent, batch.fromQuestions);
        invalidateRequest();
        replaceQuestionGroup(submission.room, data.questions);
      } else {
        let message = typeof data?.error === "string" ? data.error : `回答を送信できません（HTTP ${response.status}）`;
        if (response.status === 403) {
          batch.tokenInvalid = true;
          message += "。ハブを再起動した場合は、ページを再読み込みしてください";
        }
        submission.error = data?.id ? `${data.id}: ${message}` : message;
        if (data?.id) answerErrors.set(draftKey(submission.room, data.id), submission.error);
        if (response.status === 409) {
          submission.withdrawn = true;
          invalidateRequest();
          // 409 の情報を先に反映し、再取得に失敗しても取り下げた項目は送れなくする。
          const group = questionGroups.find((current) => current.room === submission.room);
          const question = group?.questions.find((current) => current.id === data?.id);
          if (question) {
            question.state = "withdrawn";
            question.withdrawals.push({ reason: data.reason || message, successor: data.successor });
            redrawQuestions();
          }
          try {
            const current = await fetch("/api/questions?" + new URLSearchParams({ room: submission.room }), { cache: "no-store" });
            if (!current.ok) throw new Error();
            replaceQuestionGroup(submission.room, await current.json());
          } catch { submission.error += "。確認事項を再取得できません。⟳ で更新してください"; }
        }
      }
    } catch {
      submission.error = "回答の送信結果を確認できません。ネットワーク接続を確認し、⟳ で回答の状態を確認してください。下書きは残しています。";
    }
  }
  function addAnswerGuide(room, ids, fromQuestions) {
    const guide = element("section", undefined, "answer-guide");
    const text = `確認事項に回答しました（${ids.join(", ")}）。workroom q pending を読んで反映してください。${fromQuestions ? `（部屋: ${room}）` : ""}`;
    const message = element("pre", text);
    const status = element("p", "", "muted");
    const copy = button("コピー", async () => {
      try { await navigator.clipboard.writeText(text); status.textContent = "コピーしました"; }
      catch {
        const range = document.createRange();
        range.selectNodeContents(message);
        const selection = window.getSelection();
        selection.removeAllRanges();
        selection.addRange(range);
        status.textContent = "Ctrl+C でコピーしてください";
      }
    });
    const actions = element("div", undefined, "answer-actions");
    actions.append(copy, button("閉じる", () => guide.remove()));
    guide.append(element("h2", "エージェントへの貼り付け用"), message, actions, status);
    $("answer-guides").append(guide);
  }
  function answerForm(question, room) {
    const draft = drafts.get(draftKey(room, question.id));
    const form = element("div", undefined, "answer-form");
    const textarea = element("textarea");
    textarea.rows = 4;
    textarea.dataset.answerId = question.id;
    textarea.dataset.room = room;
    textarea.value = draft?.text || "";
    let kind = draft?.kind || "answer";
    const label = element("label");
    const labelText = element("span");
    label.append(labelText, textarea);
    const explanation = element("p", "エージェントに、この確認は要らないと伝えます。理由は任意です。", "muted");
    const kindButtons = element("div", undefined, "segments");
    kindButtons.setAttribute("role", "group");
    kindButtons.setAttribute("aria-label", "回答の種類");
    for (const [value, text] of [["answer", "回答する"], ["not_needed", "回答不要にする"]]) {
      const select = button(text, () => {
        kind = value;
        updateKind();
        saveDraft(room, question.id, kind, textarea.value);
      });
      select.value = value;
      kindButtons.append(select);
    }
    function updateKind() {
      labelText.textContent = kind === "not_needed" ? "回答不要の理由（省略可）" : "回答";
      kindButtons.querySelectorAll("button").forEach((node) => node.setAttribute("aria-pressed", String(node.value === kind)));
      explanation.hidden = kind !== "not_needed";
      form.classList.toggle("not-needed", kind === "not_needed");
    }
    textarea.addEventListener("input", () => saveDraft(room, question.id, kind, textarea.value));
    textarea.addEventListener("compositionstart", () => { composingAnswer = true; });
    textarea.addEventListener("compositionend", () => {
      composingAnswer = false;
      saveDraft(room, question.id, kind, textarea.value);
      if (deferredQuestionRender) {
        const args = deferredQuestionRender;
        deferredQuestionRender = null;
        if (args[2] === generation) renderQuestions(...args);
      }
    });
    const choices = element("ul", undefined, "answer-choices");
    const info = ask(question);
    (info.choices || []).forEach((choice, index) => {
      const line = element("li");
      const insert = button("入れる", () => {
        if (submitting || kind === "not_needed") return;
        const start = textarea.selectionStart;
        textarea.setRangeText(choice, start, textarea.selectionEnd, "end");
        saveDraft(room, question.id, kind, textarea.value);
        textarea.focus({ preventScroll: true });
      });
      insert.setAttribute("aria-label", `選択肢 ${index + 1} を入れる`);
      line.append(element("span", choice + (info.recommend === index + 1 ? "（推奨）" : "")), insert);
      choices.append(line);
    });
    const send = button("この回答だけ送信", () => {
      const current = drafts.get(draftKey(room, question.id));
      if (sendable(current)) confirmAnswers([{ room, answers: [{ id: question.id, kind: current.kind, text: current.text }] }]);
    });
    send.dataset.answerSend = question.id;
    send.dataset.room = room;
    const actions = element("div", undefined, "answer-actions");
    actions.append(send);
    form.append(kindButtons, choices, label, explanation, actions);
    updateKind();
    return form;
  }
  function withdrawnDraft(question, room) {
    const draft = drafts.get(draftKey(room, question.id));
    if (!hasDraft(draft)) return null;
    const panel = element("div", undefined, "withdrawn-draft");
    panel.append(element("p", "取り下げられました。下書きは送信されていません。", "warning"), element("p", draft.kind === "not_needed" ? "下書き: 回答不要" : "下書き: 回答"), element("pre", draft.text));
    const successor = question.withdrawals.at(-1)?.successor;
    if (successor) panel.append(button("後継に写す", () => {
      const previous = drafts.get(draftKey(room, successor));
      const text = [previous?.text, draft.text].filter(Boolean).join("\n\n");
      saveDraft(room, successor, previous?.kind || draft.kind, text);
      redrawQuestions();
    }));
    panel.append(button("下書きを捨てる", () => {
      saveDraft(room, question.id, "answer", "");
      redrawQuestions();
    }));
    return panel;
  }
  function selectQuestion(room, id) {
    const values = { room, q: id };
    if (state().get("room") !== room) { values.file = ""; values.tab = ""; }
    change(values);
  }
  function questionCard(question, room, params, showRoom = false) {
    const card = element("section", undefined, "question");
    card.dataset.questionId = question.id;
    card.dataset.room = room;
    const selected = params.get("room") === room && params.get("q") === question.id;
    card.classList.toggle("selected-question", selected);
    card.classList.toggle("withdrawn", question.state === "withdrawn");
    if (selected) card.setAttribute("aria-current", "true");
    const info = ask(question);
    if (showRoom) card.append(button(room, () => selectQuestion(room, question.id), selected));
    const heading = element("h3");
    heading.append(button(`${question.id} · ${labels[question.state] || question.state}`, () => selectQuestion(room, question.id), selected));
    heading.append(element("span", "未送信", "badge pending draft-badge"));
    card.append(heading);
    if (params.get("page") === "questions") card.append(questionLink(room, question.id, "部屋のページで開く", true));
    card.append(element("p", info.question || "質問なし", "question-text"));
    if (!question.ask) card.append(element("p", "質問なし（写しから表示）", "warning"));
    const at = question.ask?.at || info.asked_at;
    if (info.by || at) card.append(element("p", `起票: ${info.by || ""} ${at || ""}`, "muted"));
    if (info.origin) {
      const origin = element("p", undefined, "muted");
      origin.append(questionLink(room, info.origin), element("span", " の続き"));
      card.append(origin);
    }
    if (info.background) card.append(element("pre", info.background));
    if (question.state === "withdrawn" && info.choices?.length) {
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
      if (withdrawal.successor) {
        const successor = element("p", "後継: ");
        successor.append(questionLink(room, withdrawal.successor));
        card.append(successor);
      }
      if (question.answers.length) card.append(element("p", "取り下げた項目への回答を記録として表示しています。", "muted"));
    }
    if (question.state === "withdrawn") {
      const draft = withdrawnDraft(question, room);
      if (draft) card.append(draft);
    } else if (question.state === "resolved" || question.state === "pending") {
      const details = element("details", undefined, "additional-answer");
      const key = draftKey(room, question.id);
      details.open = additionalAnswersOpen.has(key) || hasDraft(drafts.get(key));
      details.append(element("summary", "再回答する"), answerForm(question, room));
      details.addEventListener("toggle", () => {
        if (!details.isConnected) return;
        if (details.open) additionalAnswersOpen.add(key);
        else additionalAnswersOpen.delete(key);
      });
      card.append(details);
    } else card.append(answerForm(question, room));
    const error = element("p", "", "warning answer-error");
    error.setAttribute("role", "status");
    card.append(error);
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
    if (room.unanswered) heading.append(element("span", `${labels.unanswered} ${room.unanswered}`, "badge unanswered"));
    if (room.pending) heading.append(element("span", `${labels.pending} ${room.pending}`, "badge pending"));
    if (room.status && room.status !== room.state) {
      const status = element("span", "status: " + room.status, "room-status");
      status.title = status.textContent;
      heading.append(status);
    }
    header.append(heading);
    const tabs = element("div", undefined, "tabs");
    const tab = params.get("tab") === "files" ? "files" : "overview";
    for (const [key, text] of [["overview", "概要"], ["files", "成果物"]]) {
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
  function matchesQuestion(question, room, params) {
    const { qstate, qq } = questionFilterState(params);
    const states = qstate === "all" ? Object.keys(labels) : qstate === "closed" ? ["resolved", "withdrawn"] : qstate === "pending" ? ["pending"] : ["unanswered"];
    return states.includes(question.state) && questionSearchText(question, room).includes(qq.toLocaleLowerCase());
  }
  function renderQuestions(groups, params, ticket, roomPage) {
    const selectedGroup = groups.find((group) => group.room === params.get("room"));
    const selected = selectedGroup?.questions.find((question) => question.id === params.get("q"));
    // IME の変換中は DOM を差し替えず、確定後に最新の描画だけを行う。
    if (composingAnswer) {
      deferredQuestionRender = [groups, params, ticket, roomPage];
      return selected;
    }
    questionGroups = groups;
    renderedQuestionParams = params;
    const body = $("question-list");
    const active = document.activeElement;
    const focused = active?.matches("textarea[data-answer-id]") ? {
      room: active.dataset.room, id: active.dataset.answerId,
      start: active.selectionStart, end: active.selectionEnd, direction: active.selectionDirection,
      top: active.scrollTop, left: active.scrollLeft
    } : null;
    const scrollTop = $("question-content").scrollTop;
    body.replaceChildren();
    const items = groups.flatMap((group) => group.questions
      .filter((question) => matchesQuestion(question, group.room, params) || (question.state === "withdrawn" && hasDraft(drafts.get(draftKey(group.room, question.id)))))
      .map((question) => ({ room: group.room, question })));
    visibleQuestions = [...items];
    const matchedCount = items.filter(({ room, question }) => matchesQuestion(question, room, params)).length;
    $("question-summary").textContent = `${matchedCount} 件${items.length > matchedCount ? ` · 取り下げた下書き ${items.length - matchedCount}` : ""}`;
    const newOrder = !roomPage && params.get("qsort") === "new";
    if (newOrder) {
      for (const group of groups) {
        if (group.error || group.warnings?.length) body.append(element("h2", group.room));
        warnings(body, group);
      }
      items.sort((a, b) => b.question.id.slice(2, 10).localeCompare(a.question.id.slice(2, 10)) || Number(b.question.id.slice(11)) - Number(a.question.id.slice(11)) || a.room.localeCompare(b.room));
      for (const { question, room } of items) body.append(questionCard(question, room, params, true));
    } else {
      for (const group of groups) {
        const questions = items.filter((item) => item.room === group.room);
        if (roomPage || questions.length || group.error || group.warnings?.length || draftCount(group.room)) {
          const header = element("div", undefined, "question-room-heading");
          if (!roomPage) header.append(element("h2", group.room));
          header.append(draftCountBadge(group.room), roomSendButton(group.room));
          body.append(header);
        }
        warnings(body, group);
        for (const { question, room } of questions) body.append(questionCard(question, room, params));
      }
    }
    if (!matchedCount) body.append(element("p", "条件に合う確認事項はありません。", "muted"));
    // ブックマークや後継リンクの対象は、確認事項の条件外でも読めるようにする。
    if (selected && !items.some((item) => item.room === selectedGroup.room && item.question.id === selected.id)) {
      visibleQuestions.push({ room: selectedGroup.room, question: selected });
      body.append(element("h2", "選択中の確認事項（条件外・件数に含めません）"), questionCard(selected, selectedGroup.room, params, newOrder));
    } else if (params.get("q") && !selected) {
      body.append(element("p", "選択した確認事項は見つかりません。部屋の条件や同期状態を確認してください。", "muted"));
    }
    for (const group of groups) {
      for (const legacy of group.legacy || []) {
        const details = element("details");
        details.append(element("summary", `旧形式: ${roomPage ? "" : group.room + " / "}${legacy}`));
        body.append(details);
        details.addEventListener("toggle", () => {
          if (details.open && details.childElementCount === 1) preview(details, group.room, legacy, ticket).catch((error) => { if (ticket === generation) showError(error); });
        });
      }
    }
    updateDraftUI();
    renderConfirmation();
    let restored = false;
    if (focused) {
      const textarea = [...body.querySelectorAll("textarea[data-answer-id]")].find((node) => node.dataset.room === focused.room && node.dataset.answerId === focused.id);
      if (textarea) {
        const details = textarea.closest("details");
        if (details) details.open = true;
        textarea.focus({ preventScroll: true });
        textarea.setSelectionRange(focused.start, focused.end, focused.direction);
        textarea.scrollTop = focused.top;
        textarea.scrollLeft = focused.left;
        restored = true;
      }
    }
    $("question-content").scrollTop = scrollTop;
    if (selected) {
      // 明示的に選んだカードを、保存された開閉状態より優先して見せる。
      if (collapsedPane === "questions") setCollapsedPane("");
      if (!restored) body.querySelector(".selected-question")?.scrollIntoView({ block: "nearest" });
    }
    return selected;
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
    const params = normalizeHash();
    notice("読み込み中…");
    $("filter-summary").textContent = "…";
    $("question-summary").textContent = "…";
    content.replaceChildren();
    syncFilters(params);
    syncQuestionFilters(params);
    const page = params.get("page") || "rooms";
    document.querySelectorAll("[data-page]").forEach((node) => node.setAttribute("aria-pressed", String(node.dataset.page === page)));
    updateSidebar();
    updatePanes();
    try {
      const data = await api("/api/rooms?" + scope(params));
      if (ticket !== generation) return;
      const filterLabel = data.label.replace(/ · 未回答あり$/, ` · ${labels.unanswered}あり`);
      const summary = `${data.rooms.length} 件 · ${filterLabel}`;
      $("filter-summary").textContent = summary;
      $("filter-summary").title = summary;
      $("rooms").setAttribute("aria-label", `${filterLabel} · ${data.rooms.length} 部屋`);
      $("rooms").replaceChildren();
      const selected = data.rooms.find((room) => room.room === params.get("room")) || (page === "rooms" ? data.rooms[0] : null);
      if (page === "rooms" && selected && params.get("room") !== selected.room) {
        if (params.has("room")) { params.delete("q"); params.delete("file"); }
        params.set("room", selected.room);
        history.replaceState(null, "", "#" + params.toString());
      }
      for (const room of data.rooms) {
        const row = button("", () => change({ page: "rooms", room: room.room, file: "", q: null }), selected?.room === room.room);
        row.classList.add("room");
        row.append(element("span", `${room.date}  ${room.title}`, "room-title"));
        if (room.description) row.append(element("span", room.description, "room-desc"));
        const status = element("span", undefined, "room-state");
        status.append(element("span", room.state, statusClass(room.state)));
        if (room.unanswered) status.append(element("span", `${labels.unanswered} ${room.unanswered}`, "badge unanswered"));
        if (room.pending) status.append(element("span", `${labels.pending} ${room.pending}`, "badge pending"));
        status.append(draftCountBadge(room.room));
        row.append(status);
        if (room.status) row.append(element("span", "status: " + room.status, "room-desc"));
        if (room.modified) row.append(element("span", "最終更新 " + new Date(room.modified * 1000).toLocaleString(), "room-desc"));
        $("rooms").append(row);
      }
      updateDraftUI();
      if (page === "rooms") {
        if (selected) {
          await Promise.all([
            showRoom(selected, params, ticket),
            (async () => {
              const questions = await api("/api/questions?" + new URLSearchParams({ room: selected.room }));
              if (ticket === generation) renderQuestions([questions], params, ticket, true);
            })()
          ]);
        } else {
          renderQuestions([], params, ticket, true);
          $("question-summary").textContent = "0 件";
          $("question-list").append(element("p", "該当する部屋はありません。部屋の条件を変えてください。", "muted"));
          content.append(element("p", "該当する部屋はありません。部屋の条件を変えてください。", "muted"));
        }
      } else {
        const questions = await api("/api/questions/all?" + scope(params));
        if (ticket !== generation) return;
        const question = renderQuestions(questions.rooms, params, ticket, false);
        if (selected && question) await showRoom(selected, params, ticket);
        else content.append(element("p", "確認事項を選ぶと、その部屋のファイルを表示します", "muted"));
      }
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
  $("question-filters").addEventListener("submit", (event) => {
    event.preventDefault();
    applyQuestionFilters();
  });
  $("question-query").addEventListener("input", () => scheduleQuestionFilters(300));
  $("question-query").addEventListener("keydown", (event) => {
    if (event.key === "Enter" && !event.isComposing) {
      event.preventDefault();
      applyQuestionFilters();
    }
  });
  for (const id of ["qstate-filter", "qsort-filter"]) {
    $(id).querySelectorAll("button").forEach((node) => node.addEventListener("click", () => {
      setSegment(id, node.value);
      scheduleQuestionFilters(0);
    }));
  }
  $("clear-question-filters").addEventListener("click", () => {
    clearTimeout(questionFilterTimer);
    questionFilterTimer = null;
    controlQuestionFilterState = null;
    change(initialQuestionFilters());
  });
  $("all-answer-send").addEventListener("click", () => confirmAnswers(visibleAnswers()));
  for (const name of ["questions", "files"]) {
    const other = name === "questions" ? "files" : "questions";
    $(name + "-collapse").addEventListener("click", () => setCollapsedPane(name));
    $(name + "-expand").addEventListener("click", () => setCollapsedPane(other));
    $(name + "-restore").addEventListener("click", () => setCollapsedPane(""));
  }
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
  Promise.all([api("/api/config"), api("/api/hello")]).then(([data, hello]) => {
    rootHash = hello.root_hash;
    loadDrafts();
    config = data;
    $("root").title = "作業ルート: " + data.root;
    const params = normalizeHash();
    for (const [key, value] of Object.entries({ ...initialFilters(), page: "rooms" })) if (!params.has(key)) params.set(key, String(value));
    history.replaceState(null, "", "#" + params.toString());
    refresh();
  }).catch(showError);
})();
