#!/usr/bin/env python3
"""Push an application image to a CH32 node over the CAN bus.

Implements the host side of `docs/can-update-protocol.md` on top of
python-can. Bring up the interface first, e.g.:

    sudo ip link set can0 type can bitrate 1000000
    sudo ip link set can0 up

and wire the transceiver to the pins of the build (PA11/PA12 by default,
PB8/PB9 for `can-pb8-pb9`; the nanoCH32V305 has no onboard transceiver).

    python3 tools/can_update.py --channel can0 --node 1 application.bin

The node is found with a broadcast `GET_INFO`, reset into the CAN
bootloader with a targeted `ENTER_UPDATE` (unless it is already sitting in
one) and then flashed page by page. `--node` is the compile-time node ID
the firmware was built with; `--uid` additionally pins the factory unique
ID so a stale or colliding node ID cannot flash the wrong board.

The image must be a raw binary linked for the ACTIVE partition, not an ELF:

    llvm-objcopy -O binary \
      examples/application/target/riscv32imfc-unknown-none-elf/release/application \
      application.bin
"""

import argparse
import random
import struct
import sys
import time
import zlib

try:
    import can
except ImportError:  # pragma: no cover
    sys.exit("python-can is required: pip install python-can")

# Constants mirrored from `embassy_boot_ch32::can`; keep in sync with
# `docs/can-update-protocol.md`.
UID_LEN = 12
UID_PREFIX_LEN = 7
PAGE_BYTES = 256
FRAME_DATA_MAX = 8
INFO_LEN = 18

FUNCTIONAL_NODE = 0
SUB_CONTROL = 0
SUB_DATA = 1

CMD_PING = 0x01
CMD_ENTER_UPDATE = 0x02
CMD_SESSION_OPEN = 0x10
CMD_SESSION_CLOSE = 0x11
CMD_GET_INFO = 0x12
CMD_BEGIN = 0x20
CMD_ABORT = 0x21
CMD_FINISH = 0x30
CMD_QUERY = 0x31

ERR_NAMES = {
    1: "no session",
    2: "busy",
    3: "offset mismatch",
    4: "image too large",
    5: "flash error",
    6: "crc mismatch",
    8: "timeout",
}

STATE_NAMES = {0: "app", 1: "bootloader idle", 2: "bootloader receiving"}


def req_id(node, sub):
    return 0x400 + (node << 2) + sub


def resp_id(node, sub):
    return 0x600 + (node << 2) + sub


def is_response(msg_id):
    return (msg_id >> 9) & 0b11 == 0b11


def node_of(msg_id):
    return (msg_id >> 2) & 0x7F


def sub_of(msg_id):
    return msg_id & 0b11


class NodeError(Exception):
    pass


