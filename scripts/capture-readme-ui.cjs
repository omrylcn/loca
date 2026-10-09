// Capture the actual Wiki renderer with synthetic data, never a private server.
const http = require('node:http');
const fs = require('node:fs');
const path = require('node:path');
const { chromium } = require('@playwright/test');
const root = path.resolve(__dirname, '..');
(async () => {
  const server = http.createServer((req, res) => {
    const pathname = decodeURIComponent(new URL(req.url, 'http://localhost').pathname);
    const file = path.resolve(root, 'web', pathname === '/' ? 'index.html' : '.' + pathname);
    if (!file.startsWith(path.join(root, 'web') + path.sep) || !fs.existsSync(file) || !fs.statSync(file).isFile()) {
      res.writeHead(404); return res.end();
    }
    res.setHeader('Content-Type', file.endsWith('.js') ? 'text/javascript' : file.endsWith('.css') ? 'text/css' : 'text/html');
    fs.createReadStream(file).pipe(res);
  });
  await new Promise(resolve => server.listen(0, '127.0.0.1', resolve));
  let browser;
  try {
    browser = await chromium.launch();
    const page = await browser.newPage({ viewport: { width: 1440, height: 960 }, deviceScaleFactor: 1 });
    await page.addInitScript(() => localStorage.setItem('loca-gs-seen', '1'));
    await page.route('**/rooms**', route => route.fulfill({ json: [] }));
    await page.route('**/rooms/demo/wiki', route => route.fulfill({ json: {
      room: 'demo', editor_name: 'writer', revision: 3, reviewed_through: 12,
      reviewed_at: 1791504000000, edited_at: 1791504000000,
      pages: [
        { slug: 'overview', title: 'Overview', body: '## What we are building\nA private space where humans and coding agents work together.\n\n### Decisions\n- One room, one shared Wiki.\n- Sources stay linked to the conversation.\n\n### Next\nReview proposals in the working area before carrying them into topic pages.', sources: [12] },
        { slug: 'working', title: 'Working area', body: 'Current proposals and open questions.', sources: [] },
        { slug: 'architecture', title: 'Architecture', body: 'Established design decisions.', sources: [] }
      ]
    } }));
    await page.goto(`http://127.0.0.1:${server.address().port}`);
    await page.evaluate(() => {
      document.body.classList.remove('locked');
      document.querySelector('.main').classList.remove('global');
      document.querySelector('.main').style.setProperty('display', 'flex', 'important');
      state.room = 'demo'; state.locaContext = 'demo';
      state.members = [{ name: 'writer', type: 'agent' }, { name: 'reviewer', type: 'agent' }];
      window.isLocaOperator = () => true;
      document.querySelectorAll('input').forEach(input => { if (input.value.includes('127.0.0.1')) input.value = 'http://127.0.0.1:8787'; });
      document.querySelector('#roomList').replaceChildren();
      switchTab('wiki');
    });
    await page.locator('#wikiPages h1').waitFor();
    await page.screenshot({ path: path.join(root, 'docs/loca-ui.png') });
  } finally { if (browser) await browser.close(); server.close(); }
})().catch(error => { console.error(error); process.exitCode = 1; });
