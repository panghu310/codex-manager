import { invoke } from "@tauri-apps/api/core";

export const WINDOW_DRAG_SELECTOR = "[data-window-drag]";
export const WINDOW_NO_DRAG_SELECTOR = "button, input, textarea, select, a, [data-no-drag]";

export function shouldStartWindowDrag(event) {
  if (!event || event.button !== 0) return false;
  return !event.target?.closest?.(WINDOW_NO_DRAG_SELECTOR);
}

function pointerScreenPosition(event) {
  return {
    x: Number.isFinite(event.screenX) ? event.screenX : event.clientX,
    y: Number.isFinite(event.screenY) ? event.screenY : event.clientY
  };
}

export function nextWindowDragPosition(start, event) {
  const pointer = pointerScreenPosition(event);
  return {
    x: Math.round(start.windowX + (pointer.x - start.pointerX)),
    y: Math.round(start.windowY + (pointer.y - start.pointerY))
  };
}

export function bindWindowDragging(
  root,
  controls = {
    start: () => invoke("begin_window_drag"),
    move: (position) => invoke("move_window_to", position)
  }
) {
  if (!root || !controls?.start || !controls?.move) return;

  root.querySelectorAll(WINDOW_DRAG_SELECTOR).forEach((element) => {
    element.addEventListener("mousedown", async (event) => {
      if (!shouldStartWindowDrag(event)) return;
      event.preventDefault?.();
      const startPointer = pointerScreenPosition(event);
      let drag = null;
      let latestMoveEvent = null;
      const handleMouseMove = (moveEvent) => {
        latestMoveEvent = moveEvent;
        if (drag) moveWindow(drag, moveEvent, controls);
      };
      const handleMouseUp = () => {
        globalThis.removeEventListener?.("mousemove", handleMouseMove);
        globalThis.removeEventListener?.("mouseup", handleMouseUp);
      };

      globalThis.addEventListener?.("mousemove", handleMouseMove);
      globalThis.addEventListener?.("mouseup", handleMouseUp, { once: true });

      try {
        drag = {
          pointerX: startPointer.x,
          pointerY: startPointer.y,
          ...(await controls.start())
        };
        if (latestMoveEvent) moveWindow(drag, latestMoveEvent, controls);
      } catch (error) {
        console.warn("窗口拖动初始化失败", error);
        handleMouseUp();
      }
    });
  });
}

function moveWindow(drag, event, controls) {
  controls.move(nextWindowDragPosition(drag, event)).catch((error) => {
    console.warn("窗口移动失败", error);
  });
}
