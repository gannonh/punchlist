// Inbox prototype: three layouts over the same mock items, each with its own meaning of "done with it".
// Nothing here is wired to production code.
(function () {
  const { statuses, rule, items } = window.PUNCHLIST_INBOX_MOCK;
  const VARIANTS = ["page", "split", "panel"];
  // How each layout answers "done with it".
  const DONE = {
    page: { key: "clears", text: "<b>Acting clears it.</b> An item is derived from state and leaves when its condition ends: the issue moves, the run succeeds, a later request is allowed. No dismiss. Opening an item marks it read, which only lowers the unread count." },
    split: { key: "dismiss", text: "<b>Dismiss.</b> Acting clears an item, as on the page, and Dismiss also hides it until its condition arises again. Opening an item marks it read." },
    panel: { key: "markread", text: "<b>Mark read, keep visible.</b> An item stays, with its kind, and shows the status it moved to; Mark all read clears the unread count only." },
  };
  const KIND = { waiting: "Waiting on you", run: "Run needs you", refusal: "Refused transition" };
  const state = { variant: "page", backlog: false, ruleOpen: true, selected: null, read: new Set(), gone: new Set(), moved: {} };

  const $view = document.getElementById("view");
  const $toast = document.getElementById("toast");

  const params = new URLSearchParams(location.search);
  if (VARIANTS.includes(params.get("variant"))) state.variant = params.get("variant");

  // ---------- helpers ----------
  const esc = (s) => String(s).replace(/[&<>"']/g, (c) => ({ "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;", "'": "&#39;" }[c]));
  const byId = (id) => items.find((i) => i.id === id);
  const visible = () => items.filter((i) => (state.backlog || !i.backlog) && !state.gone.has(i.id));
  const isUnread = (i) => i.unread && !state.read.has(i.id);
  const unreadCount = () => visible().filter(isUnread).length;
  const statusOf = (i) => state.moved[i.id] || i.status;
  const label = (s) => statuses[s].label;

  const statusHtml = (s) => `<span class="status" data-status="${s}" data-field="status"><span class="status-dot" aria-hidden="true"></span>${esc(label(s))}</span>`;
  const kindHtml = (i) => `<span class="kind" data-kind-label="${i.kind}">${esc(KIND[i.kind])}</span>`;
  const unreadHtml = (i) => isUnread(i) ? `<span class="unread-dot" data-unread title="Unread"><span class="sr-only">Unread</span></span>` : `<span class="unread-dot read" aria-hidden="true"></span>`;
  const countHtml = () => `<b class="count attention" data-unread-count>${unreadCount()}</b>`;

  // What each kind says: why it waits, what the run did, or the refused request's gate and reason as plain text.
  function causeHtml(i) {
    if (state.moved[i.id]) return `<div class="cause moved" data-moved>Moved to ${esc(label(state.moved[i.id]))} by you. Its condition ended; it stays, read, until the inbox next loads.</div>`;
    if (i.kind === "waiting") return `<div class="cause">${esc(i.why)}</div>`;
    if (i.kind === "run") return `<div class="cause cause-run"><b>${esc(i.run)}:</b> <span data-run-reason>${esc(i.reason)}</span></div>`;
    return `<div class="cause cause-refusal"><b>${esc(i.agent)}</b> asked for ${esc(i.request)}. Refused: gate <code data-gate>${esc(i.gate)}</code> failed: <span data-refusal-reason>${esc(i.reason)}</span></div>`;
  }

  function actionsHtml(i) {
    const move = state.moved[i.id] ? "" : `<button type="button" class="btn" data-act="move" data-id="${i.id}">Move to ${esc(label(i.move))}</button>`;
    const dismiss = state.variant === "split" ? `<button type="button" class="btn ghost" data-act="dismiss" data-id="${i.id}">Dismiss</button>` : "";
    return `<span class="actions"><button type="button" class="btn primary" data-act="open" data-id="${i.id}">Open ${esc(i.id)}</button>${move}${dismiss}</span>`;
  }

  const itemAttrs = (i, cls) => `class="${cls}${isUnread(i) ? " is-unread" : ""}${state.selected === i.id ? " is-selected" : ""}" tabindex="0" data-item="${i.id}" data-kind="${i.kind}" aria-label="${esc(KIND[i.kind])}: ${esc(i.id)} ${esc(i.title)}${isUnread(i) ? ", unread" : ""}"`;

  function itemHtml(i, cls = "item") {
    return `<article ${itemAttrs(i, cls)}>
      <div class="item-head">${unreadHtml(i)}<span class="mono id">${esc(i.id)}</span>${kindHtml(i)}${statusHtml(statusOf(i))}<span class="ago muted">${esc(i.ago)}</span></div>
      <div class="item-title">${esc(i.title)}</div>
      ${causeHtml(i)}
      <div class="item-foot"><span class="whose muted">${esc(i.whose)}</span>${actionsHtml(i)}</div>
    </article>`;
  }

  function toast(text) {
    $toast.textContent = text;
    $toast.hidden = false;
    clearTimeout(toast.t);
    toast.t = setTimeout(() => { $toast.hidden = true; }, 2200);
  }

  // ---------- shared panels ----------
  // Mock fragments of the list and the board, showing how each reaches the inbox, with the count as it is now.
  const nav = (on) => `<nav class="mock-nav"><span>Issues</span><span class="here">Inbox ${countHtml()}</span></nav><span class="switcher-mock"><span${on === "list" ? ' class="on"' : ""}>List</span><span${on === "board" ? ' class="on"' : ""}>Board</span></span>`;
  const entry = () => ({
    page: {
      list: `<div class="mock-bar">${nav("list")}<kbd>g</kbd> <kbd>i</kbd></div>`,
      board: `<div class="mock-bar">${nav("board")}<kbd>g</kbd> <kbd>i</kbd></div>`,
      note: "An Inbox item next to Issues in the app header, with the unread count, on the list and the board. <kbd>g</kbd> <kbd>i</kbd> opens it.",
    },
    split: {
      list: `<div class="mock-bar"><span class="mock-side"><span class="here">Inbox ${countHtml()}</span><span>Issues · List</span></span><kbd>g</kbd> <kbd>i</kbd></div>`,
      board: `<div class="mock-bar"><span class="mock-side"><span class="here">Inbox ${countHtml()}</span><span>Issues · Board</span></span><kbd>g</kbd> <kbd>i</kbd></div>`,
      note: "An Inbox entry at the top of a sidebar beside the list and the board, with the unread count. <kbd>g</kbd> <kbd>i</kbd> opens it.",
    },
    panel: {
      list: `<div class="mock-bar"><span class="switcher-mock"><span class="on">List</span><span>Board</span></span><span class="inbox-btn">Inbox ${countHtml()}</span><kbd>i</kbd></div>`,
      board: `<div class="mock-bar"><span class="switcher-mock"><span>List</span><span class="on">Board</span></span><span class="inbox-btn">Inbox ${countHtml()}</span><kbd>i</kbd></div>`,
      note: "An Inbox button in the top bar opens this panel over the list or the board, which stays behind it. <kbd>i</kbd> toggles it; <kbd>Esc</kbd> closes it.",
    },
  });

  function entryHtml() {
    const e = entry()[state.variant];
    return `<details class="entry" open data-entry>
      <summary>How a person gets here</summary>
      <div class="entry-grid">
        <div class="entry-item" data-entry-from="list"><h3>From the list</h3>${e.list}</div>
        <div class="entry-item" data-entry-from="board"><h3>From the board</h3>${e.board}</div>
        <div class="entry-item"><h3>Shape</h3><div class="note">${e.note}</div></div>
      </div>
    </details>`;
  }

  function ruleHtml() {
    const waits = rule.filter((r) => r.waits);
    return `<details class="rule" data-rule ${state.ruleOpen ? "open" : ""}>
      <summary>Waiting on a person: ${waits.map((r) => `<span class="rule-status${r.status === "backlog" && !state.backlog ? " off" : ""}" data-rule-status="${r.status}">${esc(label(r.status))}</span>`).join(" ")}</summary>
      <p>An issue waits on a person when every transition out of its status that a caller can request is <code>by = ["person"]</code> and no other role. A transition an event makes (<code>on = "pr_merged"</code>) is not a request and does not count. A closed status, with no way out, never qualifies. In <code>.punchlist/workflow.toml</code> that gives Backlog, Todo, Human Review and Merging.</p>
      <label class="switch"><input type="checkbox" data-backlog ${state.backlog ? "checked" : ""} /> Include Backlog <span class="muted">(off: Backlog holds every parked idea and would bury the inbox)</span></label>
      <details class="rule-all"><summary>Every status in the workflow</summary><table class="rule-table"><thead><tr><th>Status</th><th>Transitions out</th><th>Waits on a person?</th></tr></thead><tbody>
        ${rule.map((r) => `<tr><td>${statusHtml(r.status)}</td><td>${esc(r.out)}</td><td>${r.waits ? "yes" : "no"}</td></tr>`).join("")}
      </tbody></table></details>
      <p class="muted">Whose item: an issue shows to its assignee when that is a person, to the owner of its assignee when that is an agent, and to every person when it is unassigned.</p>
    </details>`;
  }

  const doneHtml = () => `<div class="done" data-done="${DONE[state.variant].key}">${DONE[state.variant].text}</div>`;
  const summaryHtml = () => `<span class="summary muted"><span data-item-count>${visible().length}</span> items · <span data-unread-total>${unreadCount()}</span> unread</span>`;

  // ---------- variant A: a page with sections per kind ----------
  function renderPage() {
    const sections = ["refusal", "run", "waiting"].map((k) => ({ k, its: visible().filter((i) => i.kind === k) }));
    return `<h1 class="screen-title">Inbox <span class="badge">${countHtml()} unread</span> ${summaryHtml()}</h1>
      ${doneHtml()}${ruleHtml()}
      ${sections.map((s) => `<section class="a-section"><h2>${esc(KIND[s.k])} <span class="n">${s.its.length}</span></h2>
        <div class="a-items">${s.its.map((i) => itemHtml(i)).join("") || `<p class="none muted">Nothing here.</p>`}</div></section>`).join("")}`;
  }

  // ---------- variant B: two panes, list and detail ----------
  function rowB(i) {
    return `<article ${itemAttrs(i, "b-row")}>
      <div class="item-head">${unreadHtml(i)}<span class="mono id">${esc(i.id)}</span>${kindHtml(i)}${statusHtml(statusOf(i))}<span class="ago muted">${esc(i.ago)}</span></div>
      <div class="item-title">${esc(i.title)}</div>
      ${i.kind === "waiting" ? "" : causeHtml(i)}
      ${actionsHtml(i)}
    </article>`;
  }
  function renderSplit() {
    const its = visible();
    if (!its.some((i) => i.id === state.selected)) state.selected = its[0] && its[0].id;
    const sel = byId(state.selected);
    return `<h1 class="screen-title">Inbox <span class="badge">${countHtml()} unread</span> ${summaryHtml()}</h1>
      ${doneHtml()}${ruleHtml()}
      <div class="b-panes">
        <div class="b-list" aria-label="Inbox items">${its.map(rowB).join("") || `<p class="none muted">Nothing needs you.</p>`}</div>
        <aside class="b-detail" aria-label="Selected item">${sel ? `<div class="mono id">${esc(sel.id)}</div><h2>${esc(sel.title)}</h2>
          <div class="item-head">${kindHtml(sel)}${statusHtml(statusOf(sel))}<span class="ago muted">${esc(sel.ago)}</span></div>
          ${causeHtml(sel)}<p class="muted">${esc(sel.whose)}.</p>` : `<p class="muted">Nothing selected.</p>`}</aside>
      </div>`;
  }

  // ---------- variant C: a panel over the list and the board ----------
  function renderPanel() {
    const backdrop = items.slice(0, 6).map((i) => `<div class="c-bg-row"><span class="mono">${esc(i.id)}</span> ${esc(i.title)}</div>`).join("");
    return `<div class="c-stage">
      <div class="c-backdrop" aria-hidden="true"><div class="c-bg-row head">Issues · List</div>${backdrop}</div>
      <section class="c-panel" aria-label="Inbox">
        <div class="c-panel-head"><h1 class="screen-title">Inbox ${countHtml()}</h1>${summaryHtml()}
          <button type="button" class="btn" data-act="mark-all">Mark all read</button></div>
        ${doneHtml()}${ruleHtml()}
        <div class="c-items">${visible().map((i) => itemHtml(i, "item c-item")).join("")}</div>
      </section>
    </div>`;
  }

  const RENDER = { page: renderPage, split: renderSplit, panel: renderPanel };

  function render() {
    const focused = document.activeElement && (document.activeElement.dataset.item || null);
    document.querySelectorAll(".switcher button").forEach((b) => b.setAttribute("aria-pressed", String(b.dataset.variant === state.variant)));
    const $rule = document.querySelector("details[data-rule]");
    if ($rule) state.ruleOpen = $rule.open;
    $view.className = `variant-${state.variant}`;
    $view.innerHTML = entryHtml() + RENDER[state.variant]();
    if (focused) { const el = $view.querySelector(`[data-item="${focused}"]`); if (el) el.focus(); }
  }

  // Each layout starts from the mock's load state.
  function setVariant(v) {
    Object.assign(state, { variant: v, selected: null, read: new Set(), gone: new Set(), moved: {} });
    const url = new URL(location.href);
    url.searchParams.set("variant", v);
    history.replaceState(null, "", url);
    render();
  }

  function openItem(id) {
    state.read.add(id);
    state.selected = id;
    render();
    toast(`Opens ${id}'s issue page (/issues/${id})`);
  }

  function act(kind, id) {
    const i = byId(id);
    if (kind === "open") return openItem(id);
    if (kind === "mark-all") { visible().forEach((x) => state.read.add(x.id)); render(); return toast("Marked all read"); }
    if (kind === "dismiss") { state.gone.add(id); render(); return toast(`Dismissed ${id} until its condition arises again`); }
    // Move: the condition ends. The mark-read layout keeps the item, read, with its new status.
    state.read.add(id);
    if (DONE[state.variant].key === "markread") state.moved[id] = i.move;
    else state.gone.add(id);
    render();
    toast(`Moved ${id} to ${label(i.move)}`);
  }

  // ---------- events ----------
  document.addEventListener("click", (e) => {
    const sw = e.target.closest(".switcher button");
    if (sw) return setVariant(sw.dataset.variant);
    const btn = e.target.closest("[data-act]");
    if (btn) return act(btn.dataset.act, btn.dataset.id);
    const item = e.target.closest("[data-item]");
    if (item) openItem(item.dataset.item);
  });

  document.addEventListener("change", (e) => {
    if (e.target.matches("[data-backlog]")) { state.backlog = e.target.checked; render(); }
  });

  document.addEventListener("keydown", (e) => {
    if (e.metaKey || e.ctrlKey || e.altKey) return;
    const item = e.target.closest && e.target.closest("[data-item]");
    if (e.key === "Enter" && item && e.target === item) { e.preventDefault(); return openItem(item.dataset.item); }
    if (["INPUT", "SELECT", "TEXTAREA"].includes(document.activeElement.tagName)) return;
    const n = Number(e.key);
    if (n >= 1 && n <= VARIANTS.length) setVariant(VARIANTS[n - 1]);
  });

  render();
})();
