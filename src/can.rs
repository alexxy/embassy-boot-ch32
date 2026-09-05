//! The wire protocol of the CAN-bus firmware update transport, as specified by
//! [`docs/can-update-protocol.md`](https://github.com/alexxy/embassy-boot-ch32/blob/main/docs/can-update-protocol.md)
//! in the repository.
//!
//! This module is a pure, dependency-free codec: it knows the CAN ID layout,
//! the control frame opcodes, the response framing and the image CRC, but
//! nothing about CAN controllers or flash. The bootloader transport, the
//! application runtime listener and the host tool all speak exactly this
//! protocol, and because the codec has no hardware dependencies it is unit
//! tested on the host.
//!
//! All identifiers are 11-bit classic CAN IDs (the bxCAN peripherals of the
//! CH32V3 line have no CAN FD support):
//!
//! ```text
//!  bit:   10 9 | 8      2 | 1 0
//!         dir   | node id  | sub
//! ```
//!
//! `dir` is `0b10` for host-to-node requests and `0b11` for node-to-host
//! responses, `node` is the 7-bit node ID (`0` is the functional, broadcast
//! address) and `sub` distinguishes control frames (`0`) from image data
//! frames (`1`). The whole `0x000..0x3FF` range stays free for application
//! traffic.
//!
//! ```
//! use embassy_boot_ch32::can;
//!
//! // A request to node 1, control sub-channel.
//! let id = can::req_id(1, can::SUB_CONTROL);
//! assert_eq!(id, 0x404);
//! assert!(can::is_request(id));
//! assert_eq!(can::node_of(id), 1);
//!
//! // The node answers on the mirrored response ID.
//! let resp = can::Response::ok(can::cmd::QUERY, 256);
//! assert_eq!(can::resp_id(can::node_of(id), can::SUB_CONTROL), 0x604);
//! let frame = resp.encode();
//! assert_eq!(can::Response::decode(&frame), Some(resp));
//! ```

/// Protocol version reported by `GET_INFO`.
pub const PROTOCOL_VERSION: u8 = 1;

/// The node ID the example firmware listens on.
///
/// [`FUNCTIONAL_NODE`] is 0, so node IDs start at 1. Every node on a shared
/// bus needs its own; see §13 of the protocol spec for the (future) runtime
/// provisioning story.
pub const DEFAULT_NODE_ID: u8 = 1;

/// The functional (broadcast) node address: a frame sent here is meant for
/// every node on the bus.
pub const FUNCTIONAL_NODE: u8 = 0;

/// Length of the factory unique device ID (`signature::unique_id()`).
pub const UID_LEN: usize = 12;
/// Number of UID bytes an `ENTER_UPDATE` request carries: a classic CAN frame
/// has room for the opcode plus 7 bytes, and a 56-bit UID prefix is unique
/// enough to address one node of any fleet.
pub const UID_PREFIX_LEN: usize = 7;

/// Data bytes a classic CAN 2.0 frame can carry.
pub const FRAME_DATA_MAX: usize = 8;
/// Image bytes accepted per acknowledgment: one flash write chunk, i.e. 32
/// data frames per ACK (stop-and-wait, see §6.3 of the protocol spec).
pub const PAGE_BYTES: usize = 256;
/// Mid-session watchdog: a receiving node that has not seen any frame of its
/// session for this long drops the session without touching the state or the
/// active partition (§7).
pub const SESSION_TIMEOUT_MS: u32 = 5_000;

const DIR_REQUEST: u16 = 0b10;
const DIR_RESPONSE: u16 = 0b11;

/// Control sub-channel (`sub = 0`): command and response frames.
pub const SUB_CONTROL: u16 = 0;
/// Data sub-channel (`sub = 1`): raw image bytes, and the multi-frame payload
/// of [`cmd::GET_INFO`] responses.
pub const SUB_DATA: u16 = 1;

