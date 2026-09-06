import { chromium } from '/Users/dmoore/.npm/_npx/e41f203b7505f1fb/node_modules/playwright/index.mjs';
const [file, out, w, h, wait] = process.argv.slice(2);
const browser = await chromium.launch();
const page = await browser.newPage({ viewport: { width: +w, height: +h }, deviceScaleFactor: 2 });
await page.goto('file://' + file, { waitUntil: 'load' });
await page.waitForTimeout(+wait);
await page.screenshot({ path: out });
await browser.close();
console.log('shot ->', out);
