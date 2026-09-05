//! CAN-bus update transport.
//!
//! Compiled only for a `transport-can` build of the bootloader. It implements
//! the protocol of `docs/can-update-protocol.md` on top of the pure codec in
//! [`embassy_boot_ch32::can`], using the non-blocking bxCAN driver of
//! `ch32-hal`: no executor and no interrupts, frames are polled once per
//! millisecond.
//!
//! The flash side is the serial transport's: the image lands in the DFU
//! partition through [`BlockingFirmwareUpdater`], and a successful session ends
//! with `mark_updated()` and a system reset, after which the ordinary boot path
//! swaps DFU into active and keeps the old image for rollback.
//!
//! A session is *targeted*: only frames on this node's own request IDs are
//! acted upon. Functional (broadcast) frames are answered with `GET_INFO` only,
//! spread apart by a per-node delay derived from the unique ID so that a
//! discovery broadcast does not turn into one long collision.

use core::fmt::Write;

use ch32_hal::can::{
    Bit32Mode, Can, CanFifo, CanFilter, CanFrame, CanMode, Config as CanConfig, MaskMode,
};
use ch32_hal::delay::Delay;
use ch32_hal::usart;
use embassy_boot_ch32::can::{self, Request, Response};
use embassy_boot_ch32::embedded_storage::nor_flash::{NorFlash, ReadNorFlash};
use embassy_boot_ch32::{
    AlignedBuffer, BlockingFirmwareUpdater, FirmwareUpdaterConfig, dfu_size, dfu_start,
    system_reset,
};
use embedded_can::nb::Can as _;
use embedded_can::{Id, StandardId};

use crate::{CHIP_NAME, Console, FlashMutex, STATE_BUFFER_SIZE};

/// CAN node ID of this board: the 7-bit field of the protocol's 11-bit IDs.
///
/// Every node on the bus needs a distinct ID, and the application build must
/// use the same one (`src/can_runtime.rs`). The host addresses a node through
/// this constant and verifies the target separately against the factory unique
/// ID reported by `GET_INFO`; provisioning other than a compile-time constant
/// is out of scope for the example (protocol spec §13.1).
const NODE_ID: u8 = can::DEFAULT_NODE_ID;

/// CAN bitrate of the update bus. 1 Mbit/s is the protocol default, and both
/// the bootloader and the application runtime listener must use the same value.
const CAN_BITRATE: u32 = 1_000_000;

/// Poll interval of the receive loop, in milliseconds.
///
/// At 1 Mbit/s a frame takes ~130 µs, so between two ticks the FIFO cannot
/// overflow under the page-granular stop-and-wait flow, and the loop still
/// leaves time for the console.
const POLL_MS: u32 = 1;

/// How long the final ACK is given to leave the mailbox before the reset that
/// follows a manifested image.
const MANIFEST_RESET_MS: u32 = 20;

/// Chunk of the read-back CRC pass over the DFU partition.
const VERIFY_CHUNK: usize = can::PAGE_BYTES;

/// How long (in milliseconds) to wait for a free transmit mailbox before
/// giving up on a frame. bxCAN has three mailboxes and the flow is
/// stop-and-wait, so one frame goes out in ~130 µs at 1 Mbit/s; anything
/// still pending after this budget means the bus is not ACKing at all, and
/// the host's own retries are the recovery path for that.
const TX_BUDGET_MS: u32 = 10;

/// The driver this transport runs on.
///
/// The non-blocking flavour, not `Blocking`: the session is a polled loop and
/// only `NonBlocking` exposes the inherent [`Can::try_recv`] (the blocking
/// `receive()` would park for up to the driver timeout and depends on the
/// embassy time driver ticking, which a bare-metal bootloader cannot
/// guarantee).
type CanBus = Can<'static, ch32_hal::peripherals::CAN1, ch32_hal::mode::NonBlocking>;

/// Driver and pacing delay, passed around together.
///
/// Every frame this transport sends may have to wait for a free mailbox, so
/// the send helpers always want the same [`Delay`] as the driver; bundling
/// the two keeps the session handlers from threading them as separate
/// arguments.
struct Bus<'a> {
    can: &'a mut CanBus,
    delay: &'a mut Delay,
}

