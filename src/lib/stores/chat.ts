import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import { derived, get, writable } from "svelte/store";
import { defaultGroupName, getChatKind } from "$lib/chatKind";
import { extractPeerIdFromChatId } from "$lib/chatIdentity";
import {
  api,
  type ArchivedChatResult,
  type DbMessage,
  type Envelope,
  type GroupChatResult,
  type SentMediaResult,
  type TemporaryChatResult,
} from "$lib/tauri/api";

export type Message = {
  id?: string;
  sender: string;
  text: string;
  timestamp: Date;
  status?: string;
  content_type?: string;
  file_hash?: string | null;
};

export type LocalPeer = { peer_id: string; addresses: string[] };

export type ChatState = {
  activeChatId: string;
  peers: string[];
  peerAliases: Record<string, string | null>;
  chatNames: Record<string, string>;
  groupChats: Record<string, boolean>;
  pinnedPeers: string[];
  envelopes: Envelope[];
  chatAssignments: Record<string, string>;
  currentEnvelope: string | null;
  searchQuery: string;
  latestMessageTimes: Record<string, number>;
  unreadCounts: Record<string, number>;
  localPeers: LocalPeer[];
  messages: Message[];
  peerAlias: string | null;
  activeConversationIds: Set<string>;
  closedChatId: string | null;
};

const defaultChatState: ChatState = {
  activeChatId: "",
  peers: [],
  peerAliases: {},
  chatNames: {},
  groupChats: {},
  pinnedPeers: [],
  envelopes: [],
  chatAssignments: {},
  currentEnvelope: null,
  searchQuery: "",
  latestMessageTimes: {},
  unreadCounts: {},
  localPeers: [],
  messages: [],
  peerAlias: null,
  activeConversationIds: new Set(),
  closedChatId: null,
};

export const chatState = writable<ChatState>({ ...defaultChatState });

let initPromise: Promise<UnlistenFn> | null = null;
let activeUnlisten: UnlistenFn | null = null;
let chatLoadSeq = 0;
let pendingStatusCache: Record<string, string> = {};

function uiChatId(chatId: string): string {
  return chatId === "self" ? "Me" : chatId;
}

function dbChatId(chatId: string): string {
  return chatId === "Me" ? "self" : chatId;
}

function peerKey(chatId: string): string {
  const normalized = uiChatId(chatId);
  return extractPeerIdFromChatId(normalized) || normalized;
}

function outgoingStatus(chatId: string): "pending" | "delivered" {
  const kind = getChatKind(chatId);
  return kind === "dm" || kind === "tempdm" ? "pending" : "delivered";
}

function messageFromDb(msg: DbMessage): Message {
  return {
    id: msg.id,
    sender: msg.peer_id === "Me" ? "Me" : msg.sender_alias || msg.peer_id,
    text: msg.text_content || "",
    timestamp: new Date(msg.timestamp * 1000),
    status: msg.status || "delivered",
    content_type: msg.content_type,
    file_hash: msg.file_hash,
  };
}

function applySentMediaMessage(
  result: SentMediaResult,
  contentType: string,
  text = "",
) {
  const state = get(chatState);
  const status = outgoingStatus(state.activeChatId);
  const newMsg: Message = {
    id: result.msg_id,
    sender: "Me",
    text,
    timestamp: new Date(),
    status,
    content_type: contentType,
    file_hash: result.file_hash,
  };
  chatState.update((current) => ({
    ...current,
    messages: [...current.messages, newMsg],
  }));
}

function computeSortedPeers(state: ChatState): string[] {
  let allPeers = [...state.peers];

  if (state.currentEnvelope) {
    allPeers = allPeers.filter(
      (peer) => state.chatAssignments[peer] === state.currentEnvelope,
    );
  } else {
    allPeers = allPeers.filter((peer) => !state.chatAssignments[peer]);
  }

  if (state.searchQuery.trim()) {
    const query = state.searchQuery.toLowerCase().trim();
    allPeers = allPeers.filter((peer) => {
      const name = peer.toLowerCase();
      let qi = 0;
      for (let i = 0; i < name.length && qi < query.length; i++) {
        if (name[i] === query[qi]) qi++;
      }
      return qi === query.length;
    });
  }

  const pinned = allPeers.filter((peer) => state.pinnedPeers.includes(peer));
  const others = allPeers.filter((peer) => !state.pinnedPeers.includes(peer));
  const byLatest = (a: string, b: string) =>
    (state.latestMessageTimes[b] || 0) - (state.latestMessageTimes[a] || 0);

  pinned.sort(byLatest);
  others.sort((a, b) => {
    if (a === "Me") return -1;
    if (b === "Me") return 1;
    return byLatest(a, b);
  });

  return [...pinned, ...others];
}

