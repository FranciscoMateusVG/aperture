import { Terminal } from "@xterm/xterm";
import { FitAddon } from "@xterm/addon-fit";
import { WebglAddon } from "@xterm/addon-webgl";
import { commands } from "../services/tauri-commands";
import { onPtyOutput } from "../services/event-listener";

export async function createTerminal(container: HTMLElement, sessionName: string) {
  const term = new Terminal({
    cursorBlink: true,
    fontSize: 14,
    scrollback: 10000,
    fontFamily: "'MesloLGS NF', monospace",
    theme: {
      background: "#1a1a2e",
      foreground: "#e0e0e0",
      cursor: "#f39c12",
      selectionBackground: "#3a3a5e",
    },
  });

  const fitAddon = new FitAddon();
  term.loadAddon(fitAddon);

  term.open(container);

  // Try WebGL renderer with robust fallback.
  // The WebGL addon can succeed at loadAddon() but fail asynchronously during
  // rendering (common in Tauri production webviews). Listen for context loss
  // and dispose the addon if it fires.
  try {
    const webglAddon = new WebglAddon();
    webglAddon.onContextLoss(() => {
      console.warn("WebGL context lost, falling back to canvas renderer");
      webglAddon.dispose();
    });
    term.loadAddon(webglAddon);
  } catch {
    console.warn("WebGL addon failed to load, using canvas renderer");
  }

  // Delay initial fit to ensure container has layout dimensions.
  // Two-fit strategy: immediate rAF + one safety re-fit for late-settling layouts.
  requestAnimationFrame(() => {
    fitAddon.fit();
    setTimeout(() => fitAddon.fit(), 200);
  });

  // Start PTY and connect
  await commands.startPty(sessionName);

  // Listen for PTY output
  const unlisten = await onPtyOutput((data) => {
    term.write(data);
  });

  // Send keyboard input to PTY
  term.onData((data) => {
    commands.writePty(data);
  });

  // Debounced resize handler — ResizeObserver and window resize both fire here.
  // Without debouncing, panel drags produce dozens of Tauri IPC calls per second.
  let resizeTimer: ReturnType<typeof setTimeout> | null = null;
  const handleResize = () => {
    if (resizeTimer) clearTimeout(resizeTimer);
    resizeTimer = setTimeout(() => {
      fitAddon.fit();
      commands.resizePty(term.rows, term.cols);
    }, 50);
  };

  const resizeObserver = new ResizeObserver(handleResize);
  resizeObserver.observe(container);

  // Also listen to window resize events (triggered by panel toggle/drag)
  window.addEventListener("resize", handleResize);

  return {
    terminal: term,
    focus() {
      term.focus();
    },
    destroy() {
      unlisten();
      resizeObserver.disconnect();
      window.removeEventListener("resize", handleResize);
      term.dispose();
    }
  };
}
