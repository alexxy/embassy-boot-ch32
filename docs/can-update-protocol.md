# CAN-Bus Firmware Update Protocol (draft)

> **Status: draft for discussion.** This document describes a proposed protocol for
> updating firmware over a CAN bus with `embassy-boot-ch32`. Nothing here is
> implemented yet; the design is intentionally close to the existing USART/USB-DFU
> flows so that the flash-side machinery (partition maps, `mark_updated()`,
> swap/rollback) is reused unchanged.

## 1. Goals

- Update **one specific node** on a multi-drop CAN bus with many nodes attached.
- Keep the bootloader small: the whole update logic must fit next to the existing
  serial bootloader (≈16 KiB maps) on CH32V3 parts.
- Recover cleanly from any interruption: power loss, bus loss, host crash. The node
  must always boot a valid image afterwards.
- Use only classic CAN 2.0A (11-bit IDs, up to 8 data bytes): the bxCAN
  peripherals on CH32V3 parts (e.g. CAN1/CAN2 on CH32V305RBT6) have no CAN FD
  support.
- Work with the blocking CAN driver in `ch32-hal` (`ch32_hal::can`,
  `embedded_can` traits); no async runtime requirement in the bootloader.

## 2. Non-goals (v1)

- Multi-node parallel (synchronized group) flashing.
- CAN FD, J1939, J1939-73, UDS/ISO-TP (see §12 for rationale).
- Encrypted firmware images (a stub is reserved, see §10).
- Transport of application-level payloads; this is strictly a firmware-update
  protocol.

## 3. Physical and link layer

- Classic CAN 2.0A, nominal bitrate configurable; **1 Mbit/s** is the default and
  the reference timing.
- Nodes use a standard `ch32-hal` CAN peripheral with one RX filter (see §4.3).
- Host side: any socketCAN / USB-CAN adapter (PCAN, CANable, etc.). A reference
  host tool is out of scope for this document.

## 4. CAN ID allocation

All IDs are 11-bit. The layout leaves the whole `0x000–0x3FF` range free for the
user's application traffic.

### 4.1 Layout

```
 bit:   10 9 | 8      2 | 1 0
       dir   | node id  | sub
```

| Field  | Width | Meaning |
|--------|-------|---------|
| `dir`  | 2     | `0b10` = host → node request, `0b11` = node → host response |
| `node` | 7     | node ID, `0` = functional (broadcast) addressing |
| `sub`  | 2     | `0` = control, `1` = data, `2`/`3` reserved |

### 4.2 Ranges

| Direction | Formula | Range |
|-----------|---------|-------|
| request   | `base + 0x400 + (node << 2) + sub` | `0x400–0x5FF` |
| response  | `base + 0x600 + (node << 2) + sub` | `0x600–0x7FF` |

`base` is a compile-time constant (default `0`). Responses are unicast to the
responding node's ID; a node never sends on `dir = 0b10`.

### 4.3 Filtering

A node needs exactly **one** filter bank (mask mode):

```
code = 0x400 | (node_id << 2)
mask = 0x7FC   // match dir + node, ignore sub
```

plus a second filter entry for the functional request ID `0x400`. On bxCAN two
16-bit filter entries in one bank are enough. Everything else is dropped in
hardware, including other nodes' sessions.

## 5. Node identity and discovery

Each node has two identifiers:

1. **NodeID (7-bit)** — used for CAN filtering. Either a compile-time constant or
   provisioned/stored configuration (open question, §13).
2. **Unique device ID** — the factory-programmed 96-bit UID, read via
   `ch32_hal::signature::unique_id()` (`[u8; 12]`). Used to *verify* that the node
   answering a targeted request is actually the node the host intended to flash
   (protects against NodeID collisions and stale configuration).

Discovery: the host sends functional `GET_INFO`. Every node answers with a short
delay randomized from its UID (collision spread). The host builds a map
`NodeID → UID`. If two nodes with the same NodeID report different UIDs, the host
flags the bus as misconfigured and refuses to flash until resolved (e.g. via
`SET_NODEID`, see §13).

## 6. Frame formats

### 6.1 Control frames (sub = 0)

