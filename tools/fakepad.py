#!/usr/bin/env python3
"""Virtual Xbox-style gamepad for testing Polka via uinput.

Usage: fakepad.py STEP...  where STEP is a button name (a b x y start select
lb rb lt rt ls rs up down left right), `hold:x` / `release:x`, or `sleep:SEC`.
"""
import sys, time
from evdev import UInput, ecodes as e, AbsInfo

# Same key set as the kernel xpad driver, so the SDL mapping for 045e:028e
# lines up (button indices follow key-code order). Triggers are axes.
BTN = {
    "a": e.BTN_A, "b": e.BTN_B, "x": e.BTN_X, "y": e.BTN_Y,
    "start": e.BTN_START, "select": e.BTN_SELECT, "lb": e.BTN_TL, "rb": e.BTN_TR,
    "ls": e.BTN_THUMBL, "rs": e.BTN_THUMBR,
}
TRIG = {"lt": e.ABS_Z, "rt": e.ABS_RZ}
HAT = {"up": (e.ABS_HAT0Y, -1), "down": (e.ABS_HAT0Y, 1), "left": (e.ABS_HAT0X, -1), "right": (e.ABS_HAT0X, 1)}
stick = AbsInfo(0, -32768, 32767, 16, 128, 0)
hat = AbsInfo(0, -1, 1, 0, 0, 0)
caps = {
    e.EV_KEY: list(BTN.values()) + [e.BTN_MODE],
    e.EV_ABS: [(e.ABS_X, stick), (e.ABS_Y, stick), (e.ABS_RX, stick), (e.ABS_RY, stick),
               (e.ABS_Z, AbsInfo(0, 0, 255, 0, 0, 0)), (e.ABS_RZ, AbsInfo(0, 0, 255, 0, 0, 0)),
               (e.ABS_HAT0X, hat), (e.ABS_HAT0Y, hat)],
}
ui = UInput(caps, name="Polka Test Pad", vendor=0x045e, product=0x028e, version=0x110, bustype=e.BUS_USB)
time.sleep(1.5)  # let udev/gilrs pick it up

def emit(step, value):
    if step in BTN:
        ui.write(e.EV_KEY, BTN[step], value)
    elif step in TRIG:
        ui.write(e.EV_ABS, TRIG[step], 255 if value else 0)
    else:
        code, v = HAT[step]
        ui.write(e.EV_ABS, code, v if value else 0)
    ui.syn()

for step in sys.argv[1:]:
    if step.startswith("sleep:"):
        time.sleep(float(step[6:]))
    elif step.startswith("hold:"):
        emit(step[5:], 1)
    elif step.startswith("release:"):
        emit(step[8:], 0)
    else:
        emit(step, 1); time.sleep(0.06); emit(step, 0); time.sleep(0.12)
time.sleep(0.3)
ui.close()