impl Bus<'_> {
    /// Waits for `ms` milliseconds.
    fn wait(&mut self, ms: u32) {
        self.delay.delay_ms(ms);
    }

    /// Sends one response header on this node's response control ID.
    fn send(&mut self, response: Response) -> Result<(), ()> {
        self.transmit(can::SUB_CONTROL, &response.encode())
    }

    /// Sends one payload continuation frame on this node's response data ID.
    fn send_data(&mut self, payload: &[u8]) -> Result<(), ()> {
        self.transmit(can::SUB_DATA, payload)
    }

    /// The `GET_INFO` response: a header whose `next_offset` field carries
    /// the payload length, followed by the payload in data frames (§6.1).
    fn send_info(&mut self, state: u8) -> Result<(), ()> {
        let uid = ch32_hal::signature::unique_id();
        let chip = ch32_hal::signature::chip_id();
        let info = can::Info {
            protocol_version: can::PROTOCOL_VERSION,
            state,
            uid,
            // `ChipID` keeps the raw word private; rebuild it from the two halves.
            chip_id: (u32::from(chip.rev_id()) << 16) | u32::from(chip.dev_id()),
        };
        let payload = info.encode();

        self.send(Response::ok(can::cmd::GET_INFO, can::INFO_LEN as u32))?;
        for chunk in payload.chunks(can::FRAME_DATA_MAX) {
            self.send_data(chunk)?;
        }
        Ok(())
    }

    /// Puts one frame in a transmit mailbox with a bounded wait for one to
    /// free up.
    ///
    /// The non-blocking `transmit` only fails with `WouldBlock` while all
    /// three mailboxes are still pending; giving up after [`TX_BUDGET_MS`]
    /// means the bus is not being ACKed, and losing a response is what the
    /// host's stop-and-wait retries are for, so a failure is not worth
    /// reporting to the protocol.
    fn transmit(&mut self, sub: u16, payload: &[u8]) -> Result<(), ()> {
        let id = StandardId::new(can::resp_id(NODE_ID, sub)).ok_or(())?;
        let frame = CanFrame::new(Id::Standard(id), payload).ok_or(())?;
        for _ in 0..TX_BUDGET_MS {
            if self.can.transmit(&frame).is_ok() {
                return Ok(());
            }
            self.wait(1);
        }
        Err(())
    }
}

/// The CAN1 controller and its pins, as moved out of `Peripherals`.
///
/// Like the USB transport's peripherals these are taken at startup but only
/// consumed once a session is entered, so a plain boot never touches the CAN
/// hardware.
pub(crate) struct CanPeripherals {
    pub(crate) can: ch32_hal::Peri<'static, ch32_hal::peripherals::CAN1>,
    #[cfg(not(feature = "can-pb8-pb9"))]
    pub(crate) rx: ch32_hal::Peri<'static, ch32_hal::peripherals::PA11>,
    #[cfg(not(feature = "can-pb8-pb9"))]
    pub(crate) tx: ch32_hal::Peri<'static, ch32_hal::peripherals::PA12>,
    #[cfg(feature = "can-pb8-pb9")]
    pub(crate) rx: ch32_hal::Peri<'static, ch32_hal::peripherals::PB8>,
    #[cfg(feature = "can-pb8-pb9")]
    pub(crate) tx: ch32_hal::Peri<'static, ch32_hal::peripherals::PB9>,
}

/// Where the node is in the session lifecycle (§7).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Phase {
    /// No session open.
    Idle,
    /// `SESSION_OPEN` accepted, waiting for `BEGIN`.
    Open,
    /// `BEGIN` accepted, image bytes flowing.
    Receiving,
    /// All announced bytes written, waiting for `FINISH`.
    Complete,
}

impl Phase {
    /// The `state` byte a `GET_INFO` response reports for this phase.
    fn info_state(self) -> u8 {
        match self {
            Phase::Idle | Phase::Open => can::state::BOOTLOADER_IDLE,
            Phase::Receiving | Phase::Complete => can::state::BOOTLOADER_RECEIVING,
        }
    }
}

