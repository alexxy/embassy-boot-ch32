//! [`embassy-boot`](https://github.com/embassy-rs/embassy) support for WCH CH32
//! microcontrollers driven by
//! [`ch32-hal`](https://github.com/ch32-rs/ch32-hal).
//!
//! This crate is the CH32 counterpart of [`embassy-boot-stm32`]: it re-exports
//! the generic [`embassy_boot`] types and adds the two things a CH32 needs to
//! actually boot an application:
//!
//! * [`CoarseFlash`] - presents the CH32 internal flash (which is erased in
//!   256 byte pages) with a coarser erase granularity, which is required by the
//!   `embassy-boot` swap algorithm to keep the state partition reasonably small.
//! * [`BootLoader::load`] - hands control over to the application running in the
//!   active partition. On QingKe cores the application re-initializes `mtvec`,
//!   the global pointer and the stack pointer itself during its startup code, so
//!   all the bootloader has to do is to mask interrupts and jump.
//!
//! Note that the flash driver in [ch32-hal] is only implemented for the `v3`
//! flash IP used by the CH32V2 and CH32V3 lines (CH32V203, CH32V208, CH32V303,
//! CH32V305, CH32V307, ...). For the older parts (`v0`, `v1`, `x0`, `l1`) the
//! driver compiles to `unimplemented!()` stubs, so a runtime bootloader cannot
//! be built for them yet.
//!
//! The partition geometry is expected to be described by the usual
//! `__bootloader_active_start`/`__bootloader_dfu_start`/`__bootloader_state_start`
//! (and matching `_end`) linker symbols, holding offsets from the start of
//! internal flash, exactly like in the `embassy-boot` examples. See
//! [`BootLoaderConfig::from_linkerfile_blocking`] and
//! `FirmwareUpdaterConfig::from_linkerfile_blocking()` (the latter is only
//! available for `target_os = "none"`, so it is not linked here).
//!
//! ```ignore
//! use core::cell::RefCell;
//!
//! use ch32_hal::flash::{Blocking, Flash};
//! use embassy_boot_ch32::{BootLoader, BootLoaderConfig, CoarseFlash};
//! use embassy_sync::blocking_mutex::raw::NoopRawMutex;
//! use embassy_sync::blocking_mutex::Mutex;
//!
//! type BootFlash = CoarseFlash<Flash<'static, Blocking>, 8192>;
//!
//! let flash: Mutex<NoopRawMutex, RefCell<BootFlash>> =
//!     Mutex::new(RefCell::new(CoarseFlash(Flash::new_blocking(p.FLASH))));
//!
//! let config = BootLoaderConfig::from_linkerfile_blocking(&flash, &flash, &flash);
//! let loader = BootLoader::prepare::<_, _, _, 1024>(config);
//!
//! unsafe { loader.load(embassy_boot_ch32::active_start()) }
//! ```
//!
//! The [repository](https://github.com/alexxy/embassy-boot-ch32) contains a
//! complete example pair: a blocking
//! [bootloader](https://github.com/alexxy/embassy-boot-ch32/tree/main/examples/bootloader)
//! and an embassy
//! [application](https://github.com/alexxy/embassy-boot-ch32/tree/main/examples/application),
//! documented in
//! its [README](https://github.com/alexxy/embassy-boot-ch32/blob/main/README.md)
//! (chip matrix, partition maps, serial console and USB DFU transports). The
//! examples are standalone workspaces, so they are not part of these docs and
//! every link here points at GitHub.
//!
//! [`embassy-boot-stm32`]: https://docs.rs/embassy-boot-stm32
//! [ch32-hal]: https://github.com/ch32-rs/ch32-hal

#![no_std]
#![warn(missing_docs)]

use embedded_storage::nor_flash::{ErrorType, NorFlash, ReadNorFlash};

pub use embassy_boot::{
    AlignedBuffer, BlockingFirmwareState, BlockingFirmwareUpdater, BootError, BootLoaderConfig,
    FirmwareState, FirmwareUpdater, FirmwareUpdaterConfig, FirmwareUpdaterError, State,
};
pub use embedded_storage;

/// Address of the beginning of the internal flash array as seen by the core.
///
/// All offsets used by the flash driver, by the partition symbols and by
/// [`BootLoader::load`] are relative to this address.
pub const FLASH_BASE: u32 = 0x0800_0000;

/// The PFIC configuration register, used to trigger a system reset.
const PFIC_CFGR: *mut u32 = 0xE000_E048 as *mut u32;
/// Write key required by [`PFIC_CFGR`].
const PFIC_KEY3: u32 = 0xBEEF_0000;
/// `SYSRESETREQ` bit of [`PFIC_CFGR`].
const PFIC_SYSRESET: u32 = 1 << 7;

