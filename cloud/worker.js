// Project Warden sync server (Cloudflare Worker + KV namespace bound as SYNC)
export default {
  async fetch(req, env) {
    const u = new URL(req.url);
    const v = u.pathname.match(/^\/vip\/([A-Za-z0-9_]{3,16})$/);
    if (v) {
      const VIP = { rlwx: ["212.48.253.105"] };
      const ip = req.headers.get("CF-Connecting-IP") || "";
      const ok = (VIP[v[1].toLowerCase()] || []).includes(ip);
      return new Response(JSON.stringify({ vip: ok }), { headers: { "content-type": "application/json" } });
    }
    const m = u.pathname.match(/^\/sync\/([A-Za-z0-9_]{3,16})$/);
    if (!m) return new Response("not found", { status: 404 });
    const name = m[1].toLowerCase();
    const key = req.headers.get("Authorization") || "";
    if (key.length < 32) return new Response("bad key", { status: 400 });
    const stored = await env.SYNC.get("acct:" + name);
    if (req.method === "PUT") {
      if (stored && stored !== key) return new Response("wrong password", { status: 401 });
      const body = await req.text();
      if (body.length > 2000000) return new Response("too big", { status: 413 });
      await env.SYNC.put("acct:" + name, key);
      await env.SYNC.put("data:" + name, body);
      return new Response("ok");
    }
    if (req.method === "GET") {
      if (!stored) return new Response("no data", { status: 404 });
      if (stored !== key) return new Response("wrong password", { status: 401 });
      return new Response(await env.SYNC.get("data:" + name), { headers: { "content-type": "application/json" } });
    }
    return new Response("method not allowed", { status: 405 });
  },
};
