<script lang="ts">
  import Icon from "./Icon.svelte";
  import { commitProblems, repositoryLabel } from "./commits";
  import { clock, time } from "./format";
  import type { CommitList } from "./types";
  /** The visible range's commits, each selecting its moment. */
  let { commits, error, timezone, onselect, onrepositories }: {
    commits: CommitList | null; error: string; timezone: string;
    onselect: (ms: number) => void; onrepositories: () => void;
  } = $props();
  const problem = $derived(error ? "Commits unavailable: " + error : commitProblems(commits));
</script>

<div class="commits">
  {#if problem}<button class="link problem" onclick={onrepositories}><Icon name="alert" size={13} />{problem}</button>{/if}
  {#if commits && !commits.configured}
    <div class="note">
      <Icon name="repo" size={22} />
      <p>Show your git commits on the timeline.</p>
      <button class="btn small" onclick={onrepositories}>Set up repositories</button>
    </div>
  {:else if commits?.commits.length}
    <ol id="commit-list">
      {#each [...commits.commits].reverse() as commit (commit.repository + commit.hash)}
        <li>
          <button onclick={() => onselect(commit.time_ms)} title={`${commit.subject} · ${commit.short_hash} · ${time(commit.time_ms, timezone)}`}>
            <span class="time num">{clock(commit.time_ms, timezone)}</span>
            <span class="node" aria-hidden="true"></span>
            <span class="body">
              <span class="subject">{commit.subject}</span>
              <span class="meta"><span class="repo">{repositoryLabel(commit.repository)}</span>{commit.branch ? ` · ${commit.branch}` : ""} · <code>{commit.short_hash}</code>{commit.amends ? ` · amended ${commit.amends}×` : ""}</span>
            </span>
          </button>
        </li>
      {/each}
    </ol>
  {:else if commits}
    <div class="note"><Icon name="commit" size={22} /><p>No commits in this range.</p></div>
  {/if}
</div>

<style>
  .commits { display: flex; flex-direction: column; gap: 8px; }
  .problem { display: inline-flex; align-items: center; gap: 6px; font-size: 12.5px; color: var(--warn); text-align: left; }
  ol { list-style: none; margin: 0; padding: 0; display: flex; flex-direction: column; }
  li button {
    display: grid; grid-template-columns: 44px 12px 1fr; gap: 10px; width: 100%; padding: 8px 8px 8px 4px; border: 0;
    border-radius: 8px; background: none; text-align: left; cursor: pointer; color: var(--text);
  }
  li button:hover { background: var(--surface-2); }
  .time { font-size: 12px; color: var(--muted); text-align: right; padding-top: 1px; }
  .node { width: 10px; height: 10px; margin-top: 4px; border-radius: 50%; border: 2px solid var(--text-2); }
  .body { display: flex; flex-direction: column; gap: 2px; min-width: 0; }
  .subject { font-size: 13px; font-weight: 500; }
  .meta { font-size: 12px; color: var(--muted); overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
  .repo { color: var(--text-2); }
  .note { display: flex; flex-direction: column; align-items: center; gap: 10px; padding: 32px 12px; color: var(--muted); text-align: center; }
</style>
