import { defineConfig, type Plugin } from "vite";
import * as fs from "node:fs";
import * as path from "node:path";
import { createHash, randomUUID } from "node:crypto";

// The ID is assigned before rendering; it is not a circular content hash or
// supply-chain attestation. Only the explicit web-release mode changes URLs.
function releasePlugin(id: string): Plugin {
  let out: string;
  return {
    name: "aperture-versioned-ui",
    apply: "build",
    configResolved(config) {
      out = path.resolve(config.root, config.build.outDir);
      // Exclusive destination, including dangling links; never empty an old build.
      fs.mkdirSync(out, { mode: 0o700 });
    },
    transformIndexHtml() {
      return [{ tag: "meta", attrs: { name: "aperture-ui-id", content: id }, injectTo: "head" }];
    },
    closeBundle: {
      order: "post", sequential: true,
      handler() {
        // Read the actual final output, including Vite's copied public assets,
        // not the earlier generateBundle representation.
        const files: { path: string; bytes: number; sha256: string }[] = [];
        const seen = new Map<string, string>();
        let entries = 1, total = 0; // reserve the manifest entry
        function walk(dir: string, prefix = "") {
          for (const name of fs.readdirSync(dir).sort()) {
            const rel = prefix + name;
            if (++entries > 4096 || rel.length > 512 || rel.split("/").length > 8 ||
                !rel.split("/").every(p => /^[A-Za-z0-9_.-]{1,128}$/.test(p) && p !== "." && p !== "..") ||
                rel.toLowerCase() === "ui.json" || seen.has(rel.toLowerCase())) throw Error("Invalid UI tree");
            seen.set(rel.toLowerCase(), rel);
            const full = path.join(dir, name), st = fs.lstatSync(full);
            if (st.isSymbolicLink() || (st.mode & 0o7022) || st.uid !== process.geteuid!()) throw Error("Unsafe UI entry");
            if (st.isDirectory()) { walk(full, rel + "/"); continue; }
            if (!st.isFile() || st.nlink !== 1 || st.size > 8 * 1024 * 1024 ||
                !(rel === "index.html" || /\.(js|css|svg|png|woff2)$/.test(rel)) ||
                (rel === "index.html" && st.size > 256 * 1024)) throw Error("Invalid UI file");
            const bytes = fs.readFileSync(full);
            if (bytes.length !== st.size) throw Error("UI changed during build");
            total += bytes.length;
            if (total > 32 * 1024 * 1024 || files.length >= 512) throw Error("UI capacity exceeded");
            files.push({ path: rel, bytes: bytes.length, sha256: createHash("sha256").update(bytes).digest("hex") });
          }
        }
        walk(out); files.sort((a,b) => a.path < b.path ? -1 : a.path > b.path ? 1 : 0);
        if (!files.some(f => f.path === "index.html")) throw Error("Missing UI index");
        const manifest = JSON.stringify({ schema_version: 1, ui_id: id, api_schema: 1, files });
        if (Buffer.byteLength(manifest) > 128 * 1024) throw Error("UI manifest capacity exceeded");
        fs.writeFileSync(path.join(out, "UI.json"), manifest, { flag: "wx", mode: 0o600 });
      },
    },
  };
}

export default defineConfig(({ mode }) => {
  const host = process.env.TAURI_DEV_HOST;
  const id = mode === "web-release" ? randomUUID().replaceAll("-", "") : null;
  return {
    clearScreen: false,
    ...(id ? { base: `/ui/${id}/`, plugins: [releasePlugin(id)], build: { emptyOutDir: false } } : {}),
    define: { __APERTURE_WEB_BUILD__: JSON.stringify(id ? { ui_id: id, api_schema: 1 } : null) },
    server: {
      host: host || false, port: 1420, strictPort: true,
      hmr: host ? { protocol: "ws", host, port: 1421 } : undefined,
      watch: { ignored: ["**/src-tauri/**"] },
    },
  };
});