export const sortedPeers = derived(chatState, computeSortedPeers);

export async function refreshChats(): Promise<void> {
  const [
    chatList,
    aliases,
    pinnedPeers,
    envelopes,
    envelopeAssignments,
    latestTimesRaw,
    unreadRaw,
  ] = await Promise.all([
    api.getChatList(),
    api.getPeerAliases(),
    api.getPinnedPeers(),
    api.getEnvelopes(),
    api.getEnvelopeAssignments(),
    api.getChatLatestTimes(),
    api.getUnreadCounts("Me"),
  ]);

  const peers: string[] = [];
  const chatNames: Record<string, string> = {};
  const groupChats: Record<string, boolean> = {};
  for (const chat of chatList) {
    const id = uiChatId(chat.id);
    peers.push(id);
    chatNames[id] = chat.name;
    groupChats[id] = chat.is_group;
  }

  chatState.update((state) => ({
    ...state,
    peers,
    chatNames,
    groupChats,
    peerAliases: aliases,
    pinnedPeers: pinnedPeers.map(uiChatId),
    envelopes,
    chatAssignments: Object.fromEntries(
      envelopeAssignments.map((item) => [
        uiChatId(item.chat_id),
        item.envelope_id,
      ]),
    ),
    latestMessageTimes: Object.fromEntries(
      Object.entries(latestTimesRaw).map(([key, value]) => [uiChatId(key), value]),
    ),
    unreadCounts: Object.fromEntries(
      Object.entries(unreadRaw).map(([key, value]) => [uiChatId(key), value]),
    ),
  }));
}

export async function setActiveChat(chatId: string): Promise<void> {
  const normalized = uiChatId(chatId);
  const current = get(chatState);
  if (current.activeChatId === normalized && normalized) return;

  const seq = ++chatLoadSeq;
  if (!normalized) {
    chatState.update((state) => ({
      ...state,
      activeChatId: "",
      messages: [],
      peerAlias: null,
      activeConversationIds: new Set(),
    }));
    return;
  }

  chatState.update((state) => ({
    ...state,
    activeChatId: normalized,
    activeConversationIds: new Set([normalized]),
  }));

  const isCurrentLoad = () => {
    const state = get(chatState);
    return seq === chatLoadSeq && state.activeChatId === normalized;
  };

  try {
    const history = await api.getChatHistory(dbChatId(normalized));
    if (!isCurrentLoad()) return;

    const canonicalChatId = uiChatId(history[0]?.chat_id || dbChatId(normalized));
    chatState.update((state) => ({
      ...state,
      activeConversationIds: new Set([normalized, canonicalChatId]),
      messages: history.map(messageFromDb),
    }));

    if (normalized === "Me") {
      chatState.update((state) => ({ ...state, peerAlias: null }));
      return;
    }

    const chatList = await api.getChatList();
    if (!isCurrentLoad()) return;

    const chatRow = chatList.find((chat) => uiChatId(chat.id) === normalized);
    if (chatRow?.name) {
      chatState.update((state) => ({ ...state, peerAlias: chatRow.name }));
      return;
    }

    const aliases = await api.getPeerAliases();
    if (!isCurrentLoad()) return;
    chatState.update((state) => ({
      ...state,
      peerAlias: aliases[normalized] || null,
    }));
  } catch (e) {
    console.error("Failed to load history for", normalized, e);
    if (isCurrentLoad()) {
      chatState.update((state) => ({
        ...state,
        messages: [],
        peerAlias: null,
        activeConversationIds: new Set([normalized]),
      }));
    }
  }
}

export async function markChatRead(chatId: string): Promise<void> {
  const normalized = uiChatId(chatId);
  chatState.update((state) => {
    if (!state.unreadCounts[normalized]) return state;
    const { [normalized]: _, ...rest } = state.unreadCounts;
    return { ...state, unreadCounts: rest };
  });
  await api.markMessagesRead(dbChatId(normalized));
}

async function handleIncomingMessage(msg: DbMessage) {
  const relatedPeer = uiChatId(msg.chat_id);
  const relatedPeerKey = peerKey(relatedPeer);
  const now = Math.floor(Date.now() / 1000);
  const state = get(chatState);
  const isActive =
    state.activeConversationIds.has(relatedPeer) ||
    peerKey(state.activeChatId) === relatedPeerKey;

  chatState.update((current) => ({
    ...current,
    latestMessageTimes: {
      ...current.latestMessageTimes,
      [relatedPeer]: now,
    },
    unreadCounts:
      isActive || relatedPeer === current.activeChatId
        ? current.unreadCounts
        : {
            ...current.unreadCounts,
            [relatedPeer]: (current.unreadCounts[relatedPeer] || 0) + 1,
          },
    messages: isActive
      ? [...current.messages, messageFromDb(msg)]
      : current.messages,
  }));

  if (isActive && msg.peer_id !== "Me") {
    await api.markMessagesRead(msg.chat_id).catch((e) => {
      console.error("Failed to send read receipt:", e);
    });
  }
}

