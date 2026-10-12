<script lang="ts">
  import Icon from "./Icon.svelte";
  import { query, request } from "./api";
  import { categorySwatches, type CategoryNode } from "./categories";
  import { drawer, type DrawerTab } from "./drawer";
  import { clock } from "./format";
  import { initiallyOpen, repositoryTree, type RepoNode } from "./repoTree";
  import { SvelteSet } from "svelte/reactivity";
  import type { CategoryList, RepositoryList } from "./types";
  let { categoryList, settingsPath, timezone }: { categoryList: CategoryList | null; settingsPath: string | null; timezone: string } = $props();
  type Storage = { total_bytes: number; pack_bytes: number; pack_count: number; database_bytes: number;
    map_bytes: number; map_uncompressed_bytes: number; captures: number; images: number; regions: number;
    tile_reuse_percent: number | null; thumbnail_cache_bytes: number; thumbnail_cache_limit: number;
    metadata_retention_days: number; screenshot_retention_days: number; measured_ms: number };
  let dialog: HTMLDialogElement;
  const tabs: { id: DrawerTab; label: string; icon: "tag" | "repo" | "database" | "info" }[] = [
    { id: "categories", label: "Categories", icon: "tag" },
    { id: "repositories", label: "Repositories", icon: "repo" },
    { id: "storage", label: "Storage", icon: "database" },
    { id: "guide", label: "Guide", icon: "info" },
  ];
  $effect(() => {
    const tab = $drawer;
    if (tab && !dialog.open) dialog.showModal();
    if (!tab && dialog.open) dialog.close();
    if (tab === "repositories" && !repositories && !repositoryBusy) void loadRepositories();
    if (tab === "storage" && (!storage || Date.now() - storage.measured_ms > 30_000) && !storageBusy) void loadStorage();
  });
  let repositories = $state<RepositoryList>(), repositoryBusy = $state(false), repositoryError = $state("");
  const tree = $derived(repositories ? repositoryTree(repositories.repositories) : []);
  const folders = $derived.by(() => {
    const keys: string[] = [];
    const walk = (nodes: RepoNode[]) => nodes.forEach(node => { if (node.children.length) { keys.push(node.key); walk(node.children); } });
    walk(tree);
    return keys;
  });
  // Folders the reader opened, kept across rescans; null until the first list arrives.
  let opened = $state<SvelteSet<string> | null>(null);
  let storage = $state<Storage>(), storageBusy = $state(false), storageError = $state("");
  let controller = new AbortController();
  async function loadRepositories(refresh = false) {
    repositoryBusy = true; repositoryError = "";
    try {
      repositories = await query<RepositoryList>("repositories", { refresh }, controller.signal);
      opened ??= new SvelteSet(initiallyOpen(repositoryTree(repositories.repositories)));
    }
    catch (e) { repositoryError = e instanceof Error ? e.message : String(e); }
    finally { repositoryBusy = false; }
  }
  async function loadStorage() {
    storageBusy = true; storageError = "";
    try { storage = await request<Storage>("/api/storage", undefined, controller.signal); }
    catch (e) { storageError = e instanceof Error ? e.message : "Storage details unavailable"; }
    finally { storageBusy = false; }
  }
  function closed() { drawer.set(null); controller.abort(); controller = new AbortController(); repositoryBusy = storageBusy = false; }
  function bytes(value: number | undefined) {
    if (value === undefined) return "—";
    const unit = value >= 1024 ** 3 ? 3 : value >= 1024 ** 2 ? 2 : value >= 1024 ? 1 : 0;
    return `${(value / 1024 ** unit).toFixed(unit ? 1 : 0)} ${["B", "KiB", "MiB", "GiB"][unit]}`;
  }
  const nodes = $derived<CategoryNode[]>(categoryList?.categories ?? []);
  const swatch = $derived(categorySwatches(nodes));
</script>

