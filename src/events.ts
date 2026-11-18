export type Problem =
  | "NETWORK"
  | "LOGIN_CANCELLED"
  | "LOGIN_FAILED"
  | "NO_XBOX_PROFILE"
  | "ACCOUNT_RESTRICTED"
  | "CHILD_ACCOUNT"
  | "NO_JAVA_EDITION"
  | "DISK_FULL"
  | "CORRUPT_DOWNLOAD"
  | "PACK_INVALID"
  | "UNSUPPORTED_PLATFORM"
  | "GAME_EXITED";

export interface LauncherError {
  problem: Problem;
  message: string;
  disk?: { neededBytes: number; usableBytes: number };
}

export type LaunchEvent =
  | { state: "checking" }
  | { state: "waitingForSignIn" }
  | { state: "signedIn"; playerId: string; name: string }
  | { state: "preparing"; minecraftVersion: string; mods: number; shaderpacks: number }
  | { state: "downloading"; totalBytes: number; update: boolean; changedMods: number }
  | { state: "progress"; doneBytes: number; totalBytes: number; file: string }
  | { state: "launching"; server: string }
  | { state: "launched" }
  | { state: "failed"; error: LauncherError }
  | { state: "mismatch"; webPlayerId: string; webName: string; sessionPlayerId: string; sessionName: string };

export type UpdateEvent =
  | { state: "downloading"; version: string; doneBytes: number; totalBytes: number | null }
  | { state: "installing" };

export interface AppInfo {
  version: string;
  webBase: string;
  gameDirectory: string;
  totalMemoryBytes: number;
}

export interface LauncherSettings {
  memoryMegabytes: number;
  gameDirectory: string | null;
}

export type Step = "retry" | "switchAccount" | "continueAnyway";
