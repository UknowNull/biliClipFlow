import { useEffect, useRef } from "react";
import { listen } from "@tauri-apps/api/event";

const APP_DATA_CHANGED_EVENT = "app-data-changed";
const FALLBACK_POLL_MS = 30000;

// 订阅后端数据变更事件：payload[scope] 为 true 时调用 onChange。
// minGapMs 限制两次刷新的最小间隔（期间的变更合并到一次尾随刷新），另以 30 秒轮询兜底防止事件丢失。
// onChange 始终取最新的闭包，调用方无需把其依赖放进 deps。
export default function useAppDataChanged(scope, onChange, { enabled = true, minGapMs = 0 } = {}) {
  const callbackRef = useRef(onChange);
  callbackRef.current = onChange;

  useEffect(() => {
    if (!enabled) {
      return undefined;
    }
    let disposed = false;
    let lastRun = 0;
    let trailing = null;
    let unlisten = null;
    const run = () => {
      trailing = null;
      if (disposed) {
        return;
      }
      lastRun = Date.now();
      callbackRef.current();
    };
    const trigger = () => {
      if (trailing) {
        return;
      }
      const wait = lastRun + minGapMs - Date.now();
      if (wait <= 0) {
        run();
      } else {
        trailing = setTimeout(run, wait);
      }
    };
    listen(APP_DATA_CHANGED_EVENT, (event) => {
      if (event.payload?.[scope]) {
        trigger();
      }
    }).then((fn) => {
      if (disposed) {
        fn();
      } else {
        unlisten = fn;
      }
    });
    const timer = setInterval(trigger, FALLBACK_POLL_MS);
    return () => {
      disposed = true;
      clearInterval(timer);
      if (trailing) {
        clearTimeout(trailing);
      }
      if (unlisten) {
        unlisten();
      }
    };
  }, [scope, enabled, minGapMs]);
}