unsafe extern "C" {
    static __bootloader_active_start: u32;
    static __bootloader_active_end: u32;
    static __bootloader_dfu_start: u32;
    static __bootloader_dfu_end: u32;
    static __bootloader_state_start: u32;
    static __bootloader_state_end: u32;
}

/// Offset of the active partition from [`FLASH_BASE`], as described by the
/// `__bootloader_active_start` linker symbol.
///
/// This is the value that should be handed to [`BootLoader::load`].
pub fn active_start() -> u32 {
    unsafe { &__bootloader_active_start as *const u32 as u32 }
}

/// End offset (exclusive) of the active partition, see [`active_start`].
pub fn active_end() -> u32 {
    unsafe { &__bootloader_active_end as *const u32 as u32 }
}

/// Size of the active partition in bytes.
pub fn active_size() -> u32 {
    active_end() - active_start()
}

/// Offset of the dfu partition from [`FLASH_BASE`], as described by the
/// `__bootloader_dfu_start` linker symbol.
pub fn dfu_start() -> u32 {
    unsafe { &__bootloader_dfu_start as *const u32 as u32 }
}

/// End offset (exclusive) of the dfu partition, see [`dfu_start`].
pub fn dfu_end() -> u32 {
    unsafe { &__bootloader_dfu_end as *const u32 as u32 }
}

/// Size of the dfu partition in bytes.
///
/// An incoming image larger than this will not fit in the dfu partition.
/// Note that [`embassy_boot::BlockingFirmwareUpdater::write_firmware`] happily
/// asserts on that, so checking the length before writing is recommended.
pub fn dfu_size() -> u32 {
    dfu_end() - dfu_start()
}

/// Offset of the state partition from [`FLASH_BASE`], as described by the
/// `__bootloader_state_start` linker symbol.
pub fn state_start() -> u32 {
    unsafe { &__bootloader_state_start as *const u32 as u32 }
}

/// End offset (exclusive) of the state partition, see [`state_start`].
pub fn state_end() -> u32 {
    unsafe { &__bootloader_state_end as *const u32 as u32 }
}

/// Size of the state partition in bytes.
pub fn state_size() -> u32 {
    state_end() - state_start()
}

/// Masks all maskable interrupts.
///
/// Clears `mie` (all individual interrupt enables) and `mstatus.MIE` (the global
/// enable, which on QingKe cores is aliased by the `gintenr` CSR).
///
/// Peripheral interrupt enables are *not* touched, so a peripheral that was
/// configured by the current program keeps requesting service once interrupts
/// are enabled again. Use [`system_reset`] to get a clean state.
pub fn disable_interrupts() {
    #[cfg(target_arch = "riscv32")]
    unsafe {
        core::arch::asm!("csrw mie, zero", "csrc mstatus, 8", options(nostack));
    }
}

/// Requests a system reset through the PFIC and waits for it.
///
/// This is the QingKe equivalent of `NVIC_SystemReset()` from the WCH EVT and is
/// the recommended way for an application to enter the bootloader after it has
/// set the DFU state: unlike a plain jump it also clears the state of every
/// peripheral.
pub fn system_reset() -> ! {
    disable_interrupts();
    unsafe {
        core::ptr::write_volatile(PFIC_CFGR, PFIC_KEY3 | PFIC_SYSRESET);
    }
    loop {
        core::hint::spin_loop();
    }
}

/// A flash adapter that reports a coarser erase granularity than the underlying
/// driver.
///
/// The CH32 internal flash is erased in single pages of `WRITE_SIZE` bytes (256
/// on the CH32V30x family), and `ch32-hal` therefore reports
/// `ERASE_SIZE == WRITE_SIZE == 256`. `embassy-boot` uses
/// `PAGE_SIZE = max(ACTIVE::ERASE_SIZE, DFU::ERASE_SIZE)` both as the unit of
/// the copy/swap algorithm and as the divisor of its state size requirement:
///
/// ```text
/// 2 + 4 * (active_size / PAGE_SIZE) <= state_size / STATE::WRITE_SIZE
/// ```
///
/// With a 256 byte page and a 48 KiB application that would need a state
/// partition of nearly 200 KiB, which no CH32V305 has. Wrapping the flash in a
/// `CoarseFlash<_, 8192>` makes the swap algorithm work in 8 KiB blocks, which
/// reduces the state requirement to a few 8 KiB blocks while only costing a few
/// extra page erase commands.
///
/// Erases are performed as a sequence of hardware erases of `F::ERASE_SIZE`
/// bytes; everything else is passed through unchanged.
pub struct CoarseFlash<F, const ERASE_SIZE: usize>(pub F);

impl<F, const ERASE_SIZE: usize> CoarseFlash<F, ERASE_SIZE> {
    /// Get a reference to the wrapped flash driver.
    pub fn inner(&self) -> &F {
        &self.0
    }

    /// Get a mutable reference to the wrapped flash driver.
    pub fn inner_mut(&mut self) -> &mut F {
        &mut self.0
    }

