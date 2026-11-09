import { invokeCommand } from "./tauri";

/**
 * 把前端关键节点写进后端日志（auth_debug.log），用于排查打包环境下的白屏等问题。
 * 失败不影响主流程。
 */
export async function clientLog(message) {
  try {
    await invokeCommand("auth_client_log", { message: `[clip-review] ${message}` });
  } catch {
    // 日志失败不打断业务
  }
}

/** 把任意值压成一行 JSON 摘要，避免超长日志 */
export function brief(value, maxLen = 600) {
  let text;
  try {
    text = JSON.stringify(value);
  } catch {
    text = String(value);
  }
  text = String(text ?? "");
  return text.length > maxLen ? `${text.slice(0, maxLen)}…` : text;
}

/** 全局错误上报：渲染以外的错误（事件、异步）也能落盘 */
export function installGlobalErrorLogging() {
  if (typeof window === "undefined" || window.__CLIENT_ERROR_LOGGING__) return;
  window.__CLIENT_ERROR_LOGGING__ = true;

  window.addEventListener("error", (event) => {
    const err = event.error;
    void clientLog(
      `window_error message=${brief(event.message)} stack=${brief(err?.stack || "", 1200)}`,
    );
  });

  window.addEventListener("unhandledrejection", (event) => {
    const reason = event.reason;
    void clientLog(
      `unhandled_rejection reason=${brief(reason?.message || String(reason))} stack=${brief(reason?.stack || "", 1200)}`,
    );
  });
}
