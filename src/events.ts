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
  | "GAME_RUNNING"
  | "GAME_EXITED";

export interface Crash {
  code: number | null;
  hex: string | null;
  report: string | null;
}

export interface CrashReportFiled {
  outcome: "OPENED" | "APPENDED" | "REPLAYED";
  ticketId: number;
}

export interface CrashReportError {
  problem: "NO_CRASH" | "LOGIN_REQUIRED" | "ACCOUNT_CHANGED" | "RATE_LIMITED" | "EVENT_TAKEN" | "UNAVAILABLE" | "NETWORK" | "REJECTED";
  message: string;
}

export interface LauncherError {
  problem: Problem;
  message: string;
  disk?: { neededBytes: number; usableBytes: number };
  crash?: Crash;
}

export type LaunchEvent =
  | { state: "checking" }
  | { state: "waitingForSignIn" }
  | { state: "signedIn"; playerId: string; name: string }
  | { state: "preparing"; minecraftVersion: string; mods: number; shaderpacks: number }
  | { state: "downloading"; totalBytes: number; update: boolean; changedMods: number }
  | { state: "progress"; doneBytes: number; totalBytes: number; file: string }
  | { state: "ready"; server: string }
  | { state: "launching"; server: string }
  | { state: "launched" }
  | { state: "failed"; error: LauncherError }
  | { state: "mismatch"; webPlayerId: string; webName: string; sessionPlayerId: string; sessionName: string };

export type UpdateEvent =
  | { state: "downloading"; version: string; notes: string | null; doneBytes: number; totalBytes: number | null }
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
  sendCrashReports: boolean;
  showDiscordActivity: boolean;
}

export type Step = "retry" | "switchAccount" | "continueAnyway" | "play";
