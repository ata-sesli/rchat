export type DirectScope = "gh" | "lh";

export type ParsedDirectChatId = {
  scope: DirectScope;
  name: string;
  peerId: string;
};

const PEER_ID_RE = /^12D3Koo[1-9A-HJ-NP-Za-km-z]+$/;

export function parseScopedDirectChatId(chatId: string): ParsedDirectChatId | null {
  const parseWithPrefix = (prefix: DirectScope): ParsedDirectChatId | null => {
    const rawPrefix = `${prefix}:`;
    if (!chatId.startsWith(rawPrefix)) return null;
    const rest = chatId.slice(rawPrefix.length);
    const idx = rest.lastIndexOf("-");
    if (idx <= 0 || idx >= rest.length - 1) return null;
    const name = rest.slice(0, idx);
    const peerId = rest.slice(idx + 1);
    if (!name || !PEER_ID_RE.test(peerId)) return null;
    return { scope: prefix, name, peerId };
  };

  return parseWithPrefix("gh") || parseWithPrefix("lh");
}

export function extractPeerIdFromChatId(chatId: string): string | null {
  const parsed = parseScopedDirectChatId(chatId);
  if (parsed) return parsed.peerId;
  if (PEER_ID_RE.test(chatId)) return chatId;
  return null;
}

export function githubUsernameFromChatId(chatId: string): string | null {
  const parsed = parseScopedDirectChatId(chatId);
  if (parsed?.scope === "gh") return parsed.name;
  if (chatId.startsWith("gh:")) {
    const legacy = chatId.slice(3);
    return legacy || null;
  }
  return null;
}

export function displayNameFromChatId(chatId: string): string {
  const parsed = parseScopedDirectChatId(chatId);
  if (parsed) return parsed.name;
  if (chatId.startsWith("gh:")) return chatId.slice(3);
  if (chatId.startsWith("lh:")) return chatId.slice(3);
  return chatId;
}

export function directPeerKey(chatId: string): string | null {
  const peerId = extractPeerIdFromChatId(chatId);
  return peerId ? `peer:${peerId}` : null;
}
