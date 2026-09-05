//! CAN-bus runtime update listener.
//!
//! Compiled only for a `can-runtime` build of the application. It speaks a
//! small subset of `docs/can-update-protocol.md`: the listener exists to be
//! found on the bus and to be told to reset into the `transport-can`
//! bootloader, everything beyond that is the bootloader's job.
//!
//! Handled frames (on this node's request IDs, §4):
//!
//! * `PING` — answered with a plain OK header.
//! * `GET_INFO` — answered with the `Info` payload in state [`can::state::APP`];
//!   functional (broadcast) `GET_INFO` is answered too, spread apart by a
//!   per-node delay so that a discovery broadcast does not turn into one long
//!   collision (§5).
//! * `ENTER_UPDATE` — when the UID prefix in the frame matches this board's
//!   factory unique ID, an OK header is sent and [`poll`] returns `true`; the
//!   caller then marks the state partition for DFU and resets (§6.2). The reset
//!   is deliberately left to the caller because the DFU request needs the flash
//!   and the console, which live in `main.rs`.
//!
//! Everything else is ignored: session commands only mean something to the
//! bootloader, and a node that is not the target of a unicast command stays
//! silent (§6.1).
//!
//! [`poll`]: CanRuntime::poll

use ch32_hal::can::{
    Bit32Mode, Can, CanFifo, CanFilter, CanFrame, CanMode, Config as CanConfig, MaskMode,
};
use ch32_hal::usart::Instance;
use embassy_boot_ch32::can::{self, Request, Response};
use embassy_time::Timer;
use embedded_can::nb::Can as _;
use embedded_can::{Id, StandardId};

use crate::Tx;

/// CAN node ID of this board; must match the `transport-can` bootloader it
/// pairs with (`examples/bootloader/src/can_update.rs`).
const NODE_ID: u8 = can::DEFAULT_NODE_ID;

/// CAN bitrate; must match the bootloader. 1 Mbit/s is the protocol default.
const CAN_BITRATE: u32 = 1_000_000;

/// How many 1 ms waits to allow for a free transmit mailbox before dropping a
/// response. bxCAN has three mailboxes and a frame takes ~130 µs at 1 Mbit/s,
/// so giving up means the bus is not being ACKed at all; the host's own
/// retries are the recovery path for that (§7).
const TX_BUDGET_MS: u32 = 10;

/// How long the `ENTER_UPDATE` OK is given to leave the mailbox before the
/// caller resets into the bootloader.
const ENTER_RESET_MS: u64 = 5;

/// The driver, same non-blocking flavour the bootloader uses (the blocking
/// `receive()` would park and is of no use in a polled loop).
type CanBus = Can<'static, ch32_hal::peripherals::CAN1, ch32_hal::mode::NonBlocking>;

/// The CAN1 controller and its pins, as moved out of `Peripherals`.
///
/// The default pins are the CAN1 remap 0 pair PA11 (RX) / PA12 (TX); on a
/// board that uses those for USB build with `can-pb8-pb9` to get the remap 1
/// pair PB8 (RX) / PB9 (TX) instead.
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

/// The listener itself; see the module docs for what it answers.
pub(crate) struct CanRuntime {
    can: CanBus,
}

impl CanRuntime {
    /// Brings CAN1 up with the two hardware filters of the protocol (§4.3):
    /// this node's own request IDs and the functional broadcast ID. Everything
    /// else, including other nodes' update sessions, is dropped in hardware.
    pub(crate) fn new(peripherals: CanPeripherals) -> Result<Self, ()> {
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

        // Two 32-bit mask-mode entries; the raw register words come from the
        // codec, which leaves the IDE and RTR bits of the mask set, so only
        // standard data frames can match.
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

        Ok(Self { can: bus })
    }

    /// Drains the receive FIFO, answering the frames of interest. Returns
    /// `true` when a matching `ENTER_UPDATE` was answered and the caller
    /// should request DFU and reset.
    ///
    /// Intended to be called once per iteration of the console polling loop,
    /// which idles at a couple of milliseconds — fast enough for a bus at
    /// 1 Mbit/s with a three-deep FIFO and a stop-and-wait host.
    pub(crate) async fn poll<T: Instance>(&mut self, console: &mut Tx<'_, T>) -> bool {
        while let Ok(frame) = self.can.try_recv() {
            if self.handle(console, &frame).await {
                return true;
            }
        }
        false
    }