/// Request CAN ID for `node` (`FUNCTIONAL_NODE` broadcasts) and `sub`.
pub const fn req_id(node: u8, sub: u16) -> u16 {
    (DIR_REQUEST << 9) | ((node as u16 & 0x7F) << 2) | (sub & 0b11)
}

/// Response CAN ID for `node` and `sub`.
pub const fn resp_id(node: u8, sub: u16) -> u16 {
    (DIR_RESPONSE << 9) | ((node as u16 & 0x7F) << 2) | (sub & 0b11)
}

/// `true` for the `0x400..0x5FF` host-to-node request range.
pub const fn is_request(id: u16) -> bool {
    (id >> 9) == DIR_REQUEST
}

/// `true` for the `0x600..0x7FF` node-to-host response range.
pub const fn is_response(id: u16) -> bool {
    (id >> 9) == DIR_RESPONSE
}

/// The node the frame is addressed to/from; [`FUNCTIONAL_NODE`] means
/// broadcast (requests only).
pub const fn node_of(id: u16) -> u8 {
    ((id >> 2) & 0x7F) as u8
}

/// The sub-channel of the frame, [`SUB_CONTROL`] or [`SUB_DATA`].
pub const fn sub_of(id: u16) -> u16 {
    id & 0b11
}

/// Control frame opcodes (byte 0 of a control frame, §6.1).
pub mod cmd {
    /// Liveness check; answered with a plain OK header.
    pub const PING: u8 = 0x01;
    /// Ask the node whose UID prefix matches to reset into the bootloader.
    /// Handled by the application's runtime listener, not by the bootloader.
    pub const ENTER_UPDATE: u8 = 0x02;
    /// Open an update session (`nonce u32`).
    pub const SESSION_OPEN: u8 = 0x10;
    /// Give up a session.
    pub const SESSION_CLOSE: u8 = 0x11;
    /// Report version, state, UID and chip ID (multi-frame, see [`super::INFO_LEN`]).
    pub const GET_INFO: u8 = 0x12;
    /// Start an image transfer (`len u32`, `crc32 u32`).
    pub const BEGIN: u8 = 0x20;
    /// Abandon the transfer, leave the state untouched.
    pub const ABORT: u8 = 0x21;
    /// End of transfer (`len u32`, `crc32 u32`), verify and mark updated.
    pub const FINISH: u8 = 0x30;
    /// Where is the session at? Returns the next expected offset.
    pub const QUERY: u8 = 0x31;
}

/// `status` field of a response header: anything but OK carries an [`err`]
/// detail code.
pub const STATUS_OK: u8 = 0x00;
/// See [`STATUS_OK`].
pub const STATUS_ERROR: u8 = 0x01;

/// `err` detail codes of a failed response header (§8 of the protocol spec).
pub mod err {
    /// The node wants a `SESSION_OPEN` before anything else.
    pub const NO_SESSION: u8 = 1;
    /// Another session is already open on this node.
    pub const BUSY: u8 = 2;
    /// Unexpected image offset; `next_offset` says where the node actually is.
    pub const OFFSET: u8 = 3;
    /// Image larger than the DFU partition.
    pub const TOO_LARGE: u8 = 4;
    /// Flash write failed.
    pub const FLASH: u8 = 5;
    /// Final read-back CRC mismatch.
    pub const CRC: u8 = 6;
    /// The session watchdog fired.
    pub const TIMEOUT: u8 = 8;
}

/// Node state reported in a [`cmd::GET_INFO`] payload (§6.1).
pub mod state {
    /// The application is running (the runtime listener answers).
    pub const APP: u8 = 0;
    /// Bootloader, waiting for a session.
    pub const BOOTLOADER_IDLE: u8 = 1;
    /// Bootloader, a session is receiving image data.
    pub const BOOTLOADER_RECEIVING: u8 = 2;
}

