//! USB DFU 1.1 update transport.
//!
//! Compiled only for a `transport-usb` build of the bootloader, for which
//! `build.rs` also sets one of the `usb_usbd`, `usb_otg_fs` and `usb_usbhs`
//! cfgs according to the controller the selected part has.
//!
//! Note what this does *not* pull in: `embassy-executor`. The DFU handler is
//! fully synchronous, `embassy_futures::block_on()` just polls the embassy-usb
//! device loop in a busy loop until the host manifests an image, which then
//! resets the chip. That keeps the bootloader a single straight-line program
//! with one stack, at the price of running the core at 100 % while a host is
//! attached, which a bootloader can afford.

#[cfg(any(usb_otg_fs, usb_usbhs))]
use ch32_hal::usb::EndpointDataBuffer512;
use embassy_boot_ch32::{
    AlignedBuffer, BlockingFirmwareUpdater, FirmwareUpdaterConfig, system_reset,
};
use embassy_usb::Builder;
use embassy_usb::Config as UsbConfig;
use embassy_usb_dfu::consts::DfuAttributes;
use embassy_usb_dfu::{Reset, new_state, usb_dfu};

use crate::{CHIP_NAME, FlashMutex, STATE_BUFFER_SIZE};

/// Payload size of one `DFU_DNLOAD`, and the size of the endpoint buffers.
///
/// Must be a multiple of the flash write size (256) and must fit in the control
/// buffer, because a DFU download arrives over EP0. `dfu-util` never sends more
/// than the `wTransferSize` advertised in the functional descriptor, which
/// `usb_dfu()` derives from this constant.
const BLOCK_SIZE: usize = 512;

/// Number of endpoint buffers handed to the controllers that need them (the
/// USBD controllers keep theirs in on-chip USBRAM and take none).
///
/// DFU has no endpoints of its own, everything runs over EP0, so this only has
/// to cover the control endpoint.
#[cfg(any(usb_otg_fs, usb_usbhs))]
const NR_ENDPOINTS: usize = 2;

/// USB identifiers of the bootloader.
///
/// Deliberately the example VID/PID used by the embassy DFU examples rather
/// than the WCH VID, so that the bootloader cannot be mistaken for the vendor
/// ISP. Use a pair of your own for a product, and keep it identical in the
/// application so that `dfu-util -d <vid>:<pid>` matches in both modes.
const USB_VID: u16 = 0xc0de;
/// See [`USB_VID`].
const USB_PID: u16 = 0xcafe;

/// Issues the reset that `embassy-usb-dfu` asks for once an image has been
/// manifested.
struct PficReset;

impl Reset for PficReset {
    fn sys_reset(&self) {
        // Not `embassy_boot_ch32::system_reset()`'s caller being a USB
        // interrupt: the DFU handler runs in the main context here, but the
        // reset helper is what leaves both the core and the peripherals in the
        // state the application expects either way.
        system_reset()
    }
}

/// The USB controller and its data pins, as moved out of `Peripherals`.
///
/// The peripherals are taken by the caller but only consumed here, and only
/// when a session is actually entered, so a plain boot never touches the USB
/// hardware.
pub(crate) struct UsbPeripherals {
    #[cfg(usb_usbd)]
    pub(crate) usb: ch32_hal::Peri<'static, ch32_hal::peripherals::USBD>,
    #[cfg(usb_otg_fs)]
    pub(crate) usb: ch32_hal::Peri<'static, ch32_hal::peripherals::OTG_FS>,
    #[cfg(usb_usbhs)]
    pub(crate) usb: ch32_hal::Peri<'static, ch32_hal::peripherals::USBHS>,
    #[cfg(usb_usbhs)]
    pub(crate) dp: ch32_hal::Peri<'static, ch32_hal::peripherals::PB7>,
    #[cfg(usb_usbhs)]
    pub(crate) dm: ch32_hal::Peri<'static, ch32_hal::peripherals::PB6>,
    #[cfg(any(usb_usbd, usb_otg_fs))]
    pub(crate) dp: ch32_hal::Peri<'static, ch32_hal::peripherals::PA12>,
    #[cfg(any(usb_usbd, usb_otg_fs))]
    pub(crate) dm: ch32_hal::Peri<'static, ch32_hal::peripherals::PA11>,
}

