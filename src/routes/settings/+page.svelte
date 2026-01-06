<script lang="ts">
  import SettingsMenu from "../../components/settings/SettingsMenu.svelte";
  import SettingsProfile from "../../components/settings/SettingsProfile.svelte";
  import SettingsPeers from "../../components/settings/SettingsPeers.svelte";
  import SettingsTheme from "../../components/settings/SettingsTheme.svelte";
  import SettingsAbout from "../../components/settings/SettingsAbout.svelte";

  type View = "menu" | "profile" | "add-peers" | "set-theme" | "about";

  let currentView: View = "menu";

  function handleNavigate(view: string) {
    currentView = view as View;
  }

  function goBack() {
    currentView = "menu";
  }
</script>

<div
  class="h-full flex flex-col bg-theme-base-950 text-theme-base-200 p-8 overflow-y-auto"
>
  <!-- VIEW: MENU -->
  {#if currentView === "menu"}
    <!-- Header -->
    <div class="mb-8 border-b border-slate-800/50 pb-4">
      <h2 class="text-3xl font-bold text-theme-base-100 tracking-tight">
        Settings
      </h2>
      <p class="text-theme-base-400 mt-1">Manage application preferences.</p>
    </div>

    <SettingsMenu onnavigate={handleNavigate} />
  {:else if currentView === "profile"}
    <SettingsProfile onback={goBack} />
  {:else if currentView === "add-peers"}
    <SettingsPeers onback={goBack} />
  {:else if currentView === "set-theme"}
    <SettingsTheme onback={goBack} />
  {:else if currentView === "about"}
    <SettingsAbout onback={goBack} />
  {/if}
</div>
