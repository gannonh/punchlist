const fs = require('node:fs');
const path = require('node:path');
const {execFileSync} = require('node:child_process');
const assert = require('node:assert/strict');
const {chromium} = require(process.cwd()+'/node_modules/.pnpm/node_modules/playwright');
const root=process.cwd(), tmp=process.env.TMPDIR, out=root+'/verify-artifacts/pl176-final';
fs.mkdirSync(out,{recursive:true});
const control=root+'/.claude/skills/verify-punchlist/bin/control-punchlist';
const report=process.env.MIN_CHECK?JSON.parse(fs.readFileSync(out+'/results.json','utf8')):[];
function ctl(...args){console.log(args[0],args[1]);try{return execFileSync(control,args,{encoding:'utf8',stdio:['ignore','pipe','pipe']});}catch(e){console.error(e.stderr?.toString());throw e;}}
function info(run){const env=fs.readFileSync(`${tmp}/punchlist-verify/${run}/run.env`,'utf8');const url=/^SERVER_URL=(.*)$/m.exec(env)[1];const token=/^token = "(.*)"$/m.exec(fs.readFileSync(`${tmp}/punchlist-verify/${run}/config.toml`,'utf8'))[1];return {url,token};}
async function api(ctx,method,url,data,token){const r=await ctx.request[method](url,{data,headers:token?{Authorization:'Bearer '+token}:{}});assert(r.ok(),`${method} ${url}: ${r.status()} ${await r.text()}`);return r.status()===204?null:r.json();}
async function session(browser,run,record=false){const {url,token}=info(run);const ctx=await browser.newContext({baseURL:url,extraHTTPHeaders:{Origin:url},viewport:{width:1280,height:800},...(record?{recordVideo:{dir:out+'/video',size:{width:1280,height:800}}}:{})});await api(ctx,'post','/api/session',{token});return ctx;}
async function issue(ctx,body='Waits.',title='One-word body'){return (await api(ctx,'post','/api/issues',{title,body})).row.issue.id;}
async function waiting(ctx){const id=await issue(ctx,'Waits.','Waiting issue');for(const to of ['todo','start'])await api(ctx,'post',`/api/issues/${id}/transitions`,{to});const runner=await api(ctx,'post','/api/runners',{name:'verify',agents:['claude-code']});const claim=await api(ctx,'post','/api/runs/claim',{},runner.token);assert.equal(claim.issue.id,id);await api(ctx,'post',`/api/runs/${claim.run_id}/waiting`,{reason:'Choose a name.'},claim.agent_token);await api(ctx,'post',`/api/runs/${claim.run_id}/finish`,{attempt:claim.attempt,outcome:'succeeded',duration_ms:1},runner.token);return id;}
async function open(page,id,width){await page.setViewportSize({width,height:800});await page.goto('/w/PL');const row=page.locator(`[data-issue="${id}"]`);await row.waitFor();if(await row.getAttribute('data-focused')===null)await row.click();await row.click();const sheet=page.getByRole('dialog');await sheet.waitFor();await sheet.evaluate(e=>Promise.all(e.getAnimations().map(a=>a.finished)));await page.evaluate(()=>document.fonts.ready);return sheet;}
async function measure(sheet){return sheet.evaluate(e=>{const box=el=>el?{left:el.getBoundingClientRect().left,width:el.getBoundingClientRect().width}:null;return {header:box(e.querySelector('h2')),fields:box(e.querySelector('dl')),body:box(e.querySelector(':scope > .border-t')),notice:box(e.querySelector('[data-notice="waiting"]')),sheetScroll:e.scrollWidth,sheetClient:e.clientWidth,pageScroll:document.documentElement.scrollWidth,columns:getComputedStyle(e.querySelector('dl')).gridTemplateColumns};});}
function aligned(m,notice=false){for(const b of [m.fields,m.body,...(notice?[m.notice]:[])].filter(Boolean)){assert(Math.abs(b.left-m.header.left)<=1,JSON.stringify(m));assert(Math.abs(b.width-m.header.width)<=2,JSON.stringify(m));}}
async function snap(page,name){await page.screenshot({path:out+'/'+name+'.png'});}
async function fresh(browser,n,fn,record=false){if(n<Number(process.env.MIN_CHECK||1))return;const run='pl176-final-'+n;ctl('up',run);try{ctl('doctor',run);const ctx=await session(browser,run,record);const p=await ctx.newPage();const result=await fn(ctx,p);fs.writeFileSync(out+`/check${n}-rows.txt`,ctl('sql',run,'SELECT id, title, length(body) AS body_length, status FROM issue ORDER BY number')+ctl('sql',run,'SELECT issue_id, waiting_reason, outcome FROM run ORDER BY started_at'));await ctx.close();report.push({check:n,pass:true,result});fs.writeFileSync(out+'/results.json',JSON.stringify(report,null,2));}finally{ctl('down',run);}}
(async()=>{
 const browser=await chromium.launch({ignoreDefaultArgs:['--disable-dev-shm-usage']});
 try{
 const main=await session(browser,'pl176-final-main');const mp=await main.newPage();const mid=await issue(main);const baseline={};for(const w of [1280,390]){baseline[w]=await measure(await open(mp,mid,w));assert(baseline[w].body.left-baseline[w].header.left>20);await snap(mp,'main-short-'+w);}await main.close();
 await fresh(browser,1,async(c,p)=>{const id=await issue(c);const m=await measure(await open(p,id,1280));aligned(m);await snap(p,'branch-short-1280');return {baseline,branch:m};});
 await fresh(browser,2,async(c,p)=>{const m=await measure(await open(p,await issue(c),1280));aligned(m);return m;});
 await fresh(browser,3,async(c,p)=>{const id=await issue(c,'Long body '.repeat(200));const m=await measure(await open(p,id,1280));aligned(m);assert(m.sheetScroll<=m.sheetClient);await snap(p,'branch-long-1280');return m;});
 await fresh(browser,4,async(c,p)=>{const m=await measure(await open(p,await waiting(c),1280));assert(m.notice);aligned(m,true);await snap(p,'branch-waiting-1280');return m;});
 await fresh(browser,5,async(c,p)=>{const short=await issue(c);const a=await measure(await open(p,short,390));aligned(a);await snap(p,'branch-short-390');const m=await measure(await open(p,await waiting(c),390));aligned(m,true);assert(m.notice);await snap(p,'branch-waiting-390');return {short:a,waiting:m,main:baseline[390]};});
 await fresh(browser,6,async(c,p)=>{const m=await measure(await open(p,await waiting(c),390));assert(m.pageScroll<=390);return m;});
 await fresh(browser,7,async(c,p)=>{const m=await measure(await open(p,await issue(c,''),390));aligned(m);assert.equal(m.body,null);await snap(p,'branch-empty-390');return m;});
 await fresh(browser,8,async(c,p)=>{const id=await issue(c);const s=await open(p,id,1280);const response=p.waitForResponse(r=>r.request().method()==='PATCH'&&r.url().endsWith('/api/issues/'+id));await s.getByLabel('Title',{exact:true}).fill('Edited title');await s.getByLabel('Title',{exact:true}).press('Enter');assert.equal((await response).status(),200);await api(c,'patch','/api/issues/'+id,{body:'Updated body.'});await s.getByText('Updated body.',{exact:true}).waitFor();const m=await measure(s);aligned(m);const read=await api(c,'get','/api/issues/'+id);assert.equal(read.title,'Edited title');assert.equal(read.body,'Updated body.');await snap(p,'branch-edited');return {patchStatus:200,measure:m};});
 await fresh(browser,9,async(c,p)=>{
   const id=await waiting(c);const comparisons={};
   // Both builds read the same instance, rows and frozen clock, eliminating unrelated ids and ages.
   for(const [name,url] of [['issue','/issues/'+id],['inbox','/w/PL/inbox'],['pull-requests','/w/PL/pull-requests']]){
     const images=[];
     for(const build of ['main','branch']){
       const pc=await browser.newContext({baseURL:info('pl176-final-9').url,viewport:{width:1280,height:800},storageState:await c.storageState()});
       const dist=build==='main'?tmp+'/pl176-review-main/web/dist':root+'/web/dist';
       await pc.route('**/*',async route=>{const u=new URL(route.request().url());if(u.pathname.startsWith('/api/'))return route.continue();const file=path.join(dist,u.pathname.startsWith('/assets/')?u.pathname:'index.html');return route.fulfill({path:file});});
       const page=await pc.newPage();await page.clock.setFixedTime(new Date('2026-10-27T22:00:00Z'));await page.goto(url);await page.locator(name==='issue'?'[data-issue-page]':name==='inbox'?'[data-inbox-page], main':'[data-section], main').first().waitFor();await page.evaluate(()=>document.fonts.ready);await page.waitForTimeout(1200);images.push(await page.screenshot({path:out+'/'+build+'-screen-'+name+'.png',animations:'disabled'}));await pc.close();
     }
     const {PNG}=require(root+'/node_modules/.pnpm/node_modules/playwright-core/lib/utilsBundle.js');const a=PNG.sync.read(images[0]),b=PNG.sync.read(images[1]);let raw=0;for(let i=0;i<a.data.length;i+=4)if(!a.data.subarray(i,i+4).equals(b.data.subarray(i,i+4)))raw++;const {default:pixelmatch}=await import(tmp+'/pl176-image-compare/node_modules/pixelmatch/index.js');const diff=pixelmatch(a.data,b.data,null,a.width,a.height,{threshold:0.1});comparisons[name]={pixelmatchDifferentPixels:diff,rawDifferentPixels:raw,threshold:0.1};assert.equal(diff,0,`${name} screenshots differ`);
   }return comparisons;
 });
 await fresh(browser,10,async(c,p)=>{const ids=[await issue(c),await issue(c,'Long body '.repeat(200),'Long body'),await waiting(c)];const started=Date.now();for(const width of [1280,390])for(let i=0;i<ids.length;i++){const m=await measure(await open(p,ids[i],width));aligned(m,i===2);await snap(p,`video-${width}-${i}`);await p.waitForTimeout(5000);}return {durationSeconds:(Date.now()-started)/1000};},true);
 }finally{await browser.close();}
})().catch(e=>{console.error(e);fs.writeFileSync(out+'/failure.txt',String(e.stack));process.exitCode=1;});
