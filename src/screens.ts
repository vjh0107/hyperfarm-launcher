import heroUrl from "./assets/images/bg.webp";
import logoUrl from "./assets/images/logo.webp";
import { h, icon, Icons } from "./dom";
import type { Crash } from "./events";
import { gigabytes } from "./format";

const SETTINGS_HEIGHT = 440;
const CRASH_REPORT_HEIGHT = 344;
const COPY_FEEDBACK_MS = 2000;

export interface Chrome {
  minimize(): void;
  close(): void;
}

export interface Action {
  label: string;
  run(): void;
}

function iconButton(path: string, label: string, run: () => void, plain = false): HTMLButtonElement {
  return h("button", { class: plain ? "icon-button plain" : "icon-button", "aria-label": label, title: label, onClick: run }, icon(path, 16, 2));
}

function pill(label: string, style: string, run: () => void, ...leading: Node[]): HTMLButtonElement {
  return h("button", { class: `pill ${style}`, onClick: run }, ...leading, label);
}

export function head(webBase: string, playerId: string | null, size: number, radius: number): HTMLElement {
  const frame = h("span", { class: "head" });
  Object.assign(frame.style, { width: `${size}px`, height: `${size}px`, borderRadius: `${radius}px` });
  if (playerId) {
    const image = h("img", { alt: "", src: `${webBase}/api/heads/${playerId}?size=${size * 2}`, draggable: "false" });
    image.addEventListener("error", () => image.remove());
    frame.append(image);
  }
  return frame;
}

function frame(chrome: Chrome, muted: boolean, badge: Node, extra: Node | null, ...body: Node[]): HTMLElement {
  const header = h(
    "header",
    { class: "header", "data-tauri-drag-region": true },
    h("img", { class: "hero", src: heroUrl, alt: "", draggable: "false" }),
    h("div", { class: "fade" }),
    h("div", { class: "chrome" }, extra, iconButton(Icons.minimize, "최소화", chrome.minimize), iconButton(Icons.close, "닫기", chrome.close)),
  );
  return h("section", { class: muted ? "screen muted" : "screen" }, header, ...body, badge);
}

function logoBadge(): HTMLElement {
  return h("img", { class: "badge", src: logoUrl, alt: "하이퍼팜", draggable: "false" });
}

function heading(title: Node, subtitle: Node): HTMLElement {
  return h("div", { class: "heading" }, title, subtitle);
}

function actions(...buttons: Node[]): HTMLElement {
  return h("div", { class: "actions" }, ...buttons);
}

const spacer = () => h("div", { class: "spacer" });

export class ProgressScreen {
  readonly root: HTMLElement;
  private readonly title = h("h1", { class: "title" });
  private readonly subtitle = h("p", { class: "subtitle" });
  private readonly bar = h("div", { class: "track" }, h("div", { class: "fill" }));
  private readonly left = h("span", { class: "caption" });
  private readonly right = h("span", { class: "caption" });
  private readonly account = h("div", { class: "account" });
  private readonly version = h("span", { class: "caption" });
  private readonly settings: HTMLButtonElement;
  private readonly play: HTMLButtonElement;

  constructor(chrome: Chrome, private readonly webBase: string, openSettings: () => void, play: () => void) {
    this.settings = iconButton(Icons.settings, "설정", openSettings);
    this.play = pill(
      "게임 시작",
      "primary play",
      () => {
        this.play.disabled = true;
        play();
      },
      icon(Icons.play, 15, 2.4),
    );
    this.play.hidden = true;
    const progress = h("div", { class: "progress" }, this.bar, h("div", { class: "captions" }, this.left, this.right));
    this.account.hidden = true;
    this.root = frame(
      chrome,
      false,
      logoBadge(),
      this.settings,
      heading(this.title, this.subtitle),
      progress,
      spacer(),
      h("footer", { class: "footer" }, this.account, this.version, this.play),
    );
  }

  // Enter 로 누를 때도 버튼을 거쳐, 한 번 누른 뒤의 Enter 가 시작을 두 번 보내지 않는다
  pressPlay(): void {
    if (!this.play.hidden && !this.play.disabled) this.play.click();
  }

  setVersion(version: string): void {
    this.version.textContent = `런처 ${version}`;
  }

