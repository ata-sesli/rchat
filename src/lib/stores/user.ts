import { writable, get } from "svelte/store";
import { goto } from "$app/navigation";
import { api } from "$lib/tauri/api";

// Types
export type UserProfile = {
  alias: string | null;
  avatar_path: string | null;
};

// State
export const userProfile = writable<UserProfile>({
  alias: "Me",
  avatar_path: null,
});

export const isAuthenticated = writable<boolean>(false);

// Actions
export async function loadUserProfile() {
  try {
    const profile = await api.getUserProfile();
    userProfile.set(profile);
  } catch (e) {
    console.error("Failed to load user profile:", e);
  }
}

export async function checkAuth(): Promise<boolean> {
  try {
    const auth = await api.checkAuthStatus();
    
    if (!auth.is_setup || !auth.is_unlocked) {
      goto("/login");
      return false;
    }
    
    isAuthenticated.set(true);
    return true;
  } catch (e) {
    console.error("Auth check failed:", e);
    goto("/login");
    return false;
  }
}

export async function refreshData() {
  const authed = await checkAuth();
  if (!authed) return;
  
  await loadUserProfile();
}