class Bus:
    """Request/response plumbing for one node on a python-can bus."""

    def __init__(self, bus, node):
        self.bus = bus
        self.node = node

    def request(self, payload):
        self.bus.send(
            can.Message(
                arbitration_id=req_id(self.node, SUB_CONTROL),
                data=bytes(payload),
                is_extended_id=False,
            )
        )

    def recv(self, deadline):
        """Next response frame from this node, or None when `deadline` passes."""
        while True:
            timeout = deadline - time.time()
            if timeout <= 0:
                return None
            msg = self.bus.recv(timeout)
            if msg is None:
                return None
            if is_response(msg.arbitration_id) and node_of(msg.arbitration_id) == self.node:
                return msg

    def header(self, cmd, timeout):
        """Wait for one OK response header for `cmd`."""
        deadline = time.time() + timeout
        while True:
            msg = self.recv(deadline)
            if msg is None:
                raise NodeError(f"timeout waiting for {cmd_name(cmd)} response")
            if sub_of(msg.arbitration_id) != SUB_CONTROL or len(msg.data) != 8:
                continue
            data = bytes(msg.data)
            if data[1] != cmd:
                continue
            status, err, next_offset = data[2], data[3], struct.unpack("<I", data[4:8])[0]
            if status != 0:
                raise NodeError(
                    f"{cmd_name(cmd)} failed: {ERR_NAMES.get(err, f'err {err}')} "
                    f"(next offset {next_offset})"
                )
            return next_offset

    def data_payload(self, nbytes, timeout):
        """Collect payload continuation frames until `nbytes` are in."""
        deadline = time.time() + timeout
        payload = bytearray()
        while len(payload) < nbytes:
            msg = self.recv(deadline)
            if msg is None:
                raise NodeError("timeout waiting for payload frames")
            if sub_of(msg.arbitration_id) != SUB_DATA:
                continue
            payload += bytes(msg.data)
        return bytes(payload[:nbytes])

    def get_info(self, timeout):
        """`GET_INFO`: header whose `next_offset` is the payload length, then data frames."""
        self.request([CMD_GET_INFO])
        next_offset = self.header(CMD_GET_INFO, timeout)
        if next_offset != INFO_LEN:
            raise NodeError(f"GET_INFO header announced {next_offset} payload bytes")
        payload = self.data_payload(INFO_LEN, timeout)
        info = {
            "protocol_version": payload[0],
            "state": payload[1],
            "uid": payload[2:14],
            "chip_id": struct.unpack("<I", payload[14:18])[0],
        }
        if info["protocol_version"] != 1:
            raise NodeError(f"unsupported protocol version {info['protocol_version']}")
        return info

    def cmd(self, payload, timeout=2.0):
        self.request(payload)
        return self.header(payload[0], timeout)


def cmd_name(cmd):
    return {
        CMD_PING: "PING",
        CMD_ENTER_UPDATE: "ENTER_UPDATE",
        CMD_SESSION_OPEN: "SESSION_OPEN",
        CMD_SESSION_CLOSE: "SESSION_CLOSE",
        CMD_GET_INFO: "GET_INFO",
        CMD_BEGIN: "BEGIN",
        CMD_ABORT: "ABORT",
        CMD_FINISH: "FINISH",
        CMD_QUERY: "QUERY",
    }.get(cmd, f"cmd {cmd:#04x}")


def discover(bus, seconds):
    """Functional `GET_INFO`; returns {node_id: info} and flags duplicate IDs."""
    bus.send(
        can.Message(
            arbitration_id=req_id(FUNCTIONAL_NODE, SUB_CONTROL),
            data=bytes([CMD_GET_INFO]),
            is_extended_id=False,
        )
    )
    nodes = {}
    pending = None  # node whose info header was seen, awaiting payload
    deadline = time.time() + seconds
    while time.time() < deadline:
        msg = bus.recv(deadline - time.time())
        if msg is None or not is_response(msg.arbitration_id):
            continue
        node = node_of(msg.arbitration_id)
        if sub_of(msg.arbitration_id) == SUB_CONTROL:
            data = bytes(msg.data)
            pending = node if len(data) == 8 and data[1] == CMD_GET_INFO else None
        elif sub_of(msg.arbitration_id) == SUB_DATA and pending == node:
            payload = bytes(msg.data)
            if len(payload) < INFO_LEN:
                # Only the first frame carries a whole record across the three
                # data frames; collect the rest with a short grace window.
                end = time.time() + 0.2
                while len(payload) < INFO_LEN:
                    nxt = bus.recv(end - time.time())
                    if nxt is None:
                        break
                    if (
                        is_response(nxt.arbitration_id)
                        and node_of(nxt.arbitration_id) == node
                        and sub_of(nxt.arbitration_id) == SUB_DATA
                    ):
                        payload += bytes(nxt.data)
            if len(payload) >= INFO_LEN:
                info = {
                    "protocol_version": payload[0],
                    "state": payload[1],
                    "uid": payload[2:14],
                    "chip_id": struct.unpack("<I", payload[14:18])[0],
                }
                if node in nodes and nodes[node]["uid"] != info["uid"]:
                    raise NodeError(
                        f"node id {node} is claimed by two boards "
                        f"({nodes[node]['uid'].hex()} and {info['uid'].hex()}); "
                        "fix the node ids before flashing"
                    )
                nodes[node] = info
            pending = None
    return nodes


