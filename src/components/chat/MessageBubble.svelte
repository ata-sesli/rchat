<script lang="ts">
  export let msg: { sender: string; text: string; timestamp: Date };
  export let userProfile: { alias: string | null; avatar_path: string | null };
  export let activePeer: string;

  $: isMe = msg.sender === "Me";

  function formatTime(date: Date): string {
    return new Date(date).toLocaleTimeString([], {
      hour: "2-digit",
      minute: "2-digit",
    });
  }
</script>

<div
  class={`flex w-full ${isMe ? "justify-end" : "justify-start"} animate-fade-in-up`}
>
  <div
    class={`flex max-w-[80%] md:max-w-[60%] gap-3 ${isMe ? "flex-row-reverse" : "flex-row"}`}
  >
    <!-- Avatar -->
    <div class="shrink-0 self-end mb-1">
      {#if isMe}
        {#if userProfile.avatar_path}
          <img
            src={userProfile.avatar_path}
            class="w-8 h-8 rounded-full bg-teal-500 border-2 border-slate-950 object-cover"
            alt="Me"
          />
        {:else}
          <div
            class="w-8 h-8 rounded-full bg-teal-500 shadow-lg shadow-teal-500/20 border-2 border-slate-950"
          ></div>
        {/if}
      {:else}
        <img
          src={`https://github.com/${activePeer}.png?size=32`}
          class="w-8 h-8 rounded-full bg-purple-500 shadow-lg shadow-purple-500/20 border-2 border-slate-950"
          on:error={(e) =>
            ((e.currentTarget as HTMLImageElement).src =
              "https://github.com/github.png?size=32")}
          alt="Peer"
        />
      {/if}
    </div>

    <!-- Bubble -->
    <div
      class={`px-4 py-2.5 shadow-md text-sm leading-relaxed break-words flex flex-col gap-1
        ${isMe ? "bg-teal-600/90 text-white rounded-2xl rounded-tr-sm" : "bg-slate-800 text-slate-200 rounded-2xl rounded-tl-sm border border-slate-700/50"}`}
    >
      <span>{msg.text}</span>
      <span
        class={`text-[10px] ${isMe ? "text-teal-200" : "text-slate-400"} self-end`}
      >
        {formatTime(msg.timestamp)}
      </span>
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
