<script lang="ts">
  import type { GroupInvite } from "$lib/stores/chat";

  let {
    invite,
    onaccept = async (_invite: GroupInvite) => {},
    onretry = async (_invite: GroupInvite) => {},
  } = $props<{
    invite: GroupInvite;
    onaccept?: (invite: GroupInvite) => Promise<void>;
    onretry?: (invite: GroupInvite) => Promise<void>;
  }>();

  let busy = $state(false);
  let error = $state<string | null>(null);

  async function accept() {
    if (invite.status !== "ready" || busy) return;
    busy = true;
    error = null;
    try {
      await onaccept(invite);
    } catch (cause) {
      error = cause instanceof Error ? cause.message : String(cause);
    } finally {
      busy = false;
    }
  }

  async function retry() {
    if (busy) return;
    busy = true;
    error = null;
    try {
      await onretry(invite);
    } catch (cause) {
      error = cause instanceof Error ? cause.message : String(cause);
    } finally {
      busy = false;
    }
  }
</script>

<article class="rounded-xl border border-theme-base-700 bg-theme-base-900 p-4 shadow-lg">
  <div class="flex items-start justify-between gap-4">
    <div>
      <h3 class="font-semibold text-theme-base-100">{invite.groupName}</h3>
      <p class="mt-1 text-xs text-theme-base-500">
        Invitation from {invite.inviterPeerId}
      </p>
    </div>
    <span
      class={`rounded-full px-2 py-1 text-[11px] font-medium ${
        invite.status === "ready"
          ? "bg-theme-success-950 text-theme-success-300"
          : invite.status === "failed"
            ? "bg-theme-error-950 text-theme-error-300"
            : "bg-theme-warning-950 text-theme-warning-300"
      }`}
    >
      {invite.status === "ready" ? "Ready" : invite.status === "failed" ? "Sync failed" : "Syncing"}
    </span>
  </div>

  <p class="mt-3 text-sm text-theme-base-300" aria-live="polite">
    {invite.detail}
  </p>

  {#if error}
    <p class="mt-2 text-xs text-theme-error-300" role="alert">{error}</p>
  {/if}

  <div class="mt-4 flex gap-2">
    <button
      type="button"
      class="rounded-lg bg-theme-primary-600 px-3 py-2 text-sm font-medium text-white transition-colors hover:bg-theme-primary-500 disabled:cursor-not-allowed disabled:opacity-50"
      disabled={invite.status !== "ready" || busy}
      aria-disabled={invite.status !== "ready" || busy}
      onclick={accept}
    >
      {busy ? "Working…" : "Accept"}
    </button>
    {#if invite.status === "failed"}
      <button
        type="button"
        class="rounded-lg border border-theme-base-700 px-3 py-2 text-sm text-theme-base-200 transition-colors hover:bg-theme-base-800 disabled:cursor-not-allowed disabled:opacity-50"
        disabled={busy}
        onclick={retry}
      >
        Retry sync
      </button>
    {/if}
  </div>
</article>
