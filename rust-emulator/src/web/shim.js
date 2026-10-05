/* SPDX-License-Identifier: GPL-3.0-only
 * Web MIDI for pages served by the FM-1 emulator: navigator.requestMIDIAccess
 * gives one input and one output, the emulated device, over the /midi
 * WebSocket (binary frames of raw MIDI bytes; each frame from the emulator
 * is one complete message). The port name is the device's USB product
 * string plus "(FM-1 Emulator)", as GET /__fm1/info reports it, so editors
 * that look for their device by name find it. Injected by the emulator. */
(function () {
  "use strict";
  if (window.__fm1WebMidi) return;
  var WS_URL = (location.protocol === "https:" ? "wss://" : "ws://") + location.host + "/midi";
  var state = { ws: null, open: false, queue: [], name: "FM-1 Emulator", retry: 250, access: null };

  function fire(target, type, init) {
    var e = new Event(type);
    for (var k in init) Object.defineProperty(e, k, { value: init[k], enumerable: true });
    var handler = target["on" + type];
    if (typeof handler === "function") handler.call(target, e);
    target.dispatchEvent(e);
  }

  class Port extends EventTarget {
    constructor(type) {
      super();
      this.id = "fm1-emulator-" + type;
      this.type = type;
      this.manufacturer = "FM-1 Emulator";
      this.version = "1.0";
      this.connection = "closed";
      this.onstatechange = null;
    }
    get name() { return state.name; }
    get state() { return state.open ? "connected" : "disconnected"; }
    open() { this._setConnection("open"); return Promise.resolve(this); }
    close() { this._setConnection("closed"); return Promise.resolve(this); }
    _setConnection(c) {
      if (this.connection === c) return;
      this.connection = c;
      changed(this);
    }
  }

  class Input extends Port {
    constructor() { super("input"); this._handler = null; }
    get onmidimessage() { return this._handler; }
    set onmidimessage(f) { this._handler = f; if (f) this.open(); }
    addEventListener(type, listener, options) {
      if (type === "midimessage") this.open();
      super.addEventListener(type, listener, options);
    }
    _receive(bytes) {
      if (this.connection !== "open") return;
      var e = new Event("midimessage");
      Object.defineProperty(e, "data", { value: bytes, enumerable: true });
      Object.defineProperty(e, "receivedTime", { value: performance.now(), enumerable: true });
      if (typeof this._handler === "function") this._handler.call(this, e);
      this.dispatchEvent(e);
    }
  }

  class Output extends Port {
    constructor() { super("output"); }
    send(data, timestamp) {
      var bytes = Uint8Array.from(data);
      for (var i = 0; i < bytes.length; i++)
        if (bytes[i] > 255) throw new TypeError("MIDI bytes are 0..255");
      this.open();
      var delay = timestamp ? Math.max(0, timestamp - performance.now()) : 0;
      if (delay > 0) setTimeout(function () { transmit(bytes); }, delay);
      else transmit(bytes);
    }
    clear() {}
  }

  var input = new Input(), output = new Output();

  class Access extends EventTarget {
    constructor() {
      super();
      this.inputs = new Map([[input.id, input]]);
      this.outputs = new Map([[output.id, output]]);
      this.sysexEnabled = true;
      this.onstatechange = null;
    }
  }

  function changed(port) {
    fire(port, "statechange", { port: port });
    if (state.access) fire(state.access, "statechange", { port: port });
  }

  function transmit(bytes) {
    if (state.open) state.ws.send(bytes);
    else state.queue.push(bytes);
  }

  function refreshName() {
    return fetch("/__fm1/info", { cache: "no-store" })
      .then(function (r) { return r.json(); })
      .then(function (info) { if (info && info.name) state.name = String(info.name); })
      .catch(function () {});
  }

  function connect() {
    var ws = new WebSocket(WS_URL);
    ws.binaryType = "arraybuffer";
    state.ws = ws;
    ws.onopen = function () {
      refreshName().then(function () {
        state.open = true;
        state.retry = 250;
        var pending = state.queue;
        state.queue = [];
        pending.forEach(function (b) { ws.send(b); });
        changed(output);
        changed(input);
      });
    };
    ws.onmessage = function (e) {
      if (e.data instanceof ArrayBuffer) input._receive(new Uint8Array(e.data));
    };
    ws.onclose = function () {
      var was = state.open;
      state.open = false;
      state.ws = null;
      if (was) { changed(input); changed(output); }
      setTimeout(connect, state.retry);
      state.retry = Math.min(state.retry * 2, 4000);
    };
  }

  var ready = null;
  function requestMIDIAccess() {
    if (!ready) {
      connect();
      // Resolve once connected (or after 1.5 s, ports then "disconnected"
      // until the emulator answers), so editors see the device at start.
      ready = new Promise(function (resolve) {
        var t0 = Date.now();
        (function wait() {
          if (state.open || Date.now() - t0 > 1500) resolve();
          else setTimeout(wait, 20);
        })();
      });
    }
    return ready.then(function () {
      if (!state.access) state.access = new Access();
      return state.access;
    });
  }

  window.__fm1WebMidi = { input: input, output: output, state: state };
  Object.defineProperty(navigator, "requestMIDIAccess", {
    value: requestMIDIAccess, configurable: true, writable: true
  });
})();