/// One decoded control frame.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Request<'a> {
    /// [`cmd::PING`]
    Ping,
    /// [`cmd::ENTER_UPDATE`]: the first [`UID_PREFIX_LEN`] bytes of the target
    /// node's [`UID`][UID_LEN].
    EnterUpdate {
        /// The UID prefix from the frame payload.
        uid_prefix: &'a [u8],
    },
    /// [`cmd::SESSION_OPEN`]
    SessionOpen {
        /// Host-chosen session token, echoed back in the data frame that
        /// follows the OK header.
        nonce: u32,
    },
    /// [`cmd::SESSION_CLOSE`]
    SessionClose,
    /// [`cmd::GET_INFO`]
    GetInfo,
    /// [`cmd::BEGIN`]
    Begin {
        /// Image length in bytes.
        len: u32,
        /// CRC32 ([`crc32`]) over exactly those bytes.
        crc32: u32,
    },
    /// [`cmd::ABORT`]
    Abort,
    /// [`cmd::FINISH`]
    Finish {
        /// Image length in bytes, must match `BEGIN`.
        len: u32,
        /// CRC32 over exactly those bytes, must match `BEGIN`.
        crc32: u32,
    },
    /// [`cmd::QUERY`]
    Query,
    /// An opcode this protocol version does not define.
    Unknown {
        /// The opcode byte.
        cmd: u8,
    },
}

/// Decodes a control frame payload (a frame on [`SUB_CONTROL`]).
///
/// Returns `None` for an empty payload; opcodes with the wrong payload length
/// decode as [`Request::Unknown`] so the receiver can answer with an error
/// instead of silently ignoring a host that speaks a different version.
pub fn parse_control(data: &[u8]) -> Option<Request<'_>> {
    let (&opcode, rest) = data.split_first()?;

    let le4 = |b: &[u8]| u32::from_le_bytes([b[0], b[1], b[2], b[3]]);

    Some(match opcode {
        cmd::PING => Request::Ping,
        cmd::ENTER_UPDATE if rest.len() == UID_PREFIX_LEN => {
            Request::EnterUpdate { uid_prefix: rest }
        }
        cmd::SESSION_OPEN if rest.len() == 4 => Request::SessionOpen { nonce: le4(rest) },
        cmd::SESSION_CLOSE if rest.is_empty() => Request::SessionClose,
        cmd::GET_INFO if rest.is_empty() => Request::GetInfo,
        cmd::BEGIN if rest.len() == 8 => Request::Begin {
            len: le4(rest),
            crc32: le4(&rest[4..]),
        },
        cmd::ABORT if rest.is_empty() => Request::Abort,
        cmd::FINISH if rest.len() == 8 => Request::Finish {
            len: le4(rest),
            crc32: le4(&rest[4..]),
        },
        cmd::QUERY if rest.is_empty() => Request::Query,
        cmd => Request::Unknown { cmd },
    })
}

/// The 8-byte response header of §6.2, and the OK/error ACK of every command.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Response {
    /// Command being answered (byte 1, the echo; byte 0 is `0x80 | cmd`).
    pub cmd: u8,
    /// [`STATUS_OK`] or [`STATUS_ERROR`].
    pub status: u8,
    /// Detail code from [`err`], `0` when [`STATUS_OK`].
    pub err: u8,
    /// Next expected image offset; also reused as the payload length of a
    /// multi-frame [`cmd::GET_INFO`] response.
    pub next_offset: u32,
}

impl Response {
    /// Frame size of a response header.
    pub const LEN: usize = 8;

    /// A successful response for `cmd`.
    pub const fn ok(cmd: u8, next_offset: u32) -> Self {
        Self {
            cmd,
            status: STATUS_OK,
            err: 0,
            next_offset,
        }
    }

    /// A failed response for `cmd` with an [`err`] detail code.
    pub const fn failure(cmd: u8, err: u8, next_offset: u32) -> Self {
        Self {
            cmd,
            status: STATUS_ERROR,
            err,
            next_offset,
        }
    }

    /// Serialises the header for transmission on [`SUB_CONTROL`].
    pub fn encode(&self) -> [u8; Self::LEN] {
        let offset = self.next_offset.to_le_bytes();
        [
            0x80 | self.cmd,
            self.cmd,
            self.status,
            self.err,
            offset[0],
            offset[1],
            offset[2],
            offset[3],
        ]
    }