function handleStatusUpdate(payload: any) {
  const { msg_id, status } = payload || {};
  if (!msg_id || !status) return;

  const state = get(chatState);
  if (state.messages.some((message) => message.id === msg_id)) {
    chatState.update((current) => ({
      ...current,
      messages: current.messages.map((message) =>
        message.id === msg_id ? { ...message, status } : message,
      ),
    }));
  } else {
    pendingStatusCache[msg_id] = status;
  }
}

export async function initChatStore(): Promise<UnlistenFn> {
  if (activeUnlisten) return activeUnlisten;
  if (initPromise) return initPromise;

  initPromise = (async () => {
    await refreshChats();
    const cleanups: UnlistenFn[] = [];

    cleanups.push(
      await listen("local-peer-discovered", (event: any) => {
        const peer = event.payload as LocalPeer;
        chatState.update((state) => {
          if (state.localPeers.find((item) => item.peer_id === peer.peer_id)) {
            return state;
          }
          return { ...state, localPeers: [...state.localPeers, peer] };
        });
      }),
    );

    cleanups.push(
      await listen("local-peer-expired", (event: any) => {
        chatState.update((state) => ({
          ...state,
          localPeers: state.localPeers.filter(
            (peer) => peer.peer_id !== event.payload,
          ),
        }));
      }),
    );

    cleanups.push(
      await listen<DbMessage>("message-received", (event) => {
        void handleIncomingMessage(event.payload);
      }),
    );

    cleanups.push(
      await listen("message-status-updated", (event: any) => {
        handleStatusUpdate(event.payload);
      }),
    );

    cleanups.push(
      await listen("new-github-chat", (event: any) => {
        const chatId = event.payload?.chat_id;
        if (!chatId) return;
        chatState.update((state) => {
          if (state.peers.includes(chatId)) return state;
          return { ...state, peers: [...state.peers, chatId] };
        });
      }),
    );

    cleanups.push(
      await listen("temporary-chat-connected", () => {
        void refreshChats();
      }),
    );

    cleanups.push(
      await listen("temporary-chat-ended", (event: any) => {
        const chatId = event.payload?.chat_id;
        chatState.update((state) => ({
          ...state,
          closedChatId: chatId || state.closedChatId,
        }));
        void refreshChats();
      }),
    );

    activeUnlisten = () => {
      while (cleanups.length > 0) {
        cleanups.pop()?.();
      }
      activeUnlisten = null;
      initPromise = null;
    };
    return activeUnlisten;
  })().catch((e) => {
    initPromise = null;
    throw e;
  });

  return initPromise;
}

export function resetChatStore() {
  if (activeUnlisten) {
    activeUnlisten();
  }
  chatLoadSeq++;
  pendingStatusCache = {};
  chatState.set({ ...defaultChatState, activeConversationIds: new Set() });
  activeUnlisten = null;
  initPromise = null;
}

export function setSearchQuery(searchQuery: string) {
  chatState.update((state) => ({ ...state, searchQuery }));
}

export function selectEnvelope(envelopeId: string | null) {
  chatState.update((state) => ({ ...state, currentEnvelope: envelopeId }));
}

export function clearClosedChatMarker() {
  chatState.update((state) => ({ ...state, closedChatId: null }));
}

export async function togglePinPeer(peerId: string): Promise<void> {
  const isPinned = await api.togglePinPeer(peerId);
  chatState.update((state) => ({
    ...state,
    pinnedPeers: isPinned
      ? [...new Set([...state.pinnedPeers, peerId])]
      : state.pinnedPeers.filter((peer) => peer !== peerId),
  }));
}

export async function deleteChat(chatId: string): Promise<void> {
  const kind = getChatKind(chatId);
  if (kind === "group") {
    await api.leaveGroupChat(chatId);
  } else if (kind === "tempgroup" || kind === "tempdm") {
    await api.cancelTemporaryInvite().catch(() => {});
  } else {
    await api.deletePeer(chatId);
  }
  chatState.update((state) => ({
    ...state,
    peers: state.peers.filter((peer) => peer !== chatId),
  }));
}

