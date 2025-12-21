<script lang="ts">
  import { tick } from "svelte";
  import MessageBubble from "./MessageBubble.svelte";

  // Types
  type Message = { sender: string; text: string; timestamp: Date };

  // Props
  export let activePeer = "Me";
  export let messages: Message[] = [];
  export let userProfile: { alias: string | null; avatar_path: string | null } =
    { alias: null, avatar_path: null };
  export let message = "";
  export let showAttachments = false;

  // Callback props
  export let onsend = (msg: string) => {};
  export let ontoggleAttachments = (show: boolean) => {};

  // Refs
  let chatContainer: HTMLElement;
  let textarea: HTMLTextAreaElement;

  // Expose scrollToBottom
  export async function scrollToBottom() {
    await tick();
    if (chatContainer) {
      chatContainer.scrollTo({
        top: chatContainer.scrollHeight,
        behavior: "smooth",
      });
    }
  }

  function handleKeydown(e: KeyboardEvent) {
    if (e.key === "Enter" && !e.shiftKey) {
      e.preventDefault();
      sendMessage();
    }
  }

  function sendMessage() {
    if (!message.trim()) return;
    onsend(message);
    message = "";
    if (textarea) {
      textarea.style.height = "auto";
    }
  }

  function toggleAttachments() {
    showAttachments = !showAttachments;
    ontoggleAttachments(showAttachments);
  }

  function handleInput(e: Event) {
    const target = e.currentTarget as HTMLTextAreaElement;
    target.style.height = "auto";
    target.style.height = target.scrollHeight + "px";
  }

  // Auto-scroll when messages change
  $: if (messages.length > 0 && chatContainer) {
    scrollToBottom();
  }
</script>

<!-- Chat Header -->
<div
  class="h-16 flex items-center px-6 border-b border-slate-800/50 bg-slate-900/10 backdrop-blur-sm"
