// Chip table shared by the `build.rs` of both examples (they `include!` this
// file, so the two binaries can never disagree about which chip maps to which
// partition table or which target it needs).
//
// A chip is selectable here when `ch32-hal` implements its flash controller,
// i.e. when `ch32-metapac` reports `flash` IP version `v3`. That is the case
// for the CH32V2 and CH32V3 lines; the V1, V0, X0 and L1 flash controllers are
// still `unimplemented!()` in `ch32-hal/src/flash/other.rs`, so the parts of
// those families are deliberately absent.
//
// The partition maps live in `../partition-map` and are named after the
// geometry (`flash<nominal application flash>-ram<sram>.x`), because several
// parts share one geometry. The USB and CAN bootloader need a bigger
// bootloader partition than the serial one, so parts that can host them get a
// second map with a `-usb` or `-can` suffix.

/// One selectable part.
pub struct Chip {
    /// The cargo feature name, which is also the lower case part number used by
    /// `ch32-hal` and `ch32-metapac`.
    pub part: &'static str,
    /// The partition map in `partition-map/`.
    pub map: &'static str,
    /// The rust target the part is built for. Note that the CH32V2 parts have
    /// no `F` extension.
    pub target: &'static str,
    /// The `ch32-hal` USB controller of the part: `""`, `"usbd"`, `"otg_fs"` or
    /// `"usbhs"`. Empty when the part only has the `usb/v2fs` block, for which
    /// `ch32-hal` implements no driver.
    pub usb: &'static str,
    /// The partition map used by the `transport-usb` bootloader, empty when the
    /// part cannot host a USB bootloader.
    pub map_usb: &'static str,
    /// The part has a bxCAN controller that `ch32-hal` drives. The CH32V203F8P6,
    /// CH32V203F8U6 and CH32V305FBP6 have no CAN peripheral at all.
    pub can: bool,
    /// The partition map used by the `transport-can` bootloader, empty when the
    /// part has no CAN controller or too little flash to pair a CAN bootloader
    /// with a usable application partition.
    pub map_can: &'static str,
}

const IMC: &str = "riscv32imc-unknown-none-elf";
const IMFC: &str = "riscv32imfc-unknown-none-elf";

const FLASH64K_RAM20K: &str = "flash64k-ram20k.x";
const FLASH128K_RAM32K: &str = "flash128k-ram32k.x";
const FLASH128K_RAM64K: &str = "flash128k-ram64k.x";
const FLASH256K_RAM64K: &str = "flash256k-ram64k.x";

const FLASH128K_RAM32K_USB: &str = "flash128k-ram32k-usb.x";
const FLASH128K_RAM64K_USB: &str = "flash128k-ram64k-usb.x";
const FLASH256K_RAM64K_USB: &str = "flash256k-ram64k-usb.x";

const FLASH128K_RAM32K_CAN: &str = "flash128k-ram32k-can.x";
const FLASH128K_RAM64K_CAN: &str = "flash128k-ram64k-can.x";
const FLASH256K_RAM64K_CAN: &str = "flash256k-ram64k-can.x";

