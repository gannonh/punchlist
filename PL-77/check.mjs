import { chromium } from "/home/gannonh/.local/share/punchlist/worktrees/worktrees/gannonh/punchlist/PL-77/web/node_modules/@playwright/test/index.mjs";
import { execSync } from "node:child_process";
import { writeFileSync } from "node:fs";

const ROOT = process.argv[2];
const OUT = process.argv[3];
const BASE = `file://${ROOT}/prototypes/projects/index.html`;
const VARIANTS = ["sections", "panes", "cards"];
const LINK = /^\.\.\/issue-page\/\?issue=PL-\d+$/;
const lines = [];
let failed = 0;
const check = (n, variant, name, ok, detail = "") => {
  if (!ok) failed++;
  lines.push(`${ok ? "PASS" : "FAIL"}  check ${n}  ${variant.padEnd(8)}  ${name}${detail ? `  (${detail})` : ""}`);
};

const mainTree = execSync(`git -C ${ROOT} ls-tree main prototypes/projects`).toString().trim();
const branchTree = execSync(`git -C ${ROOT} ls-tree HEAD prototypes/projects`).toString().trim();
check(1, "main", "main has no prototypes/projects", mainTree === "", `git ls-tree main: ${JSON.stringify(mainTree)}`);

const browser = await chromium.launch({ slowMo: Number(process.env.SLOWMO || 0) });
const ctx = await browser.newContext({ viewport: { width: 1280, height: 860 }, recordVideo: { dir: OUT, size: { width: 1280, height: 860 } } });
const page = await ctx.newPage();
const errors = [];
page.on("console", (m) => { if (m.type() === "error") errors.push(m.text()); });
page.on("pageerror", (e) => errors.push(String(e)));

await page.goto(`${BASE}?variant=sections`);
const switcher = await page.locator(".topbar .switcher button").allInnerTexts();
check(1, "branch", "branch shows the switcher", switcher.length === 3 && branchTree !== "", switcher.join(" | ").replace(/\n/g, " "));
check(2, "all", "switcher lists exactly 3 variants", switcher.length === 3, `${switcher.length} buttons`);

const mock = await page.evaluate(() => JSON.parse(JSON.stringify(window.PUNCHLIST_PROJECTS_MOCK.projects)));
const ms = Object.fromEntries(mock.flatMap((p) => p.milestones.map((m) => [m.id, { ...m, project: p.id }])));

async function show(variant, id) {
  if (variant !== "panes") return;
  const project = ms[id]?.project ?? id;
  await page.click(`[data-key="select-p:${project}"]`);
}
const text = (sel) => page.locator(sel).first().innerText();