def enter_bootloader(node, uid, timeout):
    """`ENTER_UPDATE` + wait for the node to reappear as an idle bootloader."""
    print("sending ENTER_UPDATE, node is resetting into the bootloader")
    try:
        node.cmd([CMD_ENTER_UPDATE] + list(uid[:UID_PREFIX_LEN]), timeout=2.0)
    except NodeError:
        # The ACK may be lost across the reset; the state poll below is the
        # authoritative answer, so carry on regardless.
        pass

    deadline = time.time() + timeout
    while time.time() < deadline:
        try:
            info = node.get_info(timeout=0.5)
        except NodeError:
            time.sleep(0.2)  # mid-reset, the node simply is not there yet
            continue
        if info["state"] == 1:
            return info
        if info["state"] == 0:
            time.sleep(0.2)  # has not reset yet
            continue
        raise NodeError("node reports a receiving session, refusing to hijack it")
    raise NodeError(f"node did not appear as a bootloader within {timeout} s")


def send_page(node, image, offset):
    """One page of stop-and-wait streaming: 8-byte frames, then the ACK."""
    end = min(offset + PAGE_BYTES, len(image))
    for chunk in range(offset, end, FRAME_DATA_MAX):
        node.bus.send(
            can.Message(
                arbitration_id=req_id(node.node, SUB_DATA),
                data=image[chunk : chunk + FRAME_DATA_MAX],
                is_extended_id=False,
            )
        )


def transfer(node, image, offset):
    """Stream whole pages until only the short tail is left, then FINISH.

    The page is the unit of idempotency (§6.3): a page is either written and
    acknowledged or redone whole, so `offset` is always a page boundary when
    this is called.
    """
    while offset + PAGE_BYTES <= len(image):
        send_page(node, image, offset)
        offset = node.header(CMD_BEGIN, timeout=2.0)
    # Final short page: programmed by FINISH itself, so there is no ACK to
    # wait for; the session ACK for FINISH is the confirmation.
    if offset < len(image):
        send_page(node, image, offset)
    crc = zlib.crc32(image) & 0xFFFFFFFF
    node.cmd([CMD_FINISH, *struct.pack("<II", len(image), crc)], timeout=15.0)


def resync(node):
    """`QUERY` for the node's idea of `next_offset`."""
    deadline = time.time() + 5.0
    last = None
    while time.time() < deadline:
        try:
            node.request([CMD_QUERY])
            return node.header(CMD_QUERY, timeout=1.0)
        except NodeError as exc:
            last = exc
            time.sleep(0.2)
    raise NodeError(f"QUERY got no answer ({last})")


def finish(node, image, offset):
    crc = zlib.crc32(image) & 0xFFFFFFFF
    if offset < len(image):
        for chunk in range(offset, len(image), FRAME_DATA_MAX):
            node.bus.send(
                can.Message(
                    arbitration_id=req_id(node.node, SUB_DATA),
                    data=image[chunk : chunk + FRAME_DATA_MAX],
                    is_extended_id=False,
                )
            )
    node.cmd([CMD_FINISH, *struct.pack("<II", len(image), crc)], timeout=15.0)