/// Everything a session remembers between frames.
struct Session {
    phase: Phase,
    /// Total image length announced by `BEGIN`.
    total_len: u32,
    /// CRC32 of the whole image as announced by `BEGIN`, checked at `FINISH`.
    expected_crc: u32,
    /// Offset the next image byte belongs at. Since data frames carry no
    /// offset field (§6.3), this is the implicit sequence number, and the
    /// page-granular ACK is what keeps host and node in step.
    next_offset: u32,
    /// Page being accumulated.
    page: [u8; can::PAGE_BYTES],
    /// Bytes of `page` filled so far.
    page_used: usize,
    /// DFU-relative flash offset `page` will be programmed to.
    page_base: usize,
    /// Milliseconds since the last frame; the session watchdog (§7).
    quiet_ms: u32,
}

impl Session {
    fn new() -> Self {
        Self {
            phase: Phase::Idle,
            total_len: 0,
            expected_crc: 0,
            next_offset: 0,
            page: [0xFF; can::PAGE_BYTES],
            page_used: 0,
            page_base: 0,
            quiet_ms: 0,
        }
    }

    /// Absorbs one data frame into the page buffer, returning whether the page
    /// is now complete and should be programmed.
    ///
    /// Data frames are implicitly addressed at [`Session::next_offset`], so the
    /// only inconsistency the node can see is a frame that does not fit the
    /// page or the announced length any more: a duplicated or replayed frame.
    /// That is NACK'ed with the expected offset.
    fn push(&mut self, data: &[u8]) -> Result<bool, u8> {
        if data.is_empty()
            || self.page_used + data.len() > can::PAGE_BYTES
            || self.next_offset + data.len() as u32 > self.total_len
        {
            return Err(can::err::OFFSET);
        }
        self.page[self.page_used..self.page_used + data.len()].copy_from_slice(data);
        self.page_used += data.len();
        self.next_offset += data.len() as u32;

        Ok(self.page_used == can::PAGE_BYTES || self.next_offset == self.total_len)
    }

    /// Programs the accumulated page into the DFU partition.
    ///
    /// Erased flash reads as 0xFF, so the tail of a short final page is padded
    /// to a whole flash page, exactly as on the serial transport.
    fn flush<DFU: NorFlash, STATE: NorFlash>(
        &mut self,
        updater: &mut BlockingFirmwareUpdater<'_, DFU, STATE>,
    ) -> Result<(), u8> {
        if self.page_used == 0 {
            return Ok(());
        }
        self.page[self.page_used..].fill(0xFF);
        let base = self.page_base;

        updater
            .write_firmware(base, &self.page)
            .map_err(|_| can::err::FLASH)?;

        self.page_base += can::PAGE_BYTES;
        self.page_used = 0;
        self.page.fill(0xFF);
        Ok(())
    }
}

/// Serves the CAN update protocol until a complete, verified image has been
/// manifested, which ends in a system reset.
///
/// This never returns. A session that is abandoned (watchdog, `ABORT`, a host
/// that goes away) does not end the transport: the node drops back to idle and
/// keeps listening, so the board is never left without a way in. As on the
/// other transports the active partition is untouched until the final CRC
/// check passes, so any interruption leaves the old image bootable.
pub(crate) fn session<T: usart::Instance>(
    flash: &FlashMutex,
    console: &mut Console<'_, T>,
    delay: &mut Delay,
    peripherals: CanPeripherals,
) -> ! {
    let mut bus = match init_bus(peripherals) {
        Ok(bus) => bus,
        Err(()) => {
            // The only failure is a bitrate the CAN clock cannot produce, which
            // is a build error rather than a bus fault, and it would come back
            // on every retry. Reset rather than hang: the state partition is
            // untouched, and the console says why.
            let _ = core::write!(
                &mut console.tx,
                "CAN{CAN_BITRATE} init failed, is the bitrate achievable from this clock?\r\n"
            );
            system_reset()
        }
    };
    let mut bus = Bus {
        can: &mut bus,
        delay,
    };

    let mut aligned = AlignedBuffer([0u8; STATE_BUFFER_SIZE]);
    let config = FirmwareUpdaterConfig::from_linkerfile_blocking(flash, flash);
    let mut updater = BlockingFirmwareUpdater::new(config, aligned.as_mut());

    let mut session = Session::new();

    let _ = core::write!(
        &mut console.tx,
        "{CHIP_NAME} CAN update: node {NODE_ID}, {CAN_BITRATE} bps\r\n"
    );

    loop {
        // The inherent `try_recv`, not the blocking `receive()` of the
        // embedded-can trait, which would park for up to the driver timeout
        // instead of returning to the poll cadence.
        match bus.can.try_recv() {
            Ok(frame) => {
                if handle(&mut bus, &mut updater, flash, console, &mut session, &frame) {
                    // A verified image has been marked updated; let the ACK
                    // leave the mailbox before the reset.
                    bus.wait(MANIFEST_RESET_MS);
                    system_reset();
                }
            }
            Err(_) => {
                if session.phase == Phase::Receiving && session.quiet_ms >= can::SESSION_TIMEOUT_MS
                {
                    // Announce the drop the way a host can poll for it: the
                    // watchdog error travels as a `QUERY` failure (§7).
                    let next = session.next_offset;
                    let _ = bus.send(Response::failure(can::cmd::QUERY, can::err::TIMEOUT, next));
                    let _ = core::write!(&mut console.tx, "can session timeout\r\n");
                    session = Session::new();
                }
                bus.wait(POLL_MS);
                session.quiet_ms += POLL_MS;
            }
        }
    }
}

