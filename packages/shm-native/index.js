// index.ts
import { createHash } from "node:crypto";
import { existsSync, readFileSync } from "node:fs";
import { createRequire } from "node:module";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
import { markAsUntransferable } from "node:worker_threads";
var QUALIFIED_TEST_PROFILE = "host-test-ring-v1";
var DESCRIPTOR_SCHEMA_VERSION = 2;

class NativeStartupError extends Error {
  reason;
  constructor(reason) {
    super(`shared-memory native startup failed: ${reason}`);
    this.reason = reason;
    this.name = "NativeStartupError";
  }
}
var loaded;
var loadError;
var constructorCapability;
var PLATFORM_PACKAGES = {
  "darwin-arm64": {
    package: "@eidnara/host-darwin-arm64",
    target: "darwin-arm64",
    nativeTarget: "macos-aarch64"
  },
  "darwin-x64": {
    package: "@eidnara/host-darwin-x64",
    target: "darwin-x64",
    nativeTarget: "macos-x86_64"
  },
  "linux-x64": {
    package: "@eidnara/host-linux-x64-gnu",
    target: "linux-x64-gnu",
    nativeTarget: "linux-x86_64"
  }
};
var ADDON_PAYLOAD_PATH = "payload/native/shm_native.node";
function platformPackage() {
  const platform = PLATFORM_PACKAGES[`${process.platform}-${process.arch}`];
  if (!platform)
    throw new NativeStartupError("unsupported_platform");
  return platform;
}
function packageAddonPath(platform) {
  const require2 = createRequire(import.meta.url);
  let packageJsonPath;
  try {
    packageJsonPath = require2.resolve(`${platform.package}/package.json`);
  } catch {
    throw new NativeStartupError("missing_addon");
  }
  const packageDir = dirname(packageJsonPath);
  const manifestPath = join(packageDir, "payload-manifest.json");
  if (!existsSync(manifestPath)) {
    throw new NativeStartupError("missing_manifest");
  }
  const manifest = JSON.parse(readFileSync(manifestPath, "utf8"));
  if (manifest.package?.name !== platform.package || manifest.package.target !== platform.target) {
    throw new NativeStartupError("wrong_platform_payload");
  }
  const entry = manifest.files?.find(({ path }) => path === ADDON_PAYLOAD_PATH);
  if (!entry || !/^[0-9a-f]{64}$/.test(entry.sha256 ?? "")) {
    throw new NativeStartupError("missing_checksum");
  }
  const addonPath = join(packageDir, ADDON_PAYLOAD_PATH);
  if (!existsSync(addonPath)) {
    throw new NativeStartupError("missing_addon");
  }
  const actual = createHash("sha256").update(readFileSync(addonPath)).digest("hex");
  if (actual !== entry.sha256) {
    throw new NativeStartupError("checksum_mismatch");
  }
  return addonPath;
}
function requireAddon() {
  if (loaded)
    return loaded;
  if (loadError)
    throw loadError;
  try {
    const platform = platformPackage();
    const localPath = new URL("./shm_native.node", import.meta.url);
    const addonPath = existsSync(localPath) ? fileURLToPath(localPath) : packageAddonPath(platform);
    const native = createRequire(import.meta.url)(addonPath);
    if (native.buildProfile() !== "release") {
      throw new NativeStartupError("debug_build");
    }
    if (native.buildTarget() !== platform.nativeTarget) {
      throw new NativeStartupError("wrong_platform_binary");
    }
    loaded = native;
    return native;
  } catch (error) {
    loadError = error instanceof Error ? error : new Error(String(error));
    loaded = null;
    throw loadError;
  }
}
function capableAddon() {
  const native = requireAddon();
  const capability = constructorCapability ??= probeCapabilities();
  if (!capability.available)
    throw new NativeStartupError("capability_unavailable");
  return native;
}
function addon() {
  try {
    return requireAddon();
  } catch {
    return null;
  }
}
function protect(segments) {
  for (const segment of segments) {
    if (!(segment.buffer instanceof ArrayBuffer)) {
      throw new Error("external segment lacks ArrayBuffer backing");
    }
    markAsUntransferable(segment.buffer);
  }
}
function probeCapabilities() {
  const base = {
    napiVersion: null,
    externalArrayBuffer: false,
    exactBounds: false,
    detachment: false,
    transferPrevention: false,
    cleanupHooks: false
  };
  const native = addon();
  if (!native)
    return { available: false, ...base, reason: "addon_unavailable" };
  try {
    const napiVersion = native.napiVersion();
    if (napiVersion < 8) {
      return {
        available: false,
        ...base,
        napiVersion,
        reason: "napi_8_unavailable"
      };
    }
    if (typeof globalThis.Bun === "undefined") {
      return {
        available: false,
        ...base,
        napiVersion,
        reason: "detachment_unavailable"
      };
    }
    const view = native.createExternalProbe(31);
    const externalArrayBuffer = view instanceof Uint8Array && view.byteLength === 31;
    const exactBounds = externalArrayBuffer && view.byteOffset === 0 && view.buffer.byteLength === 31;
    if (!exactBounds) {
      return {
        available: false,
        ...base,
        napiVersion,
        externalArrayBuffer,
        reason: "external_exact_bounds_unavailable"
      };
    }
    const arrayBuffer = view.buffer;
    const subarray = view.subarray(1, 30);
    const dataView = new DataView(arrayBuffer, 1, 29);
    const bufferAlias = Buffer.from(arrayBuffer, 0, view.byteLength);
    markAsUntransferable(arrayBuffer);
    let transferPrevention = false;
    try {
      structuredClone(arrayBuffer, { transfer: [arrayBuffer] });
    } catch {
      transferPrevention = arrayBuffer.byteLength === 31;
    }
    if (!transferPrevention) {
      return {
        available: false,
        ...base,
        napiVersion,
        externalArrayBuffer,
        exactBounds,
        reason: "transfer_prevention_unavailable"
      };
    }
    const detachment = native.detachArrayBuffer(arrayBuffer);
    const aliasesDetached = detachment && Number(arrayBuffer.byteLength) === 0 && Number(view.byteLength) === 0 && subarray.byteLength === 0 && bufferAlias.byteLength === 0 && (() => {
      try {
        return dataView.byteLength === 0;
      } catch {
        return true;
      }
    })();
    if (!aliasesDetached) {
      return {
        available: false,
        ...base,
        napiVersion,
        externalArrayBuffer,
        exactBounds,
        transferPrevention,
        reason: "detachment_unavailable"
      };
    }
    if (typeof native.registerCleanupProbe !== "function") {
      return {
        available: false,
        ...base,
        napiVersion,
        externalArrayBuffer,
        exactBounds,
        detachment: true,
        transferPrevention,
        reason: "cleanup_hooks_unavailable"
      };
    }
    return {
      available: true,
      napiVersion,
      externalArrayBuffer,
      exactBounds,
      detachment: true,
      transferPrevention,
      cleanupHooks: true
    };
  } catch {
    return {
      available: false,
      ...base,
      reason: "runtime_mechanism_unavailable"
    };
  }
}

