//! The Vulkan drivers of the GPU that drives the display, found in sysfs.
//!
//! On a hybrid laptop the Vulkan loader lists every driver when a renderer
//! starts, and listing the driver of a runtime-suspended discrete card wakes
//! it. The firmware marks the GPU it used for the console with
//! `device/boot_vga` = `1`, and the compositor takes that GPU as its primary
//! one; [`display_drivers`] names the loader's driver globs for it, so that
//! only that driver is listed. Only sysfs attributes are read: nothing here
//! opens a device or could wake a card.

use std::{
    fs::{self, File},
    io::{self, Read},
    path::Path,
};

/// The PCI vendor of Intel.
const INTEL: u16 = 0x8086;

/// The PCI vendor of AMD.
const AMD: u16 = 0x1002;

/// The loader's driver globs of Intel: the current driver and the one of the
/// older generations.
pub const INTEL_DRIVERS: &str = "intel_icd*,intel_hasvk_icd*";

/// The loader's driver glob of AMD (RADV).
pub const AMD_DRIVERS: &str = "radeon_icd*";

/// How many bytes of a sysfs attribute are read: `0x8086` and its line break
/// are seven.
const ATTRIBUTE_LIMIT: u64 = 16;

/// One DRM card as sysfs shows it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Card {
    /// The PCI vendor of its device; `None` when it cannot be read.
    pub vendor: Option<u16>,
    /// Whether the firmware used its device for the console.
    pub boot_vga: bool,
}

/// The driver globs of the GPU that drives the display, from the DRM class
/// directory `drm` (`/sys/class/drm`), or `None` when the loader should list
/// every driver: see [`drivers_for`].
pub fn display_drivers(drm: &Path) -> Option<&'static str> {
    drivers_for(&cards(drm).ok()?)
}

/// The driver globs for `cards`: those of the one card marked `boot_vga`
/// when there are two cards or more and it is Intel or AMD; `None` for one
/// card, for no or several marked cards, and for any other vendor.
pub fn drivers_for(cards: &[Card]) -> Option<&'static str> {
    if cards.len() < 2 {
        return None;
    }
    let mut marked = cards.iter().filter(|card| card.boot_vga);
    let card = marked.next()?;
    if marked.next().is_some() {
        return None;
    }
    match card.vendor? {
        INTEL => Some(INTEL_DRIVERS),
        AMD => Some(AMD_DRIVERS),
        _ => None,
    }
}

/// The cards in `drm`: its entries `card<N>`, connectors such as
/// `card1-eDP-1` aside.
fn cards(drm: &Path) -> io::Result<Vec<Card>> {
    let mut cards = Vec::new();
    for entry in fs::read_dir(drm)? {
        let entry = entry?;
        let name = entry.file_name();
        let Some(number) = name.to_str().and_then(|name| name.strip_prefix("card")) else {
            continue;
        };
        if number.is_empty() || !number.bytes().all(|byte| byte.is_ascii_digit()) {
            continue;
        }
        let device = entry.path().join("device");
        cards.push(Card {
            vendor: read_attribute(&device.join("vendor"))
                .ok()
                .and_then(|vendor| parse_vendor(vendor.trim())),
            boot_vga: read_attribute(&device.join("boot_vga"))
                .is_ok_and(|boot_vga| boot_vga.trim() == "1"),
        });
    }
    Ok(cards)
}

/// A vendor as sysfs writes it, `0x` and four hexadecimal digits.
fn parse_vendor(text: &str) -> Option<u16> {
    u16::from_str_radix(text.strip_prefix("0x")?, 16).ok()
}

/// The first bytes of the regular file at `path`, as text.
fn read_attribute(path: &Path) -> io::Result<String> {
    // Before opening: opening a pipe would wait for its writer.
    if !fs::metadata(path)?.is_file() {
        return Err(io::ErrorKind::InvalidInput.into());
    }
    let mut content = String::new();
    File::open(path)?
        .take(ATTRIBUTE_LIMIT)
        .read_to_string(&mut content)?;
    Ok(content)
}

#[cfg(test)]
mod tests {
    use std::path::{Path, PathBuf};

    use super::{AMD_DRIVERS, Card, INTEL_DRIVERS, display_drivers, drivers_for};

    /// A DRM class directory of one test, removed when the test ends.
    struct Drm(PathBuf);

    impl Drm {
        fn new(tag: &str) -> Self {
            let path =
                std::env::temp_dir().join(format!("addict-gpu-{tag}-{}", std::process::id()));
            let _ = std::fs::remove_dir_all(&path);
            std::fs::create_dir_all(&path).expect("the directory of the test");
            Self(path)
        }