    /// Unwrap the adapter, returning the wrapped flash driver.
    pub fn into_inner(self) -> F {
        self.0
    }
}

impl<F, const ERASE_SIZE: usize> ErrorType for CoarseFlash<F, ERASE_SIZE>
where
    F: ErrorType,
{
    type Error = F::Error;
}

impl<F, const ERASE_SIZE: usize> ReadNorFlash for CoarseFlash<F, ERASE_SIZE>
where
    F: ReadNorFlash,
{
    const READ_SIZE: usize = F::READ_SIZE;

    fn read(&mut self, offset: u32, bytes: &mut [u8]) -> Result<(), Self::Error> {
        self.0.read(offset, bytes)
    }

    fn capacity(&self) -> usize {
        self.0.capacity()
    }
}

impl<F, const ERASE_SIZE: usize> NorFlash for CoarseFlash<F, ERASE_SIZE>
where
    F: NorFlash,
{
    const WRITE_SIZE: usize = F::WRITE_SIZE;
    const ERASE_SIZE: usize = ERASE_SIZE;

    fn erase(&mut self, from: u32, to: u32) -> Result<(), Self::Error> {
        const {
            assert!(ERASE_SIZE > 0, "CoarseFlash erase size must not be zero");
            assert!(
                ERASE_SIZE.is_multiple_of(F::ERASE_SIZE),
                "CoarseFlash erase size must be a multiple of the hardware erase size"
            );
        }

        assert!(
            from.is_multiple_of(ERASE_SIZE as u32) && to.is_multiple_of(ERASE_SIZE as u32),
            "erase range must be aligned to the coarse erase size"
        );
        assert!(from < to, "erase range must not be empty");

        let mut address = from;
        while address < to {
            self.0.erase(address, address + F::ERASE_SIZE as u32)?;
            address += F::ERASE_SIZE as u32;
        }
        Ok(())
    }

    fn write(&mut self, offset: u32, bytes: &[u8]) -> Result<(), Self::Error> {
        self.0.write(offset, bytes)
    }
}

/// A bootloader for CH32 devices.
pub struct BootLoader {
    /// The reported state of the bootloader after preparing for boot
    pub state: State,
}

impl BootLoader {
    /// Inspect the bootloader state and perform actions required before booting, such as swapping firmware.
    ///
    /// Panics if the swap can not be performed, see [`BootLoader::try_prepare`] for the fallible version.
    pub fn prepare<ACTIVE: NorFlash, DFU: NorFlash, STATE: NorFlash, const BUFFER_SIZE: usize>(
        config: BootLoaderConfig<ACTIVE, DFU, STATE>,
    ) -> Self {
        match Self::try_prepare::<ACTIVE, DFU, STATE, BUFFER_SIZE>(config) {
            Ok(loader) => loader,
            // Keep the panic message free of `{e:?}` so that this also works when
            // `BootError` does not implement `Debug` in some future version.
            Err(_) => panic!("Boot prepare error"),
        }
    }

    /// Inspect the bootloader state and perform actions required before booting, such as swapping firmware.
    pub fn try_prepare<
        ACTIVE: NorFlash,
        DFU: NorFlash,
        STATE: NorFlash,
        const BUFFER_SIZE: usize,
    >(
        config: BootLoaderConfig<ACTIVE, DFU, STATE>,
    ) -> Result<Self, BootError> {
        let mut aligned_buf = AlignedBuffer([0; BUFFER_SIZE]);
        let mut boot = embassy_boot::BootLoader::new(config);
        let state = boot.prepare_boot(aligned_buf.as_mut())?;
        Ok(Self { state })
    }

    /// Boots the application.
    ///
    /// `start` is an offset from the start of internal flash (the same numbering
    /// as the `__bootloader_*` symbols), *not* a bus address, so
    /// `loader.load(active_start())` jumps to `FLASH_BASE + active_start()`.
    ///
    /// All interrupts are masked before jumping. The application startup code of
    /// `qingke-rt` sets up the stack pointer, the global pointer, `mtvec` and the
    /// interrupt controller itself, so nothing else has to be done.
    ///
    /// # Safety
    ///
    /// This jumps to the code (and uses the stack) placed in the active
    /// partition. The caller must make sure a valid application has been
    /// programmed there.
    pub unsafe fn load(self, start: u32) -> ! {
        #[cfg(target_arch = "riscv32")]
        unsafe {
            let entry = FLASH_BASE + start;
            core::arch::asm!(
                "csrw mie, zero",
                "csrc mstatus, 8",
                "jr {0}",
                in(reg) entry,
                options(noreturn, nostack),
            );
        }

        #[cfg(not(target_arch = "riscv32"))]
        {
            let _ = start;
            unimplemented!("embassy-boot-ch32 can only load firmware on riscv32");
        }
    }
}