export async function saveTemporaryChatToArchive(
  chatId: string,
): Promise<ArchivedChatResult> {
  const result = await api.saveTemporaryChatToArchive(chatId);
  await refreshChats();
  return result;
}

export async function moveChatToEnvelope(
  chatId: string,
  envelopeId: string | null,
): Promise<void> {
  const normalized = uiChatId(chatId);
  await api.moveChatToEnvelope(dbChatId(normalized), envelopeId);
  chatState.update((state) => {
    const next = { ...state.chatAssignments };
    if (envelopeId) {
      next[normalized] = envelopeId;
    } else {
      delete next[normalized];
    }
    return { ...state, chatAssignments: next };
  });
}

export async function createEnvelope(
  name: string,
  icon?: string | null,
): Promise<void> {
  const id = `env_${Date.now()}`;
  await api.createEnvelope(id, name, icon);
  chatState.update((state) => ({
    ...state,
    envelopes: [...state.envelopes, { id, name, icon }],
  }));
}

export async function updateEnvelope(
  id: string,
  name: string,
  icon?: string | null,
): Promise<void> {
  await api.updateEnvelope(id, name, icon);
  chatState.update((state) => ({
    ...state,
    envelopes: state.envelopes.map((envelope) =>
      envelope.id === id ? { ...envelope, name, icon } : envelope,
    ),
  }));
}

export async function deleteEnvelope(id: string): Promise<void> {
  await api.deleteEnvelope(id);
  chatState.update((state) => ({
    ...state,
    envelopes: state.envelopes.filter((envelope) => envelope.id !== id),
    currentEnvelope: state.currentEnvelope === id ? null : state.currentEnvelope,
  }));
}

export async function createGroup(name: string): Promise<GroupChatResult> {
  const result = await api.createGroupChat(name || null);
  await refreshChats();
  return result;
}

export async function joinGroup(
  chatId: string,
  name: string,
): Promise<GroupChatResult> {
  const result = await api.joinGroupChat(chatId, name || defaultGroupName(chatId));
  await refreshChats();
  return result;
}

export async function openTemporaryGroup(chatId: string): Promise<void> {
  await refreshChats();
  await setActiveChat(chatId);
}

export async function redeemTemporaryInvite(
  deepLink: string,
): Promise<TemporaryChatResult> {
  const result = await api.redeemTemporaryInvite(deepLink);
  await refreshChats();
  return result;
}

export async function sendActiveChatMessage(text: string): Promise<void> {
  const trimmed = text.trim();
  if (!trimmed) return;

  const state = get(chatState);
  const activeChatId = state.activeChatId;
  if (!activeChatId || getChatKind(activeChatId) === "archived") return;

  const tempId = `temp-${Date.now()}-${Math.random().toString(36).slice(2)}`;
  const tempMsg: Message = {
    id: tempId,
    sender: "Me",
    text: trimmed,
    timestamp: new Date(),
    status: outgoingStatus(activeChatId),
  };
  chatState.update((current) => ({
    ...current,
    messages: [...current.messages, tempMsg],
  }));

  try {
    if (activeChatId === "Me") {
      await api.sendMessageToSelf(trimmed);
      chatState.update((current) => ({
        ...current,
        messages: current.messages.map((message) =>
          message.id === tempId ? { ...message, status: "read" } : message,
        ),
      }));
      return;
    }

    const msgId = await api.sendMessage(activeChatId, trimmed);
    const cachedStatus = pendingStatusCache[msgId];
    if (cachedStatus) {
      delete pendingStatusCache[msgId];
    }
    chatState.update((current) => ({
      ...current,
      messages: current.messages.map((message) =>
        message.id === tempId
          ? {
              ...message,
              id: msgId,
              status: cachedStatus || message.status,
            }
          : message,
      ),
    }));
  } catch (e) {
    console.error("Send failed:", e);
    chatState.update((current) => ({
      ...current,
      messages: current.messages.map((message) =>
        message.id === tempId ? { ...message, status: "failed" } : message,
      ),
    }));
  }
}

export function addSentImageMessage(result: SentMediaResult) {
  applySentMediaMessage(result, "image", "");
}

export function addSentDocumentMessage(result: SentMediaResult, fileName: string) {
  applySentMediaMessage(result, "document", fileName);
}

export function addSentVideoMessage(result: SentMediaResult, fileName: string) {
  applySentMediaMessage(result, "video", fileName);
}

export function addSentAudioMessage(result: SentMediaResult, fileName: string) {
  applySentMediaMessage(result, "audio", fileName);
}

export function addSentStickerMessage(result: SentMediaResult) {
  applySentMediaMessage(result, "sticker", "");
}