{#snippet branch(nodes: RepoNode[])}
  {#each nodes as node (node.key)}
    <li>
      {#if node.children.length}
        <details open={opened?.has(node.key)} ontoggle={event => {
          if (event.currentTarget.open) opened?.add(node.key); else opened?.delete(node.key);
        }}>
          <summary title={node.path ?? undefined}>
            <span class="twisty"><Icon name="chevron-right" size={13} /></span>
            <Icon name={node.path ? "repo" : "folder"} size={13} /><span class="ellipsis">{node.label}</span>
            <span class="count num">{node.count}</span>
          </summary>
          <ul class="tree">{@render branch(node.children)}</ul>
        </details>
      {:else}
        <span class="leaf" title={node.path}><Icon name="repo" size={13} /><span class="ellipsis">{node.label}</span></span>
      {/if}
    </li>
  {/each}
{/snippet}

{#snippet file()}{#if settingsPath}<code>{settingsPath}</code>{:else}the settings file{/if}{/snippet}

<dialog class="drawer" bind:this={dialog} onclose={closed} aria-label="Settings and details">
  <button class="scrim" aria-label="Close" tabindex="-1" onclick={() => drawer.set(null)}></button>
  <div class="sheet">
    <header>
      <h2>Settings &amp; details</h2>
      <button class="btn icon ghost" aria-label="Close" onclick={() => drawer.set(null)}><Icon name="x" /></button>
    </header>
    <nav class="segmented" aria-label="Sections">
      {#each tabs as tab (tab.id)}
        <button aria-pressed={$drawer === tab.id} onclick={() => drawer.set(tab.id)}><Icon name={tab.icon} size={14} />{tab.label}</button>
      {/each}
    </nav>
    <div class="content">
      {#if $drawer === "categories"}
        {#if categoryList?.problem}<p class="error">Settings were ignored: {categoryList.problem}</p>{/if}
        {#if nodes.length}
          <p class="muted">Rules classify focus by application and cleaned window title. The first matching rule in file order applies,
            at any level, with fallback rules last; a subcategory's time also counts toward its parents. Edit {@render file()} to change them;
            all history is reclassified.</p>
          <ul class="categories" id="category-rules">
            {#each categoryList!.categories as category (category.name)}
              <li style:margin-left={category.depth * 18 + "px"}>
                <div class="category-name"><span class="dot" style:background={swatch(category.name).fill}></span><strong>{category.name}</strong></div>
                {#each category.rules as rule, index (index)}
                  <div class="rule">
                    {#if rule.application}<span class="pill">app = <code>{rule.application}</code></span>{/if}
                    {#if rule.title}<span class="pill">title ~ <code>{rule.title}</code></span>{/if}
                    {#if rule.fallback}<span class="badge">fallback</span>{/if}
                  </div>
                {:else}<div class="rule muted">Only through subcategories</div>{/each}
              </li>
            {/each}
          </ul>
        {:else}
          <p>Categories group your focus into projects or kinds of work. Add them to {@render file()}:</p>
          <pre><code>[[categories]]
name = "Work"
rules = [{"{"} application = "code" {"}"}]

  [[categories.children]]
  name = "Client A"
  rules = [{"{"} title = "client-a" {"}"}]</code></pre>
        {/if}
      {:else if $drawer === "repositories"}
        <div aria-busy={repositoryBusy}>
          {#if repositories?.settings_problem}
            <p class="error">Settings were ignored: {repositories.settings_problem}</p>
          {:else if repositories && !repositories.paths.length}
            <p>Commits from your git repositories can show on the timeline. List the directories to search in {@render file()}:</p>
            <pre><code>[git]
paths = ["~/work"]</code></pre>
          {:else if repositories}
            <dl class="facts">
              <div><dt>Searched</dt><dd>{repositories.paths.join(", ")} · {repositories.depth} levels deep</dd></div>
              <div><dt>Your commits</dt><dd>by each repository's <code>user.email</code>{repositories.authors.length ? ` or ${repositories.authors.join(", ")}` : ""}</dd></div>
              <div><dt>Found</dt><dd id="repositories-found">{repositories.repositories.length} {repositories.repositories.length === 1 ? "repository" : "repositories"} in {repositories.directories.toLocaleString()} directories{repositories.scanned_ms ? ` · searched at ${clock(repositories.scanned_ms, timezone)}` : ""}{repositories.truncated ? " · stopped early: too many directories" : ""}</dd></div>
            </dl>
            {#if repositories.problems.length}
              <p>Commits are missing from these, which could not be read:</p>
              <ul class="plain" id="repository-problems">
                {#each repositories.problems as problem (problem.repository + problem.problem)}<li><code>{problem.repository}</code> · {problem.problem}</li>{/each}
              </ul>
            {/if}
            {#if tree.length}
              <div class="repos">
                {#if folders.length}
                  <div class="tree-actions">
                    <button class="btn small ghost" onclick={() => opened = new SvelteSet(folders)}>Expand all</button>
                    <button class="btn small ghost" onclick={() => opened = new SvelteSet()}>Collapse all</button>
                  </div>
                {/if}
                <ul class="tree" id="repository-tree">{@render branch(tree)}</ul>
              </div>
            {/if}
            <p class="muted">Commits are read from the repositories when shown, never recorded or changed; times are when each commit
              was made on this machine. Repositories are searched again every five minutes.</p>
          {:else if repositoryBusy}<p class="muted">Searching…</p>{/if}
          <div class="footer">
            <span class="error">{repositoryError}</span>
            <button class="btn small" onclick={() => void loadRepositories(true)} disabled={repositoryBusy}><Icon name="refresh" size={13} />Rescan</button>
          </div>
        </div>
      {:else if $drawer === "storage"}
        <div aria-busy={storageBusy}>
          <dl class="stats">
            <div><dt>Total</dt><dd>{bytes(storage?.total_bytes)}</dd></div>
            <div><dt>Image packs</dt><dd>{bytes(storage?.pack_bytes)}</dd></div>
            <div><dt>Database</dt><dd>{bytes(storage?.database_bytes)}</dd></div>
            <div><dt>Tile maps</dt><dd>{bytes(storage?.map_bytes)}</dd></div>
            <div><dt>Tile reuse</dt><dd>{storage?.tile_reuse_percent == null ? "—" : `${storage.tile_reuse_percent.toFixed(1)}%`}</dd></div>
            <div><dt>Preview cache</dt><dd>{bytes(storage?.thumbnail_cache_bytes)}</dd></div>
          </dl>
          {#if storage}
            <p class="muted">{storage.captures.toLocaleString()} captures · {storage.regions.toLocaleString()} regions. Screenshots are kept
              {storage.screenshot_retention_days} days and activity {storage.metadata_retention_days} days. Tile maps are part of the database;
              the preview cache is memory only ({bytes(storage.thumbnail_cache_limit)} limit).</p>
          {/if}
          <div class="footer">
            <span class="error">{storageError}</span>
            <button class="btn small" onclick={() => void loadStorage()} disabled={storageBusy}><Icon name="refresh" size={13} />Refresh</button>
          </div>
        </div>
      {:else if $drawer === "guide"}
        <div class="guide">
          <p>Timeline shows what was on your screen and which window had focus. It describes observed activity: not time spent on
            tasks, and not a measure of productivity.</p>
          <dl>
            <dt>Activity day</dt><dd>From 04:00 until 04:00 the next day, so late work stays with its day. Weeks start on Monday.</dd>
            <dt>Focus</dt><dd>Time a window had keyboard focus, including idle time. <em>Active</em> focus excludes idle periods,
              which begin after five minutes without input.</dd>
            <dt>Session</dt><dd>A continuous run of active focus on one category, or on one application when uncategorized. Glances
              away under a minute don't end it while they stay under a tenth of it. Idle time, pauses and switches do.</dd>
            <dt>Switches</dt><dd>Changes of application or category per hour of active focus, never across idle time or pauses.</dd>
            <dt>Deep focus</dt><dd>The share of active focus in sessions of 25 minutes or more.</dd>
            <dt>Hatching</dt><dd>Time without evidence: the recorder was paused, the screen locked, or the recorder not running.
              That is not the same as no activity.</dd>
            <dt>Comparisons</dt><dd>Weeks and months compare with the same number of days just before; a period in progress compares
              up to the same day and time.</dd>
          </dl>
          <p class="muted">Closing this page leaves recording running.</p>
        </div>
      {/if}
    </div>
  </div>
</dialog>

<style>
  .drawer { padding: 0; border: 0; margin: 0; max-width: none; max-height: none; width: 100vw; height: 100vh; background: transparent; color: var(--text); }
  .drawer::backdrop { background: rgb(0 0 0 / 0.45); }
  .scrim { position: fixed; inset: 0; border: 0; background: transparent; }
  .sheet {
    position: fixed; top: 0; right: 0; bottom: 0; width: min(520px, 100vw); display: flex; flex-direction: column; gap: 14px;
    padding: 16px 20px; background: var(--surface); border-left: 1px solid var(--border); box-shadow: var(--popover-shadow);
    animation: slide 0.18s ease-out;
  }
  @keyframes slide { from { transform: translateX(24px); opacity: 0; } }
  header { display: flex; align-items: center; justify-content: space-between; }
  h2 { font-size: 16px; font-weight: 650; }
  nav { align-self: flex-start; }
  .content { flex: 1; overflow: auto; display: flex; flex-direction: column; gap: 12px; font-size: 13.5px; color: var(--text-2); }
  .content > div { display: flex; flex-direction: column; gap: 12px; }
  pre { margin: 0; padding: 12px; border-radius: var(--radius-sm); background: var(--sunken); border: 1px solid var(--border); overflow: auto; }
  .error { color: var(--bad); }
  .categories { list-style: none; margin: 0; padding: 0; display: flex; flex-direction: column; gap: 10px; }
  .category-name { display: flex; align-items: center; gap: 8px; color: var(--text); }
  .rule { display: flex; flex-wrap: wrap; gap: 6px; margin: 4px 0 0 16px; font-size: 12px; }
  .facts { display: flex; flex-direction: column; gap: 10px; }
  .facts dt, .stats dt { font-size: 11.5px; color: var(--muted); }
  .facts dd { color: var(--text); }
  .plain { margin: 0; padding-left: 18px; }
  .repos { display: flex; flex-direction: column; gap: 4px; }
  .tree-actions { display: flex; gap: 4px; }
  .tree { list-style: none; margin: 0; padding: 0; }
  .tree .tree { margin-left: 12px; padding-left: 8px; border-left: 1px solid var(--border); }
  summary, .leaf {
    display: flex; align-items: center; gap: 6px; min-width: 0; height: 26px; padding: 0 6px; border-radius: 6px; color: var(--text);
  }
  summary { list-style: none; cursor: pointer; user-select: none; }
  summary::-webkit-details-marker { display: none; }
  summary:hover, .leaf:hover { background: var(--surface-2); }
  .leaf { padding-left: 25px; }
  .twisty { display: grid; color: var(--muted); transition: transform 0.12s; }
  details[open] > summary .twisty { transform: rotate(90deg); }
  .count { margin-left: auto; padding-left: 8px; font-size: 11.5px; color: var(--muted); }
  .stats { display: grid; grid-template-columns: repeat(3, 1fr); gap: 10px; }
  .stats > div { padding: 10px 12px; border-radius: var(--radius-sm); background: var(--surface-2); border: 1px solid var(--border); }
  .stats dd { font-size: 17px; font-weight: 600; color: var(--text); }
  .footer { display: flex; justify-content: space-between; align-items: center; gap: 12px; }
  .guide dl { display: grid; grid-template-columns: 110px 1fr; gap: 10px 14px; margin: 4px 0; }
  .guide dt { color: var(--text); font-weight: 600; }
</style>
