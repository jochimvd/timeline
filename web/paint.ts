/** How every view labels and colors focus: by application, top-level category
 *  or most specific category, following the "Color by" choice. */
import { categoryGrouping, categoryLabel, categorySwatches, type CategoryNode } from "./categories";
import { applicationSwatch, UNCATEGORIZED, type Swatch } from "./format";
import type { ColorBy } from "./preferences";
import type { FocusSession } from "./types";

export type Paint = { key: string; label: string; swatch: Swatch; /** Full category path or application, for tooltips. */ detail: string };
export type Painter = ReturnType<typeof painter>;

export function painter(choice: ColorBy, nodes: CategoryNode[]) {
  const mode: "application" | "category" = choice === "application" || !nodes.length ? "application" : "category";
  const swatchOf = categorySwatches(nodes);
  const shown = categoryGrouping(nodes, choice === "subcategory" ? "subcategory" : "category");
  /** The shown category of a classified one (null: uncategorized). */
  const category = (name: string | null): Paint => {
    const at = shown.at(name);
    return { key: "category:" + (at ?? ""), label: at ?? UNCATEGORIZED, swatch: swatchOf(at), detail: categoryLabel(nodes, name) };
  };
  const application = (name: string): Paint => ({ key: "application:" + name, label: name, swatch: applicationSwatch(name), detail: name });
  return {
    mode,
    /** Categories at the shown level, for per-day series. */
    grouping: shown,
    category,
    application,
    /** Focus on a window of `app` classified as `cat`. */
    focus: (app: string, cat: string | null) => mode === "category" ? category(cat) : application(app),
    /** A session follows its subject; by application, its most active window's. */
    session(session: FocusSession): Paint {
      const subject = session.subject;
      if (mode === "category") return category(subject.kind === "category" ? subject.name : null);
      return application(subject.kind === "application" ? subject.name : session.windows[0]?.application ?? subject.name);
    },
    /** A stacked series' color: its category, application, or neutral for the rest. */
    subject(subject: { category?: string | null; application?: string }): Swatch {
      return subject.category !== undefined ? category(subject.category).swatch
        : subject.application !== undefined ? application(subject.application).swatch
        : { fill: "var(--uncategorized)", ink: "var(--uncategorized-ink)" };
    },
    /** The name a session is about, whatever the color mode. */
    sessionTitle(session: FocusSession) {
      return session.subject.kind === "category" ? categoryLabel(nodes, session.subject.name) : session.subject.name;
    },
  };
}

/** A letter avatar for an application: its first letter on its color. */
export const initial = (application: string) => (application.match(/[\p{L}\p{N}]/u)?.[0] ?? "?").toUpperCase();
