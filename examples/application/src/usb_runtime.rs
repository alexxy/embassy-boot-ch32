//! DFU 1.1 runtime interface.
//!
//! Compiled only for a `usb-dfu` build of the application, for which `build.rs`
//! also sets one of the `usb_usbd`, `usb_otg_fs` and `usb_usbhs` cfgs according
//! to the controller the selected part has, and links the binary against the
//! same `-usb` partition map as the `transport-usb` bootloader.
//!
//! The runtime interface does exactly two things: it answers `GET_STATUS` so
//! that `dfu-util -l` can see the device, and it performs the detach sequence
//! (`DFU_DETACH` followed by a bus reset inside the timeout from the DFU
//! functional descriptor) by marking the state partition for DFU and resetting.
//! A `transport-usb` bootloader picks that mark up and serves the actual
//! transfer, so no flash programming code lives here.

use embassy_boot_ch32::{
    AlignedBuffer, BlockingFirmwareUpdater, FirmwareUpdaterConfig, system_reset,
};
use embassy_time::Duration;
use embassy_usb::Builder;
use embassy_usb::Config as UsbConfig;
use embassy_usb_dfu::application::{DfuAttributes, DfuState, Handler, usb_dfu};

#[cfg(any(usb_otg_fs, usb_usbhs))]
use ch32_hal::usb::EndpointDataBuffer512;

use crate::{CHIP_NAME, FlashMutex, STATE_BUFFER_SIZE};

/// Number of endpoint buffers handed to the controllers that need them (the
/// USBD controllers keep theirs in on-chip USBRAM and take none).
///
/// The runtime interface has no endpoints of its own, everything runs over
/// EP0, so this only has to cover the control endpoint.
#[cfg(any(usb_otg_fs, usb_usbhs))]
const NR_ENDPOINTS: usize = 2;

/// How long after a `DFU_DETACH` a bus reset still counts as a detach
/// sequence, and the value the functional descriptor advertises for it.
const DETACH_TIMEOUT: Duration = Duration::from_millis(2500);

/// USB identifiers, which have to be identical to the ones the `transport-usb`
/// bootloader uses so that `dfu-util -d <vid>:<pid>` finds the device in both
/// modes.
///
/// Deliberately the example VID/PID used by the embassy DFU examples rather
/// than the WCH VID, so that the device cannot be mistaken for the vendor ISP.
/// Use a pair of your own for a product.
const USB_VID: u16 = 0xc0de;
/// See [`USB_VID`].
const USB_PID: u16 = 0xcafe;

/// The USB controller and its data pins, as moved out of `Peripherals`.
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

#[cfg(usb_otg_fs)]
ch32_hal::bind_interrupts!(struct Irq {
    OTG_FS => ch32_hal::otg_fs::InterruptHandler<ch32_hal::peripherals::OTG_FS>;
});

#[cfg(usb_usbhs)]
ch32_hal::bind_interrupts!(struct Irq {
    USBHS => ch32_hal::usbhs::InterruptHandler<ch32_hal::peripherals::USBHS>;
    USBHS_WKUP => ch32_hal::usbhs::WakeupInterruptHandler<ch32_hal::peripherals::USBHS>;
});

/// Turns a completed detach sequence into the DFU mark in the state partition.
struct DfuHandler<'d> {
    flash: &'d FlashMutex,
}

impl Handler for DfuHandler<'_> {
    fn enter_dfu(&mut self) {
        // A failed mark cannot be reported anywhere useful, and the fallback is
        // safe: without the mark the bootloader just boots this partition again.
        let mut aligned = AlignedBuffer([0u8; STATE_BUFFER_SIZE]);
        let config = FirmwareUpdaterConfig::from_linkerfile_blocking(self.flash, self.flash);
        let mut updater = BlockingFirmwareUpdater::new(config, aligned.as_mut());
        let _ = updater.mark_dfu();

        // A plain system reset, not a jump into the bootloader: the application
        // has enabled peripheral interrupts whose vectors the bootloader does
        // not handle.
        system_reset()
    }
}

/// Serves the DFU runtime interface forever; call it as the last thing `main`
/// does so that the other embassy tasks keep running.
pub(crate) async fn run(flash: &FlashMutex, peripherals: UsbPeripherals) -> ! {
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

    let mut usb_config = UsbConfig::new(USB_VID, USB_PID);
    usb_config.manufacturer = Some("embassy-boot-ch32");
    usb_config.product = Some("CH32 DFU-capable application");
    usb_config.serial_number = Some(CHIP_NAME);
    usb_config.max_power = 100;
    // The runtime class only ever moves small control transfers.
    usb_config.max_packet_size_0 = 64;

    let mut config_descriptor = [0u8; 256];
    let mut bos_descriptor = [0u8; 256];
    let mut control_buf = [0u8; 64];

    let handler = DfuHandler { flash };
    let mut state = DfuState::new(handler, DfuAttributes::CAN_DOWNLOAD, DETACH_TIMEOUT);

    let mut builder = Builder::new(
        driver,
        usb_config,
        &mut config_descriptor,
        &mut bos_descriptor,
        // No MSOS descriptors: without them Windows needs the driver assigned
        // by hand, which is the documented setup for an example.
        &mut [],
        &mut control_buf,
    );

    usb_dfu(&mut builder, &mut state, |_| {});

    let mut device = builder.build();

    device.run().await
}
