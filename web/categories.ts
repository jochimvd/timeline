/** The category hierarchy as the views show it: top-level categories with their
 *  subcategories rolled up, or each category in full detail. */
import { categorySwatch, UNCATEGORIZED, type Swatch } from "./format";
import type { ColorBy } from "./preferences";

export type CategoryNode = { name: string; parent?: string | null; color?: string | null; palette?: number | null };
export type CategoryLevel = "category" | "subcategory";

/** Categories at one level: `at` maps a classified category (null: uncategorized)
 *  to the one shown, and `order` lists the shown ones in tree order. */
export type CategoryGrouping = { order: string[]; at: (name: string | null) => string | null };

/** The category and its ancestors, outermost first. */
export function categoryPath(nodes: CategoryNode[], name: string): string[] {
  const parents = new Map(nodes.map(node => [node.name, node.parent ?? null]));
  const path: string[] = [];
  // Bounded like the server's nesting limit, in case of inconsistent input.
  for (let current: string | null = name; current !== null && path.length < 8; current = parents.get(current) ?? null)
    path.unshift(current);
  return path;
}

/** "Work › Acme › Project X", or "Uncategorized". */
export const categoryLabel = (nodes: CategoryNode[], name: string | null) =>
  name === null ? UNCATEGORIZED : categoryPath(nodes, name).join(" › ");

/** Configured categories of overview rows, without the uncategorized row. */
export const categoryNodes = (rows: { category: string | null; parent: string | null; color?: string | null; palette?: number | null }[]): CategoryNode[] =>
  rows.flatMap(row => row.category === null ? [] : [{ name: row.category, parent: row.parent, color: row.color, palette: row.palette }]);

/** Swatches by category name (null: uncategorized). Subcategories that carry
 *  their parent's color get a distinct shade of it, in sibling order. */
export function categorySwatches(nodes: CategoryNode[]): (name: string | null) => Swatch {
  const byName = new Map(nodes.map(node => [node.name, node]));
  const shades = new Map<string, number>();
  const inheriting = new Map<string, number>();
  for (const node of nodes) {
    const parent = node.parent ? byName.get(node.parent) : undefined;
    if (!parent || node.color !== parent.color || node.palette !== parent.palette) continue;
    const shade = (inheriting.get(parent.name) ?? 0) + 1;
    inheriting.set(parent.name, shade);
    // Shade relative to the parent's own shade, so nested families stay apart.
    shades.set(node.name, (shades.get(parent.name) ?? 0) + shade);
  }
  return name => categorySwatch(name === null ? undefined : byName.get(name), name === null ? 0 : shades.get(name) ?? 0);
}

export const isNested = (nodes: CategoryNode[]) => nodes.some(node => node.parent);

/** Grouping at `level`, given categories in tree order. */
export function categoryGrouping(nodes: CategoryNode[], level: CategoryLevel): CategoryGrouping {
  if (level === "subcategory") return { order: nodes.map(node => node.name), at: name => name };
  return {
    order: nodes.filter(node => !node.parent).map(node => node.name),
    at: name => name === null ? null : categoryPath(nodes, name)[0],
  };
}

/** What views group by: applications without categories, and the top level
 *  for a remembered "subcategory" once no category is nested. */
export function grouping(choice: ColorBy, nodes: CategoryNode[]): ColorBy {
  if (!nodes.length) return "application";
  return choice === "subcategory" && !isNested(nodes) ? "category" : choice;
}
