import { build } from "esbuild";
import { compile } from "svelte/compiler";
import { readFile } from "node:fs/promises";
import { dirname, join } from "node:path";

// Keep one self-contained module at the path embedded by the Rust server.
await build({
  entryPoints: ["web/app.ts"],
  outfile: join(process.argv[2] || "web", "app.js"),
  bundle: true,
  format: "esm",
  platform: "browser",
  conditions: ["svelte", "browser"],
  target: "es2022",
  minify: true,
  legalComments: "inline",
  plugins: [{
    name: "svelte",
    setup(build) {
      build.onLoad({ filter: /\.svelte$/ }, async ({ path }) => {
        const result = compile(await readFile(path, "utf8"), { filename: path, generate: "client", css: "injected" });
        if (result.warnings.length) throw new Error(result.warnings.map(warning => warning.message).join("\n"));
        return { contents: result.js.code, loader: "js", resolveDir: dirname(path) };
      });
    },
  }],
});
