<script lang="ts">
  import { onDestroy } from "svelte";
  import { query } from "./api";
  import { clock } from "./format";
  import type { RepositoryList } from "./types";
  /** Where commits come from: configured git paths, repositories found and the
   *  problems that leave commits missing. Read on opening, or on Rescan. */
  let { settingsPath, timezone }: { settingsPath: string | null; timezone: string } = $props();
  let list = $state<RepositoryList>();
  let busy = $state(false), error = $state("");
  let controller: AbortController | undefined;
  async function load(refresh = false) {
    controller?.abort();
    const current = controller = new AbortController();
    busy = true; error = "";
    try { list = await query<RepositoryList>("repositories", { refresh }, current.signal); }
    catch (e) { if (!current.signal.aborted) error = e instanceof Error ? e.message : String(e); }
    finally { if (controller === current) busy = false; }
  }
  function toggle(event: Event) {
    if ((event.currentTarget as HTMLDetailsElement).open) void load();
    else { controller?.abort(); controller = undefined; busy = false; }
  }
  onDestroy(() => controller?.abort());
  const configured = $derived(!!list?.paths.length && !list.settings_problem);
</script>

{#snippet file()}{#if settingsPath}<code>{settingsPath}</code>{:else}the settings file{/if}{/snippet}

<details id="repositories" class="page-details" ontoggle={toggle}>
  <summary>Git repositories</summary>
  <div class="repository-body" aria-busy={busy}>
    {#if list?.settings_problem}
      <p class="error">Settings were ignored: {list.settings_problem}</p>
    {:else if list && !configured}
      <p>Commits from your git repositories can show on the timeline. List the directories to search in {@render file()}:</p>
      <pre><code>[git]
paths = ["~/work"]</code></pre>
    {:else if list}
      <dl>
        <dt>Searched</dt><dd>{list.paths.join(", ")} · {list.depth} levels deep</dd>
        <dt>Your commits</dt><dd>by each repository's <code>user.email</code>{list.authors.length ? ` or ${list.authors.join(", ")}` : ""}</dd>
        <dt>Found</dt>
        <dd id="repositories-found">{list.repositories.length} {list.repositories.length === 1 ? "repository" : "repositories"} in {list.directories.toLocaleString()} directories{list.scanned_ms ? ` · searched at ${clock(list.scanned_ms, timezone)}` : ""}{list.truncated ? " · stopped early: too many directories" : ""}</dd>
      </dl>
      {#if list.problems.length}
        <div>
          <p>Commits are missing from these, which could not be read:</p>
          <ul class="repository-problems" id="repository-problems">
            {#each list.problems as problem (problem.repository + problem.problem)}
              <li><code>{problem.repository}</code> · {problem.problem}</li>
            {/each}
          </ul>
        </div>
      {/if}
      {#if list.repositories.length}
        <details>
          <summary>Repositories ({list.repositories.length})</summary>
          <ul class="repository-names">
            {#each list.repositories as repository (repository.path)}<li title={repository.path}>{repository.name}</li>{/each}
          </ul>
        </details>
      {/if}
      <p class="muted">Defined in {@render file()}. Commits are read from the repositories when shown, never recorded or changed;
        times are when each commit was made on this machine. Repositories are searched again every five minutes.</p>
    {:else if busy}<p class="muted">Searching…</p>
    {/if}
    <div class="storage-caption">
      <span>{error || " "}</span>
      <button onclick={() => void load(true)} disabled={busy}>Rescan</button>
    </div>
  </div>
</details>
