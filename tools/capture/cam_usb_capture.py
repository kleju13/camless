"""Attach the USB logging agent to every NZXT CAM process and write what it sees.

Usage: cam_usb_capture.py AGENT.js OUT.jsonl SECONDS
Attaches to processes as they appear, so it can be started right before CAM.
"""

import json
import sys
import threading
import time

import frida

agent_path, out_path, seconds = sys.argv[1], sys.argv[2], float(sys.argv[3])
source = open(agent_path, encoding="utf-8").read()
out = open(out_path, "w", encoding="utf-8")
lock = threading.Lock()
device = frida.get_local_device()
sessions = {}
winusb_hooked = set()


frame_count = 0


def on_message(pid):
    def handler(message, data):
        global frame_count
        with lock:
            if message["type"] == "send":
                payload = message["payload"]
                if data:
                    frame_count += 1
                    name = f"{out_path}.frame{frame_count}.bin"
                    with open(name, "wb") as f:
                        f.write(data)
                    payload["saved"] = name
                if payload.get("ev") == "winusb-hooked":
                    winusb_hooked.add(pid)
                out.write(json.dumps({"proc": pid, **payload}) + "\n")
            else:
                out.write(json.dumps({"proc": pid, "error": message.get("description")}) + "\n")
            out.flush()

    return handler


def attach_new():
    for p in device.enumerate_processes():
        if p.name.lower() == "nzxt cam.exe" and p.pid not in sessions:
            try:
                s = device.attach(p.pid)
                script = s.create_script(source)
                script.on("message", on_message(p.pid))
                script.load()
                sessions[p.pid] = s
                print(f"attached {p.pid}", flush=True)
            except Exception as e:  # noqa: BLE001 - report and keep going
                sessions[p.pid] = None
                print(f"attach {p.pid} failed: {e}", flush=True)


end = time.time() + seconds
while time.time() < end:
    attach_new()
    time.sleep(0.2)

for s in sessions.values():
    if s is not None:
        try:
            s.detach()
        except Exception:  # noqa: BLE001
            pass
out.close()
print(f"done; winusb hooked in {sorted(winusb_hooked)}", flush=True)
