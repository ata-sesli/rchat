<script lang="ts">
  import { fade } from "svelte/transition";
  import { isGroupChatId, isTemporaryGroupChatId } from "$lib/chatKind";
  import { api } from "$lib/tauri/api";

  let {
    show = false,
    onclose = () => {},
    oncreate = (_name: string) => {},
    onjoin = (_chatId: string, _name: string) => {},
    ontempjoin = (_chatId: string, _name: string) => {},
  } = $props();

  let mode = $state<"create" | "join" | "temp-create" | "temp-redeem">("create");
  let createName = $state("");
  let joinId = $state("");
  let joinName = $state("");
  let tempGroupName = $state("");
  let tempInviteLink = $state("");
  let tempRedeemLink = $state("");
  let tempInviteRemaining = $state(0);
  let error = $state<string | null>(null);
  let busy = $state(false);
  let countdownTimer: ReturnType<typeof setInterval> | null = null;

  $effect(() => {
    if (!show) return;
    if (!countdownTimer) {
      countdownTimer = setInterval(() => {
        if (tempInviteRemaining > 0) {
          tempInviteRemaining -= 1;
        }
      }, 1000);
    }
    return () => {
      if (countdownTimer) {
        clearInterval(countdownTimer);
        countdownTimer = null;
      }
    };
  });

  function reset() {
    mode = "create";
    createName = "";
    joinId = "";
    joinName = "";
    tempGroupName = "";
    tempInviteLink = "";
    tempRedeemLink = "";
    tempInviteRemaining = 0;
    error = null;
    busy = false;
  }

  async function submitCreate() {
    if (busy) return;
    busy = true;
    error = null;
    try {
      await oncreate(createName.trim());
      reset();
    } catch (e: any) {
      error = e?.toString?.() || "Failed to create group";
    } finally {
      busy = false;
    }
  }

  async function submitJoin() {
    if (busy) return;
    const id = joinId.trim();
    if (!isGroupChatId(id)) {
      error = "Use a valid group id: group:<uuid>";
      return;
    }
    busy = true;
    error = null;
    try {
      await onjoin(id, joinName.trim());
      reset();
    } catch (e: any) {
      error = e?.toString?.() || "Failed to join group";
    } finally {
      busy = false;
    }
  }

  async function refreshTempInvite() {
    try {
      const active = await api.getActiveTemporaryInvite();
      if (active && active.payload.kind === "group") {
        tempInviteLink = active.deep_link;
        tempInviteRemaining = active.remaining_seconds;
      } else {
        tempInviteLink = "";
        tempInviteRemaining = 0;
      }
    } catch {
      tempInviteLink = "";
      tempInviteRemaining = 0;
    }
  }

  async function submitTempCreate() {
    if (busy) return;
    busy = true;
    error = null;
    try {
      const result = await api.createTemporaryInvite("group", tempGroupName.trim() || null);
      tempInviteLink = result.deep_link;
      tempInviteRemaining = result.remaining_seconds;
      mode = "temp-redeem";
    } catch (e: any) {
      error = e?.toString?.() || "Failed to create temporary group invite";
    } finally {
      busy = false;
    }
  }

  async function submitTempRedeem() {
    if (busy) return;
    const link = tempRedeemLink.trim();
    if (!link) {
      error = "Paste temporary invite link";
      return;
    }
    busy = true;
    error = null;
    try {
      const result = await api.redeemTemporaryInvite(link);
      if (result.kind !== "group" || !isTemporaryGroupChatId(result.chat_id)) {
        throw new Error("This temporary invite is not a group invite");
      }
      await ontempjoin(result.chat_id, result.name);
      reset();
    } catch (e: any) {
      error = e?.toString?.() || "Failed to redeem temporary group invite";
    } finally {
      busy = false;
    }
  }

  async function copyTempLink() {
    if (!tempInviteLink) return;
    try {
      await navigator.clipboard.writeText(tempInviteLink);
    } catch {
      // ignore copy errors
    }
  }

  async function cancelTempInvite() {
    if (busy) return;
    busy = true;
    error = null;
    try {
      await api.cancelTemporaryInvite();
      tempInviteLink = "";
      tempInviteRemaining = 0;
    } catch (e: any) {
      error = e?.toString?.() || "Failed to cancel temporary invite";
    } finally {
      busy = false;
    }
  }
