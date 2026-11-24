/* global process, console, fetch, setTimeout, WebSocket */
// Evaluates one expression in a WebView2 page through the DevTools Protocol
// and prints the JSON result. Used by validate-settings-ui.ps1 against a
// debug build started with --remote-debugging-port. Node 22+ (global
// WebSocket and fetch).
//
//   node scripts/cdp-eval.mjs <port> <url-substring> <expression | ->
// With "-", the expression is read from standard input.
const [port, match, argument] = process.argv.slice(2);
const expression =
  argument === "-"
    ? await new Promise((resolve) => {
        let text = "";
        process.stdin.setEncoding("utf8");
        process.stdin.on("data", (chunk) => (text += chunk));
        process.stdin.on("end", () => resolve(text));
      })
    : argument;
if (!port || !match || !expression) {
  console.error("usage: cdp-eval.mjs <port> <url-substring> <expression>");
  process.exit(2);
}

async function target() {
  for (let attempt = 0; attempt < 50; attempt++) {
    try {
      const pages = await (await fetch(`http://127.0.0.1:${port}/json`)).json();
      // "=<url>" matches the whole URL; anything else, a substring.
      const fits = (url) =>
        match.startsWith("=") ? url === match.slice(1) : url.includes(match);
      const page = pages.find(
        (entry) => entry.type === "page" && fits(entry.url),
      );
      if (page) return page.webSocketDebuggerUrl;
    } catch {
      // The browser process may still be starting.
    }
    await new Promise((resolve) => setTimeout(resolve, 200));
  }
  throw new Error(`no page matching ${match}`);
}

async function evaluate() {
  const socket = new WebSocket(await target());
  await new Promise((resolve, reject) => {
    socket.onopen = resolve;
    socket.onerror = reject;
  });
  socket.send(
    JSON.stringify({
      id: 1,
      method: "Runtime.evaluate",
      params: { expression, awaitPromise: true, returnByValue: true },
    }),
  );
  const reply = await new Promise((resolve) => {
    socket.onmessage = (event) => {
      const message = JSON.parse(event.data);
      if (message.id === 1) resolve(message);
    };
  });
  socket.close();
  return reply;
}

// The first evaluation may land in the document that the initial navigation
// replaces; only that case is retried. Any other error fails loudly.
let reply;
for (let attempt = 0; attempt < 10; attempt++) {
  reply = await evaluate();
  if (!/context was destroyed/i.test(reply.error?.message ?? "")) break;
  await new Promise((resolve) => setTimeout(resolve, 300));
}
if (reply.error) {
  console.error(reply.error.message);
  process.exit(1);
}
if (reply.result?.exceptionDetails) {
  console.error(reply.result.exceptionDetails.text);
  process.exit(1);
}
console.log(JSON.stringify(reply.result?.result?.value ?? null));
