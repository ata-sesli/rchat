<script lang="ts">
  let { onback = () => {} } = $props();

  function goBack() {
    onback();
  }
</script>

<!-- Sub-view Header -->
<div class="mb-6 flex items-center gap-4 border-b border-slate-800/50 pb-4">
  <button
    on:click={goBack}
    class="p-2 hover:bg-slate-800 rounded-lg text-slate-400 hover:text-white transition-colors"
  >
    <svg
      xmlns="http://www.w3.org/2000/svg"
      class="h-5 w-5"
      viewBox="0 0 20 20"
      fill="currentColor"
    >
      <path
        fill-rule="evenodd"
        d="M12.707 5.293a1 1 0 010 1.414L9.414 10l3.293 3.293a1 1 0 01-1.414 1.414l-4-4a1 1 0 010-1.414l4-4a1 1 0 011.414 0z"
        clip-rule="evenodd"
      />
    </svg>
  </button>
  <h2 class="text-xl font-bold text-white">Set Theme</h2>
</div>

<div
  class="p-8 text-center bg-slate-900/50 rounded-xl border border-slate-800 border-dashed animate-fade-in-up"
>
  <div
    class="inline-flex p-4 rounded-full bg-purple-500/10 text-purple-400 mb-4"
  >
    <svg
      xmlns="http://www.w3.org/2000/svg"
      class="h-8 w-8"
      fill="none"
      viewBox="0 0 24 24"
      stroke="currentColor"
    >
      <path
        stroke-linecap="round"
        stroke-linejoin="round"
        stroke-width="2"
        d="M7 21a4 4 0 01-4-4V5a2 2 0 012-2h4a2 2 0 012 2v12a4 4 0 01-4 4zm0 0h12a2 2 0 002-2v-4a2 2 0 00-2-2h-2.343M11 7.343l1.657-1.657a2 2 0 012.828 0l2.829 2.829a2 2 0 010 2.828l-8.486 8.485M7 17h.01"
      />
    </svg>
  </div>
  <h3 class="text-lg font-medium text-white">Theme Customization</h3>
  <p class="text-slate-500 mt-2 max-w-sm mx-auto">
    This feature is coming soon! You will be able to switch between Light, Dark,
    and System themes.
  </p>
</div>

<style>
  @keyframes fade-in-up {
    from {
      opacity: 0;
      transform: translateY(10px);
    }
    to {
      opacity: 1;
      transform: translateY(0);
    }
  }
  .animate-fade-in-up {
    animation: fade-in-up 0.3s ease-out forwards;
  }
</style>
