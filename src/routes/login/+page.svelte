<script lang="ts">
  import { onMount } from "svelte";
  import { goto } from "$app/navigation";
  import { get } from "svelte/store";
  import GitHubButton from "../../components/GitHubButton.svelte";
  import { getAuthGateTarget, needsLocalUsername } from "$lib/authGate";
  import { api, type AuthStatus } from "$lib/tauri/api";
  import { appSession, ensureAppReady } from "$lib/stores";

  // State
  type ViewState = "loading" | "setup" | "unlock" | "login" | "username";
  let view: ViewState = "loading";

  // Form Data
  let password = "";
  let confirmPassword = "";
  let token = "";
  let localUsername = "";
  let error = "";
  let isLoading = false;
  let pendingAuthStatus: AuthStatus | null = null;

  // GitHub Device Flow State
  let deviceCode = "";
  let userCode = "";
  let verificationUri = "";
  let isPolling = false;

  onMount(async () => {
    console.log("[Login] mounted. Checking status...");
    await checkStatus();
  });

  async function checkStatus() {
    console.log("[Login] checkStatus called");
    try {
      const status = await api.checkAuthStatus();
      console.log("[Login] Status received:", status);

      const target = getAuthGateTarget(status);
      if (target === "app") {
        await continueAfterUnlock(status);
      } else {
        view = target;
      }
    } catch (e) {
      console.error(e);
      error = "Failed to connect to backend.";
    }
  }

  async function startUnlockedSession(authStatus: AuthStatus): Promise<boolean> {
    const ready = await ensureAppReady(authStatus);
    if (!ready) {
      const startupError = get(appSession).startupError;
      error = startupError
        ? `Vault unlocked, but RChat failed to finish startup: ${startupError}`
        : "Vault unlocked, but RChat failed to finish startup. Check the backend logs.";
      return false;
    }
    return true;
  }

  async function continueAfterUnlock(authStatus: AuthStatus) {
    pendingAuthStatus = authStatus;

    if (authStatus.is_github_connected) {
      if (await startUnlockedSession(authStatus)) goto("/");
      return;
    }

    const profile = await api.getUserProfile();
    localUsername = profile.alias || "";

    if (needsLocalUsername(authStatus, profile)) {
      view = "login";
      return;
    }

    if (await startUnlockedSession(authStatus)) goto("/");
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
      const authStatus = await api.initVault(password);
      await continueAfterUnlock(authStatus);
    } catch (e: any) {
      error = e.toString();
    } finally {
      isLoading = false;
    }
  }

  async function handleUnlock() {
    if (isLoading) return;
    isLoading = true;
    error = "";
    try {
      const authStatus = await api.unlockVault(password);
      await continueAfterUnlock(authStatus);
    } catch (e: any) {
      error = e?.toString?.() || "Failed to unlock vault";
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
      await api.resetVault();
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
      const res = await api.startGithubAuth();

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
        const accessToken = await api.pollGithubAuth(deviceCode);

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
    isLoading = true;
    error = "";
    try {
      await api.saveApiToken(token);
      const authStatus = pendingAuthStatus
        ? { ...pendingAuthStatus, is_github_connected: true }
        : await api.checkAuthStatus();
      if (await startUnlockedSession(authStatus)) goto("/");
    } catch (e: any) {
      error = "Failed to save token: " + e.toString();
    } finally {
      isLoading = false;
    }
  }

  function handleSkipGithub() {
    error = "";
    view = "username";
  }

  async function handleSaveLocalUsername() {
    const username = localUsername.trim();
    if (!username) {
      error = "Username is required";
      return;
    }

    isLoading = true;
    error = "";
    try {
      await api.updateUserProfile(username, null);
      window.dispatchEvent(new CustomEvent("profile-updated"));

      const authStatus = pendingAuthStatus ?? (await api.checkAuthStatus());
      if (await startUnlockedSession(authStatus)) goto("/");
    } catch (e: any) {
      error = "Failed to save username: " + e.toString();
    } finally {
      isLoading = false;
    }
  }
</script>

<div
  class="flex items-center justify-center h-screen bg-theme-base-950 text-theme-base-200"
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
          Vault{:else if view === "username"}Choose Username{:else}Connect GitHub{/if}
      </h2>
      <p class="mt-2 text-theme-base-400">
        {#if view === "setup"}Create a master password to secure your tokens{:else if view === "unlock"}Enter
          your master password to continue{:else if view === "username"}Pick a local name before entering RChat{:else}Sync remotely or continue locally{/if}
      </p>
    </div>

    <!-- VIEW: LOADING -->
    {#if view === "loading"}
      <div class="flex justify-center p-8">
        <div
          class="animate-spin rounded-full h-8 w-8 border-b-2 border-theme-primary-500"
        ></div>
      </div>

      <!-- VIEW: SETUP -->
    {:else if view === "setup"}
      <div class="space-y-4">
        <input
          type="password"
          bind:value={password}
          placeholder="Create Master Password"
          class="w-full px-4 py-3 bg-slate-950/50 border border-theme-base-700 rounded-xl focus:border-theme-primary-500 focus:ring-1 focus:ring-teal-500 outline-none transition-all placeholder:text-theme-base-600 text-white"
        />
        <input
          type="password"
          bind:value={confirmPassword}
          placeholder="Confirm Password"
          class="w-full px-4 py-3 bg-slate-950/50 border border-theme-base-700 rounded-xl focus:border-theme-primary-500 focus:ring-1 focus:ring-teal-500 outline-none transition-all placeholder:text-theme-base-600 text-white"
        />
        <button
          onclick={handleSetup}
          disabled={isLoading}
          class="w-full py-3 bg-theme-primary-600 hover:bg-theme-primary-500 text-white rounded-xl font-semibold shadow-lg shadow-teal-500/20 transition-all hover:scale-[1.02] active:scale-[0.98] disabled:opacity-50 disabled:cursor-not-allowed"
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
          class="w-full px-4 py-3 bg-slate-950/50 border border-theme-base-700 rounded-xl focus:border-theme-primary-500 focus:ring-1 focus:ring-teal-500 outline-none transition-all placeholder:text-theme-base-600 text-white"
          onkeydown={(e) => e.key === "Enter" && handleUnlock()}
        />
        <button
          onclick={handleUnlock}
          disabled={isLoading}
          class="w-full py-3 bg-theme-primary-600 hover:bg-theme-primary-500 text-white rounded-xl font-semibold shadow-lg shadow-teal-500/20 transition-all hover:scale-[1.02] active:scale-[0.98] disabled:opacity-50 disabled:cursor-not-allowed"
        >
          {isLoading ? "Unlocking..." : "Unlock"}
        </button>
        <div class="text-center pt-2">
          <button
            onclick={handleReset}
            class="text-xs text-red-500 hover:text-theme-error-400 hover:underline transition-colors block mx-auto py-2"
          >
            Reset Vault (Delete All Data)
          </button>
        </div>
      </div>

      <!-- VIEW: LOGIN (GitHub) -->
    {:else if view === "login"}
      <div class="space-y-4">
        {#if !userCode}
          <GitHubButton onclick={handleGitHubLogin} loading={isLoading} />
        {:else}
          <!-- User Code Display -->
          <div
            class="bg-slate-950/50 p-6 rounded-xl text-center space-y-4 border border-teal-500/30 shadow-inner"
          >
            <p class="text-sm text-theme-base-300">Copy this code:</p>
            <div
              class="text-3xl font-mono font-bold text-theme-primary-400 tracking-widest select-all bg-slate-900/50 p-2 rounded-lg border border-theme-base-800"
            >
              {userCode}
            </div>
            <p class="text-xs text-theme-base-400">Then authorize at:</p>
            <a
              href={verificationUri}
              target="_blank"
              class="block text-theme-primary-400 hover:text-theme-primary-300 hover:underline font-medium transition-colors"
            >
              {verificationUri}
            </a>
            <div class="pt-4 flex justify-center items-center gap-2">
              <div
                class="animate-spin rounded-full h-4 w-4 border-b-2 border-theme-primary-500"
              ></div>
              <p class="text-xs text-theme-base-500 animate-pulse">
                Waiting for authorization...
              </p>
            </div>
          </div>
        {/if}

        <div class="relative py-4">
          <div class="absolute inset-0 flex items-center">
            <div class="w-full border-t border-theme-base-700"></div>
          </div>
          <div class="relative flex justify-center text-sm">
            <span class="px-2 bg-theme-base-900 text-theme-base-500"
              >Or use a Personal Access Token</span
            >
          </div>
        </div>

        <div class="space-y-2">
          <input
            type="password"
            bind:value={token}
            placeholder="ghp_..."
            class="w-full px-4 py-3 bg-slate-950/50 border border-theme-base-700 rounded-xl focus:border-theme-primary-500 focus:ring-1 focus:ring-teal-500 outline-none transition-all placeholder:text-theme-base-600 text-white"
          />
          <button
            onclick={handleSaveToken}
            class="w-full py-3 px-4 bg-theme-base-800 hover:bg-theme-base-700 text-theme-base-200 rounded-xl font-medium transition-all border border-theme-base-700"
          >
            Save Token
          </button>
        </div>

        <div class="relative py-4">
          <div class="absolute inset-0 flex items-center">
            <div class="w-full border-t border-theme-base-700"></div>
          </div>
          <div class="relative flex justify-center text-sm">
            <span class="px-2 bg-theme-base-900 text-theme-base-500"
              >Or continue without GitHub</span
            >
          </div>
        </div>

        <button
          onclick={handleSkipGithub}
          disabled={isLoading || isPolling}
          class="w-full py-3 px-4 bg-theme-base-800 hover:bg-theme-base-700 text-theme-base-200 rounded-xl font-medium transition-all border border-theme-base-700 disabled:opacity-50 disabled:cursor-not-allowed"
        >
          Continue Locally
        </button>
      </div>

      <!-- VIEW: USERNAME -->
    {:else if view === "username"}
      <div class="space-y-4">
        <input
          type="text"
          bind:value={localUsername}
          placeholder="Your username"
          class="w-full px-4 py-3 bg-slate-950/50 border border-theme-base-700 rounded-xl focus:border-theme-primary-500 focus:ring-1 focus:ring-teal-500 outline-none transition-all placeholder:text-theme-base-600 text-white"
          onkeydown={(e) => e.key === "Enter" && handleSaveLocalUsername()}
        />
        <button
          onclick={handleSaveLocalUsername}
          disabled={isLoading}
          class="w-full py-3 bg-theme-primary-600 hover:bg-theme-primary-500 text-white rounded-xl font-semibold shadow-lg shadow-teal-500/20 transition-all hover:scale-[1.02] active:scale-[0.98] disabled:opacity-50 disabled:cursor-not-allowed"
        >
          {isLoading ? "Saving..." : "Continue"}
        </button>
        <button
          onclick={() => {
            error = "";
            view = "login";
          }}
          disabled={isLoading}
          class="w-full py-3 px-4 bg-transparent hover:bg-theme-base-800 text-theme-base-300 rounded-xl font-medium transition-all border border-theme-base-800 disabled:opacity-50 disabled:cursor-not-allowed"
        >
          Back to GitHub Options
        </button>
      </div>
    {/if}

    {#if error}
      <div class="animate-fade-in-up">
        <p
          class="text-theme-error-400 text-sm text-center bg-red-950/30 border border-red-900/50 p-3 rounded-lg"
        >
          {error}
        </p>
      </div>
    {/if}
  </div>
</div>