def main():
    parser = argparse.ArgumentParser(
        description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter
    )
    parser.add_argument(
        "--interface", default="socketcan", help="python-can interface (default socketcan)"
    )
    parser.add_argument("--channel", default="can0", help="python-can channel (default can0)")
    parser.add_argument(
        "--bitrate", type=int, default=1_000_000, help="CAN bitrate (default 1000000)"
    )
    parser.add_argument(
        "--node", type=int, default=1, help="node ID the firmware was built with (default 1)"
    )
    parser.add_argument(
        "--uid",
        help=f"{UID_LEN * 2} hex digits of the factory unique ID, to refuse a "
        "node whose ID is stale or colliding",
    )
    parser.add_argument(
        "--no-discovery",
        action="store_true",
        help="skip the functional GET_INFO survey of the bus",
    )
    parser.add_argument("image", help="raw firmware image for the ACTIVE partition")
    args = parser.parse_args()

    with open(args.image, "rb") as f:
        image = f.read()
    if image.startswith(b"\x7fELF"):
        sys.exit(f"{args.image} is an ELF; convert it with `objcopy -O binary` first")
    if args.uid is not None:
        try:
            want_uid = bytes.fromhex(args.uid)
        except ValueError:
            sys.exit("--uid must be hex")
        if len(want_uid) != UID_LEN:
            sys.exit(f"--uid must be exactly {UID_LEN} bytes ({UID_LEN * 2} hex digits)")
    else:
        want_uid = None

    with can.Bus(
        channel=args.channel, bitrate=args.bitrate, interface=args.interface
    ) as bus:
        if not args.no_discovery:
            print(f"surveying the bus on {args.channel} ...")
            for node, info in sorted(discover(bus, 1.0).items()):
                print(
                    f"  node {node:<3} {STATE_NAMES.get(info['state'], '?'):<20} "
                    f"uid {info['uid'].hex()} chip {info['chip_id']:08x}"
                )

        node = Bus(bus, args.node)
        info = node.get_info(timeout=2.0)
        print(
            f"node {args.node}: {STATE_NAMES.get(info['state'], '?')}, "
            f"uid {info['uid'].hex()}, chip {info['chip_id']:08x}"
        )
        if want_uid is not None and info["uid"] != want_uid:
            sys.exit(
                f"node {args.node} has uid {info['uid'].hex()}, "
                f"--uid asked for {want_uid.hex()}; refusing to flash"
            )
        if info["state"] == 2:
            sys.exit("node is receiving someone else's image; refusing to interfere")
        if info["state"] == 0:
            info = enter_bootloader(node, info["uid"], timeout=15.0)

        crc = zlib.crc32(image) & 0xFFFFFFFF
        attempts = 0
        while True:
            try:
                # One session at a time; the echoed nonce proves we talk to
                # the node that accepted it (§7).
                nonce = random.getrandbits(32)
                node.request([CMD_SESSION_OPEN, *struct.pack("<I", nonce)])
                node.header(CMD_SESSION_OPEN, timeout=2.0)
                echo = node.data_payload(4, 1.0)
                if struct.unpack("<I", echo)[0] != nonce:
                    raise NodeError("SESSION_OPEN nonce echo mismatch")

                node.cmd([CMD_BEGIN, *struct.pack("<II", len(image), crc)])
                print(f"session open, sending {len(image)} bytes (crc32 {crc:08x})")
                transfer(node, image, 0)
                break
            except NodeError as exc:
                attempts += 1
                if attempts >= 3:
                    raise
                print(f"transfer failed: {exc}")
                try:
                    offset = resync(node)
                except NodeError as resync_exc:
                    raise NodeError(f"node unreachable after failure: {resync_exc}") from exc
                if offset % PAGE_BYTES != 0:
                    # Data frames carry no index, so a half-written page can
                    # only be redone from scratch: drop the session and start
                    # over from BEGIN.
                    print(f"offset {offset} is mid-page, restarting from BEGIN")
                    node.request([CMD_ABORT])
                    try:
                        node.header(CMD_ABORT, timeout=1.0)
                    except NodeError:
                        pass
                else:
                    print(f"resuming at offset {offset:#x}")
                    try:
                        transfer(node, image, offset)
                        break
                    except NodeError as second:
                        raise NodeError(f"resume failed too: {second}") from exc

    print("image verified by the node, it is swapping and booting (a few seconds)")


if __name__ == "__main__":
    try:
        main()
    except NodeError as exc:
        sys.exit(f"error: {exc}")
    except can.CanError as exc:
        sys.exit(f"can error: {exc}")
