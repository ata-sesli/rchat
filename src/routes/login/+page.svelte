<script lang="ts">
  import { invoke } from "@tauri-apps/api/core";
  import { onMount } from "svelte";
  import { goto } from "$app/navigation";
  import GitHubButton from "../../components/GitHubButton.svelte";

  // State
  type ViewState = "loading" | "setup" | "unlock" | "login";
  let view: ViewState = "loading";

  // Form Data
  let password = "";
  let confirmPassword = "";
  let token = "";
  let error = "";
  let isLoading = false;

  // GitHub Device Flow State
  let deviceCode = "";
  let userCode = "";
  let verificationUri = "";
  let isPolling = false;

  onMount(async () => {
    await checkStatus();
  });

  async function checkStatus() {
    try {
      const status = await invoke<{
        is_setup: boolean;
        is_unlocked: boolean;
        is_github_connected: boolean;
      }>("check_auth_status");

      if (!status.is_setup) {
        view = "setup";
      } else if (!status.is_unlocked) {
        view = "unlock";
      } else {
        // Vault is unlocked
        if (status.is_github_connected) {
          goto("/"); // Already connected, skip login
        } else {
          view = "login"; // Needs connection
        }
      }
    } catch (e) {
      console.error(e);
      error = "Failed to connect to backend.";
    }
  }

  async function handleSetup() {
    if (password !== confirmPassword) {
      error = "Passwords do not match";
      return;
    }
    if (password.length < 8) {
      error = "Password must be at least 8 characters";
      return;
    }

    isLoading = true;
    error = "";
    try {
      await invoke("init_vault", { password });
      await checkStatus(); // Should move to 'login'
    } catch (e: any) {
      error = e.toString();
    } finally {
      isLoading = false;
    }
  }

  async function handleUnlock() {
    isLoading = true;
    error = "";
    try {
      await invoke("unlock_vault", { password });
      await checkStatus(); // Should move to 'login'
    } catch (e: any) {
      error = "Invalid password";
      console.error(e);
    } finally {
      isLoading = false;
    }
  }

  async function handleReset() {
    console.log("Reset requested...");
    // if (!confirm("Are you sure?")) return; // Commented out for debugging

    isLoading = true;
    try {
      console.log("Calling reset_vault backend command...");
      await invoke("reset_vault");
      console.log("Reset successful. Updating UI...");

      password = "";
      confirmPassword = "";

      // Force view update immediately
      view = "setup";

      // Also check status to be sure
      // await checkStatus();
    } catch (e: any) {
      console.error("Reset failed:", e);
      error = "Failed to reset: " + e.toString();
    } finally {
      isLoading = false;
    }
  }

  async function handleGitHubLogin() {
    isLoading = true;
    error = "";
    isPolling = true;
    try {
      console.log("Initiating GitHub Auth...");
      const res = await invoke<{
        device_code: string;
        user_code: string;
        verification_uri: string;
        interval: number;
      }>("start_github_auth");

      deviceCode = res.device_code;
      userCode = res.user_code;
      verificationUri = res.verification_uri;

      // Start Polling
      pollToken(res.device_code, res.interval);
    } catch (e: any) {
      error = "Failed to start GitHub Login: " + e.toString();
      isPolling = false;
      isLoading = false;
    }
  }

  async function pollToken(deviceCode: string, interval: number) {
    while (isPolling) {
      try {
        await new Promise((r) => setTimeout(r, (interval + 1) * 1000));
        console.log("Polling for token...");
        const accessToken = await invoke<string>("poll_github_auth", {
          deviceCode,
        });

        // Success!
        token = accessToken;
        await handleSaveToken();
        isPolling = false;
        return; // Done
      } catch (e: any) {
        const msg = e.toString();
        if (msg.includes("authorization_pending")) {
          continue; // Keep waiting
        } else if (msg.includes("slow_down")) {
          await new Promise((r) => setTimeout(r, 5000)); // Wait extra
          continue;
        } else if (msg.includes("expired_token")) {
          error = "Login timed out. Please try again.";
          break;
        } else {
          console.error("Polling error:", msg);
          // error = "Polling error: " + msg; // Optional: don't show internal polling errors
          // break;
        }
      }
    }
    isLoading = false;
  }

  async function handleSaveToken() {
    if (!token) return;
    try {
      await invoke("save_api_token", { token });
      goto("/"); // Redirect to home after successful token save
    } catch (e: any) {
      error = "Failed to save token: " + e.toString();
    }
  }
</script>

<div
  class="flex items-center justify-center h-screen bg-slate-950 text-slate-200"
