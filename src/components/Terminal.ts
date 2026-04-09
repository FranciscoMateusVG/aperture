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
    fontFamily: "'JetBrains Mono', 'Fira Code', 'Cascadia Code', monospace",
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
  // Triple-fit strategy: immediate rAF, short delay, and longer delay
  // to handle production build timing where layout may settle late.
  requestAnimationFrame(() => {
    fitAddon.fit();
    setTimeout(() => fitAddon.fit(), 100);
    setTimeout(() => fitAddon.fit(), 500);
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

  // Handle resize
  const resizeObserver = new ResizeObserver(() => {
    fitAddon.fit();
    commands.resizePty(term.rows, term.cols);
  });
  resizeObserver.observe(container);

  // Also listen to window resize events (triggered by panel toggle/drag)
  const onWindowResize = () => {
    fitAddon.fit();
    commands.resizePty(term.rows, term.cols);
  };
  window.addEventListener("resize", onWindowResize);

  return {
    terminal: term,
    destroy() {
      unlisten();
      resizeObserver.disconnect();
      window.removeEventListener("resize", onWindowResize);
      term.dispose();
    }
  };
}