/// Brings CAN1 up with the two hardware filters of the protocol (§4.3): this
/// node's own request IDs and the functional broadcast ID. Everything else,
/// including other nodes' sessions, is dropped in hardware.
fn init_bus(peripherals: CanPeripherals) -> Result<CanBus, ()> {
    let CanPeripherals { can, rx, tx } = peripherals;
    let bus = Can::new_nb(
        can,
        rx,
        tx,
        CanFifo::Fifo0,
        CanMode::Normal,
        CAN_BITRATE,
        CanConfig::default(),
    )
    .map_err(|_| ())?;

    // Two 32-bit mask-mode entries. The raw register words come from the codec
    // (`can::filter_words`), which puts the standard ID in bits 31:21 and
    // leaves the IDE and RTR bits of the mask set, so only standard data frames
    // can match; the protocol never uses extended or remote frames.
    let ((own_code, own_mask), (func_code, func_mask)) = can::node_filters(NODE_ID);
    for (bank, (code, mask)) in [(0, (own_code, own_mask)), (1, (func_code, func_mask))] {
        let (id_value, id_mask) = can::filter_words(code, mask);
        bus.add_filter(CanFilter::<Bit32Mode, MaskMode> {
            bank,
            id_value,
            id_mask,
            bit_mode: Bit32Mode,
            mode: MaskMode,
        });
    }

    Ok(bus)
}