pub const CHIPS: &[Chip] = &[
    // CH32V203 (QingKe V4B, no FPU, 256 byte flash pages)
    Chip {
        part: "ch32v203c8t6",
        map: FLASH64K_RAM20K,
        target: IMC,
        usb: "usbd",
        map_usb: "",
        can: true,
        // 64 KiB of flash cannot hold a 32 KiB bootloader and still leave room
        // for an active and a dfu partition.
        map_can: "",
    },
    Chip {
        part: "ch32v203c8u6",
        map: FLASH64K_RAM20K,
        target: IMC,
        usb: "usbd",
        map_usb: "",
        can: true,
        map_can: "",
    },
    // The only CH32V203 without a USB controller at all.
    Chip {
        part: "ch32v203f8p6",
        map: FLASH64K_RAM20K,
        target: IMC,
        usb: "",
        map_usb: "",
        can: false,
        map_can: "",
    },
    Chip {
        part: "ch32v203f8u6",
        map: FLASH64K_RAM20K,
        target: IMC,
        usb: "usbd",
        map_usb: "",
        can: false,
        map_can: "",
    },
    Chip {
        part: "ch32v203g8r6",
        map: FLASH64K_RAM20K,
        target: IMC,
        usb: "usbd",
        map_usb: "",
        can: true,
        map_can: "",
    },
    Chip {
        part: "ch32v203k8t6",
        map: FLASH64K_RAM20K,
        target: IMC,
        usb: "usbd",
        map_usb: "",
        can: true,
        map_can: "",
    },
    // NOTE: CH32V203RBT6 is deliberately absent. It is the only CH32V203 part
    // with a 32-bit general purpose timer (TIM5, a `GPTM32` in ch32-metapac),
    // and ch32-hal does not compile for such a part outside CH32V208/CH32L1:
    // `TimerBits::Bits32` is `#[cfg(any(ch32l1, ch32v208))]` while the
    // `foreach_interrupt!` arm for `timer, GPTM32, UP` uses it unconditionally
    // (ch32-hal `src/timer/mod.rs`).
    //
    // CH32V208 (QingKe V4C + Bluetooth, no FPU)
    Chip {
        part: "ch32v208cbu6",
        map: FLASH128K_RAM64K,
        target: IMC,
        usb: "usbd",
        map_usb: FLASH128K_RAM64K_USB,
        can: true,
        map_can: FLASH128K_RAM64K_CAN,
    },
    Chip {
        part: "ch32v208gbu6",
        map: FLASH128K_RAM64K,
        target: IMC,
        usb: "usbd",
        map_usb: FLASH128K_RAM64K_USB,
        can: true,
        map_can: FLASH128K_RAM64K_CAN,
    },
    Chip {
        part: "ch32v208rbt6",
        map: FLASH128K_RAM64K,
        target: IMC,
        usb: "usbd",
        map_usb: FLASH128K_RAM64K_USB,
        can: true,
        map_can: FLASH128K_RAM64K_CAN,
    },
    Chip {
        part: "ch32v208wbu6",
        map: FLASH128K_RAM64K,
        target: IMC,
        usb: "usbd",
        map_usb: FLASH128K_RAM64K_USB,
        can: true,
        map_can: FLASH128K_RAM64K_CAN,
    },
    // CH32V303 (QingKe V4F). The `usb/v2fs` block of this family has no driver
    // in ch32-hal, so none of these parts can do USB.
    Chip {
        part: "ch32v303cbt6",
        map: FLASH128K_RAM32K,
        target: IMFC,
        usb: "",
        map_usb: "",
        can: true,
        map_can: FLASH128K_RAM32K_CAN,
    },
    Chip {
        part: "ch32v303rbt6",
        map: FLASH128K_RAM32K,
        target: IMFC,
        usb: "",
        map_usb: "",
        can: true,
        map_can: FLASH128K_RAM32K_CAN,
    },
    Chip {
        part: "ch32v303rct6",
        map: FLASH256K_RAM64K,
        target: IMFC,
        usb: "",
        map_usb: "",
        can: true,
        map_can: FLASH256K_RAM64K_CAN,
    },
    Chip {
        part: "ch32v303vct6",
        map: FLASH256K_RAM64K,
        target: IMFC,
        usb: "",
        map_usb: "",
        can: true,
        map_can: FLASH256K_RAM64K_CAN,
    },
    // CH32V305 (QingKe V4F, USB OTG HS).
    //
    // `ch32v305rbt6` and the CH32V307 parts have both OTG_FS and USBHS; OTG_FS
    // is preferred because that is the controller on the ISP/USB-C connector of
    // the nanoCH32V305, so one cable does both flashing and DFU.
    Chip {
        part: "ch32v305fbp6",
        map: FLASH128K_RAM32K,
        target: IMFC,
        usb: "usbhs",
        map_usb: FLASH128K_RAM32K_USB,
        can: false,
        map_can: "",
    },
    Chip {
        part: "ch32v305gbu6",
        map: FLASH128K_RAM32K,
        target: IMFC,
        usb: "usbhs",
        map_usb: FLASH128K_RAM32K_USB,
        can: true,
        map_can: FLASH128K_RAM32K_CAN,
    },
    Chip {
        part: "ch32v305rbt6",
        map: FLASH128K_RAM32K,
        target: IMFC,
        usb: "otg_fs",
        map_usb: FLASH128K_RAM32K_USB,
        can: true,
        map_can: FLASH128K_RAM32K_CAN,
    },
    // CH32V307 (QingKe V4F, Ethernet)
    Chip {
        part: "ch32v307rct6",
        map: FLASH256K_RAM64K,
        target: IMFC,
        usb: "otg_fs",
        map_usb: FLASH256K_RAM64K_USB,
        can: true,
        map_can: FLASH256K_RAM64K_CAN,
    },
    Chip {
        part: "ch32v307vct6",
        map: FLASH256K_RAM64K,
        target: IMFC,
        usb: "otg_fs",
        map_usb: FLASH256K_RAM64K_USB,
        can: true,
        map_can: FLASH256K_RAM64K_CAN,
    },
    Chip {
        part: "ch32v307wcu6",
        map: FLASH256K_RAM64K,
        target: IMFC,
        usb: "otg_fs",
        map_usb: FLASH256K_RAM64K_USB,
        can: true,
        map_can: FLASH256K_RAM64K_CAN,
    },
];

