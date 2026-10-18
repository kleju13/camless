// Logs what NZXT CAM sends to and receives from NZXT devices: HID reports
// (NtWriteFile / NtReadFile, which Rust's std uses) and WinUSB pipe writes.
// Observation only; the calls themselves are not changed.
'use strict';

const devices = new Map(); // handle string -> short device name

function hex(ptr, len, max) {
  const n = Math.min(len, max);
  if (n <= 0) return '';
  const bytes = new Uint8Array(ptr.readByteArray(n));
  return Array.from(bytes, (b) => b.toString(16).padStart(2, '0')).join(' ');
}

function shortName(path) {
  const m = /pid_([0-9a-f]{4})(&mi_0\d)?/i.exec(path);
  const kind = /^\\\\\?\\hid/i.test(path) ? 'hid' : 'usb';
  return m ? `${kind}:${m[1].toLowerCase()}${m[2] ? m[2].toLowerCase() : ''}` : path;
}

const k32 = Process.getModuleByName('kernelbase.dll');
const ntdll = Process.getModuleByName('ntdll.dll');

Interceptor.attach(k32.getExportByName('CreateFileW'), {
  onEnter(args) {
    this.path = args[0].isNull() ? '' : args[0].readUtf16String();
  },
  onLeave(ret) {
    if (this.path && /vid_1e71/i.test(this.path)) {
      const name = shortName(this.path);
      devices.set(ret.toString(), name);
      send({ t: Date.now(), ev: 'open', handle: ret.toString(), dev: name });
    }
  },
});

Interceptor.attach(k32.getExportByName('CloseHandle'), {
  onEnter(args) {
    devices.delete(args[0].toString());
  },
});

Interceptor.attach(ntdll.getExportByName('NtWriteFile'), {
  onEnter(args) {
    const dev = devices.get(args[0].toString());
    if (!dev) return;
    const len = args[6].toUInt32();
    send({ t: Date.now(), ev: 'write', dev, len, data: hex(args[5], len, 80) });
  },
});

Interceptor.attach(ntdll.getExportByName('NtReadFile'), {
  onEnter(args) {
    this.dev = devices.get(args[0].toString());
    if (!this.dev) return;
    this.buf = args[5];
    this.len = args[6].toUInt32();
  },
  onLeave(status) {
    if (!this.dev || status.toInt32() !== 0) return;
    const head = hex(this.buf, this.len, 2);
    // Periodic status reports are noise here.
    if (head === '75 02' || head === '67 02' || head === '67 04') return;
    send({ t: Date.now(), ev: 'read', dev: this.dev, data: hex(this.buf, this.len, 48) });
  },
});

let framesDumped = 0;
let winusbHooked = false;

function hookWinUsb(m) {
  if (winusbHooked) return;
  winusbHooked = true;
  Interceptor.attach(m.getExportByName('WinUsb_WritePipe'), {
    onEnter(args) {
      const len = args[3].toUInt32();
      const msg = { t: Date.now(), ev: 'bulk', pipe: args[1].toUInt32() & 0xff, len, data: hex(args[2], len, 32) };
      // Keep a few whole frames to check an encoder against.
      if (len > 1000 && framesDumped < 4) {
        framesDumped += 1;
        send(msg, args[2].readByteArray(len));
      } else {
        send(msg);
      }
    },
  });
  send({ t: Date.now(), ev: 'winusb-hooked' });
}

const winusb = Process.findModuleByName('winusb.dll');
if (winusb) {
  hookWinUsb(winusb);
} else {
  Interceptor.attach(k32.getExportByName('LoadLibraryExW'), {
    onLeave() {
      if (winusbHooked) return;
      const m = Process.findModuleByName('winusb.dll');
      if (m) hookWinUsb(m);
    },
  });
}
send({ t: Date.now(), ev: 'ready', winusb: !!winusb });