for (const v of VARIANTS) {
  errors.length = 0;
  await page.goto(`${BASE}?variant=${v}`);
  await page.waitForTimeout(300);
  check(2, v, "renders with no console error", errors.length === 0 && (await page.locator(`#view.variant-${v}`).count()) === 1, `${errors.length} errors ${errors.join("; ")}`);
  await page.screenshot({ path: `${OUT}/${v}-1-overview.png`, fullPage: true });

  // 3. PASS when
  const pw = await text(`[data-milestone="m2"] .pw-text`);
  check(3, v, "Gate 1: Dogfood PASS when verbatim", pw === ms.m2.passWhen, JSON.stringify(pw.slice(0, 60)) + "...");
  await show(v, "m4");
  const empty = await text(`[data-milestone="m4"] .pw-text`);
  check(3, v, "Groundwork milestone shows No PASS when yet", empty === "No PASS when yet", empty);
  await show(v, "m1");

  // 4. Progress
  for (const [id, want] of [["m1", "12 of 12 done"], ["m2", "7 of 10 done"], ["m3", "2 of 14 done"]]) {
    const got = await text(`[data-milestone="${id}"] [data-done-count]`);
    const segs = await page.locator(`[data-milestone="${id}"] .split .seg`).evaluateAll((els) => els.map((e) => [e.dataset.status, Number(e.dataset.count)]));
    const sum = segs.reduce((n, [, c]) => n + c, 0);
    const nonCanceled = ms[id].issues.filter((i) => i.status !== "canceled").length;
    check(4, v, `${ms[id].name}: ${want}`, got === want && sum === nonCanceled, `text "${got}", segments ${segs.map(([s, c]) => `${s}=${c}`).join(" ")} sum ${sum} of ${nonCanceled}`);
  }

  // 5. Open a milestone
  await page.click(`[data-key="open-m:m2"]`);
  const issues = page.locator(`[data-issues-of="m2"]`);
  const counted = await issues.locator(".issue-list:not([data-left-out-list]) a").count();
  const leftOut = await issues.locator("[data-left-out-list] a").count();
  const leftOutHead = await issues.locator(".issues-head.left-out").innerText().catch(() => "");
  const hrefs = await issues.locator("a[data-issue-link]").evaluateAll((els) => els.map((e) => e.getAttribute("href")));
  check(5, v, "Gate 1: Dogfood lists 10 issues plus 1 canceled marked left out", counted === 10 && leftOut === 1 && /Left out/.test(leftOutHead), `${counted} counted, ${leftOut} left out ("${leftOutHead.replace(/\n/g, " ")}")`);
  check(5, v, "every issue link matches ^../issue-page/?issue=PL-n$", hrefs.length === 11 && hrefs.every((h) => LINK.test(h)), hrefs.join(" "));
  await page.screenshot({ path: `${OUT}/${v}-2-open-milestone.png`, fullPage: v !== "cards" });
  await page.keyboard.press("Escape");

  // 6. Create
  await page.click(`[data-key="new-p:"]`);
  await page.fill("#f-name", "Docs");
  await page.keyboard.press("Enter");
  const docs = await page.evaluate(() => window.PUNCHLIST_PROJECTS_MOCK.projects.find((p) => p.name === "Docs")?.id);
  await page.click(`[data-key="new-m:${docs}"]`);
  await page.fill("#f-name", "Launch");
  await page.click(`[data-key="save"]`);
  const launch = await page.evaluate(() => window.PUNCHLIST_PROJECTS_MOCK.projects.find((p) => p.name === "Docs")?.milestones.find((m) => m.name === "Launch")?.id);
  const docsShown = (await page.locator(`[data-project="${docs}"]`).innerText().catch(() => "")).includes("Docs");
  const launchShown = launch ? await text(`[data-milestone="${launch}"] .ms-name`) : "";
  const launchCount = launch ? await text(`[data-milestone="${launch}"] [data-done-count]`) : "";
  check(6, v, "Docs and Launch show, Launch reads 0 of 0 done", docsShown && launchShown === "Launch" && launchCount === "0 of 0 done", `project ${docs} shown=${docsShown}, milestone "${launchShown}" "${launchCount}"`);
  await page.screenshot({ path: `${OUT}/${v}-3-created.png`, fullPage: true });

  // 7. Edit and validation
  await page.click(`[data-key="edit-m:${launch}"]`);
  await page.fill("#f-name", "Public launch");
  await page.fill("#f-pass", "The docs site is live.\nEvery pl command has a page.");
  await page.click(`[data-key="save"]`);
  const renamed = await text(`[data-milestone="${launch}"] .ms-name`);
  const newPw = await text(`[data-milestone="${launch}"] .pw-text`);
  check(7, v, "rename and PASS when show", renamed === "Public launch" && newPw === "The docs site is live.\nEvery pl command has a page.", `"${renamed}" / ${JSON.stringify(newPw)}`);
  await page.click(`[data-key="edit-m:${launch}"]`);
  await page.fill("#f-name", "a".repeat(101));
  await page.click(`[data-key="save"]`);
  const longErr = await text("[data-form-error]").catch(() => "");
  const afterLong = await page.evaluate((id) => window.PUNCHLIST_PROJECTS_MOCK.projects.flatMap((p) => p.milestones).find((m) => m.id === id).name, launch);
  check(7, v, "101-character name refused, nothing saved", longErr === "a milestone name is at most 100 characters" && afterLong === "Public launch", `"${longErr}", stored name "${afterLong}"`);
  await page.screenshot({ path: `${OUT}/${v}-4-validation.png`, fullPage: v !== "cards" });
  await page.fill("#f-name", "");
  await page.click(`[data-key="save"]`);
  const emptyErr = await text("[data-form-error]").catch(() => "");
  const afterEmpty = await page.evaluate((id) => window.PUNCHLIST_PROJECTS_MOCK.projects.flatMap((p) => p.milestones).find((m) => m.id === id).name, launch);
  check(7, v, "empty name refused, nothing saved", emptyErr === "a milestone needs a name" && afterEmpty === "Public launch", `"${emptyErr}", stored name "${afterEmpty}"`);
  await page.keyboard.press("Escape");
  const formGone = (await page.locator("[data-form]").count()) === 0;
  check(10, v, "Esc closes a form", formGone, `forms open after Esc: ${await page.locator("[data-form]").count()}`);

  // 8. Entry points
  await page.goto(`${BASE}?variant=${v}`);
  const header = await page.locator(".appbar").innerText();
  const entry = await page.locator(`[data-entry-from="list"]`).innerText();
  const sheetLinks = await page.locator(`[data-entry-from="sheet"] a`).count();
  check(8, v, "Projects header item and g m keys shown", /Issues/.test(header) && /Projects/.test(header) && /g\s*m/.test(entry.replace(/\n/g, " ")) && sheetLinks === 2, `header "${header.replace(/\n/g, " ")}", sheet links ${sheetLinks}`);
  await page.click(`.appbar [data-screen="list"]`);
  const onList = (await text(".screen-title")).startsWith("Issues");
  await page.screenshot({ path: `${OUT}/${v}-5-list-mock.png` });
  await page.keyboard.press("g");
  await page.keyboard.press("m");
  const title = await text(".screen-title");
  check(8, v, "g then m on the list mock opens the screen", onList && title.startsWith("Projects") && !page.url().includes("screen=list"), `list shown=${onList}, then "${title.split("\n")[0]}"`);
  // The sheet's milestone link opens the milestone in this layout.
  await page.click(`[data-entry-from="sheet"] a[data-nav="milestone=m2"]`);
  check(8, v, "issue sheet's milestone link opens Gate 1: Dogfood", (await page.locator(`[data-issues-of="m2"]`).count()) === 1);
  await page.keyboard.press("Escape");

  // 10. Keyboard
  await page.goto(`${BASE}?variant=${v}`);
  async function walk(max = 250) {
    const seen = new Set();
    await page.locator("body").focus();
    for (let i = 0; i < max; i++) {
      await page.keyboard.press("Tab");
      const id = await page.evaluate(() => { const e = document.activeElement; return e.dataset.key || e.getAttribute("href") || (e.id && `#${e.id}`) || e.tagName; });
      seen.add(id);
    }
    return seen;
  }
  const want = (ids) => ids.map((id) => `open-m:${id}`);
  let seen = await walk();
  let visible = v === "panes" ? ["m1", "m2", "m3"] : ["m1", "m2", "m3", "m4"];
  let missing = want(visible).filter((k) => !seen.has(k));
  if (v === "panes") {
    await page.focus(`[data-key="select-p:p2"]`);
    await page.keyboard.press("Enter");
    seen = await walk(80);
    missing = missing.concat(want(["m4"]).filter((k) => !seen.has(k)));
    await page.focus(`[data-key="select-p:p1"]`);
    await page.keyboard.press("Enter");
  }
  check(10, v, "Tab reaches every milestone", missing.length === 0, missing.length ? `missing ${missing}` : "4 of 4");
  await page.focus(`[data-key="open-m:m2"]`);
  await page.keyboard.press("Enter");
  const opened = (await page.locator(`[data-issues-of="m2"]`).count()) === 1;
  check(10, v, "Enter opens a milestone", opened);
  seen = await walk();
  const links = await page.locator(`[data-issues-of="m2"] a`).evaluateAll((els) => els.map((e) => e.getAttribute("href")));
  const missingLinks = links.filter((h) => !seen.has(h));
  check(10, v, "Tab reaches every issue link", links.length === 11 && missingLinks.length === 0, `${links.length - missingLinks.length} of ${links.length}`);
  await page.keyboard.press("Escape");
  await page.focus(`[data-key="edit-m:m2"]`);
  await page.keyboard.press("Enter");
  seen = await walk(40);
  const fields = ["#f-name", "#f-pass", "save", "cancel-form"].filter((k) => !seen.has(k));
  check(10, v, "Tab reaches every form field and button", fields.length === 0, fields.length ? `missing ${fields}` : "name, PASS when, Save, Cancel");
  await page.keyboard.press("Escape");
  const allSeen = await walk();
  const visibleButtons = await page.evaluate(() => [...document.querySelectorAll("#view button, #view a")].filter((b) => b.offsetParent).map((b) => b.dataset.key || b.getAttribute("href")));
  const unreached = visibleButtons.filter((b) => !allSeen.has(b));
  check(10, v, "Tab reaches every button and link on the screen", unreached.length === 0, unreached.length ? `missing ${unreached}` : `${visibleButtons.length} of ${visibleButtons.length}`);
}
await page.close();
const video = await page.video().path();

