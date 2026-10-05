# Release checklist: the real-device check

The release workflow has already installed this version's x64 installer
silently on a clean runner, run the whole Windows matrix against the
installed `xfs.exe` through WinFsp, and uninstalled it, checking nothing
was left behind. What a runner cannot do is below: a real machine, a real
disk, and a person looking at Explorer. Tick each item before announcing
the release.

## On a real Windows machine (x64, and arm64 if one is to hand)

- [ ] `winget install AntimatterStudios.xfs-win-driver` (or the
      Setup.exe from this release) installs without a prompt beyond UAC.
- [ ] SmartScreen's warning, if the installer is unsigned, is the only
      warning shown.
- [ ] A real XFS image (a Linux disk image, or one made with `mkfs.xfs`
      from a directory you know) mounts from Explorer's **Mount as xfs**
      menu entry on the `.img`.
- [ ] Explorer lists the volume's root; a nested directory opens.
- [ ] A text file and a binary file open, and their hashes match the
      source (`Get-FileHash`).
- [ ] Ending the mount (Ctrl-C in its window) releases the drive letter.
- [ ] The **XfsWatcher** service is running after a reboot, a USB
      stick or SD card with an XFS partition mounts on its own when
      plugged in, and unmounts cleanly when removed.
- [ ] Uninstalling from **Settings > Apps** removes the drive-letter
      mounts, the service, the menu entry and the install folder.

## Notes

Record anything that failed, with the Windows build (`winver`), in an
issue before publishing a fix.
