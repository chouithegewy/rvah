import assert from "node:assert/strict";
import test from "node:test";
import { MockVeilidNetwork } from "../src/mock-veilid.js";

test("two mock nodes exchange DHT values and private-route calls without external IO", async () => {
  const network = new MockVeilidNetwork("unit-test");
  let publisher;
  publisher = network.createNode(async (update) => {
    if (update.kind === "AppCall") {
      const response = Uint8Array.from(update.message, (byte) => byte ^ 0xff);
      await publisher.client.appCallReply(update.callId, response);
    }
  });
  const reader = network.createNode();

  const descriptor = await publisher.routing.createDHTRecord("VLD0", { DFLT: { oCnt: 3 } });
  await publisher.routing.setDHTValue(descriptor.key, 1, new Uint8Array([1, 2, 3]));
  await publisher.routing.setDHTValue(descriptor.key, 0, new Uint8Array([9]));
  assert.equal(await publisher.routing.flushDHTRecord(descriptor.key), true);
  await publisher.routing.closeDHTRecord(descriptor.key);

  const recordKey = reader.veilid.RecordKey.parse(descriptor.key.toString());
  await reader.routing.openDHTRecord(recordKey);
  assert.deepEqual(Array.from((await reader.routing.getDHTValue(recordKey, 1, true)).data), [1, 2, 3]);
  await assert.rejects(
    reader.routing.setDHTValue(recordKey, 2, new Uint8Array([4])),
    /not open for writing/,
  );
  await reader.routing.closeDHTRecord(recordKey);

  const route = await publisher.client.newPrivateRoute();
  const imported = reader.client.importRemotePrivateRoute(route.blob);
  assert.deepEqual(
    Array.from(await reader.routing.appCall({ RouteId: imported }, new Uint8Array([0x00, 0x55]))),
    [0xff, 0xaa],
  );
  reader.client.releasePrivateRoute(imported);
  assert.equal(network.routes.size, 1, "releasing an imported handle must not destroy its owner route");
  publisher.client.releasePrivateRoute(route.routeId);
  assert.equal(network.routes.size, 0);
  assert.equal(network.externalIoCount, 0);
});
