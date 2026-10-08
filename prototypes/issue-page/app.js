// Issue page prototype: three layouts over the same mock issues. Nothing here is wired to production code.
(function () {
  const { statuses, gates, actors, issues, liveLogMore } = window.PUNCHLIST_PAGE_MOCK;
  const statusById = Object.fromEntries(statuses.map((s) => [s.id, s]));
  const VARIANTS = ["sidebar", "tabs", "pipeline"];
  const STATES = issues.map((i) => i.key);

  const state = { variant: "sidebar", issue: "running", viewer: "gannon", tab: "overview", feed: "all", liveIndex: 0 };

  const $view = document.getElementById("view");
  const $shortcuts = document.getElementById("shortcuts");
  const $overlay = document.getElementById("overlay");
  const $viewer = document.getElementById("viewer");

  const params = new URLSearchParams(location.search);
  if (VARIANTS.includes(params.get("variant"))) state.variant = params.get("variant");
  if (STATES.includes(params.get("state"))) state.issue = params.get("state");
  if (actors[params.get("as")]) state.viewer = params.get("as");
  $viewer.value = state.viewer;

  // ---------- helpers ----------
  const esc = (s) => String(s).replace(/[&<>"']/g, (c) => ({ "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;", "'": "&#39;" }[c]));
  const ICON = {
    lock: '<svg viewBox="0 0 16 16" fill="currentColor" aria-hidden="true"><path d="M4 7V5a4 4 0 1 1 8 0v2h1a1 1 0 0 1 1 1v6a1 1 0 0 1-1 1H3a1 1 0 0 1-1-1V8a1 1 0 0 1 1-1h1Zm2 0h4V5a2 2 0 1 0-4 0v2Z"/></svg>',
    bot: '<svg viewBox="0 0 16 16" fill="none" stroke="currentColor" stroke-width="1.6" aria-hidden="true"><rect x="2.5" y="5" width="11" height="8" rx="2"/><path d="M8 2.5V5M5.5 8.5h.01M10.5 8.5h.01M6 11h4"/></svg>',
    runner: '<svg viewBox="0 0 16 16" fill="none" stroke="currentColor" stroke-width="1.6" aria-hidden="true"><rect x="2" y="3" width="12" height="4" rx="1"/><rect x="2" y="9" width="12" height="4" rx="1"/><path d="M4.5 5h.01M4.5 11h.01"/></svg>',
    github: '<svg viewBox="0 0 16 16" fill="currentColor" aria-hidden="true"><path d="M8 1a7 7 0 0 0-2.21 13.64c.35.06.48-.15.48-.34v-1.2c-1.95.42-2.36-.94-2.36-.94-.32-.81-.78-1.03-.78-1.03-.64-.44.05-.43.05-.43.7.05 1.07.72 1.07.72.63 1.07 1.65.76 2.05.58.06-.46.25-.76.45-.94-1.56-.18-3.2-.78-3.2-3.46 0-.76.27-1.39.72-1.88-.07-.18-.31-.89.07-1.85 0 0 .59-.19 1.93.72a6.7 6.7 0 0 1 3.5 0c1.34-.91 1.93-.72 1.93-.72.38.96.14 1.67.07 1.85.45.49.72 1.12.72 1.88 0 2.69-1.64 3.28-3.2 3.45.25.22.48.65.48 1.3v1.93c0 .19.13.41.48.34A7 7 0 0 0 8 1Z"/></svg>',
    server: '<svg viewBox="0 0 16 16" fill="none" stroke="currentColor" stroke-width="1.6" aria-hidden="true"><path d="M3 8h10M8 3v10M4.5 4.5l7 7M11.5 4.5l-7 7"/></svg>',
    pr: '<svg viewBox="0 0 16 16" fill="currentColor" aria-hidden="true"><path d="M4.75 1.5a2.25 2.25 0 0 1 .75 4.372v4.256a2.25 2.25 0 1 1-1.5 0V5.872A2.25 2.25 0 0 1 4.75 1.5Zm0 1.5a.75.75 0 1 0 0 1.5.75.75 0 0 0 0-1.5Zm0 8.5a.75.75 0 1 0 0 1.5.75.75 0 0 0 0-1.5ZM9.47 2.22a.75.75 0 0 1 1.06 0l1.72 1.72V4A2 2 0 0 1 12 4v6.128a2.25 2.25 0 1 1-1.5 0V5.56l-1.03 1.03a.75.75 0 0 1-1.06-1.06l2-2 .03-.03-.03-.03-2-2a.75.75 0 0 1 0-1.06l1.06 1.06Zm1.78 9.28a.75.75 0 1 0 0 1.5.75.75 0 0 0 0-1.5Z"/></svg>',
    merge: '<svg viewBox="0 0 16 16" fill="currentColor" aria-hidden="true"><path d="M5.45 5.154A4.25 4.25 0 0 0 9.25 7.5h1.378a2.251 2.251 0 1 1 0 1.5H9.25A5.734 5.734 0 0 1 5 7.123v3.505a2.25 2.25 0 1 1-1.5 0V5.372a2.25 2.25 0 1 1 1.95-.218ZM4.25 13.5a.75.75 0 1 0 0-1.5.75.75 0 0 0 0 1.5Zm8.5-4.5a.75.75 0 1 0 0-1.5.75.75 0 0 0 0 1.5ZM5 3.25a.75.75 0 1 0-1.5 0 .75.75 0 0 0 1.5 0Z"/></svg>',
  };
  const TYPE_LABEL = { person: "person", agent: "agent", runner: "runner", github: "GitHub", server: "server" };

  const current = () => issues.find((i) => i.key === state.issue);
  const viewer = () => actors[state.viewer];
  const isLocked = (statusId, actorType) => { const s = statusById[statusId]; return !!(s.lock && s.lock.includes(actorType)); };

  function avatarHtml(a) {
    if (a.type === "person") return `<span class="avatar person" aria-hidden="true">${esc(a.initials)}</span>`;
    if (a.type === "agent") return `<span class="avatar agent" aria-hidden="true">${ICON.bot}</span>`;
    if (a.type === "runner") return `<span class="avatar runner" aria-hidden="true">${ICON.runner}</span>`;
    if (a.type === "github") return `<span class="avatar github" aria-hidden="true">${ICON.github}</span>`;
    return `<span class="avatar server" aria-hidden="true">${ICON.server}</span>`;
  }
  // Actor chip: avatar, name, and a type mark so person / agent / runner / GitHub read differently.
  function actorHtml(id, opts = {}) {
    const a = actors[id];
    const type = a.type === "agent" ? `<span class="actor-type agent">${esc(a.name)}</span>` : a.type === "person" ? "" : `<span class="actor-type ${a.type}">${esc(TYPE_LABEL[a.type])}</span>`;
    const name = a.type === "agent" ? (opts.model ? `<span class="muted">${esc(a.model)}</span>` : "") : `<span class="name">${esc(a.name)}</span>`;
    return `<span class="assignee ${a.type}" data-actor-type="${a.type}" title="${esc(TYPE_LABEL[a.type])}: ${esc(a.name)}">${avatarHtml(a)}${name}${type}</span>`;
  }

  const statusDot = (id) => `<span class="status-dot" data-status="${id}" aria-hidden="true"></span>`;
  const statusPill = (id) => `<span class="status-pill" data-status="${id}">${statusDot(id)}${esc(statusById[id].label)}</span>`;
  const lockBadge = (s) => (s.lock ? `<span class="lock" title="Locked to people: ${esc(s.lock.join(" and "))} cannot act here">${ICON.lock} people only</span>` : "");

  function prChip(i) {
    const p = i.pr;
    if (!p) return `<span class="pr none">no PR</span>`;
    const icon = p.state === "merged" ? ICON.merge : ICON.pr;
    return `<a class="pr ${p.state}" href="#pr" title="PR #${p.number} ${p.state}, checks ${p.checks}">${icon}#${p.number} ${esc(p.state)} <i class="checks ${p.checks}" aria-label="checks ${p.checks}"></i></a>`;
  }

  // ---------- transitions ----------
  // What the viewer may do from the current status, and why anything is blocked.
  function transitionsFor(i) {
    const s = statusById[i.status];
    const v = viewer();
    const out = [];
    if (!s.next) return out;
    const lockedHere = isLocked(i.status, v.type);

    const forward = { to: s.next.to, by: s.next.by, event: s.next.event, gates: s.next.gates };
    const canRole = forward.by === "event" ? false : forward.by === "runner" ? v.type === "runner" : forward.by === "person" ? v.type === "person" : true;
    const fails = forward.gates.filter((g) => !(i.gates[g] && i.gates[g].pass));
    let verdict;
    if (lockedHere) verdict = { kind: "locked", text: `${esc(statusById[i.status].label)} is locked to people. ${esc(v.name)} cannot act here.` };
    else if (forward.by === "event") verdict = { kind: "event", text: `Moves on its own when GitHub reports ${esc(forward.event.replace(/_/g, " "))}.` };
    else if (isLocked(forward.to, v.type)) verdict = { kind: "locked", text: `${esc(statusById[forward.to].label)} is locked to people. ${esc(v.name)} cannot enter it.` };
    else if (!canRole) verdict = { kind: "locked", text: `Only a ${esc(forward.by)} can make this move.` };
    else if (fails.length) verdict = { kind: "blocked", gate: fails[0], text: `${esc(gates[fails[0]].label)}`, why: i.gates[fails[0]].reason, more: fails.length - 1 };
    else verdict = { kind: "ok", text: forward.gates.length ? `All ${forward.gates.length} gates pass.` : "No gates on this move." };
    out.push({ ...forward, verdict });

    // Side moves a person can always make; agents only outside locked statuses.
    if (!["done", "canceled"].includes(i.status)) {
      const sideVerdict = lockedHere ? { kind: "locked", text: `${esc(v.name)} cannot act in ${esc(s.label)}.` } : v.type === "person" ? { kind: "ok", text: "No gates on this move." } : { kind: "locked", text: "Only a person can make this move." };
      if (["agent_review", "human_review", "merging"].includes(i.status)) out.push({ to: "in_progress", by: "person", gates: [], verdict: sideVerdict, back: true });
      out.push({ to: "canceled", by: "person", gates: [], verdict: sideVerdict });
    }
    return out;
  }

  function transitionHtml(t, i) {
    const vd = t.verdict;
    const cls = vd.kind === "ok" ? "ok" : vd.kind === "blocked" ? "blocked" : vd.kind === "event" ? "event" : "locked";
    const mark = vd.kind === "ok" ? "✓" : vd.kind === "blocked" ? "✕" : vd.kind === "event" ? "↻" : ICON.lock;
    const by = t.by === "event" ? `on ${esc(t.event.replace(/_/g, " "))}` : `by ${esc(t.by)}`;
    const body = vd.kind === "blocked"
      ? `<b data-blocked-gate="${t.verdict.gate}">${vd.text}</b><span class="gate-why"> · ${esc(vd.why)}${vd.more ? ` · +${vd.more} more failing` : ""}</span>`
      : vd.text;
    const disabled = vd.kind !== "ok";
    return `<button type="button" class="transition ${cls}" data-transition="${t.to}" data-verdict="${vd.kind}" aria-disabled="${disabled}" ${disabled ? "" : `data-move="${t.to}"`}>
      <span class="arrow">${t.back ? "← back to" : "→"}</span>
      <span class="to">${statusDot(t.to)}<span class="lbl">${esc(statusById[t.to].label)}</span>${lockBadge(statusById[t.to])}</span>
      <span class="by">${by}</span>
      <span class="verdict"><span class="mark">${mark}</span><span>${body}</span></span>
    </button>`;
  }

  function transitionsPanelHtml(i) {
    const ts = transitionsFor(i);
    const v = viewer();
    const body = ts.length ? ts.map((t) => transitionHtml(t, i)).join("") : `<div class="none">No further transitions. ${esc(statusById[i.status].label)} is terminal.</div>`;
    return `<section class="panel" id="transitions" data-section="transitions">
      <h2>Status <span class="right">${statusPill(i.status)}</span></h2>
      <div class="muted" style="font-size: var(--text-caption); margin-bottom: 8px">Transitions for ${esc(v.name)} (${esc(TYPE_LABEL[v.type])})</div>
      <div class="transitions">${body}</div>
    </section>`;
  }

  // ---------- notices ----------
  function noticeHtml(i) {
    const s = statusById[i.status];
    const v = viewer();
    if (s.lock && s.lock.includes(v.type)) {
      return `<div class="notice agent-locked" data-notice="agent-locked" role="status">${ICON.lock}<div><strong>Agents cannot act on this issue.</strong> ${esc(s.label)} is locked to people. Viewing as ${esc(v.name)} (${esc(TYPE_LABEL[v.type])}): transitions, comments and runs are disabled until a person moves the issue.</div></div>`;
    }
    if (s.lock) {
      return `<div class="notice lock-notice" data-notice="locked" role="status">${ICON.lock}<div><strong>Waiting on a person.</strong> ${esc(s.label)} is locked: ${esc(s.lock.join(" and "))} cannot act here. No runs dispatch until the issue moves.</div></div>`;
    }
    if (i.status === "done") {
      return `<div class="notice done-notice" data-notice="done" role="status">${ICON.merge}<div><strong>Done.</strong> PR #${i.pr.number} merged ${esc(i.pr.mergedAt)}. Verify follows: confirm the acceptance criteria landed and record the result below.</div></div>`;
    }
    return "";
  }

  // ---------- gates ----------
  function gateRows(set, i) {
    return Object.keys(set).map((id) => {
      const g = gates[id]; const r = set[id];
      return `<div class="gate-row ${r.pass ? "pass" : "fail"}" data-gate="${id}" data-pass="${r.pass}">
        <span class="mark">${r.pass ? "✓" : "✕"}</span><span class="name">${esc(g.label)}</span>
        <span class="why">${esc(r.reason)}</span>
        <span class="ev">reads ${esc(g.reads)} · <a href="#${g.evidence}">evidence</a></span>
      </div>`;
    }).join("");
  }
  function gatesPanelHtml(i) {
    const s = statusById[i.status];
    const next = s.next && s.next.gates.length ? s.next : null;
    let title, rows, foot = "";
    if (next) {
      const pass = next.gates.filter((g) => i.gates[g] && i.gates[g].pass).length;
      title = `Gates to enter ${esc(statusById[next.to].label)} <span class="n">${pass} of ${next.gates.length} pass</span>`;
      rows = gateRows(i.gates, i);
      foot = `<div class="gate-foot"><span class="progress" aria-hidden="true"><b style="width:${(pass / next.gates.length) * 100}%"></b></span><span class="gates ${pass === next.gates.length ? "all" : ""}">${pass} of ${next.gates.length} gates pass</span></div>`;
    } else if (i.lastGates) {
      const n = Object.keys(i.lastGates).length;
      title = `Gates passed to enter ${esc(i.status === "done" ? "Human Review" : s.label)} <span class="n">${n} of ${n}</span>`;
      rows = gateRows(i.lastGates, i);
    } else {
      title = "Gates";
      rows = `<div class="muted" style="font-size: var(--text-meta)">No gates on the next move.</div>`;
    }
    return `<section class="panel" id="gates" data-section="gates"><h2>${title}</h2><div class="gate-rows">${rows}</div>${foot}</section>`;
  }

  // ---------- runs ----------
  function logHtml(run) {
    const lines = run.log.map((l) => {
      const cls = /refused|failed|exit 1/.test(l) ? "bad" : /passed|accepted|ok\b/.test(l) ? "ok" : "";
      return cls ? `<span class="${cls}">${esc(l)}</span>` : esc(l);
    }).join("\n");
    return `<pre class="log ${run.live ? "live" : ""}" ${run.live ? 'id="live-log" aria-live="polite"' : ""}>${lines}</pre>`;
  }
  function runHtml(run) {
    const a = actors[run.agent];
    return `<article class="run-card" data-run="${run.id}" data-outcome="${run.outcome}">
      <div class="top">
        <span class="id">${esc(run.id)}</span><span>attempt ${run.attempt}</span>
        ${actorHtml(run.agent, { model: true })}
        <span>on ${actorHtml(run.runner)}</span>
        <span class="outcome ${run.outcome}"><i aria-hidden="true"></i>${esc(run.outcome)}</span>
        ${run.live ? `<span class="live-tag"><i aria-hidden="true"></i>streaming</span>` : ""}
      </div>
      <div class="facts"><span><b>started</b> ${esc(run.started)}</span><span><b>duration</b> ${esc(run.duration)}</span><span><b>tokens</b> ${esc(run.tokens)}</span><span><b>cost</b> ${esc(run.cost)}</span><span><b>model</b> <span class="mono">${esc(a.model)}</span></span></div>
      ${logHtml(run)}
    </article>`;
  }
  function runsPanelHtml(i) {
    const s = statusById[i.status];
    const dispatch = s.dispatch && !s.lock ? `<span class="right muted">dispatches ${esc(s.dispatch)}</span>` : s.lock ? `<span class="right"><span class="lock">${ICON.lock} no dispatch</span></span>` : "";
    return `<section class="panel" id="runs" data-section="runs"><h2>Runs <span class="n">${i.runs.length}</span>${dispatch}</h2>${i.runs.slice().reverse().map(runHtml).join("")}</section>`;
  }

  // ---------- pull request ----------
  function prPanelHtml(i) {
    const p = i.pr;
    if (!p) return `<section class="panel" id="pr" data-section="pr"><h2>Pull request</h2><div class="proof-empty">No pull request yet.</div></section>`;
    const checks = p.checkRuns.map((c) => `<tr><td>${esc(c.name)}${c.detail ? `<div class="detail">${esc(c.detail)}</div>` : ""}</td><td><span class="st ${c.state}">${esc(c.state)}</span></td><td>${esc(c.took)}</td></tr>`).join("");
    const open = p.threads.filter((t) => t.state === "open").length;
    const threads = p.threads.length ? p.threads.map((t) => `<div class="thread ${t.state}" data-thread-state="${t.state}">
        <div class="meta"><span class="st">${t.state}</span><span>${esc(t.by)}</span><span class="path">${esc(t.path)}</span></div>
        <div>${esc(t.text)}</div>
      </div>`).join("") : `<div class="muted" style="font-size: var(--text-meta)">No review threads.</div>`;
    return `<section class="panel" id="pr" data-section="pr">
      <h2>Pull request <span class="right">${prChip(i)}</span></h2>
      <div class="pr-card">
        <div class="line"><span class="title">#${p.number} ${esc(p.title)}</span></div>
        <div class="line"><span class="branch">${esc(p.branch)}</span><span>names ${esc(i.id)}: ${p.namesIssue ? "yes" : "no"}</span><span>${p.mergeable ? "mergeable" : "conflicts"}</span>${p.mergedAt ? `<span>merged ${esc(p.mergedAt)}</span>` : ""}</div>
        <div id="checks"><div class="subhead">Checks · ${p.checks}</div><table class="checks">${checks}</table></div>
        <div id="threads"><div class="subhead">Review threads · ${open} open of ${p.threads.length}</div><div class="threads">${threads}</div></div>
      </div>
    </section>`;
  }

  // ---------- proof ----------
  function proofHtml(pf) {
    if (pf.kind === "checks") {
      return `<article class="proof checks" data-proof="checks"><div class="cap"><span class="t">${esc(pf.title)}</span><span class="m">${esc(pf.meta)}</span></div><table>${pf.rows.map(([n, r]) => `<tr><td>${esc(n)}</td><td>${esc(r)}</td></tr>`).join("")}</table></article>`;
    }
    if (pf.kind === "video") {
      return `<article class="proof" data-proof="video"><div class="thumb video"><span class="play" aria-label="play"></span><span class="dur">${esc(pf.meta.split(" · ")[0])}</span></div><div class="cap"><span class="t">${esc(pf.title)}</span><span class="m">video · ${esc(pf.meta)}</span></div></article>`;
    }
    return `<article class="proof" data-proof="image"><div class="thumb ${pf.tone}"><span class="frame" aria-hidden="true"></span></div><div class="cap"><span class="t">${esc(pf.title)}</span><span class="m">screenshot · ${esc(pf.meta)}</span></div></article>`;
  }
  function proofPanelHtml(i) {
    const body = i.proof.length ? `<div class="proof-grid">${i.proof.map(proofHtml).join("")}</div>` : `<div class="proof-empty" data-proof-empty>No proof yet. The run attaches screenshots, video and check results before it requests Agent Review.</div>`;
    return `<section class="panel" id="proof" data-section="proof"><h2>Proof <span class="n">${i.proof.length}</span></h2>${body}</section>`;
  }

  // ---------- spec ----------
  function specPanelHtml(i) {
    const sp = i.spec;
    const done = sp.ac.filter((a) => a.done).length;
    return `<section class="panel spec" id="spec" data-section="spec">
      <h2>Spec</h2>
      <p>${esc(sp.summary)}</p>
      <div class="subhead">Build</div>
      <ul>${sp.build.map((b) => `<li>${esc(b)}</li>`).join("")}</ul>
      <div class="subhead">Acceptance criteria · ${done} of ${sp.ac.length}</div>
      <ul class="ac">${sp.ac.map((a) => `<li class="${a.done ? "done" : ""}"><span class="box" aria-hidden="true">${a.done ? "✓" : ""}</span><span>${esc(a.text)}</span></li>`).join("")}</ul>
    </section>`;
  }

  // ---------- timeline ----------
  function timelineEntryHtml(e) {
    const a = actors[e.actor];
    let body = "";
    if (e.kind === "transition") {
      body = `<span class="move">${statusDot(e.from)}${esc(statusById[e.from].label)}<span class="arrow">→</span>${statusDot(e.to)}${esc(statusById[e.to].label)}${e.event ? `<span class="muted">on ${esc(e.event.replace(/_/g, " "))}</span>` : ""}</span>`
        + (e.gates ? `<div class="gate-list">${e.gates.map((g) => `<span class="pass">✓ ${esc(gates[g].label)}</span>`).join("")}</div>` : "");
    } else if (e.kind === "refused") {
      body = `Refused ${esc(actors[e.by].name)}'s request to enter ${esc(statusById[e.to].label)}: <b>${esc(gates[e.gate].label)}</b> · ${esc(e.reason)}`;
    } else {
      body = esc(e.text);
    }
    return `<li class="${e.kind}" data-actor-type="${a.type}" data-kind="${e.kind}">
      ${avatarHtml(a)}
      <div>
        <div class="who"><span class="name">${esc(a.name)}</span>${a.type === "person" ? "" : `<span class="actor-type ${a.type}">${esc(TYPE_LABEL[a.type])}</span>`}<span class="muted">${esc(e.kind === "transition" ? "moved" : e.kind === "comment" ? "commented" : e.kind === "claim" ? "claimed" : e.kind === "refused" ? "refused" : e.kind)}</span><span class="at">${esc(e.at)}</span></div>
        <div class="body">${body}</div>
      </div>
    </li>`;
  }
  const FEED = [["all", "All"], ["comments", "Comments"], ["transitions", "Transitions"]];
  function feedEntries(i) {
    if (state.feed === "comments") return i.timeline.filter((e) => e.kind === "comment");
    if (state.feed === "transitions") return i.timeline.filter((e) => e.kind === "transition" || e.kind === "refused");
    return i.timeline;
  }
  function composerHtml(i) {
    const s = statusById[i.status];
    const v = viewer();
    const locked = !!(s.lock && s.lock.includes(v.type));
    const terminal = !s.next;
    const note = locked ? `${ICON.lock} ${esc(s.label)} is locked to people. ${esc(v.name)} cannot comment here.` : terminal ? `${esc(s.label)} is terminal. Comments stay open for the verify note.` : "";
    return `<form class="composer ${locked ? "locked" : ""}" data-composer data-locked="${locked}">
      <div class="who">${actorHtml(state.viewer)}</div>
      <textarea name="body" rows="2" placeholder="${locked ? "Commenting is disabled for agents in " + esc(s.label) : "Comment as " + esc(v.name) + "…"}" ${locked ? "disabled" : ""} aria-label="New comment"></textarea>
      <div class="foot">${note ? `<span class="note">${note}</span>` : `<span class="note muted">Comments are events in the log. Agent comments end with (agent).</span>`}<button type="submit" ${locked ? "disabled" : ""}>Comment</button></div>
    </form>`;
  }
  function timelinePanelHtml(i) {
    const entries = feedEntries(i);
    const comments = i.timeline.filter((e) => e.kind === "comment").length;
    const filter = `<span class="feed-filter right" role="group" aria-label="Activity filter">${FEED.map(([id, label]) => `<button type="button" data-feed="${id}" aria-pressed="${state.feed === id}">${label}${id === "comments" ? ` <span class="n">${comments}</span>` : ""}</button>`).join("")}</span>`;
    const body = entries.length ? `<ol class="timeline">${entries.map(timelineEntryHtml).join("")}</ol>` : `<div class="proof-empty">No ${esc(state.feed)} yet.</div>`;
    return `<section class="panel" id="timeline" data-section="timeline"><h2>Activity <span class="n">${i.timeline.length}</span>${filter}</h2>${body}${composerHtml(i)}</section>`;
  }

  // ---------- header and properties ----------
  function headHtml(i) {
    const s = statusById[i.status];
    return `<div class="issue-head">
      <div class="crumbs"><span>Punchlist</span><span>›</span><span>Issues</span><span>›</span><span class="mono">${esc(i.id)}</span></div>
      <h1>${esc(i.title)}</h1>
      <div class="props">${statusPill(i.status)}${lockBadge(s)}${actorHtml(i.assignee)}${prChip(i)}<span>${i.labels.map((l) => `<span class="label">${esc(l)}</span>`).join(" ")}</span><span class="muted">${esc(i.milestone)}</span></div>
    </div>`;
  }
  function propsPanelHtml(i) {
    const s = statusById[i.status];
    return `<section class="panel rail-only-wide" data-section="props"><h2>Properties</h2>
      <dl class="props-list">
        <dt>Assignee</dt><dd>${actorHtml(i.assignee)}</dd>
        <dt>Dispatch</dt><dd>${s.dispatch ? esc(s.dispatch) : s.lock ? "none · locked to people" : "none"}</dd>
        <dt>Milestone</dt><dd>${esc(i.milestone)}</dd>
        <dt>Labels</dt><dd>${i.labels.map((l) => `<span class="label">${esc(l)}</span>`).join(" ")}</dd>
        <dt>Created</dt><dd>${esc(i.created)}</dd>
        <dt>Workflow</dt><dd class="mono">workflow.toml@v7</dd>
      </dl>
    </section>`;
  }

  // ---------- layouts ----------
  function renderSidebar(i) {
    return `<div class="page sidebar">
      <section class="panel head">${headHtml(i)}</section>
      <div class="main">
        ${noticeHtml(i)}
        ${specPanelHtml(i)}
        ${prPanelHtml(i)}
        ${proofPanelHtml(i)}
        ${timelinePanelHtml(i)}
      </div>
      <aside class="rail">
        ${transitionsPanelHtml(i)}
        ${gatesPanelHtml(i)}
        ${runsPanelHtml(i)}
        ${propsPanelHtml(i)}
      </aside>
    </div>`;
  }

  function renderTabs(i) {
    const s = statusById[i.status];
    const openThreads = i.pr ? i.pr.threads.filter((t) => t.state === "open").length : 0;
    const next = s.next && s.next.gates.length ? s.next : null;
    const failing = next ? next.gates.filter((g) => !(i.gates[g] && i.gates[g].pass)).length : 0;
    const tabs = [
      ["overview", "Overview", ""],
      ["runs", "Runs", `<span class="n">${i.runs.length}</span>`],
      ["gates", "Gates", failing ? `<span class="n bad">${failing} failing</span>` : `<span class="n">${next ? next.gates.length : Object.keys(i.lastGates || {}).length}</span>`],
      ["pr", "Pull request", openThreads ? `<span class="n bad">${openThreads} open</span>` : ""],
      ["timeline", "Activity", `<span class="n">${i.timeline.length}</span>`],
    ];
    const panel = {
      overview: `<div class="tabpanel two"><div>${specPanelHtml(i)}</div><div>${proofPanelHtml(i)}</div></div>`,
      runs: `<div class="tabpanel">${runsPanelHtml(i)}</div>`,
      gates: `<div class="tabpanel two"><div>${gatesPanelHtml(i)}</div><div>${proofPanelHtml(i)}</div></div>`,
      pr: `<div class="tabpanel">${prPanelHtml(i)}</div>`,
      timeline: `<div class="tabpanel">${timelinePanelHtml(i)}</div>`,
    }[state.tab];
    return `<div class="page tabs">
      <section class="panel head-card">
        ${headHtml(i)}
        ${noticeHtml(i)}
        ${transitionsPanelHtml(i).replace('class="panel"', 'class="panel" style="padding:0;border:0"')}
      </section>
      <nav class="tabbar" role="tablist" aria-label="Sections">
        ${tabs.map(([id, label, n]) => `<button type="button" role="tab" data-tab="${id}" aria-selected="${state.tab === id}">${label}${n}</button>`).join("")}
      </nav>
      ${panel}
    </div>`;
  }

  function renderPipeline(i) {
    const order = ["backlog", "todo", "start", "in_progress", "agent_review", "human_review", "merging", "done"];
    const cur = i.status === "canceled" ? -1 : order.indexOf(i.status);
    const steps = order.map((id, idx) => {
      const s = statusById[id];
      const cls = idx < cur ? "past" : idx === cur ? "current" : "future";
      const edge = idx < order.length - 1 ? (() => {
        const nx = s.next;
        const isNext = idx === cur;
        const chips = nx.gates.map((g) => { const r = isNext ? i.gates[g] : (idx < cur ? { pass: true } : null); return `<i class="${r ? (r.pass ? "pass" : "fail") : ""}" title="${esc(gates[g].label)}"></i>`; }).join("");
        const by = nx.by === "event" ? esc(nx.event.replace(/_/g, " ")) : esc(nx.by);
        return `<span class="edge ${isNext ? "next" : ""}"><span class="line"></span>${chips ? `<span class="chips">${chips}</span>` : ""}<span class="by">${by}</span></span>`;
      })() : "";
      return `<span class="step ${cls}" data-status="${id}" ${idx === cur ? 'aria-current="step"' : ""}>${statusDot(id)}${esc(s.label)}${s.lock ? `<span class="lock">${ICON.lock}</span>` : ""}</span>${edge}`;
    }).join("");
    return `<div class="page pipeline">
      <section class="panel">${headHtml(i)}<div class="stepper-wrap" style="margin-top:12px"><div class="stepper" aria-label="Workflow">${steps}</div></div></section>
      ${noticeHtml(i) ? `<div style="margin-top:12px">${noticeHtml(i)}</div>` : ""}
      <div class="grid">
        <div>
          ${transitionsPanelHtml(i)}
          ${runsPanelHtml(i)}
          ${gatesPanelHtml(i)}
          ${proofPanelHtml(i)}
          ${prPanelHtml(i)}
        </div>
        <div>
          ${specPanelHtml(i)}
          ${timelinePanelHtml(i)}
        </div>
      </div>
    </div>`;
  }

  function render() {
    const i = current();
    document.querySelectorAll(".switcher [data-variant]").forEach((b) => b.setAttribute("aria-pressed", String(b.dataset.variant === state.variant)));
    document.querySelectorAll(".switcher [data-state]").forEach((b) => b.setAttribute("aria-pressed", String(b.dataset.state === state.issue)));
    $view.dataset.layout = state.variant;
    $view.dataset.issueState = state.issue;
    $view.dataset.issue = i.id;
    $view.dataset.viewer = state.viewer;
    $view.innerHTML = state.variant === "sidebar" ? renderSidebar(i) : state.variant === "tabs" ? renderTabs(i) : renderPipeline(i);
    renderShortcuts();
    const wrap = $view.querySelector(".stepper-wrap"), cur = $view.querySelector(".step.current");
    if (wrap && cur) wrap.scrollLeft = Math.max(0, cur.offsetLeft - wrap.clientWidth / 2 + cur.offsetWidth / 2);
    const url = new URL(location.href);
    url.searchParams.set("variant", state.variant); url.searchParams.set("state", state.issue); url.searchParams.set("as", state.viewer);
    history.replaceState(null, "", url);
  }

  function renderShortcuts() {
    const title = { sidebar: "Sidebar", tabs: "Tabs", pipeline: "Pipeline" }[state.variant];
    const keys = [["1 2 3", "switch layout"], ["j k", "next / previous mock issue"], ["v", "view as person / agent"]];
    if (state.variant === "tabs") keys.push(["[ ]", "previous / next tab"]);
    keys.push(["g", "jump to gates"], ["t", "jump to activity"], ["c", "comment"], ["Esc", "close"]);
    $shortcuts.innerHTML = `<span class="title">${title} shortcuts</span>` + keys.map(([k, d]) => `<span>${k.split(" ").map((x) => `<kbd>${esc(x)}</kbd>`).join("")} ${esc(d)}</span>`).join("");
  }

  // ---------- live log ----------
  setInterval(() => {
    const i = issues.find((x) => x.key === "running");
    const run = i.runs.find((r) => r.live);
    if (!run) return;
    run.log.push(liveLogMore[state.liveIndex % liveLogMore.length]);
    state.liveIndex += 1;
    if (run.log.length > 12) run.log.shift();
    const $log = document.getElementById("live-log");
    if ($log) { $log.outerHTML = logHtml(run); const n = document.getElementById("live-log"); if (n) n.scrollTop = n.scrollHeight; }
  }, 1800);

  // ---------- actions ----------
  let toastTimer;
  function toast(msg) {
    $overlay.innerHTML = `<div class="toast" role="status">${esc(msg)}</div>`;
    clearTimeout(toastTimer);
    toastTimer = setTimeout(() => { $overlay.innerHTML = ""; }, 2400);
  }
  function setVariant(v) { if (VARIANTS.includes(v)) { state.variant = v; render(); } }
  function setIssue(k) { if (STATES.includes(k)) { state.issue = k; state.tab = "overview"; render(); } }
  function setViewer(a) { state.viewer = a; $viewer.value = a; render(); }
  function jump(id) { const el = document.getElementById(id); if (!el) { if (state.variant === "tabs") { state.tab = id; render(); } return; } el.scrollIntoView({ block: "start", behavior: "smooth" }); }

  document.addEventListener("keydown", (e) => {
    const focus = document.activeElement;
    if (focus && focus.matches && focus.matches("input, select, textarea")) return;
    if (e.metaKey || e.ctrlKey || e.altKey) return;
    const idx = STATES.indexOf(state.issue);
    switch (e.key) {
      case "1": setVariant("sidebar"); break;
      case "2": setVariant("tabs"); break;
      case "3": setVariant("pipeline"); break;
      case "j": case "ArrowDown": setIssue(STATES[Math.min(STATES.length - 1, idx + 1)]); e.preventDefault(); break;
      case "k": case "ArrowUp": setIssue(STATES[Math.max(0, idx - 1)]); e.preventDefault(); break;
      case "v": setViewer(state.viewer === "gannon" ? "claude" : "gannon"); break;
      case "g": jump("gates"); break;
      case "t": jump("timeline"); break;
      case "c": { jump("timeline"); const ta = document.querySelector("[data-composer] textarea"); if (ta && !ta.disabled) { setTimeout(() => ta.focus(), 50); e.preventDefault(); } break; }
      case "[": case "]": if (state.variant === "tabs") { const tabs = ["overview", "runs", "gates", "pr", "timeline"]; const t = tabs.indexOf(state.tab); state.tab = tabs[(t + (e.key === "]" ? 1 : tabs.length - 1)) % tabs.length]; render(); } break;
      case "Escape": $overlay.innerHTML = ""; break;
    }
  });

  document.addEventListener("click", (e) => {
    const v = e.target.closest(".switcher [data-variant]"); if (v) { setVariant(v.dataset.variant); return; }
    const s = e.target.closest(".switcher [data-state]"); if (s) { setIssue(s.dataset.state); return; }
    const tab = e.target.closest("[data-tab]"); if (tab) { state.tab = tab.dataset.tab; render(); return; }
    const feed = e.target.closest("[data-feed]"); if (feed) { state.feed = feed.dataset.feed; render(); return; }
    const move = e.target.closest("[data-transition]");
    if (move) {
      const i = current();
      if (move.getAttribute("aria-disabled") === "true") { toast(move.querySelector(".verdict").textContent.trim()); return; }
      toast(`Requested ${i.id}: ${statusById[i.status].label} → ${statusById[move.dataset.transition].label} (mock, nothing moves)`);
      return;
    }
    const a = e.target.closest('a[href^="#"]');
    if (a) { const id = a.getAttribute("href").slice(1); if (!document.getElementById(id) && state.variant === "tabs") { e.preventDefault(); state.tab = id === "checks" || id === "threads" ? "pr" : id; render(); setTimeout(() => jump(id), 0); } }
  });
  $viewer.addEventListener("change", () => setViewer($viewer.value));
  document.addEventListener("submit", (e) => {
    const form = e.target.closest("[data-composer]");
    if (!form) return;
    e.preventDefault();
    if (form.dataset.locked === "true") return;
    const text = form.body.value.trim();
    if (!text) return;
    const i = current();
    const v = viewer();
    i.timeline.push({ at: "just now", actor: state.viewer, kind: "comment", text: v.type === "agent" ? `${text}\n\n(agent)` : text });
    state.feed = state.feed === "transitions" ? "all" : state.feed;
    render();
    jump("timeline");
    toast(`Comment posted as ${v.name} (mock)`);
  });

  render();
})();
