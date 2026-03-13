import { invoke } from "@tauri-apps/api/core";

export const COMMANDS = {
  saveApiToken: "save_api_token",
  checkAuthStatus: "check_auth_status",
  getConnectivitySettings: "get_connectivity_settings",
  setConnectivityMode: "set_connectivity_mode",
  updateConnectivitySettings: "update_connectivity_settings",
  toggleOnlineStatus: "toggle_online_status",
  initVault: "init_vault",
  unlockVault: "unlock_vault",
  startNetwork: "start_network",
  startGithubAuth: "start_github_auth",
  pollGithubAuth: "poll_github_auth",
  resetVault: "reset_vault",
  getFriends: "get_friends",
  getTrustedPeers: "get_trusted_peers",
  getPeerAliases: "get_peer_aliases",
  addFriend: "add_friend",
  deletePeer: "delete_peer",
  removeFriend: "remove_friend",
  getUserProfile: "get_user_profile",
  getTheme: "get_theme",
  updateTheme: "update_theme",
  listThemePresets: "list_theme_presets",
  applyPreset: "apply_preset",
  getSelectedPreset: "get_selected_preset",
  generateSimpleTheme: "generate_simple_theme",
  createCustomTheme: "create_custom_theme",
  updateCustomTheme: "update_custom_theme",
  deleteCustomTheme: "delete_custom_theme",
  updateUserProfile: "update_user_profile",
  getPinnedPeers: "get_pinned_peers",
  togglePinPeer: "toggle_pin_peer",
  getChatLatestTimes: "get_chat_latest_times",
  getChatList: "get_chat_list",
  getChatDetailsOverview: "get_chat_details_overview",
  getChatStats: "get_chat_stats",
  listChatFiles: "list_chat_files",
  dropChatConnection: "drop_chat_connection",
  forceChatReconnect: "force_chat_reconnect",
  saveTemporaryChatToArchive: "save_temporary_chat_to_archive",
  createGroupChat: "create_group_chat",
  joinGroupChat: "join_group_chat",
  leaveGroupChat: "leave_group_chat",
  sendMessageToSelf: "send_message_to_self",
  sendMessage: "send_message",
  getChatHistory: "get_chat_history",
  markMessagesRead: "mark_messages_read",
  getUnreadCounts: "get_unread_counts",
  createEnvelope: "create_envelope",
  updateEnvelope: "update_envelope",
  deleteEnvelope: "delete_envelope",
  getEnvelopes: "get_envelopes",
  moveChatToEnvelope: "move_chat_to_envelope",
  getEnvelopeAssignments: "get_envelope_assignments",
  requestConnection: "request_connection",
  setFastDiscovery: "set_fast_discovery",
  sendImageMessage: "send_image_message",
  getImageData: "get_image_data",
  getImageFromPath: "get_image_from_path",
  saveImageToFile: "save_image_to_file",
  sendDocumentMessage: "send_document_message",
  saveDocumentToFile: "save_document_to_file",
  sendVideoMessage: "send_video_message",
  getVideoData: "get_video_data",
  sendAudioMessage: "send_audio_message",
  getAudioData: "get_audio_data",
  saveAudioToFile: "save_audio_to_file",
  listStickers: "list_stickers",
  addSticker: "add_sticker",
  addStickersBatch: "add_stickers_batch",
  deleteSticker: "delete_sticker",
  sendStickerMessage: "send_sticker_message",
  saveStickerFromMessage: "save_sticker_from_message",
  generateInvitePassword: "generate_invite_password",
  createInvite: "create_invite",
  redeemAndConnect: "redeem_and_connect",
  createTemporaryInvite: "create_temporary_invite",
  redeemTemporaryInvite: "redeem_temporary_invite",
  getActiveTemporaryInvite: "get_active_temporary_invite",
  cancelTemporaryInvite: "cancel_temporary_invite",
  startVoiceCall: "start_voice_call",
  acceptVoiceCall: "accept_voice_call",
  rejectVoiceCall: "reject_voice_call",
  endVoiceCall: "end_voice_call",
  setVoiceCallMuted: "set_voice_call_muted",
  startVideoCall: "start_video_call",
  acceptVideoCall: "accept_video_call",
  rejectVideoCall: "reject_video_call",
  endVideoCall: "end_video_call",
  setVideoCallMuted: "set_video_call_muted",
  setVideoCallCameraEnabled: "set_video_call_camera_enabled",
  sendVideoCallChunk: "send_video_call_chunk",
  getVoiceCallState: "get_voice_call_state",
  startScreenBroadcast: "start_screen_broadcast",
  acceptScreenBroadcast: "accept_screen_broadcast",
  rejectScreenBroadcast: "reject_screen_broadcast",
  endScreenBroadcast: "end_screen_broadcast",
  sendScreenBroadcastChunk: "send_screen_broadcast_chunk",
  getBroadcastState: "get_broadcast_state",
  getConnectedChatIds: "get_connected_chat_ids",
} as const;

