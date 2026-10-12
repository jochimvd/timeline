import { test } from "node:test";
import assert from "node:assert/strict";
import { build } from "esbuild";

const built = await build({ entryPoints: ["web/repoTree.ts"], bundle: true, write: false, format: "esm", platform: "browser" });
const tree = await import("data:text/javascript;base64," + Buffer.from(built.outputFiles[0].text).toString("base64"));
const repos = names => names.map(name => ({ name, path: "/work/" + name }));
const shape = nodes => nodes.map(node => node.children.length
  ? [node.label, node.count, node.path ? "repo" : "folder", shape(node.children)]
  : node.label);

test("repository names nest as folders, with single-folder chains joined", () => {
  const nodes = tree.repositoryTree(repos([
    "acme/repos/clients/a/api", "acme/repos/clients/a/web", "acme/repos/internal/ci",
    "acme/repos/pipelines", "acme/repos/pipelines/repos/invoice", "notes",
  ]));
  assert.deepEqual(shape(nodes), [
    ["acme/repos", 5, "folder", [
      ["clients/a", 2, "folder", ["api", "web"]],
      ["internal", 1, "folder", ["ci"]],
      // A repository holding nested repositories stays a repository, and its lone folder still joins.
      ["pipelines", 2, "repo", [["repos", 1, "folder", ["invoice"]]]],
    ]],
    "notes",
  ]);
  assert.equal(nodes[0].children[0].key, "acme/repos/clients/a");
  assert.equal(nodes[1].path, "/work/notes");
});

test("folders open level by level while the list stays short", () => {
  const nodes = tree.repositoryTree(repos(["a/x/1", "a/x/2", "a/y/1", "b/1", ...Array.from({ length: 40 }, (_, i) => `a/y/many/${i}`)]));
  // Opening "many" would add 40 rows, so the unfolding stops above it.
  assert.deepEqual([...tree.initiallyOpen(nodes)], ["a", "b", "a/x", "a/y"]);
  assert.deepEqual([...tree.initiallyOpen(nodes, 2)], []);
});
