import init, { BrowserAddon } from "../pkg/rvah_web.js";

const ready = init();
let addon;
self.onmessage = async ({ data }) => {
  try {
    await ready;
    let value;
    if (data.action === "load") {
      addon?.free();
      addon = null;
      addon = new BrowserAddon(data.archive, data.signature, data.publicKey);
      value = JSON.parse(addon.manifest_json());
    } else if (data.action === "invoke") {
      if (!addon) throw new Error("Load a signed add-on first");
      value = JSON.parse(addon.invoke(
        JSON.stringify(data.operation), JSON.stringify(data.grants), JSON.stringify(data.settings),
      ));
    } else throw new Error("Unknown add-on operation");
    self.postMessage({ id: data.id, value });
  } catch (error) {
    self.postMessage({ id: data.id, error: error instanceof Error ? error.message : String(error) });
  }
};