class ProducerCursor {
  segments;
  capacity;
  cursor = 0;
  constructor(segments, capacity) {
    this.segments = segments;
    this.capacity = capacity;
    const available = segments.reduce((sum, segment) => sum + segment.byteLength, 0);
    if (available !== capacity)
      throw new RangeError("producer spans disagree with reservation");
  }
  get written() {
    return this.cursor;
  }
  get remaining() {
    return this.capacity - this.cursor;
  }
  view() {
    let offset = this.cursor;
    for (const segment of this.segments) {
      if (offset < segment.byteLength)
        return segment.subarray(offset);
      offset -= segment.byteLength;
    }
    return new Uint8Array(0);
  }
  advance(bytes) {
    if (!Number.isSafeInteger(bytes) || bytes < 0 || bytes > this.remaining) {
      throw new RangeError("producer overflow");
    }
    this.cursor += bytes;
  }
  write(bytes) {
    if (bytes.byteLength > this.remaining)
      throw new RangeError("producer overflow");
    let source = 0;
    let offset = this.cursor;
    for (const segment of this.segments) {
      if (source === bytes.byteLength)
        break;
      if (offset >= segment.byteLength) {
        offset -= segment.byteLength;
        continue;
      }
      const take = Math.min(segment.byteLength - offset, bytes.byteLength - source);
      segment.set(bytes.subarray(source, source + take), offset);
      source += take;
      offset = 0;
    }
    this.cursor += bytes.byteLength;
  }
}

class NativeProducerReservation {
  native;
  channel;
  token;
  segments;
  active = true;
  constructor(native, channel, token, segments) {
    this.native = native;
    this.channel = channel;
    this.token = token;
    this.segments = segments;
    protect(segments);
  }
  commit(header, written, beforePublish) {
    this.assertActive();
    this.active = false;
    this.native.commitReservation(this.channel, this.token, header, written, beforePublish ?? (() => {}));
  }
  abort() {
    if (!this.active)
      return;
    this.active = false;
    this.native.abortReservation(this.channel, this.token);
  }
  assertActive() {
    if (!this.active)
      throw new Error("producer reservation is released");
  }
}