>
  <div class="flex items-center gap-3">
    <span class="text-xl font-bold text-white">
      {#if activePeer === "Me"}
        Me (You)
      {:else if activePeer === "General"}
        # General
      {:else}
        @ {activePeer}
      {/if}
    </span>
    {#if activePeer !== "Me" && activePeer !== "General"}
      <div
        class="w-2 h-2 rounded-full bg-green-500 shadow-lg shadow-green-500/50"
      ></div>
    {/if}
  </div>
</div>

<!-- Messages -->
<div
  bind:this={chatContainer}
  class="flex-1 overflow-y-auto px-6 py-6 space-y-6 scroll-smooth"
>
  {#if messages.length === 0}
    <div
      class="flex flex-col items-center justify-center h-full text-slate-500 space-y-4 opacity-0 animate-fade-in-up"
      style="animation-fill-mode: forwards;"
    >
      <div
        class="w-16 h-16 rounded-2xl bg-slate-900 border border-slate-800 flex items-center justify-center"
      >
        <span class="text-3xl">👋</span>
      </div>
      <p>
        {#if activePeer === "Me"}
          This is your personal space.
        {:else}
          Start chatting with {activePeer}!
        {/if}
      </p>
    </div>
  {/if}

  {#each messages as msg}
    <MessageBubble {msg} {userProfile} {activePeer} />
  {/each}
</div>

<!-- Input Area -->
<div class="p-6 w-full max-w-4xl mx-auto">
  <div
    class="bg-slate-900/90 backdrop-blur-md border border-slate-700 rounded-2xl p-1.5 shadow-2xl flex items-center gap-2 relative"
  >
    <!-- Attachments Button -->
    <div class="relative">
      <button
        on:click={toggleAttachments}
        class={`p-2 rounded-xl transition-all ${showAttachments ? "bg-slate-700 text-teal-400" : "text-slate-400 hover:text-white hover:bg-slate-800"}`}
        title="Add Attachment"
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
            d="M15.172 7l-6.586 6.586a2 2 0 102.828 2.828l6.414-6.586a4 4 0 00-5.656-5.656l-6.415 6.585a6 6 0 108.486 8.486L20.5 13"
          />
        </svg>
      </button>

      {#if showAttachments}
        <div
          class="absolute bottom-full left-0 mb-2 w-48 bg-slate-800 border border-slate-700 rounded-xl shadow-xl overflow-hidden z-50 animate-fade-in-up"
        >
          <button
            class="w-full text-left px-4 py-3 text-sm text-slate-200 hover:bg-slate-700 hover:text-white flex items-center gap-3 transition-colors"
          >
            <svg
              xmlns="http://www.w3.org/2000/svg"
              class="h-5 w-5 text-purple-400"
              fill="none"
              viewBox="0 0 24 24"
              stroke="currentColor"
            >
              <path
                stroke-linecap="round"
                stroke-linejoin="round"
                stroke-width="2"
                d="M4 16l4.586-4.586a2 2 0 012.828 0L16 16m-2-2l1.586-1.586a2 2 0 012.828 0L20 14m-6-6h.01M6 20h12a2 2 0 002-2V6a2 2 0 00-2-2H6a2 2 0 00-2 2v12a2 2 0 002 2z"
              />
            </svg>
            Image / Video
          </button>
          <div class="h-px bg-slate-700/50"></div>
          <button
            class="w-full text-left px-4 py-3 text-sm text-slate-200 hover:bg-slate-700 hover:text-white flex items-center gap-3 transition-colors"
          >
            <svg
              xmlns="http://www.w3.org/2000/svg"
              class="h-5 w-5 text-blue-400"
              fill="none"
              viewBox="0 0 24 24"
              stroke="currentColor"
            >
              <path
                stroke-linecap="round"
                stroke-linejoin="round"
                stroke-width="2"
                d="M9 12h6m-6 4h6m2 5H7a2 2 0 01-2-2V5a2 2 0 012-2h5.586a1 1 0 01.707.293l5.414 5.414a1 1 0 01.293.707V19a2 2 0 01-2 2z"
              />
            </svg>
            Document
          </button>
          <div class="h-px bg-slate-700/50"></div>
          <button
            class="w-full text-left px-4 py-3 text-sm text-slate-200 hover:bg-slate-700 hover:text-white flex items-center gap-3 transition-colors"
          >
            <svg
              xmlns="http://www.w3.org/2000/svg"
              class="h-5 w-5 text-pink-400"
              fill="none"
              viewBox="0 0 24 24"
              stroke="currentColor"
            >
              <path
                stroke-linecap="round"
                stroke-linejoin="round"
                stroke-width="2"
                d="M19 11a7 7 0 01-7 7m0 0a7 7 0 01-7-7m7 7v4m0 0H8m4 0h4m-4-8a3 3 0 01-3-3V5a3 3 0 116 0v6a3 3 0 01-3 3z"
              />
            </svg>
            Audio
          </button>
        </div>
      {/if}
    </div>

    <textarea
      bind:this={textarea}
      bind:value={message}
      on:keydown={handleKeydown}
      on:input={handleInput}
      placeholder={`Message ${activePeer}...`}
      rows="1"
      class="flex-1 bg-transparent text-slate-100 placeholder:text-slate-600 px-4 py-2.5 focus:outline-none min-w-0 resize-none overflow-hidden max-h-32 self-end mb-1"
    ></textarea>

    <button
      on:click={sendMessage}
      class="bg-teal-500 hover:bg-teal-400 text-slate-950 p-2.5 rounded-xl font-semibold transition-all hover:scale-105 active:scale-95 shadow-lg shadow-teal-500/20 disabled:opacity-50 disabled:cursor-not-allowed"
      disabled={!message.trim()}
    >
      <svg
        xmlns="http://www.w3.org/2000/svg"
        viewBox="0 0 20 20"
        fill="currentColor"
        class="w-5 h-5"
      >
        <path
          d="M3.105 2.289a.75.75 0 00-.826.95l1.414 4.925A1.5 1.5 0 005.135 9.25h6.115a.75.75 0 010 1.5H5.135a1.5 1.5 0 00-1.442 1.086l-1.414 4.926a.75.75 0 00.826.95 28.896 28.896 0 0015.293-7.154.75.75 0 000-1.115A28.897 28.897 0 003.105 2.289z"
        />
      </svg>
    </button>
  </div>
</div>
