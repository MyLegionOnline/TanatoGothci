// Headless layout verification for the new 420x640 two-pane layout.
// Serves src/ statically, opens it at 420x640 (window chrome is the OS's
// job; the webview is the viewport), and checks the acceptance criteria.
import http from "http";
import fs from "fs";
import path from "path";
import { chromium } from "playwright";

const root = path.resolve("src");
const mime = { ".html": "text/html", ".css": "text/css", ".js": "text/javascript" };
const srv = http.createServer((req, res) => {
  const p = path.join(root, req.url === "/" ? "index.html" : req.url.split("?")[0]);
  fs.readFile(p, (err, data) => {
    if (err) { res.writeHead(404); res.end("nf"); return; }
    res.writeHead(200, { "Content-Type": mime[path.extname(p)] || "application/octet-stream" });
    res.end(data);
  });
});

await new Promise((r) => srv.listen(0, r));
const port = srv.address().port;

// Minimal __TAURI__ stub so main.js boots outside Tauri.
const stub = `
window.__TAURI__ = {
  core: { invoke: async () => ({}) },
  event: { listen: async () => {} },
};
`;

const browser = await chromium.launch();
const page = await browser.newPage({ viewport: { width: 420, height: 640 } });

const results = {};
page.on("console", (m) => { if (m.type() === "error") console.log("CONSOLE ERR:", m.text()); });
page.on("pageerror", (e) => console.log("PAGE ERR:", e.message));

await page.goto(`http://localhost:${port}/`);
await page.evaluate(stub);
await page.reload();
await page.waitForTimeout(200);

results.sessionVisibleByDefault = await page.evaluate(() => {
  const s = document.getElementById("screen-session");
  return s.classList.contains("active") && getComputedStyle(s).display === "flex";
});
results.twoPanesVisible = await page.evaluate(() => {
  const r = document.getElementById("reasoning-log").getBoundingClientRect();
  const a = document.getElementById("ascii-art").getBoundingClientRect();
  return r.top < a.top && r.bottom <= a.top + 1 && r.height > 0 && a.height > 0
    && r.width === a.width;
});
results.paneHeights = await page.evaluate(() => {
  const r = document.getElementById("reasoning-log").getBoundingClientRect();
  const a = document.getElementById("ascii-art").getBoundingClientRect();
  return { reasoning: Math.round(r.height), ascii: Math.round(a.height) };
});
results.noHorizOverflow = await page.evaluate(() =>
  document.body.scrollWidth <= 420 && document.body.scrollHeight <= 640
);
results.asciiNoWrap = await page.evaluate(() => {
  const cs = getComputedStyle(document.getElementById("ascii-art"));
  return cs.whiteSpace === "pre" && cs.fontFamily.toLowerCase().includes("monospace");
});
results.reasoningWraps = await page.evaluate(() => {
  const cs = getComputedStyle(document.getElementById("reasoning-log"));
  return cs.whiteSpace === "pre-wrap" && cs.overflowWrap === "break-word";
});
results.reasoningScrolls = await page.evaluate(() => {
  const el = document.getElementById("reasoning-log");
  const before = el.scrollTop;
  el.scrollTop = 9999;
  return el.scrollHeight > el.clientHeight; // has potential to scroll
});

// Streaming a lot of text must not push the ASCII pane or change its height.
await page.evaluate(() => {
  for (let i = 0; i < 200; i++) {
    const s = document.createElement("span");
    s.textContent = "line of reasoning text number " + i + " padding to force wrapping and overflow";
    document.getElementById("reasoning-log").appendChild(s);
    document.getElementById("reasoning-log").appendChild(document.createTextNode("\n"));
  }
});
await page.evaluate(() => { document.getElementById("reasoning-log").scrollTop = 99999; });
results.asciiStable = await page.evaluate(() => {
  const a = document.getElementById("ascii-art").getBoundingClientRect();
  return Math.abs((document.getElementById("reasoning-log").getBoundingClientRect().bottom) - a.top) <= 2
    && a.height > 0;
});
results.reasoningScrolledToBottom = await page.evaluate(() => {
  const el = document.getElementById("reasoning-log");
  return Math.abs(el.scrollTop + el.clientHeight - el.scrollHeight) <= 2;
});

// Tab switching still works for settings, and back to session.
await page.click("#tab-settings");
results.settingsTabWorks = await page.evaluate(() => {
  const s = document.getElementById("screen-settings");
  const v = getComputedStyle(s).display;
  const sess = getComputedStyle(document.getElementById("screen-session")).display;
  return s.classList.contains("active") && v === "block" && sess === "none";
});
await page.click("#tab-session");
results.backToSession = await page.evaluate(() =>
  getComputedStyle(document.getElementById("screen-session")).display === "flex"
);

// Screenshot for the record.
await page.screenshot({ path: "layout-verification.png" });

console.log(JSON.stringify(results, null, 2));
const fail = Object.entries(results).filter(([k, v]) => v !== true && k !== "paneHeights");
console.log(fail.length ? "FAIL: " + fail.map(([k]) => k).join(", ") : "ALL CHECKS PASSED");
await browser.close();
srv.close();
process.exit(fail.length ? 1 : 0);
