// Pull requests prototype: three layouts over the same mock pull requests. Nothing here is wired to production code.
(function () {
  const { repo, statuses, mergeStates, pullRequests } = window.PUNCHLIST_PR_MOCK;
  const VARIANTS = ["page", "tab", "queue"];
  const state = { variant: "page" };

  const $view = document.getElementById("view");
  const $toast = document.getElementById("toast");

  const params = new URLSearchParams(location.search);
  if (VARIANTS.includes(params.get("variant"))) state.variant = params.get("variant");

  // ---------- helpers ----------
  const esc = (s) => String(s).replace(/[&<>"']/g, (c) => ({ "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;", "'": "&#39;" }[c]));
  const ICON = {
    pr: '<svg viewBox="0 0 16 16" fill="currentColor" aria-hidden="true"><path d="M4.75 1.5a2.25 2.25 0 0 1 .75 4.372v4.256a2.25 2.25 0 1 1-1.5 0V5.872A2.25 2.25 0 0 1 4.75 1.5Zm0 1.5a.75.75 0 1 0 0 1.5.75.75 0 0 0 0-1.5Zm0 8.5a.75.75 0 1 0 0 1.5.75.75 0 0 0 0-1.5ZM11.25 4.5a.75.75 0 0 1 .75.75v4.878a2.25 2.25 0 1 1-1.5 0V5.25a.75.75 0 0 1 .75-.75Zm0 7a.75.75 0 1 0 0 1.5.75.75 0 0 0 0-1.5Z"/></svg>',
    ext: '<svg viewBox="0 0 16 16" fill="none" stroke="currentColor" stroke-width="1.6" aria-hidden="true"><path d="M9 3h4v4M13 3 7 9M11 9.5V13H3V5h3.5"/></svg>',
    merge: '<svg viewBox="0 0 16 16" fill="currentColor" aria-hidden="true"><path d="M5.45 5.154A4.25 4.25 0 0 0 9.25 7.5h1.378a2.251 2.251 0 1 1 0 1.5H9.25A5.734 5.734 0 0 1 5 7.123v3.505a2.25 2.25 0 1 1-1.5 0V5.372a2.25 2.25 0 1 1 1.95-.218ZM4.25 13.5a.75.75 0 1 0 0-1.5.75.75 0 0 0 0 1.5Zm8.5-4.5a.75.75 0 1 0 0-1.5.75.75 0 0 0 0 1.5ZM5 3.25a.75.75 0 1 0-1.5 0 .75.75 0 0 0 1.5 0Z"/></svg>',
  };

  const waitsOnMerge = (pr) => !!pr.issue && pr.issue.status === "merging";
  const byNumber = (n) => pullRequests.find((p) => p.number === n);

  // The one link out to GitHub. Every variant and every entry point uses it.
  const ghLink = (pr, text = "Open on GitHub") =>
    `<a class="gh" href="${esc(pr.url)}" target="_blank" rel="noopener noreferrer" data-gh aria-label="${esc(text)}: pull request #${pr.number}">${esc(text)}${ICON.ext}</a>`;

  const statusHtml = (id) => `<span class="status" data-status="${id}"><span class="status-dot" aria-hidden="true"></span>${esc(statuses[id].label)}</span>`;
  const issueHtml = (pr) => pr.issue
    ? `<span class="issue" data-field="issue"><span class="mono">${esc(pr.issue.id)}</span> ${statusHtml(pr.issue.status)}</span>`
    : `<span class="issue unlinked" data-field="issue" data-unlinked>names no issue</span>`;
  const checksHtml = (pr) => {
    const { passed, total } = pr.checks;
    const tone = passed === total ? "ok" : pr.draft ? "muted" : "bad";
    return `<span class="fact ${tone}" data-field="checks" title="Checks passed of total">checks ${passed}/${total}</span>`;
  };
  const threadsHtml = (pr) => `<span class="fact ${pr.threads ? "warn" : "muted"}" data-field="threads">${pr.threads} open thread${pr.threads === 1 ? "" : "s"}</span>`;
  const mergeHtml = (pr) => { const m = mergeStates[pr.merge]; return `<span class="merge ${m.tone}" data-field="merge">${esc(m.label)}</span>`; };
  const branchHtml = (pr) => `<span class="branch mono" data-field="branch" title="${esc(pr.branch)} → ${esc(repo.defaultBranch)}">${esc(pr.branch)}</span>`;
  const numberHtml = (pr) => `<span class="num mono" data-field="number">#${pr.number}</span>`;
  const titleHtml = (pr) => `<span class="title" data-field="title">${esc(pr.title)}</span>`;

  // An issue in Merging waits on a merge on GitHub: Merging → Done moves on `pr_merged`, not on anything in Punchlist.
  const waitHtml = (pr) => waitsOnMerge(pr)
    ? `<div class="wait" data-waits-on-merge>${ICON.merge}<span><b>Waits on a merge on GitHub.</b> ${esc(pr.issue.id)} moves to Done when GitHub reports the merge.${pr.merge === "dirty" ? " Resolve the conflicts first." : ""}</span>${ghLink(pr, "Merge on GitHub")}</div>`
    : "";

  // A row opens its issue with Enter; one that names no issue opens the pull request on GitHub.
  const rowAttrs = (pr) => `tabindex="0" data-pr="${pr.number}" aria-label="Pull request #${pr.number}: ${esc(pr.title)}${pr.issue ? `, ${esc(pr.issue.id)}` : ", names no issue"}"`;

  function toast(text) {
    $toast.textContent = text;
    $toast.hidden = false;
    clearTimeout(toast.t);
    toast.t = setTimeout(() => { $toast.hidden = true; }, 2200);
  }

  // ---------- entry points ----------
  // Mock fragments of the list, the board, an issue row and the issue page, showing how each reaches this view or GitHub.
  const sample = byNumber(241);
  const ENTRY = {
    page: {
      list: `<div class="mock-bar"><nav class="mock-nav"><span>Issues</span><span class="here">Pull requests <b class="count">${pullRequests.length}</b></span></nav><span class="switcher-mock"><span class="on">List</span><span>Board</span></span><kbd>g</kbd> <kbd>p</kbd></div>`,
      board: `<div class="mock-bar"><nav class="mock-nav"><span>Issues</span><span class="here">Pull requests <b class="count">${pullRequests.length}</b></span></nav><span class="switcher-mock"><span>List</span><span class="on">Board</span></span></div>`,
      listNote: "A Pull requests item next to Issues in the app header, the same on the list and the board. <kbd>g</kbd> <kbd>p</kbd> opens it from anywhere.",
      boardNote: "Same header item on the board.",
      row: `the pull request chip on a row is a link to GitHub: <a class="chip-link" href="${esc(sample.url)}" target="_blank" rel="noopener noreferrer" data-gh aria-label="Open on GitHub: pull request #${sample.number}">${ICON.pr}#${sample.number}${ICON.ext}</a>`,
      page: `the issue page's pull request section has ${ghLink(sample)} in its header.`,
    },
    tab: {
      list: `<div class="mock-bar"><span class="switcher-mock"><span><kbd>1</kbd> List</span><span><kbd>2</kbd> Board</span><span class="on"><kbd>3</kbd> Pull requests</span></span></div>`,
      board: `<div class="mock-bar"><span class="switcher-mock"><span><kbd>1</kbd> List</span><span class="on"><kbd>2</kbd> Board</span><span><kbd>3</kbd> Pull requests</span></span></div>`,
      listNote: "A third layout in the list's switcher, next to List and Board. <kbd>3</kbd> switches to it; filters carry over.",
      boardNote: "The board's switcher has the same third layout.",
      row: `a row's pull request chip keeps opening the issue; a small GitHub link sits after it: <span class="pr-chip">${ICON.pr}#${sample.number}</span> ${ghLink(sample, "GitHub")}`,
      page: `the issue page's pull request section says what it waits on: ${waitHtml(sample)}`,
    },
    queue: {
      list: `<div class="mock-bar"><span class="switcher-mock"><span class="on">List</span><span>Board</span></span><span class="queue-btn">${ICON.pr} Pull requests <b class="count">${pullRequests.length}</b> <b class="count attention">${pullRequests.filter(waitsOnMerge).length} to merge</b></span></div>`,
      board: `<div class="mock-col"><span class="status" data-status="merging"><span class="status-dot" aria-hidden="true"></span>Merging</span> <span class="muted">2</span> <span class="queue-link">${pullRequests.filter(waitsOnMerge).length} wait on GitHub →</span></div>`,
      listNote: "A Pull requests button in the list's top bar opens this queue as a panel over the list, with a count of merges waiting.",
      boardNote: "On the board, the Merging column's header says how many wait on GitHub and opens the queue.",
      row: `a row in Merging carries ${ghLink(sample, "Merge on GitHub")} in place of the gate summary.`,
      page: `the issue page's pull request section has ${ghLink(sample)} next to the title, and the Merging notice repeats it.`,
    },
  };

  function entryHtml(v) {
    const e = ENTRY[v];
    return `<details class="entry" open data-entry>
      <summary>How a person gets here, and out to GitHub</summary>
      <div class="entry-grid">
        <div class="entry-item" data-entry-from="list"><h3>From the list</h3>${e.list}<div class="note">${e.listNote}</div></div>
        <div class="entry-item" data-entry-from="board"><h3>From the board</h3>${e.board}<div class="note">${e.boardNote}</div></div>
        <div class="entry-item" data-entry-from="row"><h3>Issue row</h3><div class="note">On the list and the board, ${e.row}</div></div>
        <div class="entry-item" data-entry-from="issue-page"><h3>Issue page</h3><div class="note">On ${esc(sample.issue.id)}, ${e.page}</div></div>
      </div>
    </details>`;
  }

  // ---------- variant A: page ----------
  // A screen of its own: one table, sections ordered by what a person has to do.
  function rowA(pr) {
    return `<div class="a-row" ${rowAttrs(pr)}>
      ${numberHtml(pr)}
      <span class="a-title">${titleHtml(pr)}${branchHtml(pr)}</span>
      ${issueHtml(pr)}
      <span class="a-facts">${checksHtml(pr)}${threadsHtml(pr)}</span>
      ${mergeHtml(pr)}
      ${ghLink(pr)}
      ${waitHtml(pr)}
    </div>`;
  }
  function renderPage() {
    const sections = [
      { title: "Waits on a merge on GitHub", note: "Issues in Merging. They move to Done when GitHub reports the merge.", prs: pullRequests.filter(waitsOnMerge) },
      { title: "In review", prs: pullRequests.filter((p) => p.issue && !p.draft && !waitsOnMerge(p)) },
      { title: "Draft", prs: pullRequests.filter((p) => p.issue && p.draft) },
      { title: "Names no issue", note: "As in <code>pl pr unlinked</code>: no issue id in the title, body or branch.", prs: pullRequests.filter((p) => !p.issue) },
    ];
    return `<h1 class="screen-title">Pull requests <span class="muted">${esc(repo.owner)}/${esc(repo.name)} · ${pullRequests.length} open</span></h1>
      ${sections.filter((s) => s.prs.length).map((s) => `<section class="a-section">
        <h2>${esc(s.title)} <span class="n">${s.prs.length}</span></h2>${s.note ? `<p class="section-note">${s.note}</p>` : ""}
        <div class="a-rows">${s.prs.map(rowA).join("")}</div>
      </section>`).join("")}`;
  }

  // ---------- variant B: a third layout of the issue list ----------
  // Grouped by the issue's status like the list, closest to Done first, with unlinked pull requests last.
  function rowB(pr) {
    return `<div class="b-row" ${rowAttrs(pr)}>
      <span class="b-main">${numberHtml(pr)} ${titleHtml(pr)}</span>
      <span class="b-meta">${issueHtml(pr)}${branchHtml(pr)}${checksHtml(pr)}${threadsHtml(pr)}${mergeHtml(pr)}</span>
      ${ghLink(pr)}
      ${waitHtml(pr)}
    </div>`;
  }
  function renderTab() {
    const ids = Object.keys(statuses).sort((a, b) => statuses[b].order - statuses[a].order);
    const groups = ids.map((id) => ({ head: statusHtml(id), prs: pullRequests.filter((p) => p.issue && p.issue.status === id) }));
    groups.push({ head: `<span class="unlinked">Names no issue</span>`, prs: pullRequests.filter((p) => !p.issue) });
    const waiting = pullRequests.filter(waitsOnMerge).length;
    return `${waiting ? `<div class="banner" data-waits-on-merge>${ICON.merge}<span><b>${waiting} issues in Merging wait on a merge on GitHub.</b> Punchlist moves them to Done when GitHub reports each merge.</span></div>` : ""}
      ${groups.filter((g) => g.prs.length).map((g) => `<section class="b-group">
        <h2 class="group-head">${g.head} <span class="n">${g.prs.length}</span></h2>
        <div class="b-rows">${g.prs.map(rowB).join("")}</div>
      </section>`).join("")}`;
  }

  // ---------- variant C: merge queue ----------
  // Lanes by what happens next, opened as a panel over the list or board.
  function cardC(pr) {
    return `<article class="c-card${waitsOnMerge(pr) ? " waiting" : ""}" ${rowAttrs(pr)}>
      <div class="c-head">${numberHtml(pr)}${mergeHtml(pr)}</div>
      ${titleHtml(pr)}
      <div class="c-meta">${issueHtml(pr)}${branchHtml(pr)}</div>
      <div class="c-meta">${checksHtml(pr)}${threadsHtml(pr)}</div>
      ${waitHtml(pr)}
      <div class="c-foot">${ghLink(pr)}</div>
    </article>`;
  }
  function renderQueue() {
    const lanes = [
      { title: "Merge on GitHub", note: "In Merging; Done follows the merge", prs: pullRequests.filter(waitsOnMerge) },
      { title: "In review", note: "Agent Review and Human Review", prs: pullRequests.filter((p) => p.issue && !p.draft && !waitsOnMerge(p)) },
      { title: "In progress", note: "Draft pull requests", prs: pullRequests.filter((p) => p.issue && p.draft) },
      { title: "Names no issue", note: "Not tracked by any issue", prs: pullRequests.filter((p) => !p.issue) },
    ];
    return `<div class="c-panel">
      <div class="c-panel-head"><h1 class="screen-title">Pull requests</h1><span class="muted">${esc(repo.owner)}/${esc(repo.name)} · panel over the list · <kbd>Esc</kbd> closes</span></div>
      <div class="c-lanes">${lanes.map((l) => `<section class="c-lane">
        <h2>${esc(l.title)} <span class="n">${l.prs.length}</span></h2><p class="section-note">${esc(l.note)}</p>
        ${l.prs.map(cardC).join("")}
      </section>`).join("")}</div>
    </div>`;
  }

  const RENDER = { page: renderPage, tab: renderTab, queue: renderQueue };

  function render() {
    document.querySelectorAll(".switcher button").forEach((b) => b.setAttribute("aria-pressed", String(b.dataset.variant === state.variant)));
    $view.className = `variant-${state.variant}`;
    $view.innerHTML = entryHtml(state.variant) + RENDER[state.variant]();
  }

  function setVariant(v) {
    state.variant = v;
    const url = new URL(location.href);
    url.searchParams.set("variant", v);
    history.replaceState(null, "", url);
    render();
  }

  // ---------- events ----------
  document.addEventListener("click", (e) => {
    const sw = e.target.closest(".switcher button");
    if (sw) setVariant(sw.dataset.variant);
  });

  document.addEventListener("keydown", (e) => {
    if (e.metaKey || e.ctrlKey || e.altKey) return;
    const row = e.target.closest && e.target.closest("[data-pr]");
    if (e.key === "Enter" && row && e.target === row) {
      const pr = byNumber(Number(row.dataset.pr));
      if (pr.issue) toast(`Opens ${pr.issue.id}'s issue page (/issues/${pr.issue.id})`);
      else window.open(pr.url, "_blank", "noopener");
      e.preventDefault();
      return;
    }
    if (["INPUT", "SELECT", "TEXTAREA"].includes(document.activeElement.tagName)) return;
    const n = Number(e.key);
    if (n >= 1 && n <= VARIANTS.length) setVariant(VARIANTS[n - 1]);
  });

  render();
})();