</script>

{#if show}
  <div
    class="fixed inset-0 bg-black/70 z-50 flex items-center justify-center p-4 animate-fade-in-up"
    transition:fade={{ duration: 150 }}
    role="button"
    tabindex="0"
    onclick={(e) => {
      if (e.target === e.currentTarget) {
        reset();
        onclose();
      }
    }}
    onkeydown={(e) => {
      if (e.key === "Escape") {
        reset();
        onclose();
      }
    }}
  >
    <div
      class="bg-theme-base-900 border border-theme-base-700 p-6 rounded-2xl w-full max-w-md shadow-2xl space-y-4"
    >
      <h3 class="text-xl font-bold text-theme-base-100">Group Chats</h3>

      <div class="flex gap-2">
        <button
          class={`px-3 py-2 rounded-lg text-sm transition-colors ${mode === "create" ? "bg-theme-base-700 text-white" : "bg-theme-base-800 text-theme-base-300 hover:text-white"}`}
          onclick={() => {
            mode = "create";
            error = null;
          }}
        >
          Create
        </button>
        <button
          class={`px-3 py-2 rounded-lg text-sm transition-colors ${mode === "join" ? "bg-theme-base-700 text-white" : "bg-theme-base-800 text-theme-base-300 hover:text-white"}`}
          onclick={() => {
            mode = "join";
            error = null;
          }}
        >
          Join
        </button>
        <button
          class={`px-3 py-2 rounded-lg text-sm transition-colors ${mode === "temp-create" ? "bg-theme-base-700 text-white" : "bg-theme-base-800 text-theme-base-300 hover:text-white"}`}
          onclick={async () => {
            mode = "temp-create";
            error = null;
            await refreshTempInvite();
          }}
        >
          Temp Create
        </button>
        <button
          class={`px-3 py-2 rounded-lg text-sm transition-colors ${mode === "temp-redeem" ? "bg-theme-base-700 text-white" : "bg-theme-base-800 text-theme-base-300 hover:text-white"}`}
          onclick={async () => {
            mode = "temp-redeem";
            error = null;
            await refreshTempInvite();
          }}
        >
          Temp Redeem
        </button>
      </div>

      {#if mode === "create"}
        <div class="space-y-3">
          <label class="block text-xs text-theme-base-400 uppercase tracking-wide"
            for="create-group-name">Group Name (optional)</label
          >
          <input
            id="create-group-name"
            type="text"
            bind:value={createName}
            placeholder="My Group"
            class="w-full rounded-lg bg-theme-base-800 border border-theme-base-700 px-3 py-2 text-sm text-theme-base-100 focus:outline-none focus:border-theme-primary-500"
          />
          <p class="text-xs text-theme-base-500">
            If empty, the app uses: <code>Group &lt;uuid-short&gt;</code>.
          </p>
        </div>
      {:else if mode === "join"}
        <div class="space-y-3">
          <label class="block text-xs text-theme-base-400 uppercase tracking-wide"
            for="join-group-id">Group ID</label
          >
          <input
            id="join-group-id"
            type="text"
            bind:value={joinId}
            placeholder="group:550e8400-e29b-41d4-a716-446655440000"
            class="w-full rounded-lg bg-theme-base-800 border border-theme-base-700 px-3 py-2 text-sm text-theme-base-100 focus:outline-none focus:border-theme-primary-500"
          />
          <label class="block text-xs text-theme-base-400 uppercase tracking-wide"
            for="join-group-name">Local Display Name (optional)</label
          >
          <input
            id="join-group-name"
            type="text"
            bind:value={joinName}
            placeholder="Team Room"
            class="w-full rounded-lg bg-theme-base-800 border border-theme-base-700 px-3 py-2 text-sm text-theme-base-100 focus:outline-none focus:border-theme-primary-500"
          />
        </div>
      {:else if mode === "temp-create"}
        <div class="space-y-3">
          <label class="block text-xs text-theme-base-400 uppercase tracking-wide"
            for="temp-group-name">Temporary Group Name (optional)</label
          >
          <input
            id="temp-group-name"
            type="text"
            bind:value={tempGroupName}
            placeholder="Temporary Squad"
            class="w-full rounded-lg bg-theme-base-800 border border-theme-base-700 px-3 py-2 text-sm text-theme-base-100 focus:outline-none focus:border-theme-warning-500"
          />
          <p class="text-xs text-theme-base-500">
            Creates an ephemeral invite link (`rchat://temp/...`) that expires in 120s.
          </p>
          {#if tempInviteLink}
            <div class="p-3 rounded-lg border border-theme-base-700 bg-theme-base-950 space-y-2">
              <p class="text-xs text-theme-base-500">
                Active invite expires in
                <span class="text-theme-warning-400 font-semibold">{tempInviteRemaining}s</span>
              </p>
              <code class="block text-xs text-theme-warning-300 break-all">
                {tempInviteLink}
              </code>
              <div class="flex gap-2">
                <button
                  onclick={copyTempLink}
                  class="px-3 py-1.5 text-xs rounded-md bg-theme-base-800 hover:bg-theme-base-700 text-theme-base-200"
                >
                  Copy Link
                </button>
                <button
                  onclick={cancelTempInvite}
                  class="px-3 py-1.5 text-xs rounded-md bg-theme-base-800 hover:bg-theme-base-700 text-theme-error-300"
                >
                  Cancel Invite
                </button>
              </div>
            </div>
          {/if}
        </div>
      {:else}
        <div class="space-y-3">
          <label class="block text-xs text-theme-base-400 uppercase tracking-wide"
            for="temp-group-link">Temporary Invite Link</label
          >
          <input
            id="temp-group-link"
            type="text"
            bind:value={tempRedeemLink}
            placeholder="rchat://temp/..."
            class="w-full rounded-lg bg-theme-base-800 border border-theme-base-700 px-3 py-2 text-sm text-theme-base-100 focus:outline-none focus:border-theme-warning-500"
            onkeydown={(e) => e.key === "Enter" && submitTempRedeem()}
          />
          {#if tempInviteLink}
            <p class="text-xs text-theme-base-500">
              Your active group invite expires in {tempInviteRemaining}s.
            </p>
          {/if}
        </div>
      {/if}

      {#if error}
        <p class="text-sm text-theme-error-400">{error}</p>
      {/if}

      <div class="flex justify-end gap-2 pt-2">
        <button
          onclick={() => {
            reset();
            onclose();
          }}
          class="px-4 py-2 text-sm text-theme-base-400 hover:text-white transition-colors"
        >
          Close
        </button>
        <button
          onclick={
            mode === "create"
              ? submitCreate
              : mode === "join"
                ? submitJoin
                : mode === "temp-create"
                  ? submitTempCreate
                  : submitTempRedeem
          }
          class="px-4 py-2 text-sm rounded-lg bg-theme-primary-600 hover:bg-theme-primary-500 text-white disabled:opacity-60"
          disabled={busy}
        >
          {#if busy}
            Working...
          {:else if mode === "create"}
            Create Group
          {:else if mode === "join"}
            Join Group
          {:else if mode === "temp-create"}
            Create Temp Invite
          {:else}
            Redeem Temp Invite
          {/if}
        </button>
      </div>
    </div>
  </div>
{/if}
