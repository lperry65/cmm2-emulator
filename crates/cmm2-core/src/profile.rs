//! What each known firmware keeps in flash across a power cycle, so that the settings file holds
//! exactly that and nothing of the firmware's code. See issue 82.

use crate::ds3231::STATE_SIZE;

/// A run of flash that the firmware writes and the user's settings file must keep.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Region {
    pub addr: u32,
    pub len: u32,
}

/// A firmware image I know, and the flash regions it keeps.
#[derive(Debug, PartialEq, Eq)]
pub struct Profile {
    pub name: &'static str,
    /// The short name that a person gives it: `b5`.
    pub key: &'static str,
    /// The regions in the order the settings file holds them.
    pub regions: &'static [Region],
    /// The first region is one that settings files from before it was kept do not have.
    pub first_is_new: bool,
    image_len: usize,
    image_hash: u64,
}

// Each firmware keeps a 32-byte block inside its own image. At boot it reads 8 bytes of it, and if
// they are FF, as they are in the image, it takes the first-start path: it wipes the function
// key strings and erases the stored program. The first run writes the block, so a settings file
// has to carry it or the next run loses both. These are the block's addresses.
const B5_BLOCK: Region = Region {
    addr: 0x080F_8540,
    len: 0x20,
};
const RC6_BLOCK: Region = Region {
    addr: 0x080F_6F00,
    len: 0x20,
};

/// What 7.00.00b5 keeps: the 32-byte block, then the stored program and saved options, which are
/// the last 896 KB of flash.
pub static B5: Profile = Profile {
    name: "7.00.00b5",
    key: "b5",
    regions: &[
        B5_BLOCK,
        Region {
            addr: 0x0812_0000,
            len: 0xE_0000,
        },
    ],
    first_is_new: true,
    image_len: 1_179_432,
    image_hash: 0x132b_0092_b085_69f9,
};

/// The options of 7.00.00RC6 are the first 4 KB of the sector at 0x0814_0000, which is inside its
/// image (every save erases the sector and writes its tail back from RAM). The rest of the
/// sector is code, so a settings file must not hold it, or loading it would put an old firmware
/// over a newer one. The program flash is from 0x0816_0000 to the end of flash.
pub static RC6: Profile = Profile {
    name: "7.00.00RC6",
    key: "rc6",
    regions: &[
        RC6_BLOCK,
        Region {
            addr: 0x0814_0000,
            len: 0x1000,
        },
        Region {
            addr: 0x0816_0000,
            len: 0xA_0000,
        },
    ],
    first_is_new: false,
    image_len: 1_409_568,
    image_hash: 0x5bfb_cced_328c_674e,
};

/// The clock's time and date in a settings file: TR and DR, a word each, which a real board's
/// battery keeps going. They come after the flash regions.
pub(crate) const CLOCK_SIZE: usize = 8;

/// The DS3231's state (`Ds3231Slot::saved`) after the clock, once the guest has set its time or
/// cleared its status.
pub(crate) const WITH_DS3231_SIZE: usize = CLOCK_SIZE + STATE_SIZE;

/// What the first version with a DS3231 saved after the clock: the DS3231's offset from the
/// host's local time, in seconds, an i64.
pub(crate) const VERSION_1_SIZE: usize = CLOCK_SIZE + 8;

static KNOWN: [&Profile; 2] = [&B5, &RC6];

/// FNV-1a over the bytes. It is only to tell my two images from any other, not to guard against
/// anyone: no crate for SHA-256 is in the build, and the length with this hash of every byte is
/// as good for that.
fn fnv1a(bytes: &[u8]) -> u64 {
    bytes.iter().fold(0xcbf2_9ce4_8422_2325, |hash, &byte| {
        (hash ^ u64::from(byte)).wrapping_mul(0x0100_0000_01b3)
    })
}

impl Profile {
    /// The profile of the image, if it is one I know.
    pub fn identify(image: &[u8]) -> Option<&'static Profile> {
        KNOWN
            .into_iter()
            .find(|p| p.image_len == image.len() && p.image_hash == fnv1a(image))
    }

    /// The profile with this name (`b5` or `rc6`, in any case), if I know it.
    pub fn named(name: &str) -> Option<&'static Profile> {
        KNOWN.into_iter().find(|p| p.key.eq_ignore_ascii_case(name))
    }

    /// The bytes the regions hold together.
    pub fn flash_size(&self) -> usize {
        regions_size(self.regions)
    }

    /// The regions of a settings file from before the first one was kept, if there are any.
    pub fn old_regions(&self) -> Option<&'static [Region]> {
        self.first_is_new.then(|| &self.regions[1..])
    }

    /// The size of the settings file of a board whose DS3231 is as it was made: the regions,
    /// then the clock's time and date.
    pub fn persistent_size(&self) -> usize {
        self.flash_size() + CLOCK_SIZE
    }

    /// The regions that a settings file of this length holds, if it is one of this profile's
    /// sizes: all of them, or all but the first for a file from before that was kept.
    pub fn regions_for(&self, len: usize) -> Option<&'static [Region]> {
        std::iter::once(self.regions)
            .chain(self.old_regions())
            .find(|regions| {
                len.checked_sub(regions_size(regions)).is_some_and(|rest| {
                    [CLOCK_SIZE, VERSION_1_SIZE, WITH_DS3231_SIZE].contains(&rest)
                })
            })
    }

    /// Another profile whose settings files are this long, if there is one.
    pub fn other_with_size(&self, len: usize) -> Option<&'static Profile> {
        KNOWN
            .into_iter()
            .find(|p| p.key != self.key && p.regions_for(len).is_some())
    }
}

pub(crate) fn regions_size(regions: &[Region]) -> usize {
    regions.iter().map(|region| region.len as usize).sum()
}

#[cfg(test)]
mod tests;
