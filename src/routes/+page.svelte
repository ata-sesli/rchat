<script lang="ts">
  import { invoke } from "@tauri-apps/api/core";
  import { listen } from "@tauri-apps/api/event";

  let message = "";
  let logs: string[] = [];
  let chatContainer: HTMLDivElement;

  // Auto-scroll to bottom when new messages arrive
  $: if (logs.length && chatContainer) {
    setTimeout(() => {
      chatContainer.scrollTop = chatContainer.scrollHeight;
    }, 0);
  }

  // 1. Send Function
  async function sendMessage() {
    if (!message.trim()) return;
    // Call Rust!
    await invoke("send_chat_message", { message });
    logs = [...logs, `Me: ${message}`];
    message = "";
  }

  // 2. Listen for Incoming P2P Messages
  // (We defined this emission in manager.rs!)
  listen("p2p-message", (event) => {
    logs = [...logs, `Peer: ${event.payload}`];
  });

  // Handle Enter key to send message
  function handleKeydown(e: KeyboardEvent) {
    if (e.key === "Enter" && !e.shiftKey) {
      e.preventDefault();
      sendMessage();
    }
  }
</script>

<main class="flex flex-col h-screen bg-gray-900 text-white">
  <!-- Header -->
  <div class="bg-gray-800 border-b border-gray-700 px-6 py-4 shadow-lg">
    <h1 class="text-2xl font-bold text-blue-400">RChat P2P</h1>
    <p class="text-xs text-gray-400 mt-1">Decentralized peer-to-peer messaging</p>
  </div>

  <!-- Chat Messages Container -->
  <div 
    bind:this={chatContainer}
    class="flex-1 overflow-y-auto px-6 py-4 space-y-3"
  >
    {#if logs.length === 0}
      <div class="flex items-center justify-center h-full">
        <p class="text-gray-500 text-center">
          No messages yet. Start a conversation!
        </p>
      </div>
    {/if}

    {#each logs as log (log)}
      <div class={`flex ${log.startsWith('Me:') ? 'justify-end' : 'justify-start'}`}>
        <div 
          class={`max-w-xs px-4 py-2 rounded-lg ${
            log.startsWith('Me:')
              ? 'bg-blue-600 text-white rounded-br-none'
              : 'bg-gray-700 text-gray-100 rounded-bl-none'
          }`}
        >
          <p class="text-sm break-words">
            {log.replace(/^(Me:|Peer:)\s*/, '')}
          </p>
        </div>
      </div>
    {/each}
  </div>

  <!-- Input Area -->
  <div class="bg-gray-800 border-t border-gray-700 px-6 py-4 shadow-lg">
    <div class="flex gap-2">
      <input
        bind:value={message}
        on:keydown={handleKeydown}
        type="text"
        placeholder="Type a message..."
        class="flex-1 bg-gray-700 text-white px-4 py-3 rounded-lg border border-gray-600 focus:border-blue-500 focus:outline-none transition"
      />
      <button
        on:click={sendMessage}
        class="bg-blue-600 hover:bg-blue-700 text-white px-6 py-3 rounded-lg font-semibold transition transform hover:scale-105 active:scale-95"
      >
        Send
      </button>
    </div>
  </div>
</main>

<style>
  :global(body) {
    margin: 0;
    padding: 0;
    overflow: hidden;
  }

  :global(html) {
    overflow: hidden;
  }
</style>