  setAccount(playerId: string, name: string): void {
    this.account.replaceChildren(head(this.webBase, playerId, 20, 4), name);
    this.account.hidden = false;
  }

  clearAccount(): void {
    this.account.replaceChildren();
    this.account.hidden = true;
  }

  checking(subtitle: string, title = "업데이트 확인 중"): void {
    this.text(title, subtitle);
    this.settings.hidden = false;
    this.showPlay(false);
    this.mode("indeterminate");
    this.captions("", "");
  }

  downloading(title: string, subtitle: string, allowSettings: boolean): void {
    this.text(title, subtitle);
    this.settings.hidden = !allowSettings;
    this.showPlay(false);
    this.mode("determinate");
    this.fraction(0);
  }

  ready(subtitle: string): void {
    this.text("게임을 시작할 수 있어요", subtitle);
    this.settings.hidden = false;
    this.mode("done");
    this.readyCaption();
    this.showPlay(true);
  }

  progress(fraction: number, left: string, right: string): void {
    this.fraction(fraction);
    this.captions(left, right);
  }

  launching(server: string): void {
    this.text("게임 시작 중", server);
    this.settings.hidden = true;
    this.showPlay(false);
    this.mode("done");
    this.readyCaption();
  }

  private readyCaption(): void {
    this.left.replaceChildren(h("span", { class: "ready" }, icon(Icons.check, 14, 3), "준비 완료"));
    this.right.textContent = "";
  }

  private showPlay(visible: boolean): void {
    this.play.hidden = !visible;
    this.play.disabled = false;
    this.version.hidden = visible;
  }

  private text(title: string, subtitle: string): void {
    this.title.textContent = title;
    this.subtitle.textContent = subtitle;
  }

  private captions(left: string, right: string): void {
    this.left.textContent = left;
    this.right.textContent = right;
  }

  private fraction(value: number): void {
    this.bar.style.setProperty("--fraction", String(Math.min(Math.max(value, 0), 1)));
  }

  private mode(mode: "indeterminate" | "determinate" | "done"): void {
    this.bar.classList.toggle("indeterminate", mode === "indeterminate");
    this.bar.classList.toggle("done", mode === "done");
  }
}

export function loginScreen(chrome: Chrome, cancel: () => void, reopen: () => void): HTMLElement {
  return frame(
    chrome,
    false,
    logoBadge(),
    null,
    heading(h("h1", { class: "title" }, "브라우저에서 로그인해 주세요"), h("p", { class: "subtitle" }, "Microsoft 로그인 창을 열었어요")),
    h("div", { class: "status" }, icon(Icons.spinner, 18, 2.5, "spinner"), "로그인을 기다리는 중"),
    spacer(),
    actions(pill("취소", "ghost", cancel), pill("다시 열기", "primary", reopen, icon(Icons.external, 15, 2.4))),
  );
}

export interface ErrorContent {
  title: string;
  body?: string;
  rows?: Node;
  secondary?: Action;
  primary: Action;
}

const errorBadge = () => h("div", { class: "error-badge" }, icon(Icons.alert, 28, 2.4));

export function errorScreen(chrome: Chrome, content: ErrorContent): HTMLElement {
  const text = h("div", { class: "text" }, h("h1", { class: "title" }, content.title), content.rows ?? h("p", { class: "body" }, content.body ?? ""));
  const buttons = [content.secondary && pill(content.secondary.label, "secondary", content.secondary.run), pill(content.primary.label, "primary", content.primary.run)];
  return frame(chrome, true, errorBadge(), null, text, spacer(), actions(...buttons.filter((button): button is HTMLButtonElement => Boolean(button))));
}

export interface CrashActions {
  openLogs(): void;
  openReport(): void;
  copy(): Promise<void>;
  close(): void;
  restart(): void;
}

function infoRow(label: string, ...values: (Node | string | null)[]): HTMLElement {
  return h("div", { class: "row" }, h("span", { class: "row-label" }, label), ...values);
}

