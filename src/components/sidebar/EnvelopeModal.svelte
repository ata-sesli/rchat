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
      class="bg-slate-900 border border-slate-700 p-6 rounded-2xl w-full max-w-sm shadow-2xl space-y-4"
    >
      <h3 class="text-xl font-bold text-white">
        {editingId ? "Edit Envelope" : "New Envelope"}
      </h3>
      <p class="text-sm text-slate-400">
        {editingId
          ? "Update the envelope details."
          : "Create a folder to organize your chats."}
      </p>

      <input
        type="text"
        bind:value={name}
        placeholder="e.g. Work, Family, Projects"
        class="w-full bg-slate-800 text-white rounded-xl px-4 py-3 border border-slate-700 focus:outline-none focus:border-teal-500 focus:ring-1 focus:ring-teal-500 transition-all"
        autofocus
        onkeydown={(e) => e.key === "Enter" && handleSubmit()}
      />

      <!-- Icon Picker -->
      <div>
        <p class="text-xs text-slate-500 uppercase font-semibold mb-2">
          Select Icon
        </p>
        <div class="grid grid-cols-6 gap-2">
          {#each icons as icon}
            <button
              onclick={() => (selectedIcon = icon)}
              class={`p-2 rounded-lg flex items-center justify-center transition-all ${selectedIcon === icon ? "bg-teal-600 text-white shadow-lg shadow-teal-500/30 ring-2 ring-teal-500/50" : "bg-slate-800 text-slate-400 hover:bg-slate-700 hover:text-white"}`}
            >
              {@html icon}
            </button>
          {/each}
        </div>
      </div>

      <div class="flex justify-end gap-3 pt-2">
        <button
          onclick={handleClose}
          class="px-4 py-2 text-slate-400 hover:text-white transition-colors"
        >
          Cancel
        </button>
        <button
          onclick={handleSubmit}
          disabled={!name.trim()}
          class="px-6 py-2 bg-teal-600 hover:bg-teal-500 text-white rounded-lg font-bold shadow-lg shadow-teal-500/20 transition-all disabled:opacity-50 disabled:cursor-not-allowed"
        >
          {editingId ? "Save" : "Create"}
        </button>
      </div>
    </div>
  </div>
{/if}