export type FriendConfig = {
  username: string;
  alias?: string | null;
  x25519_pubkey?: string | null;
  ed25519_pubkey?: string | null;
  leaf_index: number;
  encrypted_leaf_key?: string | null;
  nonce?: string | null;
};

export type AuthStatus = {
  is_setup: boolean;
  is_unlocked: boolean;
  is_github_connected: boolean;
  is_online: boolean;
  connectivity: ConnectivitySettings;
};

export type ConnectivityMode = "invisible" | "lan" | "reachable" | "custom";

export type ConnectivitySettings = {
  mode: ConnectivityMode;
  mdns_enabled: boolean;
  github_sync_enabled: boolean;
  nat_keepalive_enabled: boolean;
  punch_assist_enabled: boolean;
};

export type ConnectivitySettingsPatch = {
  mdns_enabled?: boolean;
  github_sync_enabled?: boolean;
  nat_keepalive_enabled?: boolean;
  punch_assist_enabled?: boolean;
};

export type UserProfile = {
  alias: string | null;
  avatar_path: string | null;
};

export type ThemeConfig = {
  base: {
    "950": string;
    "900": string;
    "800": string;
    "700": string;
    "600": string;
    "500": string;
    "400": string;
    "300": string;
    "200": string;
    "100": string;
  };
  primary: { "600": string; "500": string; "400": string; "300": string };
  secondary: { "600": string; "500": string; "400": string; "300": string };
  error: { "600": string; "500": string; "400": string; "300": string };
  success: { "600": string; "500": string; "400": string; "300": string };
  info: { "600": string; "500": string; "400": string; "300": string };
  warning: { "600": string; "500": string; "400": string; "300": string };
};

export type PresetInfo = {
  key: string;
  name: string;
  description: string;
  source: "builtin" | "custom";
  created_at?: number | null;
  updated_at?: number | null;
  theme?: ThemeConfig | null;
};

export type DbMessage = {
  id: string;
  chat_id: string;
  peer_id: string;
  timestamp: number;
  content_type: string;
  text_content?: string | null;
  file_hash?: string | null;
  status: string;
  content_metadata?: string | null;
  sender_alias?: string | null;
};

export type Envelope = {
  id: string;
  name: string;
  icon?: string | null;
};

export type ChatListItem = {
  id: string;
  name: string;
  is_group: boolean;
};

export type ChatConnectionView = {
  connected: boolean;
  remote_addr?: string | null;
  connected_since?: number | null;
  last_connected_at?: number | null;
  first_connected_at?: number | null;
  reconnect_count: number;
};

export type ChatDetailsOverview = {
  chat_id: string;
  peer_id: string;
  peer_name: string;
  peer_alias?: string | null;
  avatar_url?: string | null;
  connection: ChatConnectionView;
};

export type ChatContentBreakdown = {
  text: number;
  sticker: number;
  image: number;
  video: number;
  audio: number;
  document: number;
};

export type ChatStats = {
  sent_total: number;
  received_total: number;
  sent: ChatContentBreakdown;
  received: ChatContentBreakdown;
  reconnect_count: number;
};

export type ChatFileFilter =
  | "all"
  | "sticker"
  | "image"
  | "video"
  | "document"
  | "audio";

export type ChatFileRow = {
  message_id: string;
  timestamp: number;
  content_type: string;
  file_hash: string;
  file_name?: string | null;
  size_bytes?: number | null;
  mime_type?: string | null;
  sender: string;
};

export type GroupChatResult = {
  chat_id: string;
  name: string;
};

export type ArchivedChatResult = {
  chat_id: string;
  name: string;
};

export type TemporaryInvitePayload = {
  version: number;
  kind: "dm" | "group";
  chat_id: string;
  inviter_peer_id: string;
  inviter_username: string;
  inviter_addr: string;
  created_at: number;
  expires_at: number;
};

export type TemporaryInviteView = {
  deep_link: string;
  payload: TemporaryInvitePayload;
  remaining_seconds: number;
};

export type TemporaryChatResult = {
  chat_id: string;
  name: string;
  kind: "dm" | "group";
  expires_at: number;
  peer_id?: string | null;
};

export type SentMediaResult = {
  msg_id: string;
  file_hash: string;
  file_name?: string | null;
};

export type VoiceCallPhase =
  | "idle"
  | "outgoing_ringing"
  | "incoming_ringing"
  | "active"
  | "ending";

export type CallKind = "voice" | "video";
export type VideoChunkType = "key" | "delta";
export type BroadcastPhase =
  | "idle"
  | "outgoing_ringing"
  | "incoming_ringing"
  | "active"
  | "ending";
