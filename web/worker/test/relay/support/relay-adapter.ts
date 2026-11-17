// Adapts a workerd client WebSocket to the mock desktop's `FrameSocket`.
import type { FrameSocket } from "@monoptah/mock-desktop";

export class WorkerdFrameSocket implements FrameSocket {
  onmessage: ((bytes: Uint8Array) => void) | null = null;
  ontext: ((text: string) => void) | null = null;
  onclose: ((code: number) => void) | null = null;
  closed = false;
  closeCode: number | null = null;
  readonly #ws: WebSocket;

  constructor(ws: WebSocket) {
    this.#ws = ws;
    ws.binaryType = "arraybuffer";
    ws.accept();
    ws.addEventListener("message", (event) => {
      if (typeof event.data === "string") this.ontext?.(event.data);
      else this.onmessage?.(new Uint8Array(event.data as ArrayBuffer));
    });
    ws.addEventListener("close", (event) => {
      this.closed = true;
      this.closeCode = event.code;
      this.onclose?.(event.code);
    });
  }

  send(bytes: Uint8Array): void {
    this.#ws.send(bytes);
  }

  sendText(text: string): void {
    this.#ws.send(text);
  }

  close(code = 1000): void {
    this.#ws.close(code);
  }
}