/// The one chip selected through a cargo feature.
///
/// `example` is only used to make the error messages point at the right
/// `Cargo.toml`.
pub fn selected(example: &str) -> &'static Chip {
    let mut picked = None;

    for (name, value) in std::env::vars() {
        if value != "1" {
            continue;
        }
        let Some(feature) = name.strip_prefix("CARGO_FEATURE_") else {
            continue;
        };
        let feature = feature.to_ascii_lowercase();
        if !feature.starts_with("ch32") {
            continue;
        }
        match CHIPS.iter().find(|chip| chip.part == feature) {
            Some(chip) => {
                if let Some(previous) = picked.replace(chip) {
                    panic!(
                        "{example}: features `{}` and `{}` are both enabled; enable exactly one \
                         chip feature, e.g. `cargo build --no-default-features --features {}`",
                        previous.part, chip.part, chip.part
                    );
                }
            }
            // Not one of our chip features (the `dep/feature` style features of
            // ch32-hal do not show up here, only this package's own do).
            None => panic!(
                "{example}: unknown chip feature `{feature}`; this build script does not know \
                 that part, see CHIPS in examples/chips.rs"
            ),
        }
    }

    match picked {
        Some(chip) => chip,
        None => panic!(
            "{example}: no chip feature enabled. Pick one of: {}. The package enables \
             `ch32v305rbt6` by default, so `--no-default-features` without a replacement is not \
             enough.",
            CHIPS
                .iter()
                .map(|chip| chip.part)
                .collect::<Vec<_>>()
                .join(", ")
        ),
    }
}

/// Fails the build when `--target` and the selected chip do not agree.
pub fn check_target(chip: &Chip) {
    let target = std::env::var("TARGET").unwrap_or_default();
    // A JSON target spec is passed as `foo.json`, on the command line as well as
    // in `.cargo/config.toml`.
    let target = target.strip_suffix(".json").unwrap_or(&target);

    if target != chip.target {
        panic!(
            "{} is built for `{}`, but `--target {}` was requested. Build it with \
             `--target {}` (see the target table in the README).",
            chip.part, chip.target, target, chip.target
        );
    }
}
