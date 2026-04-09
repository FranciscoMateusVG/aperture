import { listen } from "@tauri-apps/api/event";

export function onPtyOutput(callback: (data: string) => void) {
  return listen<string>("pty-output", (event) => {
    callback(event.payload);
  });
}
