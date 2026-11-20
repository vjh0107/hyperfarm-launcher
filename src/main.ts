import { invoke } from "@tauri-apps/api/core";
import { LogicalSize } from "@tauri-apps/api/dpi";
import { listen } from "@tauri-apps/api/event";
import { getCurrentWindow } from "@tauri-apps/api/window";

import type { AppInfo, LaunchEvent, LauncherError, LauncherSettings, Step, UpdateEvent } from "./events";
import { bytes, remaining, summary, type PackSummary } from "./format";
import { accountRows, errorScreen, loginScreen, ProgressScreen, settingsScreen, type Chrome, type ErrorContent } from "./screens";

const WIDTH = 480;
const HEIGHT = 320;
const SETTINGS_HEIGHT = 440;
const SAMPLE_MS = 500;

const appWindow = getCurrentWindow();
const container = document.getElementById("window")!;
const chrome: Chrome = {
  minimize: () => void appWindow.minimize(),
  close: () => void invoke("quit"),
};

let info: AppInfo;
let progress: ProgressScreen;
let current: HTMLElement | null = null;
let beforeSettings: HTMLElement | null = null;
let pack: PackSummary | null = null;
let session: { playerId: string; name: string } | null = null;

function show(screen: HTMLElement): void {
  if (screen === current) return;
  const leaving = current;
  if (leaving) {
    leaving.classList.add("leaving");
    window.setTimeout(() => leaving.remove(), 180);
  }
  screen.classList.remove("leaving");
  container.append(screen);
  current = screen;
  const height = screen.classList.contains("settings") ? SETTINGS_HEIGHT : HEIGHT;
  void appWindow.setSize(new LogicalSize(WIDTH, height));
}

function run(step: Step): void {
  progress.checking(summary(pack));
  show(progress.root);
  void invoke("run_step", { step });
}

class DownloadTicker {
  private done = 0;
  private total = 0;
  private file = "";
  private update = false;
  private sampledAt = 0;
  private sampledBytes = 0;
  private bytesPerSecond = 0;
  private timer: number | undefined;

  start(total: number, update: boolean): void {
    this.stop();
    Object.assign(this, { done: 0, total, file: "", update, sampledAt: 0, sampledBytes: 0, bytesPerSecond: 0 });
    this.timer = window.setInterval(() => this.render(), 100);
  }

  record(done: number, total: number, file: string): void {
    Object.assign(this, { done, total, file });
  }

  stop(): void {
    window.clearInterval(this.timer);
    this.timer = undefined;
  }

  private render(): void {
    const now = performance.now();
    if (this.sampledAt === 0) {
      this.sampledAt = now;
      this.sampledBytes = this.done;
    } else if (now - this.sampledAt >= SAMPLE_MS) {
      const rate = ((this.done - this.sampledBytes) * 1000) / (now - this.sampledAt);
      this.bytesPerSecond = this.bytesPerSecond === 0 ? rate : this.bytesPerSecond * 0.7 + rate * 0.3;
      this.sampledAt = now;
      this.sampledBytes = this.done;
    }
    const fraction = this.total === 0 ? 0 : this.done / this.total;
    const amount = `${bytes(this.done)} / ${bytes(this.total)}`;
    if (this.update) {
      progress.progress(fraction, this.file, amount);
    } else {
      const left = this.bytesPerSecond > 0 ? (this.total - this.done) / this.bytesPerSecond : null;
      progress.progress(fraction, amount, remaining(left));
    }
  }
}

const ticker = new DownloadTicker();

function errorContent(error: LauncherError): ErrorContent {
  const close = { label: "닫기", run: chrome.close };
  const retry = { label: "다시 시도", run: () => run("retry") };
  const switchAccount = { label: "다른 계정으로 로그인", run: () => run("switchAccount") };
  switch (error.problem) {
    case "NO_JAVA_EDITION":
      return { title: "자바 에디션이 없는 계정이에요", body: "이 Microsoft 계정에는 마인크래프트 자바 에디션이 없어요.", secondary: close, primary: switchAccount };
    case "NO_XBOX_PROFILE":
      return {
        title: "Xbox 프로필이 없어요",
        body: "Xbox 프로필을 만든 뒤 다시 시도해 주세요.",
        secondary: retry,
        primary: { label: "Xbox 프로필 만들기", run: () => void invoke("open_xbox_profile") },
      };
    case "CHILD_ACCOUNT":
      return { title: "보호자 허락이 필요해요", body: "미성년 계정은 보호자의 Microsoft 가족 그룹에 추가되어야 해요.", secondary: close, primary: retry };
    case "ACCOUNT_RESTRICTED":
      return { title: "이 계정으로는 들어갈 수 없어요", body: error.message, secondary: close, primary: switchAccount };
    case "NETWORK":
      return { title: "연결하지 못했어요", body: "인터넷 연결을 확인해 주세요.", primary: retry };
    case "DISK_FULL":
      return {
        title: "저장 공간이 부족해요",
        body: error.disk ? `${bytes(error.disk.neededBytes)}가 필요한데 ${bytes(error.disk.usableBytes)} 남았어요.` : "저장 공간을 비운 뒤 다시 시도해 주세요.",
        secondary: { label: "폴더 바꾸기", run: changeFolder },
        primary: retry,
      };
    case "LOGIN_CANCELLED":
    case "LOGIN_FAILED":
      return { title: "로그인하지 못했어요", body: error.message, secondary: close, primary: retry };
    case "CORRUPT_DOWNLOAD":
    case "PACK_INVALID":
      return { title: "게임 파일을 받지 못했어요", body: error.message, secondary: close, primary: retry };
    case "UNSUPPORTED_PLATFORM":
      return { title: "이 PC에서는 실행할 수 없어요", body: error.message, primary: close };
    case "GAME_EXITED":
      return {
        title: "게임이 바로 꺼졌어요",
        body: error.message,
        secondary: { label: "로그 열기", run: () => void invoke("open_game_log") },
        primary: { label: "다시 시작", run: () => run("play") },
      };
  }
}