// 9. Narrow width
const narrow = await browser.newContext({ viewport: { width: 390, height: 844 } });
const np = await narrow.newPage();
for (const v of VARIANTS) {
  const states = { overview: "", "open milestone": "&milestone=m2", "list mock": "&screen=list" };
  const widths = [];
  for (const [name, q] of Object.entries(states)) {
    await np.goto(`${BASE}?variant=${v}${q}`);
    widths.push([name, ...(await np.evaluate(() => [document.documentElement.scrollWidth, document.documentElement.clientWidth]))]);
    if (name === "open milestone") await np.screenshot({ path: `${OUT}/${v}-6-narrow-390.png`, fullPage: v !== "cards" });
  }
  await np.goto(`${BASE}?variant=${v}`);
  await np.click(`[data-key="edit-m:m2"]`);
  widths.push(["form", ...(await np.evaluate(() => [document.documentElement.scrollWidth, document.documentElement.clientWidth]))]);
  check(9, v, "390 px: scrollWidth <= clientWidth", widths.every(([, s, c]) => s <= c), widths.map(([n, s, c]) => `${n} ${s}<=${c}`).join(", "));
}
const grep = execSync(`grep -nE '#[0-9a-fA-F]{3,8}\\b|rgba?\\(' ${ROOT}/prototypes/projects/styles.css || true`).toString();
check(9, "all", "styles.css has no hard-coded colors", grep === "", grep || "grep printed nothing");

await browser.close();
lines.push("", `${lines.filter((l) => l.startsWith("PASS")).length} passed, ${failed} failed`, `video: ${video}`);
writeFileSync(`${OUT}/live-checks.txt`, lines.join("\n") + "\n");
console.log(lines.join("\n"));
process.exit(failed ? 1 : 0);
