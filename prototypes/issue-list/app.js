// Issue list prototype: three variants behind one switcher, against mock data only.
(function () {
  const { statuses, gateLabels, actors, issues } = window.PUNCHLIST_MOCK;
  const statusById = Object.fromEntries(statuses.map((s) => [s.id, s]));

  const state = {
    variant: "list",
    selected: null, // issue id
    column: "in_progress", // board column shown on narrow screens
    search: "",
    who: "",
    hideClosed: true,
    menu: null, // "status" | "open" | null
  };

  const $view = document.getElementById("view");
  const $shortcuts = document.getElementById("shortcuts");
  const $overlay = document.getElementById("overlay");
  const $search = document.getElementById("search");
  const $who = document.getElementById("who");
  const $hideClosed = document.getElementById("hide-closed");
  const $count = document.getElementById("count");

  const params = new URLSearchParams(location.search);
  if (params.get("variant") && statusesVariant(params.get("variant"))) state.variant = params.get("variant");
  if (params.get("q")) state.search = params.get("q");
  function statusesVariant(v) { return ["list", "board", "cards"].includes(v); }

  // ---------- helpers ----------
  const esc = (s) => String(s).replace(/[&<>"']/g, (c) => ({ "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;", "'": "&#39;" }[c]));

  const ICON = {
    lock: '<svg viewBox="0 0 16 16" fill="currentColor" aria-hidden="true"><path d="M4 7V5a4 4 0 1 1 8 0v2h1a1 1 0 0 1 1 1v6a1 1 0 0 1-1 1H3a1 1 0 0 1-1-1V8a1 1 0 0 1 1-1h1Zm2 0h4V5a2 2 0 1 0-4 0v2Z"/></svg>',
    bot: '<svg viewBox="0 0 16 16" fill="none" stroke="currentColor" stroke-width="1.6" aria-hidden="true"><rect x="2.5" y="5" width="11" height="8" rx="2"/><path d="M8 2.5V5M5.5 8.5h.01M10.5 8.5h.01M6 11h4"/></svg>',
    pr: '<svg viewBox="0 0 16 16" fill="currentColor" aria-hidden="true"><path d="M4.75 1.5a2.25 2.25 0 0 1 .75 4.372v4.256a2.25 2.25 0 1 1-1.5 0V5.872A2.25 2.25 0 0 1 4.75 1.5Zm0 1.5a.75.75 0 1 0 0 1.5.75.75 0 0 0 0-1.5Zm0 8.5a.75.75 0 1 0 0 1.5.75.75 0 0 0 0-1.5ZM9.47 2.22a.75.75 0 0 1 1.06 0l1.72 1.72V4A2 2 0 0 1 12 4v6.128a2.25 2.25 0 1 1-1.5 0V5.56l-1.03 1.03a.75.75 0 0 1-1.06-1.06l2-2 .03-.03-.03-.03-2-2a.75.75 0 0 1 0-1.06l1.06 1.06Zm1.78 9.28a.75.75 0 1 0 0 1.5.75.75 0 0 0 0-1.5Z"/></svg>',
    merge: '<svg viewBox="0 0 16 16" fill="currentColor" aria-hidden="true"><path d="M5.45 5.154A4.25 4.25 0 0 0 9.25 7.5h1.378a2.251 2.251 0 1 1 0 1.5H9.25A5.734 5.734 0 0 1 5 7.123v3.505a2.25 2.25 0 1 1-1.5 0V5.372a2.25 2.25 0 1 1 1.95-.218ZM4.25 13.5a.75.75 0 1 0 0-1.5.75.75 0 0 0 0 1.5Zm8.5-4.5a.75.75 0 1 0 0-1.5.75.75 0 0 0 0 1.5ZM5 3.25a.75.75 0 1 0-1.5 0 .75.75 0 0 0 1.5 0Z"/></svg>',
    closed: '<svg viewBox="0 0 16 16" fill="currentColor" aria-hidden="true"><path d="M3.25 1A2.25 2.25 0 0 1 4 5.372v5.256a2.251 2.251 0 1 1-1.5 0V5.372A2.251 2.251 0 0 1 3.25 1Zm9.5 5.5a.75.75 0 0 1 .75.75v3.378a2.251 2.251 0 1 1-1.5 0V7.25a.75.75 0 0 1 .75-.75Zm-2.03-5.273a.75.75 0 0 1 1.06 0l.97.97.97-.97a.75.75 0 1 1 1.06 1.06l-.97.97.97.97a.75.75 0 0 1-1.06 1.06l-.97-.97-.97.97a.75.75 0 1 1-1.06-1.06l.97-.97-.97-.97a.75.75 0 0 1 0-1.06ZM2.5 3.25a.75.75 0 1 0 1.5 0 .75.75 0 0 0-1.5 0ZM3.25 12a.75.75 0 1 0 0 1.5.75.75 0 0 0 0-1.5Zm9.5 0a.75.75 0 1 0 0 1.5.75.75 0 0 0 0-1.5Z"/></svg>',
  };

  function lockBadge(status) {
    if (!status.lock) return "";
    return `<span class="lock" title="Locked to people: ${esc(status.lock.join(" and "))} cannot act here">${ICON.lock} people only</span>`;
  }

  function assigneeHtml(issue, opts = {}) {
    const a = issue.assignee ? actors[issue.assignee] : null;
    if (!a) return `<span class="assignee none"><span class="avatar none" aria-hidden="true"></span><span class="name">Unassigned</span></span>`;
    if (a.type === "agent") {
      return `<span class="assignee agent" title="Agent: ${esc(a.name)}"><span class="avatar agent" aria-hidden="true">${ICON.bot}</span><span class="kind">${esc(a.name)}</span><span class="sr-only">agent</span></span>`;
    }
    return `<span class="assignee person" title="Person: ${esc(a.name)}"><span class="avatar person" aria-hidden="true">${esc(a.initials)}</span><span class="name">${esc(opts.short ? a.name.split(" ")[0] : a.name)}</span><span class="sr-only">person</span></span>`;
  }

  const RUN_LABEL = { queued: "Queued", running: "Running", failed: "Run failed", idle: "Idle" };
  function runHtml(issue, withDetail) {
    const r = issue.run;
    const label = RUN_LABEL[r.state] + (r.since && r.state !== "idle" ? ` · ${r.since}` : "");
    const detail = withDetail && r.detail ? ` <span class="detail">${esc(r.detail)}</span>` : "";
    return `<span class="run ${r.state}" title="${esc(r.detail || label)}"><i aria-hidden="true"></i>${esc(label)}${detail}</span>`;
  }

  function prHtml(issue) {
    const p = issue.pr;
    if (!p) return `<span class="pr none">no PR</span>`;
    const icon = p.state === "merged" ? ICON.merge : p.state === "closed" ? ICON.closed : ICON.pr;
    const checks = p.checks !== "none" ? `<i class="checks ${p.checks}" title="checks ${p.checks}" aria-label="checks ${p.checks}"></i>` : "";
    const label = p.state === "closed" ? "closed" : p.state;
    return `<span class="pr ${p.state}" title="PR #${p.number} ${label}${p.checks !== "none" ? ", checks " + p.checks : ""}">${icon}#${p.number} ${label} ${checks}</span>`;
  }

  function gateSummary(issue) {
    const status = statusById[issue.status];
    const gates = status.next ? status.next.gates : [];
    if (!gates.length) {
      let why = "no gates";
      if (status.next && status.next.by === "person") why = "no gates · person moves it";
      if (status.next && status.next.by === "runner") why = "no gates · runner claims it";
      if (status.next && status.next.by === "event") why = `no gates · on ${status.next.event.replace(/_/g, " ")}`;
      return { gates, pass: 0, total: 0, text: why };
    }
    const pass = gates.filter((g) => issue.gates[g] === true).length;
    return { gates, pass, total: gates.length, text: `${pass} of ${gates.length} gates pass` };
  }

  function gatesHtml(issue) {
    const g = gateSummary(issue);
    if (!g.total) return `<span class="gates none" title="${esc(g.text)}">${esc(g.text)}</span>`;
    const bar = g.gates.map((id) => `<b class="${issue.gates[id] ? "pass" : "fail"}" title="${esc(gateLabels[id])}: ${issue.gates[id] ? "pass" : "fail"}"></b>`).join("");
    return `<span class="gates ${g.pass === g.total ? "all" : ""}" title="${esc(g.gates.map((id) => `${gateLabels[id]}: ${issue.gates[id] ? "pass" : "fail"}`).join("\n"))}"><span class="bar" aria-hidden="true">${bar}</span>${esc(g.text)}</span>`;
  }

  function gateListHtml(issue) {
    const g = gateSummary(issue);
    if (!g.total) return `<span class="gates none">${esc(g.text)}</span>`;
    return `<span class="gate-list">${g.gates.map((id) => `<span class="${issue.gates[id] ? "pass" : "fail"}">${issue.gates[id] ? "✓" : "✕"} ${esc(gateLabels[id])}</span>`).join("")}</span>`;
  }

  function labelsHtml(issue) {
    return issue.labels.map((l) => `<span class="label">${esc(l)}</span>`).join(" ");
  }

  // ---------- filtering ----------
  function visibleIssues() {
    const q = state.search.trim().toLowerCase();
    return issues.filter((i) => {
      if (state.hideClosed && (i.status === "done" || i.status === "canceled")) return false;
      if (state.who) {
        const a = i.assignee ? actors[i.assignee] : null;
        if (state.who === "none" && a) return false;
        if (state.who === "people" && (!a || a.type !== "person")) return false;
        if (state.who === "agents" && (!a || a.type !== "agent")) return false;
        if (["claude", "codex", "opencode"].includes(state.who) && i.assignee !== state.who) return false;
      }
      if (q) {
        const hay = `${i.id} ${i.title} ${i.labels.join(" ")} ${i.assignee ? actors[i.assignee].name : ""} ${statusById[i.status].label}`.toLowerCase();
        if (!hay.includes(q)) return false;
      }
      return true;
    });
  }

  function grouped(list) {
    return statuses.map((s) => ({ status: s, issues: list.filter((i) => i.status === s.id) }));
  }

  // Order in which j/k walk the issues for the current variant.
  function navOrder(list) {
    if (state.variant === "board" && isNarrow()) {
      return list.filter((i) => i.status === state.column).map((i) => i.id);
    }
    return grouped(list).flatMap((g) => g.issues.map((i) => i.id));
  }
  function isNarrow() { return window.matchMedia("(max-width: 767px)").matches; }

  // ---------- rendering ----------
  function render() {
    const list = visibleIssues();
    $count.textContent = `${list.length} of ${issues.length}`;
    document.querySelectorAll(".switcher button").forEach((b) => b.setAttribute("aria-pressed", String(b.dataset.variant === state.variant)));
    $view.className = state.variant === "list" ? "list" : state.variant === "board" ? "board-view" : "cards-view";
    $view.dataset.variant = state.variant;

    if (!list.length) {
      $view.innerHTML = emptyHtml();
      renderShortcuts();
      return;
    }
    const order = navOrder(list);
    if (!state.selected || !order.includes(state.selected)) state.selected = order[0] || null;

    if (state.variant === "list") $view.innerHTML = renderList(list);
    else if (state.variant === "board") $view.innerHTML = renderBoard(list);
    else $view.innerHTML = renderCards(list);
    renderShortcuts();
    const sel = $view.querySelector(`[data-issue="${state.selected}"]`);
    if (sel) sel.scrollIntoView({ block: "nearest" });
  }

  function emptyHtml() {
    const why = state.search ? `No issues match “${esc(state.search)}”.` : "No issues match these filters.";
    return `<section class="empty" data-empty>
      <h2>Nothing here</h2>
      <p>${why} Issues appear here as the runner picks them up and moves them through the workflow.</p>
      <button type="button" data-action="clear-filters">Clear filters</button>
    </section>`;
  }

  function groupHead(g, extra = "") {
    const s = g.status;
    return `<div class="group-head" data-status="${s.id}">
      <span class="status-dot" aria-hidden="true"></span>
      <span>${esc(s.label)}</span>
      <span class="n">${g.issues.length}</span>
      ${lockBadge(s)}
      ${s.dispatch ? `<span class="dispatch" title="Issues in this status are dispatched to ${esc(s.dispatch)}">${ICON.bot ? "" : ""}dispatches ${esc(actors[s.dispatch === "claude-code" ? "claude" : s.dispatch].name)}</span>` : ""}
      ${extra}
    </div>`;
  }

  // Variant A: dense list
  function renderList(list) {
    return grouped(list)
      .filter((g) => g.issues.length)
      .map((g) => `<section class="group" data-status="${g.status.id}">
        ${groupHead(g)}
        <div class="rows" role="listbox" aria-label="${esc(g.status.label)}">
          ${g.issues.map((i) => `<div class="row" role="option" data-issue="${i.id}" aria-selected="${i.id === state.selected}" tabindex="-1">
            <span class="id mono">${esc(i.id)}</span>
            <span class="title">${esc(i.title)}${labelsHtml(i)}</span>
            ${assigneeHtml(i)}
            <span class="meta">${runHtml(i)}${prHtml(i)}${gatesHtml(i)}</span>
          </div>`).join("")}
        </div>
      </section>`).join("");
  }

  // Variant B: board
  function renderBoard(list) {
    const groups = grouped(list);
    const tabs = `<div class="col-tabs" role="tablist" aria-label="Status columns">${groups.map((g) => `<button type="button" role="tab" data-column="${g.status.id}" aria-pressed="${g.status.id === state.column}" data-status="${g.status.id}"><span class="status-dot" aria-hidden="true"></span>${esc(g.status.label)} <span class="n">${g.issues.length}</span>${g.status.lock ? ICON.lock : ""}</button>`).join("")}</div>`;
    const cols = groups.map((g) => `<section class="col" data-status="${g.status.id}" aria-current="${g.status.id === state.column}">
      <div class="col-head">${groupHead(g)}</div>
      <div class="cards" role="listbox" aria-label="${esc(g.status.label)}">
        ${g.issues.length ? g.issues.map((i) => `<article class="card" role="option" data-issue="${i.id}" aria-selected="${i.id === state.selected}" tabindex="-1">
          <div class="head"><span class="mono">${esc(i.id)}</span>${assigneeHtml(i, { short: true })}</div>
          <div class="title">${esc(i.title)}</div>
          <div class="foot">${runHtml(i)}${prHtml(i)}</div>
          <div class="foot">${gatesHtml(i)}</div>
        </article>`).join("") : `<div class="empty-col">No issues</div>`}
      </div>
    </section>`).join("");
    return `${tabs}<div class="board">${cols}</div>`;
  }

  // Variant C: cards with the gate checklist and run detail
  function renderCards(list) {
    return grouped(list)
      .filter((g) => g.issues.length)
      .map((g) => `<section class="group" data-status="${g.status.id}">
        ${groupHead(g)}
        <div class="grid" role="listbox" aria-label="${esc(g.status.label)}">
          ${g.issues.map((i) => {
            const gs = gateSummary(i);
            return `<article class="card" role="option" data-issue="${i.id}" aria-selected="${i.id === state.selected}" tabindex="-1">
              <div class="head"><span class="mono">${esc(i.id)}</span><span>${labelsHtml(i)}</span></div>
              <div class="title">${esc(i.title)}</div>
              <div class="line"><span class="k">who</span><span class="v">${assigneeHtml(i)}</span></div>
              <div class="line"><span class="k">run</span><span class="v">${runHtml(i, true)}</span></div>
              <div class="line"><span class="k">PR</span><span class="v">${prHtml(i)}</span></div>
              <div class="line"><span class="k">gates</span><span class="v" style="white-space:normal">${gateListHtml(i)}</span></div>
              ${gs.total ? `<div class="gate-sum"><span class="progress" aria-hidden="true"><b style="width:${(gs.pass / gs.total) * 100}%"></b></span><span class="gates ${gs.pass === gs.total ? "all" : ""}">${esc(gs.text)}</span></div>` : ""}
            </article>`;
          }).join("")}
        </div>
      </section>`).join("");
  }

  function renderShortcuts() {
    const common = [
      ["j", "next issue"], ["k", "previous issue"], ["Enter", "open issue"], ["s", "change status"],
    ];
    const perVariant = {
      list: [["/", "filter"], ["1 2 3", "switch variant"]],
      board: [["h", "previous column"], ["l", "next column"], ["/", "filter"], ["1 2 3", "switch variant"]],
      cards: [["/", "filter"], ["1 2 3", "switch variant"]],
    };
    const title = { list: "List", board: "Board", cards: "Cards" }[state.variant];
    $shortcuts.innerHTML = `<span class="title">${title} shortcuts</span>` +
      [...common, ...perVariant[state.variant], ["Esc", "close"]]
        .map(([k, d]) => `<span>${k.split(" ").map((x) => `<kbd>${esc(x)}</kbd>`).join("")} ${esc(d)}</span>`).join("");
  }

  // ---------- overlays ----------
  function closeOverlay() { state.menu = null; $overlay.innerHTML = ""; }

  function openIssue(id) {
    const i = issues.find((x) => x.id === id);
    if (!i) return;
    const s = statusById[i.status];
    state.menu = "open";
    $overlay.innerHTML = `<div class="scrim" data-action="close"></div>
      <aside class="sheet" role="dialog" aria-modal="true" aria-label="${esc(i.id)}" data-status="${s.id}">
        <button type="button" class="close" data-action="close">Esc</button>
        <div class="muted mono">${esc(i.id)}</div>
        <h2>${esc(i.title)}</h2>
        <dl>
          <dt>Status</dt><dd><span class="status-dot" aria-hidden="true"></span> ${esc(s.label)} ${lockBadge(s)}</dd>
          <dt>Assignee</dt><dd>${assigneeHtml(i)}</dd>
          <dt>Run</dt><dd>${runHtml(i, true)}</dd>
          <dt>Pull request</dt><dd>${prHtml(i)}</dd>
          <dt>Gates</dt><dd>${gateListHtml(i)}</dd>
          <dt>Labels</dt><dd>${labelsHtml(i) || "<span class='muted'>none</span>"}</dd>
          <dt>Updated</dt><dd>${esc(i.updated)} ago</dd>
        </dl>
        <p class="muted" style="margin-top:18px">In the product, Enter opens the issue page (spec, runs, pull request, gates, proof, timeline). This prototype stops here.</p>
      </aside>`;
    $overlay.querySelector(".close").focus();
  }

  function openStatusMenu(id) {
    const i = issues.find((x) => x.id === id);
    if (!i) return;
    const anchor = $view.querySelector(`[data-issue="${id}"]`);
    const r = anchor ? anchor.getBoundingClientRect() : { left: 20, bottom: 80, top: 60 };
    state.menu = "status";
    const items = statuses.map((s) => `<button type="button" role="menuitemradio" aria-checked="${s.id === i.status}" data-set-status="${s.id}" data-status="${s.id}"><span class="status-dot" aria-hidden="true"></span>${esc(s.label)}${lockBadge(s)}</button>`).join("");
    $overlay.innerHTML = `<div class="scrim" data-action="close"></div><div class="menu" role="menu" aria-label="Change status of ${esc(i.id)}"><header>Move ${esc(i.id)} to…</header>${items}</div>`;
    const m = $overlay.querySelector(".menu");
    const left = Math.min(Math.max(12, r.left + 12), window.innerWidth - m.offsetWidth - 12);
    let top = r.bottom + 4;
    if (top + m.offsetHeight > window.innerHeight - 70) top = Math.max(12, r.top - m.offsetHeight - 4);
    m.style.left = `${left}px`;
    m.style.top = `${top}px`;
    const cur = m.querySelector('[aria-checked="true"]');
    (cur || m.querySelector("button")).focus();
  }

  function setStatus(id, statusId) {
    const i = issues.find((x) => x.id === id);
    if (i) { i.status = statusId; if (state.variant === "board") state.column = statusId; }
    closeOverlay();
    render();
  }

  // ---------- navigation ----------
  function move(delta) {
    const order = navOrder(visibleIssues());
    if (!order.length) return;
    const idx = Math.max(0, order.indexOf(state.selected));
    const next = Math.min(order.length - 1, Math.max(0, idx + delta));
    state.selected = order[next];
    $view.querySelectorAll("[data-issue]").forEach((el) => el.setAttribute("aria-selected", String(el.dataset.issue === state.selected)));
    const el = $view.querySelector(`[data-issue="${state.selected}"]`);
    if (el) el.scrollIntoView({ block: "nearest" });
  }

  function moveColumn(delta) {
    const ids = statuses.map((s) => s.id);
    const idx = ids.indexOf(state.column);
    state.column = ids[Math.min(ids.length - 1, Math.max(0, idx + delta))];
    state.selected = null;
    render();
  }

  function setVariant(v) {
    state.variant = v;
    const u = new URL(location.href); u.searchParams.set("variant", v); history.replaceState(null, "", u);
    render();
  }

  // ---------- events ----------
  document.addEventListener("keydown", (e) => {
    const inField = ["INPUT", "SELECT", "TEXTAREA"].includes(document.activeElement.tagName);
    if (e.key === "Escape") {
      if (state.menu) { closeOverlay(); e.preventDefault(); return; }
      if (inField) { document.activeElement.blur(); return; }
    }
    if (state.menu === "status") {
      const btns = [...$overlay.querySelectorAll("[data-set-status]")];
      const idx = btns.indexOf(document.activeElement);
      if (e.key === "j" || e.key === "ArrowDown") { btns[Math.min(btns.length - 1, idx + 1)].focus(); e.preventDefault(); }
      if (e.key === "k" || e.key === "ArrowUp") { btns[Math.max(0, idx - 1)].focus(); e.preventDefault(); }
      return;
    }
    if (state.menu) return;
    if (inField) return;
    if (e.metaKey || e.ctrlKey || e.altKey) return;
    switch (e.key) {
      case "j": case "ArrowDown": move(1); e.preventDefault(); break;
      case "k": case "ArrowUp": move(-1); e.preventDefault(); break;
      case "Enter": if (state.selected) openIssue(state.selected); e.preventDefault(); break;
      case "s": if (state.selected) openStatusMenu(state.selected); e.preventDefault(); break;
      case "h": if (state.variant === "board") { moveColumn(-1); e.preventDefault(); } break;
      case "l": if (state.variant === "board") { moveColumn(1); e.preventDefault(); } break;
      case "/": $search.focus(); e.preventDefault(); break;
      case "1": setVariant("list"); break;
      case "2": setVariant("board"); break;
      case "3": setVariant("cards"); break;
    }
  });

  document.addEventListener("click", (e) => {
    const sw = e.target.closest(".switcher button");
    if (sw) { setVariant(sw.dataset.variant); return; }
    const tab = e.target.closest("[data-column]");
    if (tab) { state.column = tab.dataset.column; state.selected = null; render(); return; }
    const set = e.target.closest("[data-set-status]");
    if (set) { setStatus(state.selected, set.dataset.setStatus); return; }
    const action = e.target.closest("[data-action]");
    if (action) {
      if (action.dataset.action === "close") closeOverlay();
      if (action.dataset.action === "clear-filters") { state.search = ""; state.who = ""; $search.value = ""; $who.value = ""; render(); }
      return;
    }
    const row = e.target.closest("[data-issue]");
    if (row) {
      if (state.selected === row.dataset.issue) openIssue(row.dataset.issue);
      else { state.selected = row.dataset.issue; move(0); }
    }
  });

  $search.addEventListener("input", () => { state.search = $search.value; render(); });
  $who.addEventListener("change", () => { state.who = $who.value; render(); });
  $hideClosed.addEventListener("change", () => { state.hideClosed = $hideClosed.checked; render(); });
  window.addEventListener("resize", () => render());

  $search.value = state.search;
  render();
})();