export type BroadcastChunkType = "key" | "delta";

export type VoiceCallState = {
  phase: VoiceCallPhase;
  call_kind?: CallKind | null;
  call_id?: string | null;
  peer_id?: string | null;
  started_at?: number | null;
  ring_expires_at?: number | null;
  muted: boolean;
  camera_enabled?: boolean;
  reason?: string | null;
};

export type BroadcastState = {
  phase: BroadcastPhase;
  session_id?: string | null;
  peer_id?: string | null;
  started_at?: number | null;
  ring_expires_at?: number | null;
  is_host: boolean;
  reason?: string | null;
};

export type StickerItem = {
  file_hash: string;
  name?: string | null;
  created_at: number;
  size_bytes: number;
};

export type AddStickerResult = {
  file_hash: string;
  name: string;
  converted: boolean;
  already_exists: boolean;
};

export type StickerImportResult = {
  file_path: string;
  file_hash?: string | null;
  error?: string | null;
};

export type StickerBatchImportResult = {
  success_count: number;
  failure_count: number;
  results: StickerImportResult[];
};

export type GithubAuthState = {
  device_code: string;
  user_code: string;
  verification_uri: string;
  interval: number;
};

type CommandSpec = {
  [COMMANDS.saveApiToken]: { args: { token: string }; result: void };
  [COMMANDS.checkAuthStatus]: { args?: undefined; result: AuthStatus };
  [COMMANDS.getConnectivitySettings]: {
    args?: undefined;
    result: ConnectivitySettings;
  };
  [COMMANDS.setConnectivityMode]: {
    args: { mode: ConnectivityMode };
    result: ConnectivitySettings;
  };
  [COMMANDS.updateConnectivitySettings]: {
    args: { patch: ConnectivitySettingsPatch };
    result: ConnectivitySettings;
  };
  [COMMANDS.toggleOnlineStatus]: { args: { online: boolean }; result: void };
  [COMMANDS.initVault]: { args: { password: string }; result: void };
  [COMMANDS.unlockVault]: { args: { password: string }; result: void };
  [COMMANDS.startNetwork]: { args?: undefined; result: void };
  [COMMANDS.startGithubAuth]: { args?: undefined; result: GithubAuthState };
  [COMMANDS.pollGithubAuth]: { args: { device_code: string }; result: string };
  [COMMANDS.resetVault]: { args?: undefined; result: void };
  [COMMANDS.getFriends]: { args?: undefined; result: FriendConfig[] };
  [COMMANDS.getTrustedPeers]: { args?: undefined; result: string[] };
  [COMMANDS.getPeerAliases]: { args?: undefined; result: Record<string, string> };
  [COMMANDS.addFriend]: {
    args: {
      username: string;
      x25519_key?: string | null;
      ed25519_key?: string | null;
    };
    result: void;
  };
  [COMMANDS.deletePeer]: { args: { peer_id: string }; result: void };
  [COMMANDS.removeFriend]: { args: { username: string }; result: void };
  [COMMANDS.getUserProfile]: { args?: undefined; result: UserProfile };
  [COMMANDS.getTheme]: { args?: undefined; result: ThemeConfig };
  [COMMANDS.updateTheme]: { args: { theme: ThemeConfig }; result: void };
  [COMMANDS.listThemePresets]: { args?: undefined; result: PresetInfo[] };
  [COMMANDS.applyPreset]: { args: { name: string }; result: ThemeConfig };
  [COMMANDS.getSelectedPreset]: { args?: undefined; result: string | null };
  [COMMANDS.generateSimpleTheme]: {
    args: { primary: string; secondary: string; text: string };
    result: ThemeConfig;
  };
  [COMMANDS.createCustomTheme]: {
    args: { name: string; description?: string | null; theme: ThemeConfig };
    result: PresetInfo;
  };
  [COMMANDS.updateCustomTheme]: {
    args: {
      key: string;
      name: string;
      description?: string | null;
      theme: ThemeConfig;
    };
    result: PresetInfo;
  };
  [COMMANDS.deleteCustomTheme]: { args: { key: string }; result: void };
  [COMMANDS.updateUserProfile]: {
    args: { alias?: string | null; avatar_path?: string | null };
    result: void;
  };
  [COMMANDS.getPinnedPeers]: { args?: undefined; result: string[] };
  [COMMANDS.togglePinPeer]: { args: { username: string }; result: boolean };
  [COMMANDS.getChatLatestTimes]: {
    args?: undefined;
    result: Record<string, number>;
  };
  [COMMANDS.getChatList]: { args?: undefined; result: ChatListItem[] };
  [COMMANDS.getChatDetailsOverview]: {
    args: { chat_id: string };
    result: ChatDetailsOverview;
  };
  [COMMANDS.getChatStats]: {
    args: { chat_id: string };
    result: ChatStats;
  };
  [COMMANDS.listChatFiles]: {
    args: {
      chat_id: string;
      filter?: ChatFileFilter;
      limit?: number;
      offset?: number;
    };
    result: ChatFileRow[];
  };
  [COMMANDS.dropChatConnection]: {
    args: { chat_id: string };
    result: void;
  };
  [COMMANDS.forceChatReconnect]: {
    args: { chat_id: string };
    result: void;
  };
  [COMMANDS.saveTemporaryChatToArchive]: {
    args: { chat_id: string };
    result: ArchivedChatResult;
  };
  [COMMANDS.createGroupChat]: {
    args: { name?: string | null };
    result: GroupChatResult;
  };
  [COMMANDS.joinGroupChat]: {
    args: { chat_id: string; name?: string | null };
    result: GroupChatResult;
  };
  [COMMANDS.leaveGroupChat]: {
    args: { chat_id: string };
    result: void;
  };
  [COMMANDS.sendMessageToSelf]: { args: { message: string }; result: void };
  [COMMANDS.sendMessage]: {
    args: { peer_id: string; message: string };
    result: string;
  };
  [COMMANDS.getChatHistory]: { args: { chat_id: string }; result: DbMessage[] };
  [COMMANDS.markMessagesRead]: {
    args: { chat_id: string };
    result: string[];
  };
  [COMMANDS.getUnreadCounts]: {
    args: { my_peer_id: string };
    result: Record<string, number>;
  };
  [COMMANDS.createEnvelope]: {
    args: { id: string; name: string; icon?: string | null };
    result: void;
  };
  [COMMANDS.updateEnvelope]: {
    args: { id: string; name: string; icon?: string | null };
    result: void;
  };
  [COMMANDS.deleteEnvelope]: { args: { id: string }; result: void };
  [COMMANDS.getEnvelopes]: { args?: undefined; result: Envelope[] };
  [COMMANDS.moveChatToEnvelope]: {
    args: { chat_id: string; envelope_id: string | null };
    result: void;
  };
  [COMMANDS.getEnvelopeAssignments]: {
    args?: undefined;
    result: Array<{ chat_id: string; envelope_id: string }>;
  };
  [COMMANDS.requestConnection]: { args: { peer_id: string }; result: void };
  [COMMANDS.setFastDiscovery]: { args: { enabled: boolean }; result: void };
  [COMMANDS.sendImageMessage]: {
    args: { peer_id: string; file_path: string };
    result: SentMediaResult;
  };
  [COMMANDS.getImageData]: { args: { file_hash: string }; result: string };
  [COMMANDS.getImageFromPath]: { args: { file_path: string }; result: string };
  [COMMANDS.saveImageToFile]: {
    args: { file_hash: string; target_path: string };
    result: void;
  };
  [COMMANDS.sendDocumentMessage]: {
    args: { peer_id: string; file_path: string };
    result: SentMediaResult;
  };
  [COMMANDS.saveDocumentToFile]: {
    args: { file_hash: string; target_path: string };
    result: void;
  };
  [COMMANDS.sendVideoMessage]: {
    args: { peer_id: string; file_path: string };
    result: SentMediaResult;
  };
  [COMMANDS.getVideoData]: { args: { file_hash: string }; result: string };
  [COMMANDS.sendAudioMessage]: {
    args: { peer_id: string; file_path: string };
    result: SentMediaResult;
  };
  [COMMANDS.getAudioData]: { args: { file_hash: string }; result: string };
  [COMMANDS.saveAudioToFile]: {
    args: { file_hash: string; target_path: string };
    result: void;
  };
  [COMMANDS.listStickers]: { args?: undefined; result: StickerItem[] };
  [COMMANDS.addSticker]: {
    args: { filePath: string };
    result: AddStickerResult;
  };
  [COMMANDS.addStickersBatch]: {
    args: { filePaths: string[] };
    result: StickerBatchImportResult;
  };
  [COMMANDS.deleteSticker]: { args: { file_hash: string }; result: void };
  [COMMANDS.sendStickerMessage]: {
    args: { peer_id: string; file_hash: string };
    result: SentMediaResult;
  };
  [COMMANDS.saveStickerFromMessage]: {
    args: { file_hash: string };
    result: AddStickerResult;
  };
  [COMMANDS.generateInvitePassword]: { args?: undefined; result: string };
  [COMMANDS.createInvite]: {
    args: { invitee: string; password: string };
    result: void;
  };
  [COMMANDS.redeemAndConnect]: {
    args: { inviter: string; password: string };
    result: string;
  };
  [COMMANDS.createTemporaryInvite]: {
    args: { kind: "dm" | "group"; name?: string | null };
    result: TemporaryInviteView;
  };
  [COMMANDS.redeemTemporaryInvite]: {
    args: { deep_link: string };
    result: TemporaryChatResult;
  };
  [COMMANDS.getActiveTemporaryInvite]: {
    args?: undefined;
    result: TemporaryInviteView | null;
  };
  [COMMANDS.cancelTemporaryInvite]: { args?: undefined; result: void };
  [COMMANDS.startVoiceCall]: { args: { peer_id: string }; result: void };
  [COMMANDS.acceptVoiceCall]: { args: { call_id: string }; result: void };
  [COMMANDS.rejectVoiceCall]: { args: { call_id: string }; result: void };
  [COMMANDS.endVoiceCall]: { args: { call_id: string }; result: void };
  [COMMANDS.setVoiceCallMuted]: {
    args: { call_id: string; muted: boolean };
    result: void;
  };
  [COMMANDS.startVideoCall]: { args: { peer_id: string }; result: void };
  [COMMANDS.acceptVideoCall]: { args: { call_id: string }; result: void };
  [COMMANDS.rejectVideoCall]: { args: { call_id: string }; result: void };
  [COMMANDS.endVideoCall]: { args: { call_id: string }; result: void };
  [COMMANDS.setVideoCallMuted]: {
    args: { call_id: string; muted: boolean };
    result: void;
  };
  [COMMANDS.setVideoCallCameraEnabled]: {
    args: { call_id: string; enabled: boolean };
    result: void;
  };
  [COMMANDS.sendVideoCallChunk]: {
    args: {
      call_id: string;
      seq: number;
      timestamp: number;
      mime: string;
      codec: string;
      chunk_type: VideoChunkType;
      payload: Uint8Array;
    };
    result: void;
  };
  [COMMANDS.getVoiceCallState]: { args?: undefined; result: VoiceCallState };
  [COMMANDS.startScreenBroadcast]: { args: { peer_id: string }; result: void };
  [COMMANDS.acceptScreenBroadcast]: {
    args: { session_id: string };
    result: void;
  };
  [COMMANDS.rejectScreenBroadcast]: {
    args: { session_id: string };
    result: void;
  };
  [COMMANDS.endScreenBroadcast]: {
    args: { session_id: string };
    result: void;
  };
  [COMMANDS.sendScreenBroadcastChunk]: {
    args: {
      session_id: string;
      seq: number;
      timestamp: number;
      mime: string;
      codec: string;
      chunk_type: BroadcastChunkType;
      payload: Uint8Array;
    };
    result: void;
  };
  [COMMANDS.getBroadcastState]: { args?: undefined; result: BroadcastState };
  [COMMANDS.getConnectedChatIds]: { args?: undefined; result: string[] };
};