        /// A card `name` whose device has `vendor` and, when given, `boot_vga`.
        fn card(&self, name: &str, vendor: &str, boot_vga: Option<&str>) -> &Self {
            let device = self.0.join(name).join("device");
            std::fs::create_dir_all(&device).expect("the device");
            std::fs::write(device.join("vendor"), vendor).expect("the vendor");
            if let Some(boot_vga) = boot_vga {
                std::fs::write(device.join("boot_vga"), boot_vga).expect("boot_vga");
            }
            self
        }

        /// A connector `name`, with nothing inside.
        fn connector(&self, name: &str) -> &Self {
            std::fs::create_dir_all(self.0.join(name)).expect("the connector");
            self
        }

        fn path(&self) -> &Path {
            &self.0
        }
    }

    impl Drop for Drm {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn card(vendor: u16, boot_vga: bool) -> Card {
        Card {
            vendor: Some(vendor),
            boot_vga,
        }
    }

    #[test]
    fn the_marked_intel_or_amd_card_of_two_gives_its_drivers() {
        assert_eq!(
            drivers_for(&[card(0x10de, false), card(0x8086, true)]),
            Some(INTEL_DRIVERS)
        );
        assert_eq!(
            drivers_for(&[card(0x1002, true), card(0x10de, false)]),
            Some(AMD_DRIVERS)
        );
    }

    #[test]
    fn any_other_case_gives_nothing() {
        // NVIDIA drives the display.
        assert_eq!(
            drivers_for(&[card(0x10de, true), card(0x8086, false)]),
            None
        );
        // One GPU, and none.
        assert_eq!(drivers_for(&[card(0x8086, true)]), None);
        assert_eq!(drivers_for(&[]), None);
        // No card marked, and two.
        assert_eq!(
            drivers_for(&[card(0x10de, false), card(0x8086, false)]),
            None
        );
        assert_eq!(drivers_for(&[card(0x1002, true), card(0x8086, true)]), None);
        // A marked card of an unknown vendor.
        let unknown = Card {
            vendor: None,
            boot_vga: true,
        };
        assert_eq!(drivers_for(&[card(0x10de, false), unknown]), None);
    }

    #[test]
    fn a_hybrid_tree_with_intel_on_the_display_gives_intel() {
        let drm = Drm::new("intel");
        drm.card("card0", "0x10de\n", Some("0\n"))
            .connector("card0-DP-1")
            .connector("card0-eDP-2")
            .card("card1", "0x8086\n", Some("1\n"))
            .connector("card1-eDP-1")
            .connector("renderD128")
            .connector("version");
        assert_eq!(display_drivers(drm.path()), Some(INTEL_DRIVERS));
    }

    #[test]
    fn a_hybrid_tree_with_amd_on_the_display_gives_amd() {
        let drm = Drm::new("amd");
        drm.card("card0", "0x1002\n", Some("1\n"))
            .card("card1", "0x10de\n", Some("0\n"));
        assert_eq!(display_drivers(drm.path()), Some(AMD_DRIVERS));
    }

    #[test]
    fn a_tree_with_nvidia_on_the_display_one_card_or_no_mark_gives_nothing() {
        let nvidia = Drm::new("nvidia");
        nvidia
            .card("card0", "0x10de\n", Some("1\n"))
            .card("card1", "0x8086\n", Some("0\n"));
        assert_eq!(display_drivers(nvidia.path()), None);

        let one = Drm::new("one");
        one.card("card0", "0x8086\n", Some("1\n"))
            .connector("card0-eDP-1");
        assert_eq!(display_drivers(one.path()), None);

        // A card without boot_vga, as a device that is not VGA has.
        let unmarked = Drm::new("unmarked");
        unmarked
            .card("card0", "0x10de\n", None)
            .card("card1", "0x8086\n", Some("0\n"));
        assert_eq!(display_drivers(unmarked.path()), None);

        let both = Drm::new("both");
        both.card("card0", "0x1002\n", Some("1\n"))
            .card("card1", "0x8086\n", Some("1\n"));
        assert_eq!(display_drivers(both.path()), None);

        // A vendor that cannot be read, and no tree at all.
        let garbled = Drm::new("garbled");
        garbled
            .card("card0", "0x10de\n", Some("0\n"))
            .card("card1", "intel\n", Some("1\n"));
        assert_eq!(display_drivers(garbled.path()), None);
        assert_eq!(display_drivers(&nvidia.path().join("missing")), None);
    }
}
