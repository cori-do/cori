// SPDX-License-Identifier: Apache-2.0
// A headless Chrome tab driven over the DevTools protocol (Node 22+, no dependency), for the e2e scripts whose page signs
// in through other origins before it renders (`--dump-dom` takes its snapshot at the first page's load):
//   node scripts/browse.mjs <port> <url> <text> [timeout ms]
// opens <url> in a new tab of the Chrome listening on <port> (`--remote-debugging-port`), waits until the page's text has
// <text> (20 s at most), and prints { url, text, html, console } as JSON: where the tab ended, its text and DOM, and what
// the browser logged on the way (a policy violation among them).
const [port, url, want, timeout = "20000"] = process.argv.slice(2);
const tab = await (await fetch(`http://127.0.0.1:${port}/json/new?about:blank`, { method: "PUT" })).json();
const ws = new WebSocket(tab.webSocketDebuggerUrl);
await new Promise((ok, ko) => { ws.onopen = ok; ws.onerror = ko; });
let last = 0;
const waiting = new Map(), logged = [];
ws.onmessage = (m) => {
  const msg = JSON.parse(m.data);
  if (msg.id) { waiting.get(msg.id)?.(msg); waiting.delete(msg.id); return; }
  const p = msg.params;
  if (msg.method === "Log.entryAdded") logged.push(`${p.entry.level}: ${p.entry.text}`);
  if (msg.method === "Runtime.consoleAPICalled") logged.push(`${p.type}: ${p.args.map((a) => a.value ?? a.description ?? "").join(" ")}`);
  if (msg.method === "Runtime.exceptionThrown") logged.push(`exception: ${p.exceptionDetails.exception?.description ?? p.exceptionDetails.text}`);
  if (msg.method === "Audits.issueAdded") logged.push(`issue: ${p.issue.code} ${JSON.stringify(p.issue.details)}`);
};
const send = (method, params = {}) => new Promise((ok) => { const id = ++last; waiting.set(id, ok); ws.send(JSON.stringify({ id, method, params })); });
// the page's own value of `expression`; undefined while the tab is between documents
const value = async (expression) => (await send("Runtime.evaluate", { expression, returnByValue: true })).result?.result?.value;
const text = async () => (await value("document.body ? document.body.innerText : ''")) ?? "";
for (const domain of ["Page", "Runtime", "Log", "Audits"]) await send(`${domain}.enable`);
await send("Page.navigate", { url });
for (const end = Date.now() + Number(timeout); Date.now() < end && !(await text()).includes(want);) await new Promise((r) => setTimeout(r, 250));
await new Promise((r) => setTimeout(r, 500));   // what the page loads once it shows (its images)
console.log(JSON.stringify({ url: await value("location.href"), text: await text(), html: (await value("document.documentElement.outerHTML")) ?? "", console: logged }));
ws.close();
