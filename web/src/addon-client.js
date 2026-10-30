// Guest execution is isolated from the UI and host-owned enrollment/session keys.
export class AddonClient {
  constructor() {
    this.worker = new Worker(new URL("./addon-worker.js", import.meta.url), { type: "module" });
    this.pending = new Map();
    this.sequence = 0;
    this.worker.onmessage = ({ data }) => {
      const pending = this.pending.get(data.id);
      if (!pending) return;
      clearTimeout(pending.timer);
      this.pending.delete(data.id);
      if (data.error) pending.reject(new Error(data.error));
      else pending.resolve(data.value);
    };
    this.worker.onerror = () => this.close("Add-on worker failed");
  }

  call(data) {
    if (!this.worker) return Promise.reject(new Error("Add-on worker closed; load the package again"));
    if (this.pending.size) return Promise.reject(new Error("An add-on operation is already running"));
    const id = ++this.sequence;
    return new Promise((resolve, reject) => {
      const timer = setTimeout(() => this.close("Add-on exceeded the 15-second execution deadline"), 15_000);
      this.pending.set(id, { resolve, reject, timer });
      this.worker.postMessage({ ...data, id });
    });
  }

  close(reason = "Add-on unloaded") {
    this.worker?.terminate();
    this.worker = null;
    for (const { reject, timer } of this.pending.values()) {
      clearTimeout(timer);
      reject(new Error(reason));
    }
    this.pending.clear();
  }
}
