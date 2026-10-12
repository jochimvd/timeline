/** Repository names as a folder tree, for the repositories list. */
export type RepoNode = {
  /** One path segment, or several joined by "/" where a folder holds only one folder. */
  label: string;
  /** Full relative path, unique among siblings' subtrees. */
  key: string;
  /** Set when this folder is itself a repository, possibly with nested ones below. */
  path: string | null;
  children: RepoNode[];
  /** Repositories in this subtree, itself included. */
  count: number;
};

export function repositoryTree(repositories: { name: string; path: string }[]): RepoNode[] {
  const root: RepoNode = { label: "", key: "", path: null, children: [], count: 0 };
  for (const repository of repositories) {
    let node = root;
    for (const segment of repository.name.split("/").filter(Boolean)) {
      const key = node.key ? `${node.key}/${segment}` : segment;
      let child = node.children.find(c => c.label === segment);
      if (!child) node.children.push(child = { label: segment, key, path: null, children: [], count: 0 });
      node = child;
    }
    if (node !== root) node.path = repository.path;
  }
  return finish(root).children;
}

function finish(node: RepoNode): RepoNode {
  node.children = node.children.map(finish).sort((a, b) =>
    Number(!a.children.length) - Number(!b.children.length) || a.label.localeCompare(b.label));
  node.count = (node.path ? 1 : 0) + node.children.reduce((sum, child) => sum + child.count, 0);
  // A plain folder holding a single folder reads better as one row.
  const only = node.children[0];
  if (node.label && !node.path && node.children.length === 1 && !only.path && only.children.length)
    return { ...only, label: `${node.label}/${only.label}` };
  return node;
}

/** Keys of folders to open at first: enough to see something without unfolding everything. */
export function initiallyOpen(nodes: RepoNode[], budget = 30): Set<string> {
  const open = new Set<string>();
  let shown = nodes.length, level = nodes;
  while (level.length) {
    const folders = level.filter(node => node.children.length);
    const more = folders.reduce((sum, node) => sum + node.children.length, 0);
    if (!folders.length || shown + more > budget) break;
    folders.forEach(node => open.add(node.key));
    shown += more;
    level = folders.flatMap(node => node.children);
  }
  return open;
}
