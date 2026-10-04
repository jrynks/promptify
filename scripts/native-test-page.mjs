import { createServer } from "node:http";

export async function openNativeTestPage(browser, title, label) {
  const server = createServer((_request, response) => {
    response.writeHead(200, { "Content-Type": "text/html; charset=utf-8", "Cache-Control": "no-store" });
    response.end("<!doctype html><title>Native test</title><label><span></span><textarea rows='20' cols='80'></textarea></label>");
  });
  await new Promise((accept, reject) => {
    server.once("error", reject);
    server.listen(0, "127.0.0.1", accept);
  });
  const close = () => new Promise((accept, reject) => server.close((error) => error ? reject(error) : accept()));
  try {
    const address = server.address();
    if (!address || typeof address === "string") throw new Error("The native test server did not bind a TCP port.");
    const page = await browser.newPage();
    await page.goto(`http://127.0.0.1:${address.port}`);
    await page.evaluate(({ title, label }) => {
      document.title = title;
      document.querySelector("span").textContent = label;
      document.querySelector("textarea").setAttribute("aria-label", label);
    }, { title, label });
    return { page, close };
  } catch (error) {
    await close();
    throw error;
  }
}
