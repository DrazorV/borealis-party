// Android smoke test: reads the app's page through its web view's DevTools (forwarded to 127.0.0.1:<port> by
// android-smoke.sh) until its text matches <pattern>, for up to 90 s. Node 22 (global fetch and WebSocket).
// Usage: node party/ci/android-smoke-page.mjs <port> <pattern>
const [port, pattern] = process.argv.slice(2);
const wanted = new RegExp(pattern, "i");
const deadline = Date.now() + 90_000;
let last = "";

function evaluate(url, expression) {
  return new Promise((resolve, reject) => {
    const ws = new WebSocket(url);
    const timer = setTimeout(() => { ws.close(); reject(new Error("no DevTools answer")); }, 10_000);
    ws.onopen = () => ws.send(JSON.stringify({ id: 1, method: "Runtime.evaluate", params: { expression, returnByValue: true } }));
    ws.onmessage = (m) => {
      const data = JSON.parse(String(m.data));
      if (data.id !== 1) return;
      clearTimeout(timer);
      ws.close();
      resolve(String(data.result?.result?.value ?? ""));
    };
    ws.onerror = () => { clearTimeout(timer); reject(new Error("DevTools socket error")); };
  });
}

while (Date.now() < deadline) {
  try {
    const pages = await (await fetch(`http://127.0.0.1:${port}/json`)).json();
    const page = pages.find((p) => p.type === "page" && p.webSocketDebuggerUrl);
    if (page) {
      last = `${page.url}\n${await evaluate(page.webSocketDebuggerUrl, "document.body ? document.body.innerText : ''")}`;
      if (wanted.test(last)) {
        console.log(`the app's first screen: ${last.slice(0, 300)}`);
        process.exit(0);
      }
    }
  } catch (e) {
    last = String(e);
  }
  await new Promise((r) => setTimeout(r, 2_000));
}
console.log(`::error::the app's first screen never showed /${pattern}/; last seen: ${last.slice(0, 500)}`);
process.exit(1);
