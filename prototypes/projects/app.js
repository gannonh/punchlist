// Projects prototype: three layouts of a projects and milestones screen over the same mock data. Nothing here is wired to production code.
(function () {
  const { statuses, projects } = window.PUNCHLIST_PROJECTS_MOCK;
  const statusById = Object.fromEntries(statuses.map((s) => [s.id, s]));
  const VARIANTS = ["sections", "panes", "cards"];
  const MAX_NAME_CHARS = 100;
  // The bar runs from Done to Backlog, so the done share reads from the left.
  const BAR_ORDER = ["done", "merging", "human_review", "agent_review", "in_progress", "start", "todo", "backlog"];

  const state = {
    variant: "sections",
    screen: "projects",
    listLayout: "list",
    project: projects[0].id,
    open: null,
    form: null,
    focus: null,
  };

  const $view = document.getElementById("view");

  const params = new URLSearchParams(location.search);
  if (VARIANTS.includes(params.get("variant"))) state.variant = params.get("variant");
  if (params.get("screen") === "list") state.screen = "list";
  if (findProject(params.get("project"))) state.project = params.get("project");
  if (findMilestone(params.get("milestone"))) openMilestone(params.get("milestone"));

  // ---------- helpers ----------
  const esc = (s) => String(s).replace(/[&<>"']/g, (c) => ({ "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;", "'": "&#39;" }[c]));
  const plural = (n, word) => `${n} ${word}${n === 1 ? "" : "s"}`;

  function findProject(id) { return projects.find((p) => p.id === id); }
  function findMilestone(id) {
    for (const project of projects) {
      const milestone = project.milestones.find((m) => m.id === id);
      if (milestone) return { project, milestone };
    }
    return null;
  }
  function openMilestone(id) {
    state.open = id;
    state.project = findMilestone(id).project.id;
  }
  let nextId = 100;
  const newId = (prefix) => `${prefix}${nextId++}`;

  // What counts: an issue in Done is done; a canceled issue is left out of the total.
  function progress(m) {
    const by = {};
    for (const i of m.issues) by[i.status] = (by[i.status] || 0) + 1;
    const canceled = by.canceled || 0;
    return { by, done: by.done || 0, total: m.issues.length - canceled, canceled };
  }

  const statusHtml = (id) => `<span class="status" data-status="${id}"><span class="status-dot" aria-hidden="true"></span>${esc(statusById[id].label)}</span>`;

  function progressHtml(m) {
    const p = progress(m);
    const parts = BAR_ORDER.filter((s) => p.by[s]);
    const label = p.total ? `${p.done} of ${p.total} done: ${parts.map((s) => `${p.by[s]} ${statusById[s].label}`).join(", ")}` : "No issues yet";
    return `<div class="progress-block" data-progress-of="${m.id}">
      <div class="progress-line">
        <span class="done-count" data-done-count>${p.done} of ${p.total} done</span>
        ${p.canceled ? `<span class="muted" data-left-out>${p.canceled} canceled, left out</span>` : ""}
      </div>
      <div class="split${p.total ? "" : " is-empty"}" role="img" aria-label="${esc(label)}" data-total="${p.total}">
        ${parts.map((s) => `<span class="seg" data-status="${s}" data-count="${p.by[s]}" style="flex-grow:${p.by[s]}" title="${p.by[s]} ${esc(statusById[s].label)}"></span>`).join("")}
      </div>
      ${parts.length ? `<div class="legend">${parts.map((s) => `<span data-status="${s}"><span class="status-dot" aria-hidden="true"></span>${p.by[s]} ${esc(statusById[s].label)}</span>`).join("")}</div>` : ""}
    </div>`;
  }

  const passWhenHtml = (m) => m.passWhen.trim()
    ? `<div class="pass-when" data-pass-when><span class="pw-label">PASS when</span><span class="pw-text">${esc(m.passWhen)}</span></div>`
    : `<div class="pass-when is-empty" data-pass-when><span class="pw-label">PASS when</span><span class="pw-text">No PASS when yet</span></div>`;

  const issueItem = (i) => `<li><a href="../issue-page/?issue=${esc(i.id)}" data-issue-link><span class="mono id">${esc(i.id)}</span><span class="ititle">${esc(i.title)}</span></a>${statusHtml(i.status)}</li>`;

  function issuesHtml(m) {
    const counted = m.issues.filter((i) => i.status !== "canceled");
    const left = m.issues.filter((i) => i.status === "canceled");
    const order = (i) => BAR_ORDER.indexOf(i.status);
    counted.sort((a, b) => order(b) - order(a));
    return `<div class="issues" data-issues-of="${m.id}">
      <h4 class="issues-head">${plural(counted.length, "issue")}</h4>
      ${counted.length ? `<ul class="issue-list">${counted.map(issueItem).join("")}</ul>` : `<p class="muted">No issues in this milestone yet. Pick it in an issue's sheet.</p>`}
      ${left.length ? `<h4 class="issues-head left-out">Left out of the total <span class="muted">canceled</span></h4><ul class="issue-list" data-left-out-list>${left.map(issueItem).join("")}</ul>` : ""}
    </div>`;
  }

  // ---------- buttons ----------
  const btn = (act, id, text, extra = "") => `<button type="button" class="btn ${extra}" data-act="${act}" data-id="${id ?? ""}" data-key="${act}:${id ?? ""}">${text}</button>`;
  const openBtn = (m, tag = "span") => `<button type="button" class="ms-open" data-act="open-m" data-id="${m.id}" data-key="open-m:${m.id}" aria-expanded="${state.open === m.id}"><${tag} class="ms-name">${esc(m.name)}</${tag}></button>`;
  const editMilestoneBtn = (m) => btn("edit-m", m.id, `Edit<span class="sr-only"> ${esc(m.name)}</span>`, "ghost");
  const renameProjectBtn = (p) => btn("edit-p", p.id, `Rename<span class="sr-only"> ${esc(p.name)}</span>`, "ghost");
  const newMilestoneBtn = (p) => btn("new-m", p.id, `+ New milestone<span class="sr-only"> in ${esc(p.name)}</span>`);

  // ---------- form: create a project, create a milestone, edit either ----------
  function formHtml() {
    const f = state.form;
    const what = f.kind;
    const project = findProject(f.projectId);
    const title = f.mode === "create"
      ? (what === "project" ? "New project" : `New milestone in ${esc(project.name)}`)
      : (what === "project" ? "Rename project" : "Edit milestone");
    const scope = what === "project" ? "the workspace" : esc(project.name);
    const n = [...f.name.trim()].length;
    return `<form class="pform" data-form novalidate aria-labelledby="f-title">
      <h2 id="f-title">${title}</h2>
      <label for="f-name">Name</label>
      <input id="f-name" name="name" autocomplete="off" value="${esc(f.name)}" aria-describedby="f-hint${f.error ? " f-error" : ""}"${f.error ? ' aria-invalid="true"' : ""} />
      <p id="f-hint" class="hint">1 to ${MAX_NAME_CHARS} characters, unique in ${scope}. <span id="f-count" class="${n > MAX_NAME_CHARS ? "over" : ""}">${n}/${MAX_NAME_CHARS}</span></p>
      ${f.error ? `<p id="f-error" class="error" role="alert" data-form-error>${esc(f.error)}</p>` : ""}
      ${what === "milestone" ? `<label for="f-pass">PASS when</label>
      <textarea id="f-pass" name="passWhen" rows="3" aria-describedby="f-pass-hint">${esc(f.passWhen)}</textarea>
      <p id="f-pass-hint" class="hint">What has to be true for the milestone to pass. Optional; empty reads "No PASS when yet".</p>` : ""}
      <div class="form-actions">
        <button type="submit" class="btn primary" data-key="save">${f.mode === "create" ? `Create ${what}` : "Save"}</button>
        <button type="button" class="btn" data-act="cancel-form" data-key="cancel-form">Cancel</button>
        <span class="muted"><kbd>Esc</kbd> cancels</span>
      </div>
    </form>`;
  }

  // The server's rules (crates/punchlist-server/src/projects.rs): trimmed, 1 to 100 characters, unique.
  function validate(f) {
    const name = f.name.trim();
    if (!name) return `a ${f.kind} needs a name`;
    if ([...name].length > MAX_NAME_CHARS) return `a ${f.kind} name is at most ${MAX_NAME_CHARS} characters`;
    if (f.kind === "project") {
      if (projects.some((p) => p.name === name && p.id !== f.id)) return `a project named ${name} already exists`;
    } else {
      const project = findProject(f.projectId);
      if (project.milestones.some((m) => m.name === name && m.id !== f.id)) return `project ${project.name} already has a milestone named ${name}`;
    }
    return null;
  }

  function startForm(form, trigger) {
    state.form = { name: "", passWhen: "", error: null, ...form, returnFocus: trigger };
    state.focus = "#f-name";
    render();
  }

  function closeForm(focusKey) {
    if (!state.form) return;
    state.focus = focusKey || state.form.returnFocus;
    state.form = null;
    render();
  }

  function submitForm() {
    const f = state.form;
    f.error = validate(f);
    if (f.error) {
      state.focus = "#f-name";
      render();
      return;
    }
    const name = f.name.trim();
    let focus;
    if (f.kind === "project" && f.mode === "create") {
      const p = { id: newId("p"), name, milestones: [] };
      projects.push(p);
      state.project = p.id;
      focus = `new-m:${p.id}`;
    } else if (f.kind === "project") {
      findProject(f.id).name = name;
      focus = f.returnFocus;
    } else if (f.mode === "create") {
      const m = { id: newId("m"), name, passWhen: f.passWhen, issues: [] };
      findProject(f.projectId).milestones.push(m);
      focus = `open-m:${m.id}`;
    } else {
      Object.assign(findMilestone(f.id).milestone, { name, passWhen: f.passWhen });
      focus = f.returnFocus;
    }
    closeForm(focus);
  }

  const isForm = (kind, mode, id) => state.form && state.form.kind === kind && state.form.mode === mode && (id === undefined || state.form.id === id || state.form.projectId === id);

  // ---------- entry points ----------
  // How a person reaches this screen from the list and the board, and how the issue sheet's fields link here.
  const SHEET_LINK = {
    sections: "scrolls to the project's section with the milestone open",
    panes: "selects the project on the left and opens the milestone on the right",
    cards: "opens the milestone's sheet over the cards",
  };
  function entryHtml() {
    const { project, milestone } = findMilestone("m2");
    const link = (q, text) => `<a class="entry-link" href="?variant=${state.variant}&${q}" data-nav="${q}">${esc(text)} →</a>`;
    return `<details class="entry" open data-entry>
      <summary>How a person gets here</summary>
      <div class="entry-grid">
        <div class="entry-item" data-entry-from="list">
          <h3>From the list and the board</h3>
          <div class="mock-bar"><nav class="mock-nav" aria-label="Mock app header"><span>Issues</span><span class="here">Projects</span></nav><span class="switcher-mock"><span class="on">List</span><span>Board</span></span><span><kbd>g</kbd> <kbd>m</kbd></span></div>
          <div class="note">A <b>Projects</b> item next to <b>Issues</b> in the app header, the same on the list and the board. <kbd>g</kbd> then <kbd>m</kbd> opens it from anywhere outside a text field; <kbd>g</kbd> <kbd>i</kbd> goes back. Try it: open Issues above, then press <kbd>g</kbd> <kbd>m</kbd>.</div>
        </div>
        <div class="entry-item" data-entry-from="sheet">
          <h3>From the issue sheet</h3>
          <dl class="mock-sheet">
            <dt>Project</dt><dd><span class="mock-select">${esc(project.name)} ▾</span>${link(`project=${project.id}`, "Open project")}</dd>
            <dt>Milestone</dt><dd><span class="mock-select">${esc(milestone.name)} ▾</span>${link(`milestone=${milestone.id}`, "Open milestone")}</dd>
          </dl>
          <div class="note">The sheet keeps its two pickers and the milestone's PASS when; a link after each opens this screen. In this layout the milestone link ${SHEET_LINK[state.variant]}. The issue page's project and milestone names link the same way.</div>
        </div>
      </div>
    </details>`;
  }

  function screenHead() {
    const milestones = projects.reduce((n, p) => n + p.milestones.length, 0);
    return `<div class="screen-head">
      <h1 class="screen-title">Projects <span class="muted">${plural(projects.length, "project")} · ${plural(milestones, "milestone")}</span></h1>
      ${state.variant === "panes" ? "" : btn("new-p", "", "+ New project", "primary")}
    </div>
    <p class="counting" data-counting>Progress counts issues in <b>Done</b> out of the milestone's issues. <b>Canceled</b> issues are left out of the total. The bar is split by status, Done first.</p>`;
  }

  // ---------- variant A: sections ----------
  // One page, each project a section of milestone rows; a milestone opens in place; forms are dialogs.
  function rowA(m) {
    const open = state.open === m.id;
    return `<article class="a-ms${open ? " open" : ""}" data-milestone="${m.id}">
      <div class="a-main">${openBtn(m)}${passWhenHtml(m)}</div>
      ${progressHtml(m)}
      <div class="a-actions">${editMilestoneBtn(m)}</div>
      ${open ? `<div class="a-issues">${issuesHtml(m)}</div>` : ""}
    </article>`;
  }
  function renderSections() {
    return projects.map((p) => `<section class="a-project" data-project="${p.id}">
      <header class="project-head"><h2>${esc(p.name)} <span class="n">${plural(p.milestones.length, "milestone")}</span></h2>${renameProjectBtn(p)}<span class="spacer"></span>${newMilestoneBtn(p)}</header>
      <div class="a-rows">${p.milestones.length ? p.milestones.map(rowA).join("") : `<p class="empty-note">No milestones yet.</p>`}</div>
    </section>`).join("") + (state.form ? `<dialog class="pdialog" aria-labelledby="f-title">${formHtml()}</dialog>` : "");
  }

  // ---------- variant B: two panes ----------
  // Projects on the left; one project's milestones on the right, a milestone drills in; forms are inline.
  function renderPanes() {
    const p = findProject(state.project);
    const left = projects.map((x) => isForm("project", "edit", x.id)
      ? `<li class="b-inline">${formHtml()}</li>`
      : `<li><button type="button" class="b-proj" data-act="select-p" data-id="${x.id}" data-key="select-p:${x.id}" aria-current="${x.id === p.id}"><span class="b-proj-name">${esc(x.name)}</span><span class="muted">${plural(x.milestones.length, "milestone")}</span></button></li>`).join("");
    const newProject = isForm("project", "create") ? `<div class="b-inline">${formHtml()}</div>` : btn("new-p", "", "+ New project");

    let right;
    const found = state.open && findMilestone(state.open);
    if (found && found.project.id === p.id) {
      const m = found.milestone;
      right = `<div class="b-drill" data-milestone="${m.id}">
        <button type="button" class="btn ghost back" data-act="close-m" data-id="${m.id}" data-key="close-m:${m.id}">← ${esc(p.name)}</button>
        ${isForm("milestone", "edit", m.id) ? `<div class="b-inline">${formHtml()}</div>` : `<header class="project-head"><h2 class="ms-name" tabindex="-1" data-key="drill:${m.id}">${esc(m.name)}</h2>${editMilestoneBtn(m)}</header>${passWhenHtml(m)}`}
        ${progressHtml(m)}
        ${issuesHtml(m)}
      </div>`;
    } else {
      const rows = p.milestones.map((m) => isForm("milestone", "edit", m.id)
        ? `<div class="b-inline">${formHtml()}</div>`
        : `<article class="b-ms" data-milestone="${m.id}"><div class="b-ms-main">${openBtn(m)}${passWhenHtml(m)}</div>${progressHtml(m)}<div class="a-actions">${editMilestoneBtn(m)}</div></article>`).join("");
      right = `${isForm("project", "edit", p.id) ? "" : `<header class="project-head"><h2>${esc(p.name)}</h2>${renameProjectBtn(p)}</header>`}
        ${isForm("milestone", "create", p.id) ? `<div class="b-inline">${formHtml()}</div>` : `<div>${newMilestoneBtn(p)}</div>`}
        <div class="b-rows">${rows || `<p class="empty-note">No milestones yet.</p>`}</div>`;
    }
    return `<div class="b-wrap">
      <nav class="b-left" aria-label="Projects"><ul class="b-list">${left}</ul>${newProject}</nav>
      <section class="b-right" data-project="${p.id}" aria-label="${esc(p.name)}">${right}</section>
    </div>`;
  }

  // ---------- variant C: cards ----------
  // Milestone cards in a grid under each project; a milestone and every form open in a side sheet.
  function cardC(m) {
    return `<article class="c-card" data-milestone="${m.id}">
      <h3 class="c-title">${openBtn(m)}</h3>
      ${passWhenHtml(m)}
      ${progressHtml(m)}
      <div class="c-foot">${editMilestoneBtn(m)}</div>
    </article>`;
  }
  function renderCards() {
    const grid = projects.map((p) => `<section class="c-project" data-project="${p.id}">
      <header class="project-head"><h2>${esc(p.name)} <span class="n">${plural(p.milestones.length, "milestone")}</span></h2>${renameProjectBtn(p)}</header>
      <div class="c-grid">${p.milestones.map(cardC).join("")}<button type="button" class="c-new" data-act="new-m" data-id="${p.id}" data-key="new-m:${p.id}">+ New milestone<span class="sr-only"> in ${esc(p.name)}</span></button></div>
    </section>`).join("");
    const found = state.open && findMilestone(state.open);
    let sheet = "";
    if (state.form) {
      sheet = formHtml();
    } else if (found) {
      const m = found.milestone;
      sheet = `<div data-milestone="${m.id}" class="c-sheet-body">
        <p class="muted crumb">${esc(found.project.name)}</p>
        <header class="project-head"><h2 class="ms-name" tabindex="-1" data-key="drill:${m.id}">${esc(m.name)}</h2>${editMilestoneBtn(m)}</header>
        ${passWhenHtml(m)}${progressHtml(m)}${issuesHtml(m)}
      </div>`;
    }
    return grid + (sheet ? `<div class="scrim" data-act="close-sheet"></div><aside class="side-sheet" role="dialog" aria-modal="true" aria-label="${state.form ? "Form" : "Milestone"}">
      <button type="button" class="btn ghost sheet-close" data-act="close-sheet" data-key="close-sheet">Close <kbd>Esc</kbd></button>${sheet}</aside>` : "");
  }

  // ---------- the list mock, for the entry point ----------
  function renderList() {
    const sample = ["PL-96", "PL-100", "PL-103", "PL-84", "PL-77", "PL-104", "PL-106", "PL-121"].map((id) => {
      for (const p of projects) for (const m of p.milestones) { const i = m.issues.find((x) => x.id === id); if (i) return { ...i, project: p, milestone: m }; }
    }).filter(Boolean);
    const chip = (i) => `<a class="ms-chip" href="?screen=projects&variant=${state.variant}&milestone=${i.milestone.id}" data-nav="milestone=${i.milestone.id}">${esc(i.project.name)} · ${esc(i.milestone.name)}</a>`;
    const body = state.listLayout === "list"
      ? `<ul class="l-rows">${sample.map((i) => `<li><span class="mono muted">${esc(i.id)}</span><span class="ititle">${esc(i.title)}</span>${statusHtml(i.status)}${chip(i)}</li>`).join("")}</ul>`
      : `<div class="l-board">${BAR_ORDER.slice().reverse().filter((s) => sample.some((i) => i.status === s)).map((s) => `<div class="l-col"><h3>${statusHtml(s)}</h3>${sample.filter((i) => i.status === s).map((i) => `<div class="l-card"><span class="mono muted">${esc(i.id)}</span><span class="ititle">${esc(i.title)}</span>${chip(i)}</div>`).join("")}</div>`).join("")}</div>`;
    return `<div class="screen-head"><h1 class="screen-title">Issues <span class="muted">list mock</span></h1>
      <div class="switcher" role="group" aria-label="Issue layout">${["list", "board"].map((l) => `<button type="button" data-act="list-layout" data-id="${l}" data-key="list-layout:${l}" aria-pressed="${state.listLayout === l}">${l === "list" ? "List" : "Board"}</button>`).join("")}</div></div>
      <p class="counting" data-list-hint>Press <kbd>g</kbd> then <kbd>m</kbd>, or pick <b>Projects</b> in the header, to open the projects screen in the ${esc(state.variant)} layout. A row's project and milestone chip opens its milestone there.</p>
      ${body}`;
  }

  const RENDER = { sections: renderSections, panes: renderPanes, cards: renderCards };

  function render() {
    document.querySelectorAll(".topbar .switcher button").forEach((b) => b.setAttribute("aria-pressed", String(b.dataset.variant === state.variant)));
    document.querySelectorAll(".appbar [data-screen]").forEach((a) => (a.dataset.screen === state.screen ? a.setAttribute("aria-current", "page") : a.removeAttribute("aria-current")));
    $view.className = `variant-${state.variant} screen-${state.screen}`;
    $view.innerHTML = state.screen === "list" ? renderList() : entryHtml() + screenHead() + RENDER[state.variant]();

    const dialog = $view.querySelector("dialog");
    if (dialog) {
      dialog.showModal();
      dialog.addEventListener("cancel", (e) => { e.preventDefault(); closeForm(); });
    }
    if (state.focus) {
      // A key can repeat (a card and its open sheet); the last one is the one in front.
      const el = state.focus.startsWith("#") ? document.querySelector(state.focus) : [...document.querySelectorAll(`[data-key="${CSS.escape(state.focus)}"]`)].pop();
      if (el) {
        el.focus();
        if (el.id === "f-name") el.setSelectionRange(el.value.length, el.value.length);
      }
      state.focus = null;
    }
  }

  function syncUrl() {
    const url = new URL(location.href);
    url.search = "";
    url.searchParams.set("variant", state.variant);
    if (state.screen === "list") url.searchParams.set("screen", "list");
    else if (state.open) url.searchParams.set("milestone", state.open);
    history.replaceState(null, "", url);
  }

  function update(changes) {
    Object.assign(state, changes);
    syncUrl();
    render();
  }

  function navigate(query) {
    const q = new URLSearchParams(query);
    state.screen = "projects";
    state.form = null;
    state.open = null;
    if (findProject(q.get("project"))) state.project = q.get("project");
    if (findMilestone(q.get("milestone"))) openMilestone(q.get("milestone"));
    syncUrl();
    render();
    scrollToTarget(q.get("milestone") ? `[data-milestone="${q.get("milestone")}"]` : `[data-project="${q.get("project")}"]`);
  }

  function scrollToTarget(selector) {
    const el = $view.querySelector(selector);
    if (el) el.scrollIntoView({ block: "start" });
    const focusable = $view.querySelector(`${selector} .ms-open, ${selector} .ms-name[tabindex]`);
    if (focusable && state.variant !== "cards") focusable.focus({ preventScroll: true });
    else if (state.variant === "cards") { const h = $view.querySelector(".side-sheet .ms-name"); if (h) h.focus(); }
  }

  // ---------- events ----------
  const ACTIONS = {
    "open-m": (id) => {
      if (state.variant === "sections" && state.open === id) update({ open: null, focus: `open-m:${id}` });
      else { openMilestone(id); update({ focus: state.variant === "sections" ? `open-m:${id}` : `drill:${id}` }); }
    },
    "close-m": (id) => update({ open: null, focus: `open-m:${id}` }),
    "close-sheet": () => (state.form ? closeForm() : update({ open: null, focus: state.open ? `open-m:${state.open}` : null })),
    "select-p": (id) => update({ project: id, open: null, form: null, focus: `select-p:${id}` }),
    "new-p": (_, key) => startForm({ kind: "project", mode: "create" }, key),
    "edit-p": (id, key) => startForm({ kind: "project", mode: "edit", id, projectId: id, name: findProject(id).name }, state.variant === "panes" ? `select-p:${id}` : key),
    "new-m": (id, key) => startForm({ kind: "milestone", mode: "create", projectId: id }, key),
    "edit-m": (id, key) => {
      const { project, milestone } = findMilestone(id);
      startForm({ kind: "milestone", mode: "edit", id, projectId: project.id, name: milestone.name, passWhen: milestone.passWhen }, key);
    },
    "cancel-form": () => closeForm(),
    "list-layout": (id) => update({ listLayout: id, focus: `list-layout:${id}` }),
  };

  document.addEventListener("click", (e) => {
    const sw = e.target.closest(".topbar .switcher button");
    if (sw) { update({ variant: sw.dataset.variant, form: null }); return; }
    const screen = e.target.closest(".appbar [data-screen]");
    if (screen) { e.preventDefault(); update({ screen: screen.dataset.screen, form: null }); return; }
    const nav = e.target.closest("[data-nav]");
    if (nav) { e.preventDefault(); navigate(nav.dataset.nav); return; }
    const act = e.target.closest("[data-act]");
    if (act && ACTIONS[act.dataset.act]) ACTIONS[act.dataset.act](act.dataset.id, act.dataset.key);
  });

  document.addEventListener("submit", (e) => {
    if (!e.target.matches("[data-form]")) return;
    e.preventDefault();
    submitForm();
  });

  document.addEventListener("input", (e) => {
    if (!state.form) return;
    if (e.target.id === "f-name") {
      state.form.name = e.target.value;
      const n = [...e.target.value.trim()].length;
      const count = document.getElementById("f-count");
      count.textContent = `${n}/${MAX_NAME_CHARS}`;
      count.className = n > MAX_NAME_CHARS ? "over" : "";
    }
    if (e.target.id === "f-pass") state.form.passWhen = e.target.value;
  });

  let gAt = 0;
  document.addEventListener("keydown", (e) => {
    if (e.metaKey || e.ctrlKey || e.altKey) return;
    if (e.key === "Escape") {
      if (state.form) { e.preventDefault(); closeForm(); }
      else if (state.open && state.screen === "projects") { e.preventDefault(); ACTIONS[state.variant === "cards" ? "close-sheet" : "close-m"](state.open); }
      return;
    }
    if (["INPUT", "SELECT", "TEXTAREA"].includes(document.activeElement.tagName)) return;
    if (e.key === "g") { gAt = Date.now(); return; }
    if (Date.now() - gAt < 1500 && (e.key === "m" || e.key === "i")) {
      gAt = 0;
      update({ screen: e.key === "m" ? "projects" : "list", form: null, focus: null });
      return;
    }
    const n = Number(e.key);
    if (n >= 1 && n <= VARIANTS.length) update({ variant: VARIANTS[n - 1], form: null });
  });

  syncUrl();
  render();
  if (state.open) scrollToTarget(`[data-milestone="${state.open}"]`);
})();