type KnownCommand = keyof CommandSpec;

type ArgsFor<K extends KnownCommand> = CommandSpec[K]["args"];
type ResultFor<K extends KnownCommand> = CommandSpec[K]["result"];

function toCamelKey(key: string): string {
  return key.replace(/_([a-z])/g, (_, c: string) => c.toUpperCase());
}

function toSnakeKey(key: string): string {
  return key
    .replace(/([A-Z])/g, "_$1")
    .replace(/-/g, "_")
    .toLowerCase();
}

function withArgAliases(
  payload: Record<string, unknown> | undefined
): Record<string, unknown> | undefined {
  if (!payload) return undefined;

  const aliased: Record<string, unknown> = {};
  for (const [key, value] of Object.entries(payload)) {
    aliased[key] = value;

    const camel = toCamelKey(key);
    if (!(camel in aliased)) {
      aliased[camel] = value;
    }

    const snake = toSnakeKey(key);
    if (!(snake in aliased)) {
      aliased[snake] = value;
    }
  }

  return aliased;
}

export async function invokeCommand<K extends KnownCommand>(
  command: K,
  ...args: ArgsFor<K> extends undefined ? [] : [ArgsFor<K>]
): Promise<ResultFor<K>> {
  const payload = withArgAliases(
    (args[0] ?? undefined) as Record<string, unknown> | undefined
  );
  return (await invoke(command, payload)) as ResultFor<K>;
}

