export type ChatKind =
  | "self"
  | "dm"
  | "group"
  | "tempdm"
  | "tempgroup"
  | "archived";

const GROUP_ID_RE =
  /^group:[0-9a-fA-F]{8}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{12}$/;
const TEMP_GROUP_ID_RE =
  /^temp-group:[0-9a-fA-F]{8}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{12}$/;
const TEMP_DM_ID_RE =
  /^tempdm:[0-9a-fA-F]{8}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{12}$/;

export function isArchivedChatId(chatId: string): boolean {
  return chatId.startsWith("archived:");
}

export function isGroupChatId(chatId: string): boolean {
  return GROUP_ID_RE.test(chatId);
}

export function isTemporaryGroupChatId(chatId: string): boolean {
  return TEMP_GROUP_ID_RE.test(chatId);
}

export function isTemporaryDirectChatId(chatId: string): boolean {
  return TEMP_DM_ID_RE.test(chatId);
}

export function isTemporaryChatId(chatId: string): boolean {
  return isTemporaryDirectChatId(chatId) || isTemporaryGroupChatId(chatId);
}

export function getChatKind(chatId: string): ChatKind {
  if (chatId === "Me" || chatId === "self") return "self";
  if (isArchivedChatId(chatId)) return "archived";
  if (isGroupChatId(chatId)) return "group";
  if (isTemporaryGroupChatId(chatId)) return "tempgroup";
  if (isTemporaryDirectChatId(chatId)) return "tempdm";
  return "dm";
}

export function defaultGroupName(chatId: string): string {
  const suffix = chatId.split(":")[1]?.split("-")[0] ?? "unknown";
  return `Group ${suffix}`;
}

export function defaultTemporaryGroupName(chatId: string): string {
  const suffix = chatId.split(":")[1]?.split("-")[0] ?? "unknown";
  return `Temporary Group ${suffix}`;
}