/// Handles one received frame. Returns `true` when an image has been marked
/// updated and the caller should reset.
fn handle<T: usart::Instance, DFU: NorFlash, STATE: NorFlash>(
    bus: &mut Bus,
    updater: &mut BlockingFirmwareUpdater<'_, DFU, STATE>,
    flash: &FlashMutex,
    console: &mut Console<'_, T>,
    session: &mut Session,
    frame: &CanFrame,
) -> bool {
    session.quiet_ms = 0;

    // The filters already admit only this node's request IDs and the functional
    // request ID; drop anything else defensively (extended IDs, responses).
    let Id::Standard(id) = *frame.id() else {
        return false;
    };
    let raw = id.as_raw();
    if !can::is_request(raw) {
        return false;
    }
    let node = can::node_of(raw);
    let sub = can::sub_of(raw);

    if sub == can::SUB_DATA {
        return handle_data(bus, updater, console, session, frame, node == NODE_ID);
    }
    if sub != can::SUB_CONTROL {
        return false;
    }
    let Some(request) = can::parse_control(frame.data()) else {
        return false;
    };

    if node == can::FUNCTIONAL_NODE {
        // Broadcast: only `GET_INFO` may be answered, and not all at once. A
        // node that is not the target of a unicast command stays silent (§6.1).
        if matches!(request, Request::GetInfo) {
            bus.wait(uid_spread());
            let _ = bus.send_info(session.phase.info_state());
        }
        return false;
    }
    if node != NODE_ID {
        return false;
    }

    match request {
        Request::Ping => {
            let _ = bus.send(Response::ok(can::cmd::PING, 0));
            false
        }
        Request::GetInfo => {
            let _ = bus.send_info(session.phase.info_state());
            false
        }
        Request::SessionOpen { nonce } => {
            match session.phase {
                Phase::Receiving | Phase::Complete => {
                    let _ = bus.send(Response::failure(
                        can::cmd::SESSION_OPEN,
                        can::err::BUSY,
                        session.next_offset,
                    ));
                }
                _ => {
                    *session = Session {
                        phase: Phase::Open,
                        ..Session::new()
                    };
                    let _ = bus.send(Response::ok(can::cmd::SESSION_OPEN, 0));
                    // The nonce comes back in one data frame so that the host
                    // can tell the node that accepted its session from another
                    // one talking over the same IDs (§7).
                    let _ = bus.send_data(&nonce.to_le_bytes());
                }
            }
            false
        }
        Request::Begin { len, crc32 } => {
            match session.phase {
                Phase::Idle => {
                    let _ = bus.send(Response::failure(can::cmd::BEGIN, can::err::NO_SESSION, 0));
                }
                Phase::Receiving | Phase::Complete => {
                    let _ = bus.send(Response::failure(
                        can::cmd::BEGIN,
                        can::err::BUSY,
                        session.next_offset,
                    ));
                }
                Phase::Open => {
                    if len == 0 || len > dfu_size() {
                        let _ =
                            bus.send(Response::failure(can::cmd::BEGIN, can::err::TOO_LARGE, 0));
                    } else {
                        let mut fresh = Session::new();
                        fresh.phase = Phase::Receiving;
                        fresh.total_len = len;
                        fresh.expected_crc = crc32;
                        *session = fresh;
                        let _ = bus.send(Response::ok(can::cmd::BEGIN, 0));
                        let _ = core::write!(&mut console.tx, "can begin {} bytes\r\n", len);
                    }
                }
            }
            false
        }
        Request::Finish { len, crc32 } => finish(bus, updater, flash, console, session, len, crc32),
        Request::Query => {
            // Answered in every phase, which is what lets a host resync after
            // its own state was lost (§7).
            let _ = bus.send(Response::ok(can::cmd::QUERY, session.next_offset));
            false
        }
        Request::Abort | Request::SessionClose => {
            let cmd = if matches!(request, Request::Abort) {
                can::cmd::ABORT
            } else {
                can::cmd::SESSION_CLOSE
            };
            let _ = core::write!(&mut console.tx, "can session closed by host\r\n");
            *session = Session::new();
            let _ = bus.send(Response::ok(cmd, 0));
            false
        }
        // `ENTER_UPDATE` belongs to the runtime listener in the application: a
        // node that is already in the bootloader has nowhere further to detach.
        // Answer OK so a host that asks does not retry forever.
        Request::EnterUpdate { .. } => {
            let _ = bus.send(Response::ok(can::cmd::ENTER_UPDATE, 0));
            false
        }
        Request::Unknown { cmd } => {
            // No response: `§8` defines no code for an unknown opcode, and a
            // host that hears nothing learns that this node does not speak it.
            let _ = core::write!(&mut console.tx, "can unknown opcode {cmd:#04x}\r\n");
            false
        }
    }
}

/// Data-channel frames: accumulate a page, program it, ACK it (§7).
///
/// One ACK per 256-byte page is also the unit of idempotency: a page is either
/// written and acknowledged, or the host repeats the whole page after a
/// `QUERY`, never a partial frame sequence.
fn handle_data<DFU: NorFlash, STATE: NorFlash>(
    bus: &mut Bus,
    updater: &mut BlockingFirmwareUpdater<'_, DFU, STATE>,
    console: &mut Console<'_, impl usart::Instance>,
    session: &mut Session,
    frame: &CanFrame,
    targeted: bool,
) -> bool {
    if !targeted {
        return false;
    }
    if session.phase != Phase::Receiving {
        let err = if session.phase == Phase::Complete {
            can::err::OFFSET
        } else {
            can::err::NO_SESSION
        };
        let _ = bus.send(Response::failure(can::cmd::BEGIN, err, session.next_offset));
        return false;
    }

    match session.push(frame.data()) {
        // Mid-page: nothing to program and nothing to say. The host waits for
        // the page ACK, so silence between ACKs is normal traffic.
        Ok(false) => false,
        Ok(true) => match session.flush(updater) {
            Ok(()) => {
                if session.next_offset == session.total_len {
                    session.phase = Phase::Complete;
                }
                let _ = bus.send(Response::ok(can::cmd::BEGIN, session.next_offset));
                false
            }
            Err(err) => {
                // A flash error is fatal to the session (§8), but not to the
                // board: the state partition still says "boot the old image".
                let next = session.next_offset;
                let _ = core::write!(&mut console.tx, "can flash write failed at {next}\r\n");
                *session = Session::new();
                let _ = bus.send(Response::failure(can::cmd::BEGIN, err, next));
                false
            }
        },
        Err(err) => {
            let _ = bus.send(Response::failure(can::cmd::BEGIN, err, session.next_offset));
            false
        }
    }
}