    /// Parses a received response header.
    pub fn decode(data: &[u8]) -> Option<Self> {
        let bytes: &[u8; Self::LEN] = data.try_into().ok()?;
        if bytes[0] & 0x80 == 0 {
            return None;
        }
        Some(Self {
            cmd: bytes[1],
            status: bytes[2],
            err: bytes[3],
            next_offset: u32::from_le_bytes([bytes[4], bytes[5], bytes[6], bytes[7]]),
        })
    }
}

/// Length of the [`cmd::GET_INFO`] payload: `protocol_version u8`,
/// `state u8`, `uid[12]`, `chip_id u32`. It does not fit in one classic CAN
/// frame, so it follows the response header as
/// `(INFO_LEN + FRAME_DATA_MAX - 1) / FRAME_DATA_MAX` data frames
/// ([`info_frame_count`]).
pub const INFO_LEN: usize = 18;

/// Number of data frames a [`cmd::GET_INFO`] payload is sent in.
pub const fn info_frame_count() -> usize {
    INFO_LEN.div_ceil(FRAME_DATA_MAX)
}

/// The decoded [`cmd::GET_INFO`] payload.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Info {
    /// [`PROTOCOL_VERSION`] the node speaks.
    pub protocol_version: u8,
    /// One of [`state`].
    pub state: u8,
    /// Factory unique device ID.
    pub uid: [u8; UID_LEN],
    /// Chip ID from `signature::chip_id()`.
    pub chip_id: u32,
}

impl Info {
    /// Serialises the payload for the data frames that follow the header.
    pub fn encode(&self) -> [u8; INFO_LEN] {
        let mut out = [0u8; INFO_LEN];
        out[0] = self.protocol_version;
        out[1] = self.state;
        out[2..2 + UID_LEN].copy_from_slice(&self.uid);
        out[2 + UID_LEN..].copy_from_slice(&self.chip_id.to_le_bytes());
        out
    }

    /// Parses a reassembled [`cmd::GET_INFO`] payload.
    pub fn decode(data: &[u8]) -> Option<Self> {
        let data: &[u8; INFO_LEN] = data.try_into().ok()?;
        let mut uid = [0u8; UID_LEN];
        uid.copy_from_slice(&data[2..2 + UID_LEN]);
        Some(Self {
            protocol_version: data[0],
            state: data[1],
            uid,
            chip_id: u32::from_le_bytes(data[2 + UID_LEN..].try_into().unwrap()),
        })
    }
}

/// Incremental CRC32 (IEEE 802.3, reflected, init `0xFFFF_FFFF`, final
/// inversion) - the same function as Python's `zlib.crc32`. Used for the image
/// integrity check of `BEGIN`/`FINISH`; the bootloader feeds it one flash read
/// chunk at a time, so it is split into a running state.
#[derive(Debug, Clone, Copy)]
pub struct Crc32(u32);

impl Crc32 {
    /// The CRC of nothing so far.
    pub const fn new() -> Self {
        Self(0xFFFF_FFFF)
    }

    /// Absorbs `data` into the running CRC.
    pub fn update(&mut self, mut data: &[u8]) {
        while let Some((&byte, rest)) = data.split_first() {
            data = rest;
            self.0 ^= byte as u32;
            for _ in 0..8 {
                // Conditional polynomial xor without a branch.
                let mask = (self.0 & 1).wrapping_neg();
                self.0 = (self.0 >> 1) ^ (0xEDB8_8320 & mask);
            }
        }
    }

    /// The CRC of everything fed in so far.
    pub const fn finalize(self) -> u32 {
        !self.0
    }
}

impl Default for Crc32 {
    fn default() -> Self {
        Self::new()
    }
}

/// One-shot CRC32 over a complete buffer; see [`Crc32`].
pub fn crc32(data: &[u8]) -> u32 {
    let mut crc = Crc32::new();
    crc.update(data);
    crc.finalize()
}

