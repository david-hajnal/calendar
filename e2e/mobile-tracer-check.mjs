import { webkit } from 'playwright';
import { mkdir, writeFile } from 'node:fs/promises';
const browser = await webkit.launch();
const page = await browser.newPage({ viewport: { width: 390, height: 844 }, ignoreHTTPSErrors: true });
const mutations = [];
await page.route('**/api/**', async route => {
  if (route.request().method() !== 'GET') mutations.push(route.request().url());
  const body = route.request().url().endsWith('/auth/session')
    ? { user: { id: 7, email: 'person@example.test', display_name: 'Person', is_superadmin: false }, csrf_token: 'sample-csrf', created_at: 1, last_seen_at: 2, expires_at: 9999999999 }
    : [];
  await route.fulfill({ json: body });
});
await page.goto('https://127.0.0.1:5173/dashboard');
await page.getByRole('button', { name: 'Preview mobile Agenda' }).click();
await page.getByRole('heading', { name: 'Sample Agenda' }).waitFor();
const evidence = new URL('../docs/plans/mobile-usability/evidence', import.meta.url).pathname;
await mkdir(evidence, { recursive: true });
await page.screenshot({ path: `${evidence}/slice-01-agenda-390.png`, fullPage: true });
const preview = await page.locator('.mobile-calendar').evaluate(el => ({ width: el.getBoundingClientRect().width, viewport: innerWidth, font: getComputedStyle(el).fontSize, buttons: [...el.querySelectorAll('button')].map(b => b.getBoundingClientRect().height), focusedHeading: document.activeElement?.id }));
if (preview.width > preview.viewport || preview.buttons.some(h => h < 44) || preview.focusedHeading !== 'sample-agenda-heading') throw new Error(JSON.stringify(preview));
await page.evaluate(() => window.scrollTo(0, document.documentElement.scrollHeight));
const lastCardClearsNav = await page.locator('.mobile-calendar__events article').last().evaluate(el => el.getBoundingClientRect().bottom <= document.querySelector('.bottom-nav').getBoundingClientRect().top);
if (!lastCardClearsNav) throw new Error('Last sample card cannot clear navigation');
await page.screenshot({ path: `${evidence}/slice-01-agenda-390-scrolled.png` });
await page.getByRole('button', { name: 'Return to your calendar' }).click();
await page.getByRole('button', { name: 'Preview mobile Agenda' }).waitFor();
if (mutations.length) throw new Error('Unexpected mutations');
await writeFile(`${evidence}/slice-01-browser.json`, JSON.stringify({ browser: 'WebKit', width: 390, samplePreview: preview, returnVerified: true, lastCardClearsNav, mutations }, null, 2));
await browser.close();
