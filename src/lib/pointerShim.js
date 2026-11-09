// 某些环境（如经 CGEvent 注入的远程桌面）不派发 pointerdown/pointerup，
// 导致所有以 PointerEvent 实现的拖拽失效。这里在检测到缺失时用 mouse 事件合成。
// 环境正常时完全静默，不改变任何现有行为。

const REAL_POINTER_WINDOW_MS = 80;
// 浏览器真实 pointerId 是小整数（0-20 量级），用一个大值避开
const SYNTHETIC_POINTER_ID = 1000001;

export function installPointerShim(options = {}) {
  if (typeof window === "undefined" || !window.PointerEvent) return;
  if (window.__POINTER_SHIM_INSTALLED__ && !options.force) return;
  window.__POINTER_SHIM_INSTALLED__ = true;

  const forceBroken = options.forceBroken === true;
  const state = {
    broken: forceBroken ? true : null,
    active: false,
    target: null,
    lastRealPointerDown: -Infinity,
  };

  if (!forceBroken) {
    window.addEventListener("pointerdown", () => {
      state.lastRealPointerDown = performance.now();
      if (state.broken !== true) state.broken = false;
    }, true);
  }

  const isOwnPointer = (event) => state.broken && event.pointerId === SYNTHETIC_POINTER_ID;

  const make = (type, source) => new PointerEvent(type, {
    bubbles: true,
    cancelable: true,
    composed: true,
    pointerId: SYNTHETIC_POINTER_ID,
    pointerType: "mouse",
    isPrimary: true,
    button: type === "pointerup" ? (source.button ?? 0) : 0,
    buttons: type === "pointerup" ? 0 : 1,
    clientX: source.clientX,
    clientY: source.clientY,
    screenX: source.screenX,
    screenY: source.screenY,
    ctrlKey: source.ctrlKey,
    shiftKey: source.shiftKey,
    altKey: source.altKey,
    metaKey: source.metaKey,
  });

  const proto = Element.prototype;
  const originalSet = proto.setPointerCapture;
  const originalRelease = proto.releasePointerCapture;
  const originalHas = proto.hasPointerCapture;

  proto.setPointerCapture = function setPointerCapture(id) {
    if (state.broken && id === SYNTHETIC_POINTER_ID) return;
    return originalSet.call(this, id);
  };
  proto.releasePointerCapture = function releasePointerCapture(id) {
    if (state.broken && id === SYNTHETIC_POINTER_ID) return;
    return originalRelease.call(this, id);
  };
  proto.hasPointerCapture = function hasPointerCapture(id) {
    if (state.broken && id === SYNTHETIC_POINTER_ID) return false;
    return originalHas.call(this, id);
  };

  window.addEventListener("mousedown", (event) => {
    if (event.button !== 0) return;
    if (performance.now() - state.lastRealPointerDown < REAL_POINTER_WINDOW_MS) return;
    state.broken = true;
    state.active = true;
    state.target = event.target;
    event.target.dispatchEvent(make("pointerdown", event));
  }, true);

  window.addEventListener("pointermove", (event) => {
    if (!state.active || isOwnPointer(event)) return;
    event.stopImmediatePropagation();
  }, true);

  window.addEventListener("mousemove", (event) => {
    if (!state.active || !state.target) return;
    state.target.dispatchEvent(make("pointermove", event));
  }, true);

  window.addEventListener("pointerup", (event) => {
    if (!state.active || isOwnPointer(event)) return;
    event.stopImmediatePropagation();
  }, true);

  window.addEventListener("pointercancel", (event) => {
    if (!state.active || isOwnPointer(event)) return;
    event.stopImmediatePropagation();
  }, true);

  window.addEventListener("mouseup", (event) => {
    if (!state.active || !state.target) return;
    const target = state.target;
    state.active = false;
    state.target = null;
    target.dispatchEvent(make("pointerup", event));
  }, true);

  window.addEventListener("blur", () => {
    state.active = false;
    state.target = null;
  }, true);
}