/// Raw register words `(id_value, id_mask)` for a 32-bit *mask mode* bxCAN
/// filter entry matching `code` under `mask`.
///
/// The register layout is `STID[31:21]:EXID[20:3]:IDE(2):RTR(1)` (the
/// `STID:EXID:IDE:RTR:0` encoding of `ch32_hal::can::CanFilter`). An 11-bit
/// standard ID lives in bits 31:21, and bits 1 and 2 of the mask are set so
/// that only standard (non-extended), non-remote frames match: the protocol
/// never uses either.
pub const fn filter_words(code: u16, mask: u16) -> (u32, u32) {
    let shift = 21;
    (
        (code as u32 & 0x7FF) << shift,
        ((mask as u32 & 0x7FF) << shift) | 0b110,
    )
}

/// The two filter entries a node needs (§4.3): its own request IDs (any
/// sub-channel) and the functional broadcast ID.
///
/// Returns `((code, mask), (code, mask))`.
pub const fn node_filters(node: u8) -> ((u16, u16), (u16, u16)) {
    (
        (req_id(node, SUB_CONTROL), 0x7FC),
        (req_id(FUNCTIONAL_NODE, SUB_CONTROL), 0x7FF),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn crc32_reference_vectors() {
        // The standard check value of the reflected CRC-32/ISO-HDLC.
        assert_eq!(crc32(b"123456789"), 0xCBF4_3926);
        assert_eq!(crc32(b""), 0x0000_0000);
        // The all-ones check value: CRC32 of four 0xFF bytes is 0xFFFF_FFFF.
        assert_eq!(crc32(&[0xFFu8; 4]), 0xFFFF_FFFF);

        // The incremental form agrees with the one-shot form when fed in
        // awkward chunks (this is how the bootloader verifies the image).
        let image = b"the quick brown fox jumps over the lazy dog";
        let mut crc = Crc32::new();
        for chunk in image.chunks(7) {
            crc.update(chunk);
        }
        assert_eq!(crc.finalize(), crc32(image));
    }

    #[test]
    fn id_layout() {
        assert_eq!(req_id(FUNCTIONAL_NODE, SUB_CONTROL), 0x400);
        assert_eq!(req_id(1, SUB_CONTROL), 0x404);
        assert_eq!(req_id(1, SUB_DATA), 0x405);
        assert_eq!(req_id(0x7F, SUB_DATA), 0x5FD);
        assert_eq!(resp_id(1, SUB_CONTROL), 0x604);
        assert_eq!(resp_id(0x7F, SUB_DATA), 0x7FD);

        for id in [0x400u16, 0x404, 0x405, 0x5FD] {
            assert!(is_request(id), "{id:#x} is not a request");
            assert!(!is_response(id));
        }
        for id in [0x600u16, 0x604, 0x7FD] {
            assert!(is_response(id), "{id:#x} is not a response");
            assert!(!is_request(id));
        }

        // Node/sub survive a round trip through the ID.
        for node in [FUNCTIONAL_NODE, 1, 42, 0x7F] {
            for sub in [SUB_CONTROL, SUB_DATA] {
                let id = req_id(node, sub);
                assert_eq!(node_of(id), node);
                assert_eq!(sub_of(id), sub);
            }
        }
    }

    #[test]
    fn response_round_trip() {
        let ok = Response::ok(cmd::QUERY, 256);
        let bytes = ok.encode();
        assert_eq!(bytes[0], 0x80 | cmd::QUERY);
        assert_eq!(Response::decode(&bytes), Some(ok));

        let bad = Response::failure(cmd::BEGIN, err::TOO_LARGE, 0);
        assert_eq!(Response::decode(&bad.encode()), Some(bad));

        // A control frame request is not a response...
        assert_eq!(Response::decode(&[cmd::PING, 0, 0, 0, 0, 0, 0, 0]), None);
        // ...and neither is a short frame.
        assert_eq!(Response::decode(&[0x81, 0x01]), None);
    }

    #[test]
    fn control_frames() {
        assert_eq!(parse_control(&[cmd::PING]), Some(Request::Ping));
        assert_eq!(
            parse_control(&[cmd::SESSION_OPEN, 0x78, 0x56, 0x34, 0x12]),
            Some(Request::SessionOpen { nonce: 0x1234_5678 })
        );
        assert_eq!(parse_control(&[cmd::GET_INFO]), Some(Request::GetInfo));
        assert_eq!(
            parse_control(&[cmd::SESSION_CLOSE]),
            Some(Request::SessionClose)
        );
        assert_eq!(parse_control(&[cmd::ABORT]), Some(Request::Abort));
        assert_eq!(parse_control(&[cmd::QUERY]), Some(Request::Query));

        assert_eq!(
            parse_control(&[cmd::BEGIN, 0x00, 0xC0, 0x00, 0x00, 0x11, 0x22, 0x33, 0x44]),
            Some(Request::Begin {
                len: 0xC000,
                crc32: 0x4433_2211
            })
        );

        let mut enter = [0u8; 1 + UID_PREFIX_LEN];
        enter[0] = cmd::ENTER_UPDATE;
        enter[1..].copy_from_slice(&[1, 2, 3, 4, 5, 6, 7]);
        assert_eq!(
            parse_control(&enter),
            Some(Request::EnterUpdate {
                uid_prefix: &[1, 2, 3, 4, 5, 6, 7]
            })
        );

        // Wrong payload lengths are reported, not silently dropped...
        assert_eq!(
            parse_control(&[cmd::BEGIN, 0, 0]),
            Some(Request::Unknown { cmd: cmd::BEGIN })
        );
        // ...empty payloads and unknown opcodes as well.
        assert_eq!(parse_control(&[]), None);
        assert_eq!(parse_control(&[0x77]), Some(Request::Unknown { cmd: 0x77 }));
    }

    #[test]
    fn info_payload() {
        let info = Info {
            protocol_version: PROTOCOL_VERSION,
            state: state::BOOTLOADER_RECEIVING,
            uid: [0xA0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11],
            chip_id: 0x3054_0001,
        };
        let bytes = info.encode();
        assert_eq!(bytes.len(), INFO_LEN);
        assert_eq!(Info::decode(&bytes), Some(info));
        assert_eq!(Info::decode(&bytes[..17]), None);

        // The payload fits in whole data frames, last one short.
        assert_eq!(info_frame_count(), 3);
        assert_eq!(2 * FRAME_DATA_MAX + 2, INFO_LEN);
    }

    #[test]
    fn filter_encoding() {
        let ((own, own_mask), (functional, functional_mask)) = node_filters(5);
        assert_eq!(own, req_id(5, SUB_CONTROL));
        // Own mask ignores the sub-channel, the functional one matches node 0
        // requests only.
        assert_eq!(own_mask, 0x7FC);
        assert_eq!(functional, 0x400);
        assert_eq!(functional_mask, 0x7FF);

        let (value, mask) = filter_words(own, own_mask);
        assert_eq!(value, (0x414u32) << 21);
        // Bits 1 and 2 (RTR/IDE) must match as 0; the ID bits match under the
        // mask; bit 0 is don't-care.
        assert_eq!(mask & 0b110, 0b110);
        assert_eq!(mask & (0x7FF << 21), (0x7FCu32) << 21);

        // Incoming frames either match the masked value or not.
        let matches = |rx: u16| filter_words(rx, 0x7FF).0 & mask == value;
        assert!(matches(req_id(5, SUB_CONTROL)));
        assert!(matches(req_id(5, SUB_DATA)));
        assert!(!matches(req_id(6, SUB_CONTROL)));
        assert!(!matches(req_id(4, SUB_DATA)));
        assert!(!matches(req_id(FUNCTIONAL_NODE, SUB_CONTROL)));
    }
}