// `bind_interrupts!` has no syntax for per-arm `cfg`, so bind once per
// controller. OTG_FS is bound even though its driver takes no binding
// argument: the vector entry exists only through this binding, so forgetting it
// leaves the bus stone dead without any error to point at.
#[cfg(usb_usbd)]
ch32_hal::bind_interrupts!(struct Irq {
    USB_LP_CAN1_RX0 => ch32_hal::usbd::InterruptHandler<ch32_hal::peripherals::USBD>;
});

// No `Irq` value is ever constructed for OTG_FS (the driver registers the
// interrupt itself), which the lint does not flag because the generated
// bindings reference the type.
#[cfg(usb_otg_fs)]
ch32_hal::bind_interrupts!(struct Irq {
    OTG_FS => ch32_hal::otg_fs::InterruptHandler<ch32_hal::peripherals::OTG_FS>;
});

#[cfg(usb_usbhs)]
ch32_hal::bind_interrupts!(struct Irq {
    USBHS => ch32_hal::usbhs::InterruptHandler<ch32_hal::peripherals::USBHS>;
    USBHS_WKUP => ch32_hal::usbhs::WakeupInterruptHandler<ch32_hal::peripherals::USBHS>;
});

/// Serves the DFU partition to a host until it manifests an image, which resets
/// the device.
///
/// This never returns.
pub(crate) fn session(flash: &FlashMutex, peripherals: UsbPeripherals) -> ! {
    // The state partition is accessed through a scratch buffer of exactly
    // `STATE::WRITE_SIZE` bytes; the DFU data path uses its own `BLOCK_SIZE`
    // buffer inside the handler.
    let mut aligned = AlignedBuffer([0u8; STATE_BUFFER_SIZE]);
    let config = FirmwareUpdaterConfig::from_linkerfile_blocking(flash, flash);
    let updater = BlockingFirmwareUpdater::new(config, aligned.as_mut());

    // `MANIFESTATION_TOLERANT` tells the host that a reset does not interrupt
    // the transfer, which is exactly what `finish()` does.
    let mut state = new_state::<_, _, _, BLOCK_SIZE>(
        updater,
        DfuAttributes::CAN_DOWNLOAD | DfuAttributes::MANIFESTATION_TOLERANT,
        PficReset,
    );

    let mut config_descriptor = [0u8; 256];
    let mut bos_descriptor = [0u8; 256];
    // A download arrives as the data stage of a control request, so this has to
    // hold a whole block.
    let mut control_buf = [0u8; BLOCK_SIZE];

    let mut usb_config = UsbConfig::new(USB_VID, USB_PID);
    usb_config.manufacturer = Some("embassy-boot-ch32");
    usb_config.product = Some("CH32 DFU Bootloader");
    usb_config.serial_number = Some(CHIP_NAME);
    // 500 mA is what an unpowered-from-VBUS board can honestly claim.
    usb_config.max_power = 100;
    usb_config.max_packet_size_0 = 64;

    #[cfg(usb_usbd)]
    let driver = {
        // The USBD controllers keep their buffers in on-chip USBRAM, so this
        // driver takes no endpoint buffers.
        let UsbPeripherals { usb, dp, dm } = peripherals;
        ch32_hal::usbd::Driver::new(usb, Irq, dp, dm)
    };

    // Declared here rather than next to the driver: the driver borrows it for
    // as long as it lives.
    #[cfg(any(usb_otg_fs, usb_usbhs))]
    let mut endpoint_buffers: [EndpointDataBuffer512; NR_ENDPOINTS] =
        core::array::from_fn(|_| EndpointDataBuffer512::default());

    #[cfg(usb_otg_fs)]
    let driver = {
        let UsbPeripherals { usb, dp, dm } = peripherals;
        ch32_hal::otg_fs::Driver::new(usb, dp, dm, &mut endpoint_buffers)
    };

    #[cfg(usb_usbhs)]
    let driver = {
        let UsbPeripherals { usb, dp, dm } = peripherals;
        ch32_hal::usbhs::Driver::new(usb, Irq, dp, dm, &mut endpoint_buffers)
    };

    let mut builder = Builder::new(
        driver,
        usb_config,
        &mut config_descriptor,
        &mut bos_descriptor,
        // No MSOS descriptors: without them Windows needs the driver assigned
        // by hand, which is the documented setup for a one-off bootloader.
        &mut [],
        &mut control_buf,
    );

    usb_dfu::<_, _, _, _, BLOCK_SIZE>(&mut builder, &mut state, |_| {});

    let mut device = builder.build();

    embassy_futures::block_on(device.run())
}
