const fs = require('node:fs');
const path = require('node:path');
const assert = require('node:assert/strict');
const { chromium, expect } = require(process.cwd() + '/node_modules/.pnpm/node_modules/@playwright/test');
const root = process.env.TMPDIR + '/punchlist-verify/';
const out = path.resolve('verify-artifacts/pl167-independent');
const results = process.env.CHECKS ? JSON.parse(fs.readFileSync(out+'/observations.json','utf8')).filter(r=>!process.env.CHECKS.split(',').includes(String(r.check))) : [];
function config(name) {
  const dir = root + name;
  const env = fs.readFileSync(dir + '/run.env', 'utf8');
  return { dir, url: /^SERVER_URL=(.*)$/m.exec(env)[1], token: /^token = "(.*)"$/m.exec(fs.readFileSync(dir + '/config.toml', 'utf8'))[1] };
}
const branch = config(process.env.BRANCH_RUN || 'pl167-independent'), main = config(process.env.MAIN_RUN || 'pl167-main-independent');
async function api(site, route, data, method) {
  const res = await fetch(site.url + '/api' + route, {method: method || (data ? 'POST' : 'GET'), headers: {Authorization: 'Bearer ' + site.token, 'Content-Type': 'application/json'}, body: data && JSON.stringify(data)});
  if (!res.ok) throw new Error(route + ': ' + res.status + ' ' + await res.text());
  return res.status === 204 ? null : res.json();
}
async function check(n, fn) {
  if(process.env.CHECKS && !process.env.CHECKS.split(',').includes(String(n))) return;
  try { const observed = await fn(); results.push({check:n, pass:n!==10, ...(n===10 ? {partial:'Recording passed; lasting phone URL and private access remain unavailable in an attempt-scoped instance.'}:{}), observed}); console.log(n===10?'PARTIAL':'PASS',n,JSON.stringify(observed)); }
  catch(e) {results.push({check:n,pass:false,error:e.stack}); console.log('FAIL',n,e.stack);}
  fs.writeFileSync(out + '/observations.json',JSON.stringify(results,null,2));
}
const strip = (p,a) => p.locator(`[data-strip] [data-action="${a}"]`);
const typing = p => p.evaluate(() => { const a=document.activeElement; return a instanceof HTMLElement && (a.isContentEditable || ['INPUT','TEXTAREA'].includes(a.tagName)); });
async function signed(browser,site,mobile=true,video=false) {
  const ctx=await browser.newContext({viewport: mobile ? {width:390,height:844} : {width:1280,height:800}, hasTouch:mobile,isMobile:mobile,...(video ? {recordVideo:{dir:out+'/video',size:{width:390,height:844}}}: {})});
  await ctx.request.post(site.url+'/api/session',{data:{token:site.token}}).then(r=>assert(r.ok()));
  const p=await ctx.newPage();
  return {ctx,p};
}
async function list(p,site) {await p.goto(site.url+'/w/PL'); await expect(p.locator('[data-issue]')).toHaveCount(3);}
async function focus(p) { return p.locator('[data-issue][data-focused]').getAttribute('data-issue'); }
async function shot(p,name) {await p.mouse.move(0,0); await p.waitForTimeout(200); await p.screenshot({path:out+'/'+name+'.png',animations:'disabled'});}
(async()=>{
  const browser=await chromium.launch({ignoreDefaultArgs:['--disable-dev-shm-usage']});
  for(const site of [main,branch]) for(let i=1;i<=3;i++) {
    const row=await api(site,'/issues',{title:'Touch issue '+i,body:'Live shortcut verification.'});
    await api(site,'/issues/'+row.row.issue.id,{labels:['touch-fixture']},'PATCH');
  }
  const b=await signed(browser,branch), m=await signed(browser,main);
  await check(1,async()=>{
    await list(m.p,main); assert.equal(await m.p.locator('footer button').count(),0);
    const before=await focus(m.p); const key=m.p.locator('footer span').filter({hasText:/^j k next/}).first();
    // Main's grouped j/k text has no handler.
    await m.p.locator('footer kbd').filter({hasText:/^j$/}).tap(); await m.p.locator('footer kbd').filter({hasText:/^j$/}).tap();
    assert.equal(await focus(m.p),before); await shot(m.p,'c1-main');
    await list(b.p,branch); const ids=await b.p.locator('[data-issue]').evaluateAll(rs=>rs.map(r=>r.dataset.issue));
    assert.equal(await focus(b.p),ids[0]); await strip(b.p,'next').tap(); await strip(b.p,'next').tap();
    assert.equal(await focus(b.p),ids[2]); await shot(b.p,'c1-branch'); await strip(b.p,'open').tap();
    await expect(b.p).toHaveURL(branch.url+'/issues/'+ids[2]);
    return {mainFocused:before,mainButtons:0,branchFocused:ids[2],url:b.p.url()};
  });
  await check(9,async()=>{
    const ds=[];
    const screenshotTime=new Date(Date.now()+120000);
    for(const [name,site] of [['main',main],['branch',branch]]) {
      const d=await signed(browser,site,false); ds.push(d);
      await d.p.clock.setFixedTime(screenshotTime);
      await list(d.p,site); await d.p.evaluate(()=>document.fonts.ready); await shot(d.p,'c9-'+name+'-list');
      assert.equal(await d.p.evaluate(()=>matchMedia('(pointer: fine)').matches),true);
      const first=await focus(d.p); await d.p.keyboard.press('j'); const next=await focus(d.p); assert.notEqual(first,next);
      await d.p.keyboard.press('k'); assert.equal(await focus(d.p),first);
      await d.p.keyboard.press('s'); await expect(d.p.getByRole('menuitemradio',{name:'Todo',exact:true})).toBeVisible();
      await d.p.keyboard.press('Escape'); await expect(d.p.getByRole('menu')).toHaveCount(0);
      await d.p.keyboard.press('x'); await expect(d.p.locator('[data-selection]')).toContainText('1 selected');
      await d.p.keyboard.press('Escape'); await expect(d.p.locator('[data-selection]')).toHaveCount(0);
      await d.p.goto(site.url+'/issues/PL-1'); await expect(d.p.getByLabel('New comment')).toBeVisible(); await d.p.evaluate(()=>document.fonts.ready); await shot(d.p,'c9-'+name+'-issue');
      for(const [key,id] of [['g','gates'],['t','timeline']]) {
        await d.p.keyboard.press(key); const top=await d.p.locator('#'+id).evaluate(e=>e.getBoundingClientRect().top); assert(top>=0 && top<800);
      }
      await d.p.keyboard.press('c'); await expect(d.p.getByLabel('New comment')).toBeFocused();
      await d.p.keyboard.press('Escape'); assert.equal(await typing(d.p),false); await d.p.keyboard.press('Escape'); await expect(d.p).toHaveURL(site.url+'/w/PL');
    }
    const {PNG}=require(process.env.TMPDIR+'/pl167-image-tools/node_modules/pngjs');
    const pixelmatch=(await import(process.env.TMPDIR+'/pl167-image-tools/node_modules/pixelmatch/index.js')).default;
    const counts={};
    for(const screen of ['list','issue']) { const a=PNG.sync.read(fs.readFileSync(out+'/c9-main-'+screen+'.png')), z=PNG.sync.read(fs.readFileSync(out+'/c9-branch-'+screen+'.png')); counts[screen]=pixelmatch(a.data,z.data,null,a.width,a.height,{threshold:0}); assert.equal(counts[screen],0); }
    for(const d of ds) await d.ctx.close(); return {pixelDiff:counts,keys:'j k s x g t c Escape passed on both'};
  });
  await check(2,async()=>{
    await list(b.p,branch); const ids=await b.p.locator('[data-issue]').evaluateAll(rs=>rs.map(r=>r.dataset.issue));
    await b.p.locator(`[data-issue="${ids[1]}"]`).tap(); assert.equal(await focus(b.p),ids[1]);
    await strip(b.p,'status').tap(); await shot(b.p,'c2-menu'); await b.p.getByRole('menuitemradio',{name:'Todo',exact:true}).tap();
    await expect.poll(async()=> (await api(branch,'/issues/'+ids[1])).status).toBe('todo');
    const timeline=await api(branch,'/issues/'+ids[1]+'/events');
    fs.writeFileSync(out+'/c2-timeline.json',JSON.stringify(timeline,null,2));
    const transitions=await api(branch,'/issues/'+ids[1]+'/transitions');
    const changes=timeline.filter(e=>e.detail?.kind==='transition' || e.kind==='transition');
    assert.equal(changes.length,1); assert.equal(changes[0].detail.to,'todo'); assert.equal(changes[0].actor.role,'person');
    fs.writeFileSync(out+'/c2-transitions.json',JSON.stringify(transitions,null,2));
    return {issue:ids[1],status:'todo',timeline,transitions};
  });
  await check(3,async()=>{
    await b.p.goto(branch.url+'/w/PL?label=absent-fixture'); await expect(b.p.locator('[data-empty]')).toBeVisible();
    for(const a of ['status','assignee','labels','project','milestone']) { await expect(strip(b.p,a)).toBeDisabled(); await expect(strip(b.p,a)).toHaveAttribute('title','Focus an issue first'); }
    await shot(b.p,'c3-disabled'); return 'Five actions disabled with title Focus an issue first';
  });
  await check(4,async()=>{
    await list(b.p,branch); await strip(b.p,'select').tap(); await strip(b.p,'range').tap(); await strip(b.p,'next').tap(); await strip(b.p,'next').tap(); await strip(b.p,'select').tap();
    await expect(b.p.locator('[data-selection]')).toContainText('3 selected'); await shot(b.p,'c4-range');
    const patches=[]; const listener=r=> {if(r.method()==='PATCH' && /\/api\/issues\/PL-/.test(r.url())) patches.push({url:r.url(),body:r.postDataJSON()});}; b.p.on('request',listener);
    await strip(b.p,'labels').tap(); await b.p.getByRole('menuitemcheckbox',{name:'touch-fixture',exact:true}).tap();
    await expect.poll(()=>patches.length).toBe(3); await expect(b.p.locator('[data-sonner-toast]')).toContainText('3 issues');
    await shot(b.p,'c4-toast'); b.p.off('request',listener);
    for(const id of ['PL-1','PL-2','PL-3']) assert(!(await api(branch,'/issues/'+id)).labels.includes('touch-fixture'));
    return patches;
  });
  await check(5,async()=>{
    await api(branch,'/issues/PL-1/transitions',{to:'todo'}); await b.p.goto(branch.url+'/w/PL/inbox'); await expect(b.p.locator('[data-item]')).toHaveCount(2);
    const ids=await b.p.locator('[data-item]').evaluateAll(rs=>rs.map(r=>r.dataset.item));
    await strip(b.p,'next').tap(); await strip(b.p,'next').tap(); assert.equal(await b.p.locator('[data-item]:focus').getAttribute('data-item'),ids[1]);
    await shot(b.p,'c5-inbox'); await strip(b.p,'open').tap(); await expect(b.p).toHaveURL(branch.url+'/issues/'+ids[1]);
    const inbox=await api(branch,'/inbox'); assert.equal(inbox.items.find(i=>i.issue.id===ids[1]).unread,false); return {ids,inbox};
  });
  await check(6,async()=>{
    await b.p.goto(branch.url+'/w/PL/pull-requests'); await strip(b.p,'issues').tap(); await expect(b.p).toHaveURL(branch.url+'/w/PL');
    await b.p.goto(branch.url+'/issues/PL-1'); await expect(b.p.getByLabel('New comment')).toBeVisible(); const rects={};
    for(const [action,id] of [['gates','gates'],['activity','timeline']]) {await strip(b.p,action).tap(); await b.p.waitForTimeout(300); const r=await b.p.locator('#'+id).evaluate(e=>({top:e.getBoundingClientRect().top,height:innerHeight})); assert(r.top>=0 && r.top<r.height,JSON.stringify(r)); rects[action]=r; await shot(b.p,'c6-'+action);}
    return rects;
  });
  await check(7,async()=>{
    const observed=[];
    for(const screen of ['list','issue']) {
      const url=branch.url+(screen==='list'?'/w/PL':'/issues/PL-1'); await b.p.goto(url); await expect(b.p.locator('[data-strip]')).toBeVisible();
      const actions=await b.p.locator('[data-strip] button').evaluateAll(bs=>bs.map(x=>x.dataset.action));
      for(const action of actions) {
        await b.p.goto(url); await expect(strip(b.p,action)).toBeVisible(); await expect(strip(b.p,action)).toBeEnabled();
        await strip(b.p,action).tap(); const isTyping=await typing(b.p); assert.equal(isTyping,['filter','comment'].includes(action),screen+' '+action);
        observed.push({screen,action,typing:isTyping,active:await b.p.evaluate(()=>document.activeElement?.tagName)});
      }
    } return observed;
  });
  await check(8,async()=>{
    for(let i=4;i<=60;i++) await api(branch,'/issues',{title:'Touch issue '+i,body:''});
    await b.p.goto(branch.url+'/w/PL'); await expect(b.p.locator('[data-issue]')).toHaveCount(60); await b.p.evaluate(()=>window.scrollTo(0,document.body.scrollHeight)); await b.p.waitForTimeout(200);
    const r=await b.p.evaluate(()=>({bottom:[...document.querySelectorAll('[data-issue]')].at(-1).getBoundingClientRect().bottom,top:document.querySelector('[data-strip]').getBoundingClientRect().top,heights:[...document.querySelectorAll('[data-strip] button')].map(x=>x.getBoundingClientRect().height)}));
    assert(r.bottom<=r.top,JSON.stringify(r)); assert(Math.min(...r.heights)>=44); await shot(b.p,'c8-end'); return r;
  });
  await check(10,async()=>{
    const v=await signed(browser,branch,true,true); const started=Date.now();
    await v.p.goto(branch.url+'/w/PL'); await expect(v.p.locator('[data-issue]')).toHaveCount(60); await v.p.waitForTimeout(1800);
    await strip(v.p,'next').tap(); await v.p.waitForTimeout(1800); await strip(v.p,'next').tap(); const id=await focus(v.p); await v.p.waitForTimeout(1800); await strip(v.p,'open').tap(); await expect(v.p).toHaveURL(branch.url+'/issues/'+id); await v.p.waitForTimeout(1800);
    await strip(v.p,'comment').tap(); assert(await typing(v.p)); await v.p.waitForTimeout(1800); await strip(v.p,'gates').tap(); assert(!(await typing(v.p))); await v.p.waitForTimeout(1800); await strip(v.p,'activity').tap(); await v.p.waitForTimeout(1800); await strip(v.p,'issues').tap();
    await v.p.goto(branch.url+'/w/PL?status=backlog'); await expect(v.p.locator('[data-issue]')).toHaveCount(58); await strip(v.p,'next').tap(); const statusId=await focus(v.p); await strip(v.p,'status').tap(); assert(!(await typing(v.p))); await v.p.waitForTimeout(1800); await v.p.getByRole('menuitemradio',{name:'Todo',exact:true}).tap(); await expect.poll(async()=> (await api(branch,'/issues/'+statusId)).status).toBe('todo'); await v.p.waitForTimeout(1800);
    for(const action of ['filter','next','select','range','labels']) {await strip(v.p,action).tap(); assert.equal(await typing(v.p),action==='filter'); await v.p.waitForTimeout(1800);}
    await v.p.keyboard.press('Escape'); await expect(v.p.getByRole('menu')).toHaveCount(0); await strip(v.p,'open').tap(); await strip(v.p,'comment').tap(); assert(await typing(v.p)); await v.p.waitForTimeout(1800); await strip(v.p,'gates').tap(); assert(!(await typing(v.p)));
    await v.p.waitForTimeout(Math.max(1000,34000-(Date.now()-started))); const video=v.p.video(); await v.ctx.close(); await video.saveAs(out+'/c10-phone-flow.webm');
    return {url:branch.url,seconds:(Date.now()-started)/1000,video:out+'/c10-phone-flow.webm',phoneHandoff:'Instance is attempt-scoped; token kept private per verification skill.'};
  });
  await b.ctx.close(); await m.ctx.close(); await browser.close();
})().catch(e=>{console.error(e);process.exitCode=1;});