>
  <div
    class="w-full max-w-md p-8 space-y-8 bg-slate-900/90 backdrop-blur-md rounded-2xl shadow-2xl border border-slate-700/50"
  >
    <!-- Header -->
    <div class="text-center">
      <div
        class="inline-flex p-3 rounded-2xl bg-slate-800/50 border border-slate-700/50 mb-4 shadow-lg"
      >
        <img src="/logo.svg" alt="RChat Logo" class="h-12 w-12" />
      </div>
      <h2 class="text-3xl font-bold text-white tracking-tight">
        {#if view === "setup"}Setup Vault{:else if view === "unlock"}Unlock
          Vault{:else}Welcome Back{/if}
      </h2>
      <p class="mt-2 text-slate-400">
        {#if view === "setup"}Create a master password to secure your tokens{:else if view === "unlock"}Enter
          your master password to continue{:else}Sign in to sync your peers{/if}
      </p>
    </div>

    <!-- VIEW: LOADING -->
    {#if view === "loading"}
      <div class="flex justify-center p-8">
        <div
          class="animate-spin rounded-full h-8 w-8 border-b-2 border-teal-500"
        ></div>
      </div>

      <!-- VIEW: SETUP -->
    {:else if view === "setup"}
      <div class="space-y-4">
        <input
          type="password"
          bind:value={password}
          placeholder="Create Master Password"
          class="w-full px-4 py-3 bg-slate-950/50 border border-slate-700 rounded-xl focus:border-teal-500 focus:ring-1 focus:ring-teal-500 outline-none transition-all placeholder:text-slate-600 text-white"
        />
        <input
          type="password"
          bind:value={confirmPassword}
          placeholder="Confirm Password"
          class="w-full px-4 py-3 bg-slate-950/50 border border-slate-700 rounded-xl focus:border-teal-500 focus:ring-1 focus:ring-teal-500 outline-none transition-all placeholder:text-slate-600 text-white"
        />
        <button
          on:click={handleSetup}
          disabled={isLoading}
          class="w-full py-3 bg-teal-600 hover:bg-teal-500 text-white rounded-xl font-semibold shadow-lg shadow-teal-500/20 transition-all hover:scale-[1.02] active:scale-[0.98] disabled:opacity-50 disabled:cursor-not-allowed"
        >
          {isLoading ? "Creating..." : "Create Vault"}
        </button>
      </div>

      <!-- VIEW: UNLOCK -->
    {:else if view === "unlock"}
      <div class="space-y-4">
        <input
          type="password"
          bind:value={password}
          placeholder="Master Password"
          class="w-full px-4 py-3 bg-slate-950/50 border border-slate-700 rounded-xl focus:border-teal-500 focus:ring-1 focus:ring-teal-500 outline-none transition-all placeholder:text-slate-600 text-white"
          on:keydown={(e) => e.key === "Enter" && handleUnlock()}
        />
        <button
          on:click={handleUnlock}
          disabled={isLoading}
          class="w-full py-3 bg-teal-600 hover:bg-teal-500 text-white rounded-xl font-semibold shadow-lg shadow-teal-500/20 transition-all hover:scale-[1.02] active:scale-[0.98] disabled:opacity-50 disabled:cursor-not-allowed"
        >
          {isLoading ? "Unlocking..." : "Unlock"}
        </button>
        <div class="text-center pt-2">
          <button
            on:click={handleReset}
            class="text-xs text-red-500 hover:text-red-400 hover:underline transition-colors block mx-auto py-2"
          >
            Reset Vault (Delete All Data)
          </button>
        </div>
      </div>

      <!-- VIEW: LOGIN (GitHub) -->
    {:else if view === "login"}
      <div class="space-y-4">
        {#if !userCode}
          <GitHubButton on:click={handleGitHubLogin} loading={isLoading} />
        {:else}
          <!-- User Code Display -->
          <div
            class="bg-slate-950/50 p-6 rounded-xl text-center space-y-4 border border-teal-500/30 shadow-inner"
          >
            <p class="text-sm text-slate-300">Copy this code:</p>
            <div
              class="text-3xl font-mono font-bold text-teal-400 tracking-widest select-all bg-slate-900/50 p-2 rounded-lg border border-slate-800"
            >
              {userCode}
            </div>
            <p class="text-xs text-slate-400">Then authorize at:</p>
            <a
              href={verificationUri}
              target="_blank"
              class="block text-teal-400 hover:text-teal-300 hover:underline font-medium transition-colors"
            >
              {verificationUri}
            </a>
            <div class="pt-4 flex justify-center items-center gap-2">
              <div
                class="animate-spin rounded-full h-4 w-4 border-b-2 border-teal-500"
              ></div>
              <p class="text-xs text-slate-500 animate-pulse">
                Waiting for authorization...
              </p>
            </div>
          </div>
        {/if}

        <div class="relative py-4">
          <div class="absolute inset-0 flex items-center">
            <div class="w-full border-t border-slate-700"></div>
          </div>
          <div class="relative flex justify-center text-sm">
            <span class="px-2 bg-slate-900 text-slate-500"
              >Or use a Personal Access Token</span
            >
          </div>
        </div>

        <div class="space-y-2">
          <input
            type="password"
            bind:value={token}
            placeholder="ghp_..."
            class="w-full px-4 py-3 bg-slate-950/50 border border-slate-700 rounded-xl focus:border-teal-500 focus:ring-1 focus:ring-teal-500 outline-none transition-all placeholder:text-slate-600 text-white"
          />
          <button
            on:click={handleSaveToken}
            class="w-full py-3 px-4 bg-slate-800 hover:bg-slate-700 text-slate-200 rounded-xl font-medium transition-all border border-slate-700"
          >
            Save Token
          </button>
        </div>
      </div>
    {/if}

    {#if error}
      <div class="animate-fade-in-up">
        <p
          class="text-red-400 text-sm text-center bg-red-950/30 border border-red-900/50 p-3 rounded-lg"
        >
          {error}
        </p>
      </div>
    {/if}
  </div>
</div>