Byte 0 is the command opcode. All multi-byte fields are little-endian.

| Opcode | Name | Request payload | Accepted mode |
|--------|------|-----------------|---------------|
| `0x01` | `PING` | — | runtime listener / BL |
| `0x02` | `ENTER_UPDATE` | `uid[0..12]` | runtime listener only |
| `0x10` | `SESSION_OPEN` | `nonce u32` | BL |
| `0x11` | `SESSION_CLOSE` | — | BL |
| `0x12` | `GET_INFO` | — | runtime listener / BL |
| `0x20` | `BEGIN` | `len u32`, `crc32 u32` | BL |
| `0x21` | `ABORT` | — | BL |
| `0x30` | `FINISH` | `len u32`, `crc32 u32` | BL |
| `0x31` | `QUERY` | — | BL |

`ENTER_UPDATE` carries the full 12-byte UID (3 frames, or a 7-byte truncated UID
prefix — see §13). Only the node whose UID matches sets the "enter bootloader"
magic (the same mechanism the USART/USB transports use) and resets; all other
nodes ignore the frame entirely. On functional addressing (NodeID 0) *every* node
would enter the bootloader — reserved, not used in v1.

`GET_INFO` response: `protocol_version u8`, `state u8` (`0`=app, `1`=BL-idle,
`2`=BL-receiving), `uid[0..12]`, `chip_id u32` (from `signature::chip_id()`).

### 6.2 Responses (dir = `0b11`, sub = 0)

Common header, then command-specific bytes:

| Byte | Field |
|------|-------|
| 0 | `0x80 \| cmd` (response opcode) |
| 1 | `cmd` (echo) |
| 2 | `status` (`0` = OK, else error) |
| 3 | `err` (error detail code) |
| 4–7 | `next_offset u32` (next expected image offset) |

### 6.3 Data frames (sub = 1)

Up to 8 raw image bytes, always sent at the exact `next_offset` the node expects
(frames are implicitly addressed by offset; no sequence numbers in the frame —
the stop-and-wait ACK *is* the sequence check). A frame arriving at an unexpected
offset is NACK'ed with the expected offset, making retransmission idempotent.

## 7. Session lifecycle

```
Host                                 Node (bootloader)
 |  SESSION_OPEN (nonce)               |
 |------------------------------------>|  one session at a time; reject if busy
 |  <ACK status=OK>                    |
 |  BEGIN {len, crc32}                 |
 |------------------------------------>|  validate len <= DFU size; erase DFU
 |  <ACK next_offset=0>                |  (~ms for 48 KiB on CH32V3)
 |                                     |
 |  data x 32 frames (256-byte page)   |
 |------------------------------------>|  program one flash page
 |  <ACK next_offset += 256>           |  one ACK per page (stop-and-wait)
 |              ...                    |
 |  FINISH {len, crc32}                |
 |------------------------------------>|  read-back CRC32 over DFU
 |  <ACK status=OK>                    |  write updated magic -> reset
 |                                     |  bootloader swaps DFU -> active,
 |  (node silent; reappears as app)    |  old image kept for rollback
```

- **Watchdog:** if no valid frame is seen for ~5 s mid-session, the node drops the
  session without touching the state/active partitions. The old image still boots.
- **QUERY** returns `next_offset` at any time, letting a host resync after its own
  state was lost (e.g. tool restart).
- Only the host that holds the session (matching nonce echoed in ACKs) may drive
  it; `SESSION_CLOSE` or the watchdog frees it.

## 8. Error handling

| `err` | Meaning | Host action |
|-------|---------|-------------|
| `1` | no session (`SESSION_OPEN` first) | open session |
| `2` | busy (another session open) | retry later |
| `3` | offset mismatch (see `next_offset`) | resync from `next_offset` |
| `4` | image too large for DFU slot | abort |
| `5` | flash write error | abort |
| `6` | final CRC mismatch | reflash or abort |
| `8` | timeout / session dropped | reopen session |

Flash errors are deliberately fatal to the session: the active image is untouched
until the read-back CRC passes, so an abort always leaves a bootable device.

## 9. Throughput

