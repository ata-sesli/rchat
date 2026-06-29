export type AuthGateStatus = {
  is_setup: boolean;
  is_unlocked: boolean;
  is_github_connected?: boolean;
};

export type AuthGateTarget = "setup" | "unlock" | "app";

export type LocalProfileStatus = {
  alias?: string | null;
};

export function getAuthGateTarget(status: AuthGateStatus): AuthGateTarget {
  if (!status.is_setup) return "setup";
  if (!status.is_unlocked) return "unlock";
  return "app";
}

export function hasLocalUsername(profile: LocalProfileStatus): boolean {
  return !!profile.alias?.trim();
}

export function needsLocalUsername(
  status: AuthGateStatus,
  profile: LocalProfileStatus,
): boolean {
  return (
    status.is_setup &&
    status.is_unlocked &&
    !status.is_github_connected &&
    !hasLocalUsername(profile)
  );
}
