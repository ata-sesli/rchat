<script lang="ts">
  // Props with callbacks instead of events
  let {
    show = false,
    name = $bindable(""),
    selectedIcon = $bindable(""),
    editingId = null as string | null,
    icons = [] as string[],
    onclose = () => {},
    onsubmit = (data: { name: string; icon: string }) => {},
  } = $props();

  function handleSubmit() {
    if (!name.trim()) return;
    onsubmit({ name, icon: selectedIcon });
  }

  function handleClose() {
    onclose();
  }
</script>

{#if show}
  <div
    class="fixed inset-0 bg-black/70 z-50 flex items-center justify-center p-4 animate-fade-in-up"
  >
    <div
      class="bg-theme-base-900 border border-theme-base-700 p-6 rounded-2xl w-full max-w-sm shadow-2xl space-y-4"
    >
      <h3 class="text-xl font-bold text-theme-base-100">
        {editingId ? "Edit Envelope" : "New Envelope"}
      </h3>
      <p class="text-sm text-theme-base-400">
        {editingId
          ? "Update the envelope details."
          : "Create a folder to organize your chats."}
      </p>

      <input
        type="text"
        bind:value={name}
        placeholder="e.g. Work, Family, Projects"
        class="w-full bg-theme-base-800 text-white rounded-xl px-4 py-3 border border-theme-base-700 focus:outline-none focus:border-theme-primary-500 focus:ring-1 focus:ring-teal-500 transition-all"
        autofocus
        onkeydown={(e) => e.key === "Enter" && handleSubmit()}
      />

      <!-- Icon Picker -->
      <div>
        <p class="text-xs text-theme-base-500 uppercase font-semibold mb-2">
          Select Icon
        </p>
        <div class="grid grid-cols-6 gap-2">
          {#each icons as icon}
            <button
              onclick={() => (selectedIcon = icon)}
              class={`p-2 rounded-lg flex items-center justify-center transition-all ${selectedIcon === icon ? "bg-theme-primary-600 text-white shadow-lg shadow-teal-500/30 ring-2 ring-teal-500/50" : "bg-theme-base-800 text-theme-base-400 hover:bg-theme-base-700 hover:text-white"}`}
            >
              {@html icon}
            </button>
          {/each}
        </div>
      </div>

      <div class="flex justify-end gap-3 pt-2">
        <button
          onclick={handleClose}
          class="px-4 py-2 text-theme-base-400 hover:text-white transition-colors"
        >
          Cancel
        </button>
        <button
          onclick={handleSubmit}
          disabled={!name.trim()}
          class="px-6 py-2 bg-theme-primary-600 hover:bg-theme-primary-500 text-white rounded-lg font-bold shadow-lg shadow-teal-500/20 transition-all disabled:opacity-50 disabled:cursor-not-allowed"
        >
          {editingId ? "Save" : "Create"}
        </button>
      </div>
    </div>
  </div>
{/if}