/// `FINISH`: flush the short final page, verify the read-back CRC, mark the
/// image updated. Returns `true` when the caller should reset.
fn finish<T: usart::Instance, DFU: NorFlash, STATE: NorFlash>(
    bus: &mut Bus,
    updater: &mut BlockingFirmwareUpdater<'_, DFU, STATE>,
    flash: &FlashMutex,
    console: &mut Console<'_, T>,
    session: &mut Session,
    len: u32,
    crc: u32,
) -> bool {
    if session.phase != Phase::Receiving && session.phase != Phase::Complete {
        let _ = bus.send(Response::failure(can::cmd::FINISH, can::err::NO_SESSION, 0));
        return false;
    }
    if len != session.total_len || session.next_offset != session.total_len {
        // `next_offset` says exactly how much is missing, which is the host's
        // cue to send the rest or to start over.
        let _ = bus.send(Response::failure(
            can::cmd::FINISH,
            can::err::OFFSET,
            session.next_offset,
        ));
        return false;
    }
    if let Err(err) = session.flush(updater) {
        let _ = bus.send(Response::failure(
            can::cmd::FINISH,
            err,
            session.next_offset,
        ));
        return false;
    }

    if crc != session.expected_crc {
        // The host announced one CRC at `BEGIN` and a different one at
        // `FINISH`: its own state is broken, refuse to boot it.
        let _ = core::write!(&mut console.tx, "can finish crc disagrees with begin\r\n");
        let _ = bus.send(Response::failure(
            can::cmd::FINISH,
            can::err::CRC,
            session.next_offset,
        ));
        return false;
    }

    match verify(flash, session.total_len as usize, crc) {
        Ok(()) => match updater.mark_updated() {
            Ok(()) => {
                let _ = bus.send(Response::ok(can::cmd::FINISH, session.next_offset));
                let _ = core::write!(&mut console.tx, "can image verified, swapping\r\n");
                true
            }
            Err(_) => {
                let _ = core::write!(&mut console.tx, "can mark updated failed\r\n");
                let _ = bus.send(Response::failure(
                    can::cmd::FINISH,
                    can::err::FLASH,
                    session.next_offset,
                ));
                false
            }
        },
        Err(()) => {
            let _ = core::write!(&mut console.tx, "can read-back crc mismatch\r\n");
            *session = Session::new();
            let _ = bus.send(Response::failure(
                can::cmd::FINISH,
                can::err::CRC,
                session.next_offset,
            ));
            false
        }
    }
}

/// Streams the DFU partition back and compares its CRC32 with the one the host
/// announced. A mismatch leaves the session dead but the board fully bootable:
/// nothing has been written to the state partition yet.
fn verify(flash: &FlashMutex, len: usize, expected: u32) -> Result<(), ()> {
    let mut crc = can::Crc32::new();
    let mut buf = [0u8; VERIFY_CHUNK];
    let mut done = 0;

    while done < len {
        let want = core::cmp::min(VERIFY_CHUNK, len - done);
        flash
            .lock(|f| {
                f.borrow_mut()
                    .read(dfu_start() + done as u32, &mut buf[..want])
                    .map_err(|_| ())
            })
            .map_err(|_| ())?;
        crc.update(&buf[..want]);
        done += want;
    }

    if crc.finalize() == expected {
        Ok(())
    } else {
        Err(())
    }
}

/// Milliseconds to wait before answering a functional `GET_INFO`, derived from
/// the unique ID so that nodes discovering at the same time spread their replies
/// apart instead of colliding (§5).
fn uid_spread() -> u32 {
    (ch32_hal::signature::unique_id()[11] % 16) as u32
}
