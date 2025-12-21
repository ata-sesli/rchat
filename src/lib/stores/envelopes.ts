import { writable, get } from "svelte/store";
import { invoke } from "@tauri-apps/api/core";

// Types
export type Envelope = { id: string; name: string; icon?: string };

// State
export const envelopes = writable<Envelope[]>([]);
export const chatAssignments = writable<Record<string, string>>({});
export const currentEnvelope = writable<string | null>(null);

// Modal state
export const showEnvelopeModal = writable<boolean>(false);
export const editingEnvelopeId = writable<string | null>(null);
export const newEnvelopeName = writable<string>("");
export const newEnvelopeIcon = writable<string>("");

// Available icons
export const AVAILABLE_ICONS = [
  '<svg xmlns="http://www.w3.org/2000/svg" class="h-5 w-5" viewBox="0 0 20 20" fill="currentColor"><path d="M2 6a2 2 0 012-2h5l2 2h5a2 2 0 012 2v6a2 2 0 01-2 2H4a2 2 0 01-2-2V6z" /></svg>',
  '<svg xmlns="http://www.w3.org/2000/svg" class="h-5 w-5" viewBox="0 0 20 20" fill="currentColor"><path fill-rule="evenodd" d="M6 6V5a3 3 0 013-3h2a3 3 0 013 3v1h2a2 2 0 012 2v3.57A22.952 22.952 0 0110 13a22.95 22.95 0 01-8-1.43V8a2 2 0 012-2h2zm2-1a1 1 0 011-1h2a1 1 0 011 1v1H8V5zm1 5a1 1 0 011-1h.01a1 1 0 110 2H10a1 1 0 01-1-1z" clip-rule="evenodd" /></svg>',
  '<svg xmlns="http://www.w3.org/2000/svg" class="h-5 w-5" viewBox="0 0 20 20" fill="currentColor"><path d="M10.707 2.293a1 1 0 00-1.414 0l-7 7a1 1 0 001.414 1.414L4 10.414V17a1 1 0 001 1h2a1 1 0 001-1v-2a1 1 0 011-1h2a1 1 0 011 1v2a1 1 0 001 1h2a1 1 0 001-1v-6.586l.293.293a1 1 0 001.414-1.414l-7-7z" /></svg>',
  '<svg xmlns="http://www.w3.org/2000/svg" class="h-5 w-5" viewBox="0 0 20 20" fill="currentColor"><path fill-rule="evenodd" d="M3.172 5.172a4 4 0 015.656 0L10 6.343l1.172-1.171a4 4 0 115.656 5.656L10 17.657l-6.828-6.829a4 4 0 010-5.656z" clip-rule="evenodd" /></svg>',
  '<svg xmlns="http://www.w3.org/2000/svg" class="h-5 w-5" viewBox="0 0 20 20" fill="currentColor"><path fill-rule="evenodd" d="M11.3 1.046A1 1 0 0112 2v5h4a1 1 0 01.82 1.573l-7 10A1 1 0 018 18v-5H4a1 1 0 01-.82-1.573l7-10a1 1 0 011.12-.38z" clip-rule="evenodd" /></svg>',
  '<svg xmlns="http://www.w3.org/2000/svg" class="h-5 w-5" viewBox="0 0 20 20" fill="currentColor"><path d="M9.049 2.927c.3-.921 1.603-.921 1.902 0l1.07 3.292a1 1 0 00.95.69h3.462c.969 0 1.371 1.24.588 1.81l-2.8 2.034a1 1 0 00-.364 1.118l1.07 3.292c.3.921-.755 1.688-1.54 1.118l-2.8-2.034a1 1 0 00-1.175 0l-2.8 2.034c-.784.57-1.838-.197-1.539-1.118l1.07-3.292a1 1 0 00-.364-1.118L2.98 8.72c-.783-.57-.38-1.81.588-1.81h3.461a1 1 0 00.951-.69l1.07-3.292z" /></svg>',
];

// Actions
export async function loadEnvelopes() {
  try {
    const data = await invoke<Envelope[]>("get_envelopes");
    envelopes.set(data);
    
    const assignments = await invoke<Record<string, string>>("get_chat_assignments");
    chatAssignments.set(assignments);
  } catch (e) {
    console.error("Failed to load envelopes:", e);
  }
}

export function enterEnvelope(envelopeId: string) {
  currentEnvelope.set(envelopeId);
}

export function exitEnvelope() {
  currentEnvelope.set(null);
}

export async function createEnvelope(name: string, icon: string) {
  const id = `env_${Date.now()}`;
  const newEnv: Envelope = { id, name, icon };
  
  try {
    await invoke("create_envelope", { envelope: newEnv });
    envelopes.update((e) => [...e, newEnv]);
    return id;
  } catch (e) {
    console.error("Failed to create envelope:", e);
    throw e;
  }
}

export async function updateEnvelope(id: string, name: string, icon: string) {
  try {
    await invoke("update_envelope", { envelopeId: id, name, icon });
    envelopes.update((envs) =>
      envs.map((e) => (e.id === id ? { ...e, name, icon } : e))
    );
  } catch (e) {
    console.error("Failed to update envelope:", e);
    throw e;
  }
}

export async function deleteEnvelope(envelopeId: string) {
  try {
    await invoke("delete_envelope", { envelopeId });
    envelopes.update((e) => e.filter((env) => env.id !== envelopeId));
    
    // Clear assignments for this envelope
    chatAssignments.update((a) => {
      const updated = { ...a };
      Object.keys(updated).forEach((k) => {
        if (updated[k] === envelopeId) delete updated[k];
      });
      return updated;
    });
    
    // Exit if currently in deleted envelope
    if (get(currentEnvelope) === envelopeId) {
      currentEnvelope.set(null);
    }
  } catch (e) {
    console.error("Failed to delete envelope:", e);
    throw e;
  }
}

export async function moveChatToEnvelope(chatId: string, envelopeId: string | null) {
  try {
    await invoke("move_chat_to_envelope", { chatId, envelopeId });
    
    chatAssignments.update((a) => {
      const updated = { ...a };
      if (envelopeId) {
        updated[chatId] = envelopeId;
      } else {
        delete updated[chatId];
      }
      return updated;
    });
  } catch (e) {
    console.error("Failed to move chat:", e);
    throw e;
  }
}

export function openEnvelopeModal(editId: string | null = null) {
  if (editId) {
    const env = get(envelopes).find((e) => e.id === editId);
    if (env) {
      newEnvelopeName.set(env.name);
      newEnvelopeIcon.set(env.icon || AVAILABLE_ICONS[0]);
      editingEnvelopeId.set(editId);
    }
  } else {
    newEnvelopeName.set("");
    newEnvelopeIcon.set(AVAILABLE_ICONS[0]);
    editingEnvelopeId.set(null);
  }
  showEnvelopeModal.set(true);
}

export function closeEnvelopeModal() {
  showEnvelopeModal.set(false);
  editingEnvelopeId.set(null);
  newEnvelopeName.set("");
  newEnvelopeIcon.set("");
}
