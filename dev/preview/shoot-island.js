// Dev only. With the repo root served on :8791 (python3 -m http.server 8791) and a playwright-cli session open
// with deviceScaleFactor 2: playwright-cli run-code --filename=dev/preview/shoot-island.js
// Writes dev/preview/shots/<scenario>.png: the Figma variant, then classic, then Windows 11.
async (page) => {
  const dir = 'dev/preview/shots'; // relative to where playwright-cli was started (the repo root)
  const cdp = await page.context().newCDPSession(page);
  await cdp.send('Network.setCacheDisabled', { cacheDisabled: true });
  await page.goto('http://127.0.0.1:8791/dev/preview/index.html');
  await page.waitForTimeout(2500); // the width transition and the cover
  const names = await page.$$eval('section.case', (s) => s.map((e) => e.id));
  for (const name of names) await page.locator(`#${name}`).screenshot({ path: `${dir}/${name}.png`, scale: 'device' });
  return names.length;
}
