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
    class="p-2 hover:bg-theme-base-800 rounded-lg text-theme-base-400 hover:text-white transition-colors"
    aria-label="Go Back"
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
  <h2 class="text-xl font-bold text-theme-base-100">About RChat</h2>
</div>

<div class="space-y-6 animate-fade-in-up">
  <!-- Logo -->
  <div class="flex flex-col items-center text-center py-6">
    <div
      class="w-20 h-20 rounded-2xl shadow-xl mb-4 bg-teal-500/20 text-theme-primary-400 flex items-center justify-center"
    >
      <!-- Placeholder for logo if image fails -->
      <span class="text-4xl font-bold">R</span>
    </div>
    <h3 class="text-2xl font-bold text-white">RChat</h3>
    <p class="text-theme-base-400 mt-1">Version 0.1.0</p>
  </div>

  <!-- Info Cards -->
  <div class="space-y-3">
    <div class="p-4 bg-theme-base-900 border border-theme-base-800 rounded-xl">
      <h4 class="font-medium text-theme-base-200 mb-1">About</h4>
      <p class="text-sm text-theme-base-400">
        RChat is a decentralized, peer-to-peer chat application built with
        privacy and security in mind. Your messages stay yours.
      </p>
    </div>

    <div class="p-4 bg-theme-base-900 border border-theme-base-800 rounded-xl">
      <h4 class="font-medium text-theme-base-200 mb-1">Technology</h4>
      <p class="text-sm text-theme-base-400">
        Built with Tauri, SvelteKit, Rust, and libp2p for secure peer-to-peer
        communication.
      </p>
    </div>

    <div class="p-4 bg-theme-base-900 border border-theme-base-800 rounded-xl">
      <h4 class="font-medium text-theme-base-200 mb-1">License</h4>
      <p class="text-sm text-theme-base-400">
        Open source software. MIT License.
      </p>
    </div>
  </div>
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
