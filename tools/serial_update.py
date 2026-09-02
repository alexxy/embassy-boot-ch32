#!/usr/bin/env python3
"""Push an application image to the CH32V305 bootloader over the serial console.

Wire up USART1 (PA9 = TX -> adapter RX, PA10 = RX -> adapter TX, 115200 8N1) and
ideally connect RTS to RESET so the script can reset the board by itself.

    python3 tools/serial_update.py /dev/ttyUSB0 application.bin

The image must be a raw binary linked for the ACTIVE partition, not an ELF:

    llvm-objcopy -O binary \
      examples/application/target/riscv32imfc-unknown-none-elf/release/application-ch32v305 \
      application.bin
"""

import argparse
import sys
import time

import serial

GRACE_SECONDS = 5.0


def expect(port, needle, timeout):
    """Read until `needle` appears in the stream, or give up."""
    deadline = time.time() + timeout
    buf = bytearray()
    while time.time() < deadline:
        buf += port.read(port.in_waiting or 1)
        if needle in buf:
            return True
    print("timeout waiting for", needle, "- got:", bytes(buf[-200:]))
    return False


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("port", nargs="?", default="/dev/ttyUSB0", help="serial port")
    parser.add_argument("image", nargs="?", default="application.bin", help="raw firmware image")
    parser.add_argument("--chunk", type=int, default=256, help="bytes per acknowledgment")
    args = parser.parse_args()

    with open(args.image, "rb") as f:
        image = f.read()

    if image.startswith(b"\x7fELF"):
        sys.exit(f"{args.image} is an ELF; convert it with `objcopy -O binary` first")

    print(f"sending {len(image)} bytes ({len(image):#x}) from {args.image}")

    with serial.Serial(args.port, 115200, timeout=0.2) as port:
        # Reset the board (wire RTS to RESET, or press the button by hand), then
        # hold the bootloader by pressing a key inside the grace window.
        port.setDTR(False)
        port.setRTS(True)
        time.sleep(0.1)
        port.setRTS(False)

        if not expect(port, b"press a key", GRACE_SECONDS):
            sys.exit("bootloader did not start (reset the board and retry)")

        port.write(b"d")
        if not expect(port, b"send 'f ", 10.0):
            sys.exit("no update session")

        port.write(("f %x\n" % len(image)).encode())
        if not expect(port, b"receiving", 10.0):
            sys.exit("header rejected")

        for offset in range(0, len(image), args.chunk):
            port.write(image[offset : offset + args.chunk])
            # Stop and wait: one byte of acknowledgment means "this chunk is in
            # flash, send the next one". The USART has no FIFO and flash
            # programming takes milliseconds, so streaming will overrun it.
            if not port.read(1):
                sys.exit(f"no acknowledgment at offset {offset:#x}")

        if not expect(port, b"image marked updated", 15.0):
            sys.exit("update did not complete")

    print("done, the bootloader is swapping and booting the new image")


if __name__ == "__main__":
    main()
