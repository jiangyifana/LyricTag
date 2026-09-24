/**
 * Tauri 通信的最小桥接层。
 *
 * **刻意不引入 `@tauri-apps/api`**：本项目的构建链路里没有打包器，
 * 裸模块名（`import { invoke } from "@tauri-apps/api/core"`）在 WebView 里
 * 无法解析。而这层需要的功能只有 invoke / listen / 窗口控制三件事，
 * 直接对接 Tauri 暴露的 `__TAURI_INTERNALS__` 即可，约 80 行、零依赖。
 */

interface TauriInternals {
  invoke(cmd: string, args?: Record<string, unknown>, options?: unknown): Promise<unknown>;
  transformCallback(callback?: (payload: unknown) => void, once?: boolean): number;
  metadata?: { currentWindow?: { label?: string } };
}

declare global {
  interface Window {
    __TAURI_INTERNALS__?: TauriInternals;
  }
}

function internals(): TauriInternals {
  const api = window.__TAURI_INTERNALS__;
  if (!api) throw new Error("当前不在应用环境中运行");
  return api;
}

/**
 * 调用一个后端命令。
 *
 * 后端命令的错误一律映射成**面向用户的中文句子**（§6.5），
 * 因此这里直接把错误信息交给调用方展示，不再包装。
 */
export async function invoke<T>(cmd: string, args?: Record<string, unknown>): Promise<T> {
  const raw = await internals().invoke(cmd, args ?? {});
  return raw as T;
}

export type UnlistenFn = () => void;

interface RawEvent<T> {
  event?: string;
  id?: number;
  payload?: T;
}

/**
 * 订阅一个后端事件。
 *
 * 后端推送的载荷有两种可能形态（事件对象 `{event,id,payload}` 或直接是载荷本身），
 * 这里统一成「直接给载荷」，调用方不需要关心。
 */
export async function listen<T>(event: string, handler: (payload: T) => void): Promise<UnlistenFn> {
  const api = internals();
  const id = api.transformCallback((raw: unknown) => {
    if (raw && typeof raw === "object" && "payload" in raw) {
      handler((raw as RawEvent<T>).payload as T);
    } else {
      handler(raw as T);
    }
  });
  const eventId = await invoke<number>("plugin:event|listen", {
    event,
    target: { kind: "Any" },
    handler: id,
  });
  return () => {
    void invoke("plugin:event|unlisten", { event, eventId });
  };
}

// ── 窗口控制（无边框窗口需要自绘按钮） ──────────────────────────────────

function windowLabel(): string {
  return internals().metadata?.currentWindow?.label ?? "main";
}

export const appWindow = {
  minimize: () => invoke<void>("plugin:window|minimize", { label: windowLabel() }),
  toggleMaximize: () => invoke<void>("plugin:window|toggle_maximize", { label: windowLabel() }),
  close: () => invoke<void>("plugin:window|close", { label: windowLabel() }),
};

// ── 系统原生对话框（§3.2：目录选择**必须**走系统原生选择框） ─────────────

/** 选择音乐库目录。返回绝对路径。 */
export async function pickDirectory(defaultPath?: string): Promise<string | null> {
  const selected = await invoke<string | null>("plugin:dialog|open", {
    options: {
      directory: true,
      multiple: false,
      title: "选择音乐库目录",
      defaultPath: defaultPath && defaultPath.length > 0 ? defaultPath : undefined,
    },
  });
  return selected ?? null;
}