async function changeFolder(): Promise<void> {
  const chosen = await invoke<string | null>("choose_game_directory");
  if (chosen) {
    info = await invoke<AppInfo>("app_info");
    run("retry");
  }
}

async function openSettings(): Promise<void> {
  beforeSettings = current;
  const settings = await invoke<LauncherSettings>("load_settings");
  const footer = `런처 ${info.version}${pack ? ` · 마인크래프트 ${pack.minecraftVersion} · Fabric` : ""}`;
  show(
    settingsScreen(
      chrome,
      {
        memoryMegabytes: settings.memoryMegabytes,
        totalMemoryBytes: info.totalMemoryBytes,
        gameDirectory: info.gameDirectory,
        session,
        webBase: info.webBase,
        footer,
        onMemory: (memoryMegabytes) => void invoke("save_settings", { settings: { ...settings, memoryMegabytes } }),
        onOpenFolder: () => void invoke("open_game_directory"),
        onSignOut: () => {
          session = null;
          progress.clearAccount();
          void invoke("sign_out");
        },
      },
      () => show(beforeSettings ?? progress.root),
    ),
  );
}

function onLaunch(event: LaunchEvent): void {
  switch (event.state) {
    case "checking":
      progress.checking(summary(pack));
      if (!current?.classList.contains("settings")) show(progress.root);
      break;
    case "waitingForSignIn":
      show(loginScreen(chrome, () => void invoke("cancel_sign_in"), () => void invoke("reopen_sign_in")));
      break;
    case "signedIn":
      session = { playerId: event.playerId, name: event.name };
      progress.setAccount(event.playerId, event.name);
      progress.checking(summary(pack));
      if (!current?.classList.contains("settings")) show(progress.root);
      break;
    case "preparing":
      pack = event;
      progress.checking(summary(pack));
      break;
    case "downloading": {
      const changed = event.update && event.changedMods > 0 && pack;
      const subtitle = changed ? `마인크래프트 ${pack!.minecraftVersion} · 모드 ${event.changedMods}개 바뀜` : summary(pack);
      progress.downloading(event.update ? "업데이트 적용 중" : "게임 파일 받는 중", subtitle, !event.update);
      ticker.start(event.totalBytes, event.update);
      break;
    }
    case "progress":
      ticker.record(event.doneBytes, event.totalBytes, event.file);
      break;
    case "ready":
      ticker.stop();
      progress.ready(summary(pack));
      if (!current?.classList.contains("settings")) show(progress.root);
      break;
    case "launching":
      ticker.stop();
      progress.launching(event.server);
      show(progress.root);
      break;
    case "launched":
      break;
    case "failed":
      ticker.stop();
      show(errorScreen(chrome, errorContent(event.error)));
      break;
    case "mismatch":
      ticker.stop();
      show(
        errorScreen(chrome, {
          title: "웹과 다른 계정이에요",
          rows: accountRows(info.webBase, [
            { label: "웹", playerId: event.webPlayerId, name: event.webName },
            { label: "게임", playerId: event.sessionPlayerId, name: event.sessionName },
          ]),
          secondary: { label: "계정 바꾸기", run: () => run("switchAccount") },
          primary: { label: "이 계정으로 시작", run: () => run("continueAnyway") },
        }),
      );
      break;
  }
}

function onUpdate(event: UpdateEvent): void {
  if (event.state === "installing") {
    progress.checking("곧 다시 켜져요", "런처 업데이트 중");
    return;
  }
  if (current !== progress.root || event.doneBytes === 0) {
    progress.downloading("런처 업데이트 중", event.notes ? `${event.version} · ${event.notes}` : `런처 ${event.version}`, false);
    show(progress.root);
  }
  const total = event.totalBytes ?? 0;
  progress.progress(total === 0 ? 0 : event.doneBytes / total, "", total === 0 ? bytes(event.doneBytes) : `${bytes(event.doneBytes)} / ${bytes(total)}`);
}

async function start(): Promise<void> {
  info = await invoke<AppInfo>("app_info");
  progress = new ProgressScreen(
    chrome,
    info.webBase,
    () => void openSettings(),
    () => void invoke("run_step", { step: "play" satisfies Step }),
  );
  progress.setVersion(info.version);
  progress.checking("");
  show(progress.root);
  await listen<LaunchEvent>("launch", ({ payload }) => onLaunch(payload));
  await listen<UpdateEvent>("update", ({ payload }) => onUpdate(payload));
  await invoke("begin");
}

document.addEventListener("contextmenu", (event) => event.preventDefault());
document.addEventListener("keydown", (event) => {
  if (event.key === "Enter" && !event.repeat && current === progress?.root) progress.pressPlay();
});
void start();