export const api = {
  saveApiToken: (token: string) =>
    invokeCommand(COMMANDS.saveApiToken, { token }),
  checkAuthStatus: () => invokeCommand(COMMANDS.checkAuthStatus),
  getConnectivitySettings: () => invokeCommand(COMMANDS.getConnectivitySettings),
  setConnectivityMode: (mode: ConnectivityMode) =>
    invokeCommand(COMMANDS.setConnectivityMode, { mode }),
  updateConnectivitySettings: (patch: ConnectivitySettingsPatch) =>
    invokeCommand(COMMANDS.updateConnectivitySettings, { patch }),
  toggleOnlineStatus: (online: boolean) =>
    invokeCommand(COMMANDS.toggleOnlineStatus, { online }),
  initVault: (password: string) => invokeCommand(COMMANDS.initVault, { password }),
  unlockVault: (password: string) =>
    invokeCommand(COMMANDS.unlockVault, { password }),
  startNetwork: () => invokeCommand(COMMANDS.startNetwork),
  startGithubAuth: () => invokeCommand(COMMANDS.startGithubAuth),
  pollGithubAuth: (deviceCode: string) =>
    invokeCommand(COMMANDS.pollGithubAuth, { device_code: deviceCode }),
  resetVault: () => invokeCommand(COMMANDS.resetVault),
  getFriends: () => invokeCommand(COMMANDS.getFriends),
  getTrustedPeers: () => invokeCommand(COMMANDS.getTrustedPeers),
  getPeerAliases: () => invokeCommand(COMMANDS.getPeerAliases),
  addFriend: (
    username: string,
    x25519Key?: string | null,
    ed25519Key?: string | null
  ) =>
    invokeCommand(COMMANDS.addFriend, {
      username,
      x25519_key: x25519Key,
      ed25519_key: ed25519Key,
    }),
  deletePeer: (peerId: string) =>
    invokeCommand(COMMANDS.deletePeer, { peer_id: peerId }),
  removeFriend: (username: string) =>
    invokeCommand(COMMANDS.removeFriend, { username }),
  getUserProfile: () => invokeCommand(COMMANDS.getUserProfile),
  getTheme: () => invokeCommand(COMMANDS.getTheme),
  updateTheme: (theme: ThemeConfig) =>
    invokeCommand(COMMANDS.updateTheme, { theme }),
  listThemePresets: () => invokeCommand(COMMANDS.listThemePresets),
  applyPreset: (name: string) => invokeCommand(COMMANDS.applyPreset, { name }),
  getSelectedPreset: () => invokeCommand(COMMANDS.getSelectedPreset),
  generateSimpleTheme: (primary: string, secondary: string, text: string) =>
    invokeCommand(COMMANDS.generateSimpleTheme, { primary, secondary, text }),
  createCustomTheme: (
    name: string,
    description: string | null | undefined,
    theme: ThemeConfig
  ) => invokeCommand(COMMANDS.createCustomTheme, { name, description, theme }),
  updateCustomTheme: (
    key: string,
    name: string,
    description: string | null | undefined,
    theme: ThemeConfig
  ) =>
    invokeCommand(COMMANDS.updateCustomTheme, {
      key,
      name,
      description,
      theme,
    }),
  deleteCustomTheme: (key: string) =>
    invokeCommand(COMMANDS.deleteCustomTheme, { key }),
  updateUserProfile: (alias?: string | null, avatarPath?: string | null) =>
    invokeCommand(COMMANDS.updateUserProfile, {
      alias,
      avatar_path: avatarPath,
    }),
  getPinnedPeers: () => invokeCommand(COMMANDS.getPinnedPeers),
  togglePinPeer: (username: string) =>
    invokeCommand(COMMANDS.togglePinPeer, { username }),
  getChatLatestTimes: () => invokeCommand(COMMANDS.getChatLatestTimes),
  getChatList: () => invokeCommand(COMMANDS.getChatList),
  getChatDetailsOverview: (chatId: string) =>
    invokeCommand(COMMANDS.getChatDetailsOverview, { chat_id: chatId }),
  getChatStats: (chatId: string) =>
    invokeCommand(COMMANDS.getChatStats, { chat_id: chatId }),
  listChatFiles: (
    chatId: string,
    filter: ChatFileFilter = "all",
    limit = 50,
    offset = 0
  ) =>
    invokeCommand(COMMANDS.listChatFiles, {
      chat_id: chatId,
      filter,
      limit,
      offset,
    }),
  dropChatConnection: (chatId: string) =>
    invokeCommand(COMMANDS.dropChatConnection, { chat_id: chatId }),
  forceChatReconnect: (chatId: string) =>
    invokeCommand(COMMANDS.forceChatReconnect, { chat_id: chatId }),
  saveTemporaryChatToArchive: (chatId: string) =>
    invokeCommand(COMMANDS.saveTemporaryChatToArchive, { chat_id: chatId }),
  createGroupChat: (name?: string | null) =>
    invokeCommand(COMMANDS.createGroupChat, { name }),
  joinGroupChat: (chatId: string, name?: string | null) =>
    invokeCommand(COMMANDS.joinGroupChat, { chat_id: chatId, name }),
  leaveGroupChat: (chatId: string) =>
    invokeCommand(COMMANDS.leaveGroupChat, { chat_id: chatId }),
  sendMessageToSelf: (message: string) =>
    invokeCommand(COMMANDS.sendMessageToSelf, { message }),
  sendMessage: (peerId: string, message: string) =>
    invokeCommand(COMMANDS.sendMessage, { peer_id: peerId, message }),
  getChatHistory: (chatId: string) =>
    invokeCommand(COMMANDS.getChatHistory, { chat_id: chatId }),
  markMessagesRead: (chatId: string) =>
    invokeCommand(COMMANDS.markMessagesRead, { chat_id: chatId }),
  getUnreadCounts: (myPeerId: string) =>
    invokeCommand(COMMANDS.getUnreadCounts, { my_peer_id: myPeerId }),
  createEnvelope: (id: string, name: string, icon?: string | null) =>
    invokeCommand(COMMANDS.createEnvelope, { id, name, icon }),
  updateEnvelope: (id: string, name: string, icon?: string | null) =>
    invokeCommand(COMMANDS.updateEnvelope, { id, name, icon }),
  deleteEnvelope: (id: string) => invokeCommand(COMMANDS.deleteEnvelope, { id }),
  getEnvelopes: () => invokeCommand(COMMANDS.getEnvelopes),
  moveChatToEnvelope: (chatId: string, envelopeId: string | null) =>
    invokeCommand(COMMANDS.moveChatToEnvelope, {
      chat_id: chatId,
      envelope_id: envelopeId,
    }),
  getEnvelopeAssignments: () => invokeCommand(COMMANDS.getEnvelopeAssignments),
  requestConnection: (peerId: string) =>
    invokeCommand(COMMANDS.requestConnection, { peer_id: peerId }),
  setFastDiscovery: (enabled: boolean) =>
    invokeCommand(COMMANDS.setFastDiscovery, { enabled }),
  sendImageMessage: (peerId: string, filePath: string) =>
    invokeCommand(COMMANDS.sendImageMessage, { peer_id: peerId, file_path: filePath }),
  getImageData: (fileHash: string) =>
    invokeCommand(COMMANDS.getImageData, {
      fileHash,
      file_hash: fileHash,
    } as unknown as CommandSpec[typeof COMMANDS.getImageData]["args"]),
  getImageFromPath: (filePath: string) =>
    invokeCommand(COMMANDS.getImageFromPath, { file_path: filePath }),
  saveImageToFile: (fileHash: string, targetPath: string) =>
    invokeCommand(COMMANDS.saveImageToFile, { file_hash: fileHash, target_path: targetPath }),
  sendDocumentMessage: (peerId: string, filePath: string) =>
    invokeCommand(COMMANDS.sendDocumentMessage, {
      peer_id: peerId,
      file_path: filePath,
    }),
  saveDocumentToFile: (fileHash: string, targetPath: string) =>
    invokeCommand(COMMANDS.saveDocumentToFile, { file_hash: fileHash, target_path: targetPath }),
  sendVideoMessage: (peerId: string, filePath: string) =>
    invokeCommand(COMMANDS.sendVideoMessage, { peer_id: peerId, file_path: filePath }),
  getVideoData: (fileHash: string) =>
    invokeCommand(COMMANDS.getVideoData, { file_hash: fileHash }),
  sendAudioMessage: (peerId: string, filePath: string) =>
    invokeCommand(COMMANDS.sendAudioMessage, { peer_id: peerId, file_path: filePath }),
  getAudioData: (fileHash: string) =>
    invokeCommand(COMMANDS.getAudioData, { file_hash: fileHash }),
  saveAudioToFile: (fileHash: string, targetPath: string) =>
    invokeCommand(COMMANDS.saveAudioToFile, { file_hash: fileHash, target_path: targetPath }),
  listStickers: () => invokeCommand(COMMANDS.listStickers),
  addSticker: (filePath: string) =>
    invokeCommand(COMMANDS.addSticker, {
      filePath,
      file_path: filePath,
    } as unknown as CommandSpec[typeof COMMANDS.addSticker]["args"]),
  addStickersBatch: (filePaths: string[]) =>
    invokeCommand(COMMANDS.addStickersBatch, {
      filePaths,
      file_paths: filePaths,
    } as unknown as CommandSpec[typeof COMMANDS.addStickersBatch]["args"]),
  deleteSticker: (fileHash: string) =>
    invokeCommand(COMMANDS.deleteSticker, {
      fileHash,
      file_hash: fileHash,
    } as unknown as CommandSpec[typeof COMMANDS.deleteSticker]["args"]),
  sendStickerMessage: (peerId: string, fileHash: string) =>
    invokeCommand(COMMANDS.sendStickerMessage, {
      peerId,
      peer_id: peerId,
      fileHash,
      file_hash: fileHash,
    } as unknown as CommandSpec[typeof COMMANDS.sendStickerMessage]["args"]),
  saveStickerFromMessage: (fileHash: string) =>
    invokeCommand(COMMANDS.saveStickerFromMessage, {
      fileHash,
      file_hash: fileHash,
    } as unknown as CommandSpec[typeof COMMANDS.saveStickerFromMessage]["args"]),
  generateInvitePassword: () => invokeCommand(COMMANDS.generateInvitePassword),
  createInvite: (invitee: string, password: string) =>
    invokeCommand(COMMANDS.createInvite, { invitee, password }),
  redeemAndConnect: (inviter: string, password: string) =>
    invokeCommand(COMMANDS.redeemAndConnect, { inviter, password }),
  createTemporaryInvite: (kind: "dm" | "group", name?: string | null) =>
    invokeCommand(COMMANDS.createTemporaryInvite, { kind, name }),
  redeemTemporaryInvite: (deepLink: string) =>
    invokeCommand(COMMANDS.redeemTemporaryInvite, {
      deepLink,
      deep_link: deepLink,
    } as unknown as CommandSpec[typeof COMMANDS.redeemTemporaryInvite]["args"]),
  getActiveTemporaryInvite: () => invokeCommand(COMMANDS.getActiveTemporaryInvite),
  cancelTemporaryInvite: () => invokeCommand(COMMANDS.cancelTemporaryInvite),
  startVoiceCall: (peerId: string) =>
    invokeCommand(COMMANDS.startVoiceCall, { peer_id: peerId }),
  acceptVoiceCall: (callId: string) =>
    invokeCommand(COMMANDS.acceptVoiceCall, { call_id: callId }),
  rejectVoiceCall: (callId: string) =>
    invokeCommand(COMMANDS.rejectVoiceCall, { call_id: callId }),
  endVoiceCall: (callId: string) =>
    invokeCommand(COMMANDS.endVoiceCall, { call_id: callId }),
  setVoiceCallMuted: (callId: string, muted: boolean) =>
    invokeCommand(COMMANDS.setVoiceCallMuted, { call_id: callId, muted }),
  startVideoCall: (peerId: string) =>
    invokeCommand(COMMANDS.startVideoCall, { peer_id: peerId }),
  acceptVideoCall: (callId: string) =>
    invokeCommand(COMMANDS.acceptVideoCall, { call_id: callId }),
  rejectVideoCall: (callId: string) =>
    invokeCommand(COMMANDS.rejectVideoCall, { call_id: callId }),
  endVideoCall: (callId: string) =>
    invokeCommand(COMMANDS.endVideoCall, { call_id: callId }),
  setVideoCallMuted: (callId: string, muted: boolean) =>
    invokeCommand(COMMANDS.setVideoCallMuted, { call_id: callId, muted }),
  setVideoCallCameraEnabled: (callId: string, enabled: boolean) =>
    invokeCommand(COMMANDS.setVideoCallCameraEnabled, {
      call_id: callId,
      enabled,
    }),
  sendVideoCallChunk: (
    callId: string,
    seq: number,
    timestamp: number,
    mime: string,
    codec: string,
    chunkType: VideoChunkType,
    payload: Uint8Array,
  ) =>
    invokeCommand(COMMANDS.sendVideoCallChunk, {
      call_id: callId,
      seq,
      timestamp,
      mime,
      codec,
      chunk_type: chunkType,
      payload,
    }),
  getVoiceCallState: () => invokeCommand(COMMANDS.getVoiceCallState),
  startScreenBroadcast: (peerId: string) =>
    invokeCommand(COMMANDS.startScreenBroadcast, { peer_id: peerId }),
  acceptScreenBroadcast: (sessionId: string) =>
    invokeCommand(COMMANDS.acceptScreenBroadcast, { session_id: sessionId }),
  rejectScreenBroadcast: (sessionId: string) =>
    invokeCommand(COMMANDS.rejectScreenBroadcast, { session_id: sessionId }),
  endScreenBroadcast: (sessionId: string) =>
    invokeCommand(COMMANDS.endScreenBroadcast, { session_id: sessionId }),
  sendScreenBroadcastChunk: (
    sessionId: string,
    seq: number,
    timestamp: number,
    mime: string,
    codec: string,
    chunkType: BroadcastChunkType,
    payload: Uint8Array,
  ) =>
    invokeCommand(COMMANDS.sendScreenBroadcastChunk, {
      session_id: sessionId,
      seq,
      timestamp,
      mime,
      codec,
      chunk_type: chunkType,
      payload,
    }),
  getBroadcastState: () => invokeCommand(COMMANDS.getBroadcastState),
  getConnectedChatIds: () => invokeCommand(COMMANDS.getConnectedChatIds),
};
