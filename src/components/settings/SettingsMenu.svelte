<script lang="ts">
  let { onnavigate = (view: string) => {} } = $props();
</script>

<div class="space-y-3">
  <!-- Profile Button -->
  <button
    onclick={() => onnavigate("profile")}
    class="w-full flex items-center justify-between p-4 bg-theme-base-900 border border-theme-base-800 rounded-xl hover:bg-theme-base-800 transition-all group shadow-sm hover:shadow-md"
  >
    <div class="flex items-center gap-4">
      <div
        class="p-2.5 rounded-lg bg-blue-500/10 text-theme-info-400 group-hover:bg-blue-500/20 group-hover:text-blue-300 transition-colors"
      >
        <svg
          xmlns="http://www.w3.org/2000/svg"
          class="h-6 w-6"
          fill="none"
          viewBox="0 0 24 24"
          stroke="currentColor"
        >
          <path
            stroke-linecap="round"
            stroke-linejoin="round"
            stroke-width="2"
            d="M16 7a4 4 0 11-8 0 4 4 0 018 0zM12 14a7 7 0 00-7 7h14a7 7 0 00-7-7z"
          />
        </svg>
      </div>
      <div class="text-left">
        <span
          class="block text-lg font-medium text-theme-base-200 group-hover:text-white transition-colors"
          >Profile</span
        >
        <span class="text-sm text-theme-base-500">Set alias and photo</span>
      </div>
    </div>
    <svg
      xmlns="http://www.w3.org/2000/svg"
      class="h-5 w-5 text-theme-base-500 group-hover:translate-x-1 transition-transform"
      viewBox="0 0 20 20"
      fill="currentColor"
    >
      <path
        fill-rule="evenodd"
        d="M7.293 14.707a1 1 0 010-1.414L10.586 10 7.293 6.707a1 1 0 011.414-1.414l4 4a1 1 0 010 1.414l-4 4a1 1 0 01-1.414 0z"
        clip-rule="evenodd"
      />
    </svg>
  </button>

  <!-- Set Theme Button -->
  <button
    onclick={() => onnavigate("set-theme")}
    class="w-full flex items-center justify-between p-4 bg-theme-base-900 border border-theme-base-800 rounded-xl hover:bg-theme-base-800 transition-all group shadow-sm hover:shadow-md"
  >
    <div class="flex items-center gap-4">
      <div
        class="p-2.5 rounded-lg bg-purple-500/10 text-theme-secondary-400 group-hover:bg-purple-500/20 group-hover:text-purple-300 transition-colors"
      >
        <svg
          xmlns="http://www.w3.org/2000/svg"
          class="h-6 w-6"
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
      <div class="text-left">
        <span
          class="block text-lg font-medium text-theme-base-200 group-hover:text-white transition-colors"
          >Set Theme</span
        >
        <span class="text-sm text-theme-base-500">Customize appearance</span>
      </div>
    </div>
    <svg
      xmlns="http://www.w3.org/2000/svg"
      class="h-5 w-5 text-theme-base-500 group-hover:translate-x-1 transition-transform"
      viewBox="0 0 20 20"
      fill="currentColor"
    >
      <path
        fill-rule="evenodd"
        d="M7.293 14.707a1 1 0 010-1.414L10.586 10 7.293 6.707a1 1 0 011.414-1.414l4 4a1 1 0 010 1.414l-4 4a1 1 0 01-1.414 0z"
        clip-rule="evenodd"
      />
    </svg>
  </button>

  <!-- About Button -->
  <button
    onclick={() => onnavigate("stickers")}
    class="w-full flex items-center justify-between p-4 bg-theme-base-900 border border-theme-base-800 rounded-xl hover:bg-theme-base-800 transition-all group shadow-sm hover:shadow-md"
  >
    <div class="flex items-center gap-4">
      <div
        class="p-2.5 rounded-lg bg-pink-500/10 text-pink-400 group-hover:bg-pink-500/20 group-hover:text-pink-300 transition-colors"
      >
        <svg
          xmlns="http://www.w3.org/2000/svg"
          class="h-6 w-6"
          fill="none"
          viewBox="0 0 24 24"
          stroke="currentColor"
        >
          <path
            stroke-linecap="round"
            stroke-linejoin="round"
            stroke-width="2"
            d="M14 10H3m3-6h11l4 4v11a2 2 0 01-2 2h-5M8 16l3 3 5-5"
          />
        </svg>
      </div>
      <div class="text-left">
        <span
          class="block text-lg font-medium text-theme-base-200 group-hover:text-white transition-colors"
          >Stickers</span
        >
        <span class="text-sm text-theme-base-500">Manage your sticker library</span>
      </div>
    </div>
    <svg
      xmlns="http://www.w3.org/2000/svg"
      class="h-5 w-5 text-theme-base-500 group-hover:translate-x-1 transition-transform"
      viewBox="0 0 20 20"
      fill="currentColor"
    >
      <path
        fill-rule="evenodd"
        d="M7.293 14.707a1 1 0 010-1.414L10.586 10 7.293 6.707a1 1 0 011.414-1.414l4 4a1 1 0 010 1.414l-4 4a1 1 0 01-1.414 0z"
        clip-rule="evenodd"
      />
    </svg>
  </button>

  <!-- About Button -->
  <button
    onclick={() => onnavigate("about")}
    class="w-full flex items-center justify-between p-4 bg-theme-base-900 border border-theme-base-800 rounded-xl hover:bg-theme-base-800 transition-all group shadow-sm hover:shadow-md"
  >
    <div class="flex items-center gap-4">
      <div
        class="p-2.5 rounded-lg bg-amber-500/10 text-orange-400 group-hover:bg-amber-500/20 group-hover:text-orange-300 transition-colors"
      >
        <svg
          xmlns="http://www.w3.org/2000/svg"
          class="h-6 w-6"
          fill="none"
          viewBox="0 0 24 24"
          stroke="currentColor"
        >
          <path
            stroke-linecap="round"
            stroke-linejoin="round"
            stroke-width="2"
            d="M13 16h-1v-4h-1m1-4h.01M21 12a9 9 0 11-18 0 9 9 0 0118 0z"
          />
        </svg>
      </div>
      <div class="text-left">
        <span
          class="block text-lg font-medium text-theme-base-200 group-hover:text-white transition-colors"
          >About RChat</span
        >
        <span class="text-sm text-theme-base-500">Version and info</span>
      </div>
    </div>
    <svg
      xmlns="http://www.w3.org/2000/svg"
      class="h-5 w-5 text-theme-base-500 group-hover:translate-x-1 transition-transform"
      viewBox="0 0 20 20"
      fill="currentColor"
    >
      <path
        fill-rule="evenodd"
        d="M7.293 14.707a1 1 0 010-1.414L10.586 10 7.293 6.707a1 1 0 011.414-1.414l4 4a1 1 0 010 1.414l-4 4a1 1 0 01-1.414 0z"
        clip-rule="evenodd"
      />
    </svg>
  </button>
</div>