Per 256-byte page: 32 data frames + 1 ACK. At 1 Mbit/s (~130 µs/frame worst case)
a page takes ≈ 4.5 ms of bus time plus flash program time (CH32V3 page program is
on the order of tens of µs). Estimate:

- ≈ 55 KB/s effective;
- a 48 KiB application image flashes in roughly **1 s** of bus time.

This is comfortably fast for CH32V3 flash sizes; no sliding-window optimization is
planned for v1.

## 10. Integrity and security

- Integrity: per-frame CAN CRC + stop-and-wait offset checks + final full-image
  CRC32 (already required by `BEGIN`/`FINISH`).
- Identity: the UID check in `ENTER_UPDATE` prevents flashing an unintended node
  that happens to share a NodeID.
- Security: **none in v1** (trusted bus assumption). The response `status/err`
  space and the `SESSION_OPEN` nonce leave room for a UDS-like seed/key
  authentication step later without changing framing. Signed images would replace
  the CRC32 in `BEGIN`/`FINISH` with a hash + signature check; the layout allows
  it.

## 11. Mapping to embassy-boot-ch32

- Bootloader: a new `transport-can` feature implementing the same internal
  transport shape as the current serial transport, reusing
  `BlockingFirmwareUpdater` from `embassy-boot` unchanged. Flash geometry and
  partition maps stay as they are.
- Runtime listener (for `PING` / `GET_INFO` / `ENTER_UPDATE`): analogous to the
  existing `examples/application/src/usb_runtime.rs`, using
  `ch32_hal::can::Can::new_blocking` (no executor needed).
- Memory fit: bxCAN registers + minimal driver code are expected to fit the 16 KiB
  serial bootloader maps; if not, a `-can` map variant with a 32 KiB bootloader
  (like `-usb`) is the fallback. **To be verified during implementation.**
- Parts note: on CH32V305 CAN2 is the secondary bxCAN unit (master/slave pairing
  with CAN1, shared message RAM); v1 uses a single CAN controller per node, CAN1
  by default.

## 12. Why not UDS (ISO 14229) / ISO-TP / CANopen?

UDS with ISO-TP is the industrial standard for exactly this, and the protocol here
is deliberately UDS-*flavoured* (service-style opcodes, the
download-transfer-exit lifecycle, stop-and-wait flow control). However:

- an ISO-TP layer plus a UDS security-access state machine and NRC handling is a
  meaningful chunk of code — likely too much for the 16 KiB bootloader budget on
  these parts;
- we do not need DIDs/RIDs, multiple diagnostic sessions, or services beyond
  flash write;
- a small fixed protocol is far easier to write a throwaway host script against.

If demand appears, a real UDS transport can be layered later; the framing choices
here were made so that migration stays possible.

## 13. Open questions

1. **NodeID provisioning:** compile-time constant only (simple, example-friendly)
   vs. stored in flash with a `SET_NODEID` command (flexible, needs a settings
   page and collision policy). Proposal: constant in v1, `SET_NODEID` later.
2. **`ENTER_UPDATE` UID width:** the full 12-byte UID needs 2 frames (12 bytes
   won't fit one classic CAN frame alongside the opcode). Options: two-frame
   control message, a 7-byte UID prefix (collision probability negligible in
   practice), or a 4-byte UID hash. Proposal: 7-byte prefix.
3. **Base ID configurability** beyond the compile-time constant (e.g. 29-bit
   extended addressing for shared buses) — probably YAGNI, but easy to add.
4. **ACK-per-page granularity** (256 B): acceptable latency vs. ACK-per-frame for
   simpler recovery? Proposal: page granularity; 32 frames per ACK is still a
   small retry window and the offset check makes recovery trivial.
5. **Security stub scope** in v1: reserve opcodes only, or implement a fixed-key
   seed/key now? Proposal: reserve only.
6. **`GET_INFO` response spacing:** a randomized delay is crude on a shared
   1 Mbit bus; alternative is the host iterating NodeIDs it knows. Needs
   measurement with a realistic node count.
7. Whether a **group/broadcast** update (identical image to all nodes, no ACKs,
   host verifies afterwards via `GET_INFO`) is worth adding in v1.
