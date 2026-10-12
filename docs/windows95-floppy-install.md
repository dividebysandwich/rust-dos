# Windows 95 manual floppy-controller installation

Issue [#95](https://github.com/kibertoad/rust-dos/issues/95) reported that
manually selecting Standard Floppy Disk Controller in Add New Hardware
produced a resource-conflict warning with the controller added by
PR [#93](https://github.com/kibertoad/rust-dos/pull/93).

## Reproduction

Tested PR #93 at `decd949` in a release build on Windows, with SDL's dummy
video and audio drivers and the emulator's debug server. The guest was a
copy of a Windows 95 4.00.950 B FAT16 hard disk image installed under
DOSBox-X. Configuration: `machine=svga_s3`, `cpu=pentium`, `memsize=64`,
and the default enabled floppy controller. Boot commands:

```text
imgmount 2 copy.img -size 512,63,16,520 -t hdd -fs none
boot -l c
```

Choose Normal at the Windows startup menu. Run `control sysdm.cpl @1`,
then choose Next, No, Floppy disk controllers, and Standard Floppy Disk
Controller. The wizard reports that the installed device conflicts with
other devices. Finish and run `control sysdm.cpl,,1` to open Device Manager.

The controller's General tab reports **This device is disabled. (Code 22.)**
and **Disable in this hardware profile** is checked. Its Resources tab
initially says that the device is not using resources because it is not
enabled or has a problem; its conflicting-device list is empty.

Clicking **Set Configuration Manually** selects **Basic configuration 0**:

| Resource | Setting |
| --- | --- |
| Input/Output Range | 03F2–03F5 |
| Interrupt Request | 06 |
| Direct Memory Access | 02 |

The conflicting-device list then says **No conflicts**. Thus the wizard's
warning does not identify an actual overlapping resource in this case.

## Repair and verification

After assigning Basic configuration 0, return to General, clear
**Disable in this hardware profile**, apply the changes, and restart
Windows. After boot, the controller reports **This device is working
properly**, and Resources retains the settings above with **No conflicts**.

A newly created 1.44 MB FAT12 image was mounted in A: through the debug
server. Notepad saved `A:\FDC95.TXT`; reading the image on the host confirmed
the text in its data area. Closing Notepad and reopening that file from A:
displayed the saved text. This verifies guest reads and writes after the
manual installation is repaired.

## What the evidence rules out

The original image's `HKLM\Enum\Root` has no PNP0700 controller entry.
The image's floppy INF requests 03F2–03F5, IRQ 6 and DMA 2 for the primary
manual configuration; it does not request 03F7. The suspected overlap
between IDE and floppy at 03F7 therefore does not explain this warning.
The PnP BIOS resource list is also not used by this Windows installation.

The observed failure is a disabled, unconfigured manually added device,
and assigning its existing resources and enabling it resolves it without
changing the emulator. The exact reason Windows' wizard chooses that
state was not established, and the manual path was not compared under
DOSBox-X. These findings do not claim that every Windows image's resource
warning is spurious: inspect the Resources tab before applying the repair.

For a new installation, prefer **Yes (Recommended)** in Add New Hardware.
PR #93's original validation found that detection installs the controller
without the warning and enables Windows' native floppy driver.

Guest images, registry dumps, Microsoft INF files, save states and
screenshots used during the investigation are local test artifacts and
are not included in the repository.
