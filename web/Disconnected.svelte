<script lang="ts">
  import Icon, { type IconName } from "./Icon.svelte";
  import type { Lost } from "./connection";
  import { notify } from "./toasts";
  let { reason }: { reason: Lost } = $props();
  const copy: Record<Lost, { icon: IconName; title: string; text: string; command: string; after: string }> = {
    expired: {
      icon: "lock", title: "Session expired", text: "Browser sessions last 12 hours. Start a new one from your terminal:",
      command: "timeline open", after: "This tab reconnects on its own once you do.",
    },
    link_used: {
      icon: "lock", title: "This link was already used", text: "Each launch link works once, within a minute. Get a new one from your terminal:",
      command: "timeline open", after: "This tab reconnects on its own once you do.",
    },
    unreachable: {
      icon: "power", title: "Can't reach timeline", text: "The recorder isn't answering. Check whether it's running:",
      command: "timeline status", after: "This tab reconnects on its own once it's back.",
    },
  };
  const shown = $derived(copy[reason]);
  async function copyCommand() {
    try { await navigator.clipboard.writeText(shown.command); notify("Copied to clipboard."); }
    catch { notify("Couldn't copy. Select the command instead.", { tone: "error", key: "copy" }); }
  }
</script>

<!-- Stands in for the whole view: without a server there is nothing to show or pick. -->
<section id="disconnected" class="disconnected" role="alert" aria-labelledby="disconnected-title">
  <span class="mark"><Icon name={shown.icon} size={22} /></span>
  <h1 id="disconnected-title">{shown.title}</h1>
  <p>{shown.text}</p>
  <div class="command">
    <span class="prompt" aria-hidden="true">$</span><code>{shown.command}</code>
    <button class="btn small icon ghost" aria-label="Copy command" title="Copy command" onclick={() => void copyCommand()}>
      <Icon name="copy" size={14} />
    </button>
  </div>
  <small><span class="waiting" aria-hidden="true"></span>{shown.after}</small>
</section>

<style>
  .disconnected {
    display: flex; flex-direction: column; align-items: center; gap: 10px; max-width: 440px; margin: auto; padding: 48px 16px;
    text-align: center; color: var(--text-2);
  }
  .mark {
    display: grid; place-items: center; width: 48px; height: 48px; margin-bottom: 4px; border-radius: 50%;
    background: color-mix(in oklab, var(--bad), transparent 86%); color: var(--bad);
  }
  h1 { margin: 0; font-size: 18px; font-weight: 650; letter-spacing: -0.01em; color: var(--text); }
  p { margin: 0; font-size: 13.5px; line-height: 1.5; text-wrap: balance; }
  .command {
    display: flex; align-items: center; gap: 8px; margin: 6px 0 4px; padding: 4px 4px 4px 12px;
    border: 1px solid var(--border-strong); border-radius: var(--radius-sm); background: var(--sunken);
  }
  .command code { margin-right: 6px; font-size: 13.5px; color: var(--text); user-select: all; }
  .prompt { font-family: ui-monospace, monospace; font-size: 13px; color: var(--faint); user-select: none; }
  small { display: inline-flex; align-items: center; gap: 8px; font-size: 12px; color: var(--muted); }
  .waiting { width: 6px; height: 6px; border-radius: 50%; background: var(--faint); animation: wait 1.6s ease-in-out infinite; }
  @keyframes wait { 50% { opacity: 0.25; } }
  @media (prefers-reduced-motion: reduce) { .waiting { animation: none; } }
</style>
