# The settings file format v2

This is the contract for `settings.flash` when the board runs a firmware the emulator
recognises (issue 82). The code is `crates/cmm2-core/src/settings.rs`, and this table is the
agreement: a reader accepts exactly what is written here, a writer writes exactly this, and a
change to either has to change this page.

The raw layout that scratch runs (`--firmware` with `--flash`) still read and write is described
in [usage.md](usage.md#the-settings-layout-per-firmware), and does not change here. A board's
v2 file and a raw file never mix: a raw run that is given a v2 file refuses it and says so, and
a board refuses a file that starts with the v2 magic but does not read as v2.

## Which file a run uses

Only the board of a firmware that `Profile::identify` recognised from its bytes uses v2. A
recognised firmware is one whose length and hash match a known image; a layout named by
`CMM2_SETTINGS_LAYOUT` or `CMM2_BOARD_LAYOUT_FOR_TESTS` is an assumption about an image I do not
know, and an assumption has no place in a header that states the image as fact. Everything else
- every scratch run, and a board of a labelled layout - keeps the raw file of before.

A v2 file is refused, never guessed at, when it is newer than this emulator, when it carries
flags or reserved bits I do not know, when its header does not name the image I am running, or
when it fails any check below. Nothing of a refused file is loaded, and it is left on disk.

## Header (fixed 48 bytes, little-endian)

```
  off  size  field
  0    4     magic  b"CMM2"
  4    1     format version = 2
  5    1     flags = 0 (write 0; a reader rejects non-zero)
  6    2     reserved = 0 (write 0; a reader rejects non-zero)
  8    4     image_len  - the baseline image length in bytes
  12   32    image_hash - SHA-256 of the baseline image
  44   4     entry_count
```

Why a hash of the image and no profile name: the running image identifies itself. If the hash
matches a profile I know I name it in messages; if not, the message names both hashes. A name
field could only lie, and so could `image_len` on its own, which is why a refusal names a release
only where the length and the hash together are one I know. `entry_count` is checked against the
number of 32-byte rows the firmware's regions hold before anything of the table is read, so a
four-byte lie cannot ask for gigabytes.

## The largest file either format can be

A v1 file is the regions and a tail. A v2 file is the header, 8 bytes per entry, the rows, a
tail and the checksum; entries neither overlap nor cross a region, so the rows together are
never more than the regions and there are never more entries than rows. The longest file either
format can be, over both releases, is therefore fixed: 1,146,990 bytes. A file larger than that
is refused on its size before a byte of it is read, so a stranger's 1 GiB file costs nothing
but a `stat`.

## Entries (entry_count of them)

```
  4 bytes  addr   (flash address, a multiple of 32)
  4 bytes  len    (bytes, a multiple of 32, at least 32)
  len bytes value
```

Entries are sorted by address and do not overlap. Every entry lies whole inside one region of
the firmware's profile. The 32-byte row is the unit because it is the STM32H743's programming
row; contiguous changed rows are stored as one entry.

## Tail and trailer

After the entries comes today's tail: 8 clock bytes (the RTC's time and date), plus the
10-byte DS3231 state when it is not as it was made. Those are the only two tail sizes; the v1
16-byte DS3231 offset form is converted on migration and never written again. The last 4 bytes
are the CRC-32 over everything before it: the one zip and gzip carry (CRC-32/ISO-HDLC - the
reflected IEEE polynomial, an all-ones start, a complemented end). It is not the one the POSIX
`cksum` command computes, which is a different checksum.

## What "diff-based" means, concretely

The baseline byte at address A is `image[A - 0x08000000]` inside the image, and FF past its
end, which is exactly what an erased flash reads. At save the emulator walks the profile's
regions row by row: a row that differs from the baseline becomes (or extends) an entry; a row
equal to the baseline is not written. At load, the machine is untouched beyond the entries: the
flash already reads the image, then FF - the same bytes the old full dump wrote back. The end
state is word for word the same; the file shrinks from ~900 KB (b5) to the few KB actually
changed. If every row of the regions differs, the entries carry every row, as the raw file does;
the header, table and checksum on top are why the bound above is bigger than the largest raw
file. The format degrades instead of switching modes, so there is no second mode.

Regions still decide what may be saved - only the profile's regions - which keeps the
self-healing property: a runaway guest write into code flash is dropped at save, not carried
forever. The diff only decides which rows inside those regions go in the file.

## Reading rules, in order

A check that fails stops the read; there is no fall-through to another kind of file.

1. No file: a first run, nothing to load.
2. Bigger than the largest file either format can be (above), on the length `stat` gives: corrupt
   on its size, before a byte of it is read.
3. Bytes starting with the magic: version above 2 is refused as newer; version below 2 or a
   header that ends short is corrupt; non-zero flags or reserved bits are refused as belonging
   to a newer emulator; an `image_len` and hash that differ from the running image are refused
   as saved for a different firmware.
4. Then the entries: entry count, row multiples, alignment, region containment (with checked
   bounds), order and overlap, and the file reaching past each entry are all checked. A table the
   file does not hold is said as the file ending inside the entry table, never as an entry at an
   address the file never named.
5. Then the tail size, and last the CRC-32. Any failure is corrupt: nothing was loaded.
6. No magic, but the length is one of this profile's raw sizes: a legacy file, read by the old
   reader and then migrated (below).
7. No magic and the length is another release's raw size: the error says the file probably
   belongs to that release. Only when no legitimate kind of file can it be is the word corrupt
   said.
8. Last, as a file of either kind is applied, the clock chip's own range: an offset more than 200
   years from the host's time is refused, and said as the chip's refusal rather than as corrupt.
   Both readers take it before a row or a clock word is written, so a file refused here has
   nothing applied either.

## Migrating a v1 file

On the board's first read of a raw file of this profile: it is loaded exactly as before, the
machine is encoded back as v2 against the running image and read back to check it, the original
bytes are copied to `settings.flash.v1` beside it - never overwriting a backup that is already
there, which is the older copy - and only then is the live file replaced, through the same
temporary file and rename as every other write. A crash between any two steps leaves the old
file or the new one, never half of either. The emulator never deletes the backup. That copy is
made as the live file is made - replaced, never followed - and a name there that a symbolic link
or a directory holds is refused rather than written through, so the old bytes land beside the
live file or nowhere.

## Versioning story

- The header version byte is authoritative; the magic never changes.
- Writers write 2. Readers accept 2; anything above is refused with the newer-emulator message,
  never guessed at.
- A future change bumps the byte, keeps 2 readable, and migrates on read the way v1 does here.
- Flags and reserved bits are the escape hatch for a bit-sized change with no version bump: a
  writer writes 0, a reader rejects non-zero, and no reader ever invents a meaning for 1.
- Entries are self-describing (address and length), so a layout-table change never needs a
  format change - which is exactly why v1 could grow into v2 with only a header.
