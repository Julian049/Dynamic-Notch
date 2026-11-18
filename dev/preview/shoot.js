// Screenshots of the Ajustes pages for comparison with the Figma frames.
// From the repo root, with `python3 -m http.server 8765` running: playwright-cli run-code --filename=dev/preview/shoot.js
async (page) => {
  const base = 'http://127.0.0.1:8765/dev/preview/settings.html';
  const shots = [
    ['general', 'general', 1016, ''],
    ['elementos', 'elementos', 1361, ''],
    ['conexiones', 'conexiones', 762, ''],
    ['disabled', 'general', 1016, '&set=autohide:false'],
  ];
  for (const [name, p, h, extra] of shots) {
    await page.setViewportSize({ width: 1120, height: h });
    await page.goto(`${base}?page=${p}${extra}`);
    await page.waitForSelector('#status:not(:empty)');
    await page.evaluate(() => document.fonts.ready);
    await page.screenshot({ path: `dev/preview/shots/${name}.png` });
  }
}