    /// Handles one received frame; see the module docs for the subset.
    async fn handle<T: Instance>(&mut self, console: &mut Tx<'_, T>, frame: &CanFrame) -> bool {
        // The filters already admit only this node's request IDs and the
        // functional request ID; drop anything else defensively.
        let Id::Standard(id) = *frame.id() else {
            return false;
        };
        let raw = id.as_raw();
        if !can::is_request(raw) {
            return false;
        }
        let node = can::node_of(raw);
        if can::sub_of(raw) != can::SUB_CONTROL {
            // Data frames belong to a bootloader session; while the
            // application runs they are just other traffic.
            return false;
        }
        let Some(request) = can::parse_control(frame.data()) else {
            return false;
        };

        match request {
            Request::Ping if node == NODE_ID => {
                let _ = self.send(Response::ok(can::cmd::PING, 0)).await;
                false
            }
            Request::GetInfo if node == NODE_ID => {
                let _ = self.send_info().await;
                false
            }
            Request::GetInfo if node == can::FUNCTIONAL_NODE => {
                Timer::after_millis(uid_spread() as u64).await;
                let _ = self.send_info().await;
                false
            }
            Request::EnterUpdate { uid_prefix } if node == NODE_ID && matches_uid(uid_prefix) => {
                console.line("can: update requested, entering the bootloader");
                let _ = self.send(Response::ok(can::cmd::ENTER_UPDATE, 0)).await;
                // Let the OK leave the mailbox before the caller's flash write
                // and reset.
                Timer::after_millis(ENTER_RESET_MS).await;
                true
            }
            // Not our node ID, a functional `ENTER_UPDATE` (which cannot name a
            // target), or a session command that only the bootloader answers.
            _ => false,
        }
    }

    /// The `GET_INFO` response: a header whose `next_offset` field carries the
    /// payload length, followed by the `Info` in data frames (§6.1).
    async fn send_info(&mut self) -> Result<(), ()> {
        let uid = ch32_hal::signature::unique_id();
        let chip = ch32_hal::signature::chip_id();
        let info = can::Info {
            protocol_version: can::PROTOCOL_VERSION,
            state: can::state::APP,
            uid,
            // `ChipID` keeps the raw word private; rebuild it from the halves.
            chip_id: (u32::from(chip.rev_id()) << 16) | u32::from(chip.dev_id()),
        };
        let payload = info.encode();

        self.send(Response::ok(can::cmd::GET_INFO, can::INFO_LEN as u32))
            .await?;
        for chunk in payload.chunks(can::FRAME_DATA_MAX) {
            self.transmit(can::SUB_DATA, chunk).await?;
        }
        Ok(())
    }

    /// Sends one response header on this node's response control ID.
    async fn send(&mut self, response: Response) -> Result<(), ()> {
        self.transmit(can::SUB_CONTROL, &response.encode()).await
    }

    /// Puts one frame in a transmit mailbox with a bounded wait for one to
    /// free up. Losing a response is what the host's retries are for, so a
    /// failure is not worth reporting to the protocol.
    async fn transmit(&mut self, sub: u16, payload: &[u8]) -> Result<(), ()> {
        let id = StandardId::new(can::resp_id(NODE_ID, sub)).ok_or(())?;
        let frame = CanFrame::new(Id::Standard(id), payload).ok_or(())?;
        for _ in 0..TX_BUDGET_MS {
            if self.can.transmit(&frame).is_ok() {
                return Ok(());
            }
            Timer::after_millis(1).await;
        }
        Err(())
    }
}

/// Whether `uid_prefix` is the beginning of this board's factory unique ID.
fn matches_uid(uid_prefix: &[u8]) -> bool {
    uid_prefix.len() == can::UID_PREFIX_LEN
        && uid_prefix == &ch32_hal::signature::unique_id()[..can::UID_PREFIX_LEN]
}

/// Milliseconds to wait before answering a functional `GET_INFO`, derived from
/// the unique ID so that nodes discovered at the same time spread their replies
/// apart instead of colliding (§5). Must match the bootloader.
fn uid_spread() -> u32 {
    (ch32_hal::signature::unique_id()[11] % 16) as u32
}
