<script lang="ts">
  import { invoke } from "@tauri-apps/api/core";
  import { listen } from "@tauri-apps/api/event";
  import { onMount, onDestroy } from "svelte";
  import QRCode from "qrcode";
  import { Html5Qrcode } from "html5-qrcode";

  type StepType =
    | "select-network"
    | "local-scan"
    | "online"
    | "create-invite-user"
    | "create-invite-code"
    | "accept-invite-user"
    | "accept-invite-code";

  let {
    show = false,
    step = $bindable("select-network" as StepType),
    localPeers = [] as { peer_id: string; addresses: string[] }[],
    onclose = () => {},
    onconnect = (peerId: string) => {},
  } = $props();

  // Track which peer we're waiting for
  let waitingForPeer = $state<string | null>(null);
  let unlistenConnected: (() => void) | null = null;

  onMount(async () => {
    // Listen for successful connection event
    unlistenConnected = await listen<string>("peer-connected", (event) => {
      console.log("Peer connected:", event.payload);
      waitingForPeer = null;
      onconnect(event.payload);
      handleClose();
    });
  });

  onDestroy(() => {
    if (unlistenConnected) unlistenConnected();
  });

  function handleClose() {
    // Disable fast discovery when closing
    invoke("set_fast_discovery", { enabled: false }).catch(console.error);
    waitingForPeer = null;
    // Reset invitation state
    createInvitee = "";
    createPassword = "";
    showQrCode = false;
    qrDataUrl = "";
    createError = "";
    acceptInviter = "";
    acceptPassword = "";
    acceptError = "";
    onclose();
  }

  function setStep(newStep: StepType) {
    // Toggle fast discovery mode based on step
    if (newStep === "local-scan" && step !== "local-scan") {
      invoke("set_fast_discovery", { enabled: true }).catch(console.error);
    } else if (step === "local-scan" && newStep !== "local-scan") {
      invoke("set_fast_discovery", { enabled: false }).catch(console.error);
    }
    step = newStep;
  }

  // Get parent step for back navigation
  function getParentStep(currentStep: StepType): StepType {
    switch (currentStep) {
      case "create-invite-user":
      case "accept-invite-user":
        return "online";
      case "create-invite-code":
        return "create-invite-user";
      case "accept-invite-code":
        return "accept-invite-user";
      default:
        return "select-network";
    }
  }

  async function handleConnect(peerId: string) {
    console.log("Requesting connection to:", peerId);
    waitingForPeer = peerId;

    try {
      await invoke("request_connection", { peerId });
    } catch (e) {
      console.error("Failed to request connection:", e);
      waitingForPeer = null;
    }
  }

  // ============================================================================
  // Invitation State & Handlers
  // ============================================================================

  // Create Invitation state
  let createInvitee = $state("");
  let createPassword = $state("");
  let showQrCode = $state(false);
  let qrDataUrl = $state("");
  let createError = $state("");
  let createLoading = $state(false);

  // Accept Invitation state
  let acceptInviter = $state("");
  let acceptPassword = $state("");
  let acceptError = $state("");
  let acceptLoading = $state(false);

  // QR Scanner state
  let showQrScanner = $state(false);
  let qrScanner: Html5Qrcode | null = null;

  async function startQrScanner() {
    showQrScanner = true;
    acceptError = "";

    // Wait for DOM to update
    await new Promise((resolve) => setTimeout(resolve, 100));

    try {
      qrScanner = new Html5Qrcode("qr-reader");
      await qrScanner.start(
        { facingMode: "environment" },
        { fps: 10, qrbox: { width: 200, height: 200 } },
        (decodedText) => {
          acceptPassword = decodedText;
          stopQrScanner();
        },
        (errorMessage) => {
          // Ignore scan failures - just means no QR found yet
        }
      );
    } catch (e: any) {
      acceptError = `Camera error: ${e.message || e}`;
      showQrScanner = false;
    }
  }

  async function stopQrScanner() {
    if (qrScanner) {
      try {
        await qrScanner.stop();
      } catch (e) {
        // Ignore stop errors
      }
      qrScanner = null;
    }
    showQrScanner = false;
  }

  async function startCreateInvite() {
    if (!createInvitee.trim()) {
      createError = "Please enter a GitHub username";
      return;
    }
    createError = "";
    createLoading = true;

    try {
      // Generate password
      createPassword = await invoke<string>("generate_invite_password");
      // Generate QR code
      qrDataUrl = await QRCode.toDataURL(createPassword, {
        width: 200,
        margin: 2,
        color: { dark: "#0f172a", light: "#f1f5f9" },
      });
      setStep("create-invite-code");
    } catch (e: any) {
      createError = e.toString();
    } finally {
      createLoading = false;
    }
  }

  async function confirmCreateInvite() {
    createLoading = true;
    createError = "";

    try {
      await invoke("create_invite", {
        invitee: createInvitee.trim(),
        password: createPassword,
      });
      console.log("Invite created successfully");
      handleClose();
    } catch (e: any) {
      createError = e.toString();
    } finally {
      createLoading = false;
    }
  }

  async function copyPassword() {
    try {
      await navigator.clipboard.writeText(createPassword);
    } catch (e) {
      console.error("Failed to copy:", e);
    }
  }

  async function startAcceptInvite() {
    if (!acceptInviter.trim()) {
      acceptError = "Please enter inviter's GitHub username";
      return;
    }
    acceptError = "";
    setStep("accept-invite-code");
  }

  async function confirmRedeemInvite() {
    if (acceptPassword.length !== 14) {
      acceptError = "Password must be exactly 14 characters";
      return;
    }
    acceptLoading = true;
    acceptError = "";

    try {
      const peer_id = await invoke<string>("redeem_and_connect", {
        inviter: acceptInviter.trim(),
        password: acceptPassword,
      });
      console.log("Invite redeemed, connected with:", peer_id);

      // Close modal and emit event for navigation
      handleClose();

      // Dispatch event for parent to navigate to chat
      window.dispatchEvent(
        new CustomEvent("open-chat", { detail: { peerId: peer_id } })
      );
    } catch (e: any) {
      acceptError = e.toString();
    } finally {
      acceptLoading = false;
    }
  }
