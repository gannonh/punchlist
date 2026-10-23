import { chromium } from "/home/gannonh/.local/share/punchlist/worktrees/worktrees/gannonh/punchlist/PL-77/web/node_modules/@playwright/test/index.mjs";
const [ROOT, OUT] = process.argv.slice(2);
const BASE = `file://${ROOT}/prototypes/projects/index.html`;
const browser = await chromium.launch({ slowMo: 90 });
const ctx = await browser.newContext({ viewport: { width: 1280, height: 800 }, recordVideo: { dir: OUT, size: { width: 1280, height: 800 } } });
const page = await ctx.newPage();
const pause = (ms = 900) => page.waitForTimeout(ms);
await page.goto(`${BASE}?variant=sections&screen=list`);
await pause(1500);
await page.keyboard.press("g"); await page.keyboard.press("m");
await pause(1500);
await page.locator(`[data-milestone="m1"]`).scrollIntoViewIfNeeded(); await pause(1500);
await page.click(`[data-key="open-m:m2"]`); await pause(2500);
await page.mouse.wheel(0, 400); await pause(1500);
await page.click(`[data-key="new-p:"]`); await page.type("#f-name", "Docs", { delay: 80 }); await page.keyboard.press("Enter"); await pause();
const docs = await page.evaluate(() => window.PUNCHLIST_PROJECTS_MOCK.projects.find((p) => p.name === "Docs").id);
await page.click(`[data-key="new-m:${docs}"]`); await page.type("#f-name", "Launch", { delay: 80 });
await page.click(`[data-key="save"]`); await pause(1500);
for (const v of ["panes", "cards"]) {
  await page.keyboard.press(v === "panes" ? "2" : "3"); await page.evaluate(() => scrollTo(0, 0)); await pause(1500);
  if (v === "panes") await page.click(`[data-key="select-p:p1"]`);
  await page.locator(`[data-milestone="m2"]`).first().scrollIntoViewIfNeeded(); await pause(1200);
  await page.click(`[data-key="open-m:m2"]`); await pause(2500);
  if (v === "cards") { await page.keyboard.press("Escape"); await pause(); }
  if (v === "panes") { await page.click(`[data-key="close-m:m2"]`); await pause(); }
  const launch = await page.evaluate(() => window.PUNCHLIST_PROJECTS_MOCK.projects.flatMap((p) => p.milestones).find((m) => m.name.includes("aunch")).id);
  if (v === "panes") await page.click(`[data-key="select-p:${docs}"]`);
  await page.locator(`[data-key="edit-m:${launch}"]`).first().scrollIntoViewIfNeeded();
  await page.click(`[data-key="edit-m:${launch}"]`);
  await page.fill("#f-name", "a".repeat(101)); await page.click(`[data-key="save"]`); await pause(1500);
  await page.fill("#f-name", ""); await page.click(`[data-key="save"]`); await pause(1200);
  await page.type("#f-name", v === "panes" ? "Public launch" : "Launch day", { delay: 50 });
  await page.type("#f-pass", "The docs site is live.", { delay: 30 });
  await page.click(`[data-key="save"]`); await pause(1500);
}
await page.setViewportSize({ width: 390, height: 800 }); await page.evaluate(() => scrollTo(0, 0)); await pause(1500);
await page.mouse.wheel(0, 900); await pause(1500);
await page.close();
console.log(await page.video().path());
await browser.close();