export function crashScreen(chrome: Chrome, crash: Crash, on: CrashActions): HTMLElement {
  const code =
    crash.code === null
      ? [h("span", { class: "row-name" }, "알 수 없음")]
      : [h("span", { class: "row-name" }, String(crash.code)), crash.hex && h("span", { class: "caption" }, crash.hex)];
  const rows = h("div", { class: "rows" }, infoRow("코드", ...code), crash.report && infoRow("보고서", h("span", { class: "row-name" }, crash.report)));
  const opens = h(
    "div",
    { class: "text-actions" },
    pill("로그 폴더 열기", "secondary small", on.openLogs),
    crash.report && pill("크래시 보고서 열기", "secondary small", on.openReport),
  );
  const copy = pill("정보 복사", "ghost aside", () => {
    void on.copy().then(
      () => feedback("복사했어요"),
      () => feedback("복사하지 못했어요"),
    );
  });
  const feedback = (label: string) => {
    copy.textContent = label;
    window.setTimeout(() => (copy.textContent = "정보 복사"), COPY_FEEDBACK_MS);
  };
  const screen = frame(
    chrome,
    true,
    errorBadge(),
    null,
    h("div", { class: "text" }, h("h1", { class: "title" }, "게임이 꺼졌어요"), rows, opens),
    spacer(),
    actions(copy, pill("닫기", "secondary", on.close), pill("다시 시작", "primary", on.restart)),
  );
  if (crash.report) screen.dataset.height = String(CRASH_REPORT_HEIGHT);
  return screen;
}

export function accountRows(webBase: string, rows: { label: string; playerId: string; name: string }[]): HTMLElement {
  return h(
    "div",
    { class: "rows" },
    ...rows.map((row) =>
      h("div", { class: "row" }, h("span", { class: "row-label" }, row.label), head(webBase, row.playerId, 18, 4), h("span", { class: "row-name" }, row.name)),
    ),
  );
}

export interface SettingsModel {
  memoryMegabytes: number;
  totalMemoryBytes: number;
  gameDirectory: string;
  session: { playerId: string; name: string } | null;
  webBase: string;
  footer: string;
  onMemory(megabytes: number): void;
  onOpenFolder(): void;
  onSignOut(): void;
}

const MEMORY_GIGABYTES = [4, 6, 8, 12];

export function settingsScreen(chrome: Chrome, model: SettingsModel, back: () => void): HTMLElement {
  const header = h(
    "header",
    { class: "settings-header", "data-tauri-drag-region": true },
    iconButton(Icons.back, "뒤로", back, true),
    h("h1", { class: "settings-title" }, "설정"),
    iconButton(Icons.close, "닫기", chrome.close, true),
  );

  const options = MEMORY_GIGABYTES.map((size) =>
    h("button", { class: "option", "aria-pressed": String(model.memoryMegabytes === size * 1024) }, `${size} GB`),
  );
  options.forEach((option, index) =>
    option.addEventListener("click", () => {
      options.forEach((other) => other.setAttribute("aria-pressed", String(other === option)));
      model.onMemory(MEMORY_GIGABYTES[index] * 1024);
    }),
  );
  const memory = h(
    "div",
    { class: "card memory" },
    h("div", { class: "card-top" }, h("span", { class: "card-title" }, "메모리"), h("span", { class: "caption" }, `PC 메모리 ${gigabytes(model.totalMemoryBytes)}`)),
    h("div", { class: "options" }, ...options),
  );

  const kind = h("span", { class: "caption" }, "Microsoft 계정");
  const signOut = pill("로그아웃", "danger small", () => {
    model.onSignOut();
    kind.textContent = "로그아웃했어요";
    signOut.disabled = true;
  });
  signOut.disabled = model.session === null;
  const places = h(
    "div",
    { class: "card places" },
    h(
      "div",
      { class: "item" },
      h("div", { class: "item-text" }, h("span", { class: "item-title" }, "게임 폴더"), h("span", { class: "caption path", title: model.gameDirectory }, model.gameDirectory)),
      pill("열기", "secondary small", model.onOpenFolder),
    ),
    h(
      "div",
      { class: "item" },
      head(model.webBase, model.session?.playerId ?? null, 28, 6),
      h("div", { class: "item-text" }, h("span", { class: "item-title" }, model.session?.name ?? "로그인 전"), kind),
      signOut,
    ),
  );

  return h(
    "section",
    { class: "screen settings", "data-height": String(SETTINGS_HEIGHT) },
    header,
    h("div", { class: "settings-body" }, memory, places, spacer(), h("p", { class: "caption settings-footer" }, model.footer)),
  );
}