</script>

{#if show}
  <div
    class="fixed inset-0 bg-black/70 z-50 flex items-center justify-center p-4 animate-fade-in-up"
    on:click|self={handleClose}
  >
    <div
      class="bg-slate-900 border border-slate-700 p-6 rounded-2xl w-full max-w-md shadow-2xl space-y-4"
    >
      <!-- Header with Back Button -->
      <div class="flex items-center gap-3">
        {#if step !== "select-network"}
          <button
            on:click={() => setStep(getParentStep(step))}
            class="p-1 rounded-lg hover:bg-slate-800 text-slate-400 hover:text-white transition-colors"
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
        {/if}
        <h3 class="text-xl font-bold text-white">
          {#if step === "select-network"}
            Add New Person
          {:else if step === "local-scan"}
            Local Network
          {:else if step === "online"}
            Online (GitHub)
          {:else if step === "create-invite-user" || step === "create-invite-code"}
            Create Invitation
          {:else if step === "accept-invite-user" || step === "accept-invite-code"}
            Accept Invitation
          {/if}
        </h3>
      </div>

      <!-- Step 1: Network Selection -->
      {#if step === "select-network"}
        <p class="text-sm text-slate-400">
          Choose how to find people to connect with.
        </p>
        <div class="space-y-3">
          <button
            on:click={() => setStep("local-scan")}
            class="w-full flex items-center gap-4 p-4 bg-slate-800/50 hover:bg-slate-800 border border-slate-700 rounded-xl transition-colors text-left group"
          >
            <div
              class="w-12 h-12 rounded-xl bg-teal-500/10 text-teal-400 flex items-center justify-center group-hover:bg-teal-500/20 transition-colors"
            >
              <svg
                xmlns="http://www.w3.org/2000/svg"
                class="h-6 w-6"
                viewBox="0 0 20 20"
                fill="currentColor"
              >
                <path
                  fill-rule="evenodd"
                  d="M17.778 8.222c-4.296-4.296-11.26-4.296-15.556 0A1 1 0 01.808 6.808c5.076-5.077 13.308-5.077 18.384 0a1 1 0 01-1.414 1.414zM14.95 11.05a7 7 0 00-9.9 0 1 1 0 01-1.414-1.414 9 9 0 0112.728 0 1 1 0 01-1.414 1.414zM12.12 13.88a3 3 0 00-4.242 0 1 1 0 01-1.415-1.415 5 5 0 017.072 0 1 1 0 01-1.415 1.415zM9 16a1 1 0 011-1h.01a1 1 0 110 2H10a1 1 0 01-1-1z"
                  clip-rule="evenodd"
                />
              </svg>
            </div>
            <div class="flex-1">
              <div class="font-semibold text-white">Local Network (Wi-Fi)</div>
              <div class="text-sm text-slate-400">
                Find RChat users on the same network
              </div>
            </div>
            <svg
              xmlns="http://www.w3.org/2000/svg"
              class="h-5 w-5 text-slate-500"
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

          <button
            on:click={() => setStep("online")}
            class="w-full flex items-center gap-4 p-4 bg-slate-800/50 hover:bg-slate-800 border border-slate-700 rounded-xl transition-colors text-left group"
          >
            <div
              class="w-12 h-12 rounded-xl bg-purple-500/10 text-purple-400 flex items-center justify-center group-hover:bg-purple-500/20 transition-colors"
            >
              <svg
                xmlns="http://www.w3.org/2000/svg"
                class="h-6 w-6"
                viewBox="0 0 20 20"
                fill="currentColor"
              >
                <path
                  fill-rule="evenodd"
                  d="M4.083 9h1.946c.089-1.546.383-2.97.837-4.118A6.004 6.004 0 004.083 9zM10 2a8 8 0 100 16 8 8 0 000-16zm0 2c-.076 0-.232.032-.465.262-.238.234-.497.623-.737 1.182-.389.907-.673 2.142-.766 3.556h3.936c-.093-1.414-.377-2.649-.766-3.556-.24-.56-.5-.948-.737-1.182C10.232 4.032 10.076 4 10 4zm3.971 5c-.089-1.546-.383-2.97-.837-4.118A6.004 6.004 0 0115.917 9h-1.946zm-2.003 2H8.032c.093 1.414.377 2.649.766 3.556.24.56.5.948.737 1.182.233.23.389.262.465.262.076 0 .232-.032.465-.262.238-.234.498-.623.737-1.182.389-.907.673-2.142.766-3.556zm1.166 4.118c.454-1.147.748-2.572.837-4.118h1.946a6.004 6.004 0 01-2.783 4.118zm-6.268 0C6.412 13.97 6.118 12.546 6.03 11H4.083a6.004 6.004 0 002.783 4.118z"
                  clip-rule="evenodd"
                />
              </svg>
            </div>
            <div class="flex-1">
              <div class="font-semibold text-white">Online (GitHub)</div>
              <div class="text-sm text-slate-400">
                Connect with anyone globally
              </div>
            </div>
            <svg
              xmlns="http://www.w3.org/2000/svg"
              class="h-5 w-5 text-slate-500"
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
      {/if}

      <!-- Step 2a: Local Network Scan -->
      {#if step === "local-scan"}
        <div class="space-y-4">
          <!-- Scanning Indicator -->
          <div
            class="flex items-center gap-3 p-3 bg-teal-500/10 border border-teal-500/20 rounded-lg text-teal-400"
          >
            <div class="relative">
              <div
                class="w-3 h-3 bg-teal-400 rounded-full animate-ping absolute"
              ></div>
              <div class="w-3 h-3 bg-teal-400 rounded-full"></div>
            </div>
            <span class="text-sm">Scanning local network...</span>
          </div>

          <!-- Peers List -->
          <div class="space-y-2 max-h-64 overflow-y-auto">
            {#if localPeers.length === 0}
              <div class="text-center py-8 text-slate-500">
                <svg
                  xmlns="http://www.w3.org/2000/svg"
                  class="h-12 w-12 mx-auto mb-3 opacity-50"
                  fill="none"
                  viewBox="0 0 24 24"
                  stroke="currentColor"
                >
                  <path
                    stroke-linecap="round"
                    stroke-linejoin="round"
                    stroke-width="1.5"
                    d="M17.982 18.725A7.488 7.488 0 0012 15.75a7.488 7.488 0 00-5.982 2.975m11.963 0a9 9 0 10-11.963 0m11.963 0A8.966 8.966 0 0112 21a8.966 8.966 0 01-5.982-2.275M15 9.75a3 3 0 11-6 0 3 3 0 016 0z"
                  />
                </svg>
                <p class="text-sm">No peers found yet</p>
                <p class="text-xs text-slate-600 mt-1">
                  Make sure RChat is running on other devices
                </p>
              </div>
            {:else}
              {#each localPeers as peer}
                <div
                  class="flex items-center gap-3 p-3 bg-slate-800/50 rounded-lg border border-slate-700"
                >
                  <div
                    class="w-10 h-10 rounded-full bg-gradient-to-br from-teal-400 to-cyan-500 flex items-center justify-center text-white font-bold text-sm"
                  >
                    {peer.peer_id.slice(-4).toUpperCase()}
                  </div>
                  <div class="flex-1 min-w-0">
                    <div class="text-sm font-medium text-white truncate">
                      Peer {peer.peer_id.slice(-8)}
                    </div>
                    <div class="text-xs text-slate-500 truncate">
                      {peer.addresses[0] || "No address"}
                    </div>
                  </div>
                  {#if waitingForPeer === peer.peer_id}
                    <div
                      class="px-3 py-1.5 bg-amber-600 text-white text-sm rounded-lg font-medium flex items-center gap-2"
                    >
                      <svg class="animate-spin h-4 w-4" viewBox="0 0 24 24">
                        <circle
                          class="opacity-25"
                          cx="12"
                          cy="12"
                          r="10"
                          stroke="currentColor"
                          stroke-width="4"
                          fill="none"
                        ></circle>
                        <path
                          class="opacity-75"
                          fill="currentColor"
                          d="M4 12a8 8 0 018-8V0C5.373 0 0 5.373 0 12h4z"
                        ></path>
                      </svg>
                      Waiting...
                    </div>
                  {:else}
                    <button
                      on:click={() => handleConnect(peer.peer_id)}
                      class="px-3 py-1.5 bg-teal-600 hover:bg-teal-500 text-white text-sm rounded-lg font-medium transition-colors"
                    >
                      Connect
                    </button>
                  {/if}
                </div>
              {/each}
            {/if}
          </div>
        </div>
      {/if}

      <!-- Step 2b: Online - Choice Menu -->
      {#if step === "online"}
        <p class="text-sm text-slate-400">
          Connect with anyone globally via encrypted invitations.
        </p>
        <div class="space-y-3">
          <button
            on:click={() => setStep("create-invite-user")}
            class="w-full flex items-center gap-4 p-4 bg-slate-800/50 hover:bg-slate-800 border border-slate-700 rounded-xl transition-colors text-left group"
          >
            <div
              class="w-12 h-12 rounded-xl bg-teal-500/10 text-teal-400 flex items-center justify-center group-hover:bg-teal-500/20 transition-colors"
            >
              <svg
                xmlns="http://www.w3.org/2000/svg"
                class="h-6 w-6"
                viewBox="0 0 20 20"
                fill="currentColor"
              >
                <path
                  d="M8 9a3 3 0 100-6 3 3 0 000 6zM8 11a6 6 0 016 6H2a6 6 0 016-6zM16 7a1 1 0 10-2 0v1h-1a1 1 0 100 2h1v1a1 1 0 102 0v-1h1a1 1 0 100-2h-1V7z"
                />
              </svg>
            </div>
            <div class="flex-1">
              <div class="font-semibold text-white">Create Invitation</div>
              <div class="text-sm text-slate-400">
                Send encrypted invite to a friend
              </div>
            </div>
            <svg
              xmlns="http://www.w3.org/2000/svg"
              class="h-5 w-5 text-slate-500"
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

          <button
            on:click={() => setStep("accept-invite-user")}
            class="w-full flex items-center gap-4 p-4 bg-slate-800/50 hover:bg-slate-800 border border-slate-700 rounded-xl transition-colors text-left group"
          >
            <div
              class="w-12 h-12 rounded-xl bg-purple-500/10 text-purple-400 flex items-center justify-center group-hover:bg-purple-500/20 transition-colors"
            >
              <svg
                xmlns="http://www.w3.org/2000/svg"
                class="h-6 w-6"
                viewBox="0 0 20 20"
                fill="currentColor"
              >
                <path
                  d="M2.003 5.884L10 9.882l7.997-3.998A2 2 0 0016 4H4a2 2 0 00-1.997 1.884z"
                />
                <path
                  d="M18 8.118l-8 4-8-4V14a2 2 0 002 2h12a2 2 0 002-2V8.118z"
                />
              </svg>
            </div>
            <div class="flex-1">
              <div class="font-semibold text-white">Accept Invitation</div>
              <div class="text-sm text-slate-400">
                Redeem invite from a friend
              </div>
            </div>
            <svg
              xmlns="http://www.w3.org/2000/svg"
              class="h-5 w-5 text-slate-500"
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
      {/if}

      <!-- Create Invitation - Username -->
      {#if step === "create-invite-user"}
        <div class="space-y-4">
          <p class="text-sm text-slate-400">
            Enter the GitHub username of the person you want to invite.
          </p>
          <div class="flex flex-col sm:flex-row gap-3">
            <div class="flex-1 relative">
              <div
                class="absolute inset-y-0 left-0 pl-3 flex items-center pointer-events-none"
              >
                <span class="text-slate-500">@</span>
              </div>
              <input
                type="text"
                bind:value={createInvitee}
                placeholder="github_username"
                class="w-full pl-8 pr-4 py-2.5 bg-slate-900 border border-slate-700 rounded-lg text-slate-200 focus:outline-none focus:border-teal-500 focus:ring-1 focus:ring-teal-500 transition-all placeholder:text-slate-600"
                on:keydown={(e) => e.key === "Enter" && startCreateInvite()}
              />
            </div>
            <button
              on:click={startCreateInvite}
              disabled={createLoading || !createInvitee.trim()}
              class="px-6 py-2.5 bg-teal-600 hover:bg-teal-500 text-slate-950 font-semibold rounded-lg shadow-lg shadow-teal-500/20 transition-all disabled:opacity-50 disabled:cursor-not-allowed"
            >
              {createLoading ? "Generating..." : "Next"}
            </button>
          </div>
          {#if createError}
            <p class="text-sm text-red-400">{createError}</p>
          {/if}
        </div>
      {/if}

      <!-- Create Invitation - Password & QR -->
      {#if step === "create-invite-code"}
        <div class="space-y-4">
          <p class="text-sm text-slate-400">
            Share this code with <span class="text-teal-400 font-semibold"
              >@{createInvitee}</span
            >:
          </p>

          {#if showQrCode && qrDataUrl}
            <div class="flex justify-center">
              <img src={qrDataUrl} alt="QR Code" class="rounded-lg" />
            </div>
          {/if}

          <div
            class="flex items-center gap-2 p-3 bg-slate-950 border border-slate-700 rounded-lg"
          >
            <code
              class="flex-1 text-xl font-mono text-teal-400 tracking-wider text-center"
              >{createPassword}</code
            >
            <button
              on:click={copyPassword}
              class="px-3 py-1.5 bg-slate-800 hover:bg-slate-700 text-slate-300 text-sm rounded-lg transition-colors flex items-center gap-1"
            >
              <svg
                xmlns="http://www.w3.org/2000/svg"
                class="h-4 w-4"
                viewBox="0 0 20 20"
                fill="currentColor"
              >
                <path d="M8 3a1 1 0 011-1h2a1 1 0 110 2H9a1 1 0 01-1-1z" />
                <path
                  d="M6 3a2 2 0 00-2 2v11a2 2 0 002 2h8a2 2 0 002-2V5a2 2 0 00-2-2 3 3 0 01-3 3H9a3 3 0 01-3-3z"
                />
              </svg>
              Copy
            </button>
          </div>

          <label class="flex items-center gap-2 cursor-pointer">
            <input
              type="checkbox"
              bind:checked={showQrCode}
              class="rounded border-slate-600 bg-slate-800 text-teal-500 focus:ring-teal-500"
            />
            <span class="text-sm text-slate-400">Show QR Code</span>
          </label>

          <div class="flex gap-2 pt-2">
            <button
              on:click={() => setStep("create-invite-user")}
              class="px-4 py-2 text-sm text-slate-400 hover:text-white transition-colors"
            >
              ← Back
            </button>
            <button
              on:click={confirmCreateInvite}
              disabled={createLoading}
              class="flex-1 px-6 py-2.5 bg-teal-600 hover:bg-teal-500 text-slate-950 font-semibold rounded-lg transition-all disabled:opacity-50"
            >
              {createLoading ? "Creating..." : "Confirm"}
            </button>
          </div>
          {#if createError}
            <p class="text-sm text-red-400">{createError}</p>
          {/if}
        </div>
      {/if}

      <!-- Accept Invitation - Username -->
      {#if step === "accept-invite-user"}
        <div class="space-y-4">
          <p class="text-sm text-slate-400">
            Enter the GitHub username of the person who invited you.
          </p>
          <div class="flex flex-col sm:flex-row gap-3">
            <div class="flex-1 relative">
              <div
                class="absolute inset-y-0 left-0 pl-3 flex items-center pointer-events-none"
              >
                <span class="text-slate-500">@</span>
              </div>
              <input
                type="text"
                bind:value={acceptInviter}
                placeholder="github_username"
                class="w-full pl-8 pr-4 py-2.5 bg-slate-900 border border-slate-700 rounded-lg text-slate-200 focus:outline-none focus:border-purple-500 focus:ring-1 focus:ring-purple-500 transition-all placeholder:text-slate-600"
                on:keydown={(e) => e.key === "Enter" && startAcceptInvite()}
              />
            </div>
            <button
              on:click={startAcceptInvite}
              disabled={!acceptInviter.trim()}
              class="px-6 py-2.5 bg-purple-600 hover:bg-purple-500 text-white font-semibold rounded-lg shadow-lg shadow-purple-500/20 transition-all disabled:opacity-50 disabled:cursor-not-allowed"
            >
              Next
            </button>
          </div>
          {#if acceptError}
            <p class="text-sm text-red-400">{acceptError}</p>
          {/if}
        </div>
      {/if}

      <!-- Accept Invitation - Code Input -->
      {#if step === "accept-invite-code"}
        <div class="space-y-4">
          <p class="text-sm text-slate-400">
            Enter the code from <span class="text-purple-400 font-semibold"
              >@{acceptInviter}</span
            >:
          </p>

          <!-- QR Scanner Toggle -->
          <div class="flex gap-2 mb-2">
            <button
              on:click={() =>
                showQrScanner ? stopQrScanner() : startQrScanner()}
              class={`flex-1 px-4 py-2 text-sm rounded-lg transition-all flex items-center justify-center gap-2
                ${
                  showQrScanner
                    ? "bg-purple-600 text-white"
                    : "bg-slate-800 text-slate-400 hover:bg-slate-700 hover:text-white"
                }`}
            >
              <svg
                xmlns="http://www.w3.org/2000/svg"
                class="h-4 w-4"
                viewBox="0 0 20 20"
                fill="currentColor"
              >
                <path
                  fill-rule="evenodd"
                  d="M3 4a1 1 0 011-1h3a1 1 0 011 1v3a1 1 0 01-1 1H4a1 1 0 01-1-1V4zm2 2V5h1v1H5zM3 13a1 1 0 011-1h3a1 1 0 011 1v3a1 1 0 01-1 1H4a1 1 0 01-1-1v-3zm2 2v-1h1v1H5zM13 3a1 1 0 00-1 1v3a1 1 0 001 1h3a1 1 0 001-1V4a1 1 0 00-1-1h-3zm1 2v1h1V5h-1z"
                  clip-rule="evenodd"
                />
                <path
                  d="M11 16v-.5a.5.5 0 01.5-.5H12v-1h-.5a.5.5 0 01-.5-.5v-1a.5.5 0 01.5-.5H14v-1h-1.5a.5.5 0 01-.5-.5V10h-1v1.5a.5.5 0 01-.5.5H10v1h.5a.5.5 0 01.5.5v.5h1v.5a.5.5 0 00.5.5h1v.5a.5.5 0 01-.5.5H11v1h1.5a.5.5 0 00.5-.5V16h-1v-.5a.5.5 0 00-.5-.5H11z"
                />
              </svg>
              {showQrScanner ? "Stop Scanner" : "Scan QR Code"}
            </button>
          </div>

          <!-- QR Scanner Viewport -->
          {#if showQrScanner}
            <div
              class="relative rounded-lg overflow-hidden bg-slate-950 border border-slate-700"
            >
              <div
                id="qr-reader"
                class="w-full"
                style="min-height: 250px;"
              ></div>
              <p class="text-xs text-slate-500 text-center py-2">
                Point camera at QR code
              </p>
            </div>
          {:else}
            <!-- Manual Code Input -->
            <input
              type="text"
              bind:value={acceptPassword}
              placeholder="14-character code"
              maxlength="14"
              class="w-full px-4 py-3 bg-slate-950 border border-slate-700 rounded-lg text-xl font-mono text-center text-purple-400 tracking-wider focus:outline-none focus:border-purple-500 focus:ring-1 focus:ring-purple-500 transition-all placeholder:text-slate-600 placeholder:text-base placeholder:font-sans"
              on:keydown={(e) => e.key === "Enter" && confirmRedeemInvite()}
            />

            <p class="text-xs text-slate-500 text-center">
              {acceptPassword.length}/14 characters
            </p>
          {/if}

          <div class="flex gap-2 pt-2">
            <button
              on:click={() => {
                stopQrScanner();
                setStep("accept-invite-user");
              }}
              class="px-4 py-2 text-sm text-slate-400 hover:text-white transition-colors"
            >
              ← Back
            </button>
            <button
              on:click={confirmRedeemInvite}
              disabled={acceptLoading || acceptPassword.length !== 14}
              class="flex-1 px-6 py-2.5 bg-purple-600 hover:bg-purple-500 text-white font-semibold rounded-lg transition-all disabled:opacity-50"
            >
              {acceptLoading ? "Redeeming..." : "Redeem Invite"}
            </button>
          </div>
          {#if acceptError}
            <p class="text-sm text-red-400">{acceptError}</p>
          {/if}
        </div>
      {/if}

      <!-- Footer -->
      <div class="flex justify-end gap-2 pt-2 border-t border-slate-800">
        <button
          on:click={handleClose}
          class="px-4 py-2 text-sm text-slate-400 hover:text-white transition-colors"
        >
          Close
        </button>
      </div>
    </div>
  </div>
{/if}