class NativeReceiveLease {
  native;
  channel;
  token;
  segments;
  header;
  released = false;
  constructor(native, channel, token, segments, header) {
    this.native = native;
    this.channel = channel;
    this.token = token;
    this.segments = segments;
    this.header = header;
    protect(segments);
  }
  get byteLength() {
    this.assertActive();
    return this.segments.reduce((sum, segment) => sum + segment.byteLength, 0);
  }
  get segmentCount() {
    this.assertActive();
    return this.segments.length;
  }
  segment(index) {
    this.assertActive();
    const segment = this.segments[index];
    if (!segment)
      throw new RangeError("receive segment does not exist");
    return segment;
  }
  release() {
    if (this.released)
      throw new Error("receive lease is already released");
    this.released = true;
    this.native.release(this.channel, this.token);
  }
  [Symbol.dispose]() {
    if (this.released)
      return;
    this.release();
  }
  assertActive() {
    if (this.released)
      throw new Error("receive lease is released");
  }
}

class NativeChannel {
  native;
  id;
  closed = false;
  constructor(native, id) {
    this.native = native;
    this.id = id;
  }
  static attach(descriptor) {
    const native = capableAddon();
    return new NativeChannel(native, native.attach(descriptor));
  }
  static connectSetup(options) {
    const native = capableAddon();
    return new NativeChannel(native, native.connectSetup(options));
  }
  static createTestPair() {
    const native = capableAddon();
    const pair = native.createTestPair();
    return {
      first: new NativeChannel(native, pair.first),
      second: new NativeChannel(native, pair.second),
      descriptorDepth: pair.descriptorDepth,
      arenaBytes: pair.arenaBytes
    };
  }
  produce(header, capacity, fill, beforePublish, timeoutMs = 0) {
    this.assertOpen();
    this.native.produce(this.id, header, capacity, timeoutMs, (segments) => {
      protect(segments);
      const cursor = new ProducerCursor(segments, capacity);
      fill(cursor);
      if (cursor.written !== capacity)
        throw new RangeError("producer underfill");
      return cursor.written;
    }, beforePublish ?? (() => {}));
  }
  reserve(capacity, timeoutMs = 0) {
    this.assertOpen();
    let token;
    let segments;
    this.native.reserve(this.id, capacity, timeoutMs, (reservedToken, reservedSegments) => {
      token = reservedToken;
      segments = reservedSegments;
    });
    if (token === undefined || segments === undefined) {
      throw new Error("native reservation callback did not run");
    }
    return new NativeProducerReservation(this.native, this.id, token, segments);
  }
  poll(deliver) {
    this.assertOpen();
    return this.native.poll(this.id, (token, header, segments) => {
      deliver(new NativeReceiveLease(this.native, this.id, token, segments, header));
    });
  }
  peerClosed() {
    if (this.closed)
      return true;
    return this.native.peerClosed(this.id);
  }
  close() {
    if (this.closed)
      return;
    this.native.close(this.id);
    this.closed = true;
  }
  forceClose() {
    if (this.closed)
      return;
    this.native.forceClose(this.id);
    this.closed = true;
  }
  assertOpen() {
    if (this.closed)
      throw new Error("native channel is closed");
  }
}
function registerCleanupProbe(path) {
  const native = addon();
  if (!native)
    return false;
  native.registerCleanupProbe(path);
  return true;
}
function nativeLeakDiagnostics() {
  return addon()?.nativeLeakDiagnostics() ?? 0;
}
function activeExternalRefs() {
  return addon()?.activeExternalRefCount() ?? 0;
}
function setExternalViewCreationFailpoint(call) {
  addon()?.setExternalViewFailpoint(call);
}
function activeNativeChannels() {
  return addon()?.activeChannelCount() ?? 0;
}
export {
  setExternalViewCreationFailpoint,
  registerCleanupProbe,
  probeCapabilities,
  nativeLeakDiagnostics,
  activeNativeChannels,
  activeExternalRefs,
  QUALIFIED_TEST_PROFILE,
  ProducerCursor,
  NativeStartupError,
  NativeReceiveLease,
  NativeProducerReservation,
  NativeChannel,
  DESCRIPTOR_SCHEMA_VERSION
};
