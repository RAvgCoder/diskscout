diskscout
Finds where your disk space went, and clears the parts that are safe to clear.

Needs an Apple silicon Mac (M1 or later). Nothing else to install.


INSTALL

  1. Double-click the zip to unzip it.
  2. Open Terminal. Type "cd " with a space after it, drag this folder into
     the Terminal window, and press Return.
  3. Run:

         bash install.sh

  4. Open a new Terminal window.

Run it with "bash install.sh" rather than double-clicking. macOS blocks
programs that were not downloaded from the App Store or checked by Apple, and
this one is neither. The installer clears that block on the installed copy,
which is the same thing as clicking "Open Anyway" in System Settings.


USE

  diskscout
      Scans your home folder and prints a report. Deletes nothing.
      The full report is saved to /tmp/diskscout-report.txt.

  diskscout --delete-safe
      Lists everything it considers safe to delete, asks once, then deletes it.
      This is permanent. Nothing goes to the Trash.

      To keep something, pass the name the list shows for it:

          diskscout --delete-safe --except macos-caches,app-web-caches

  Read the report before your first delete.


UNINSTALL

  Delete the file ~/.local/bin/diskscout
  Optionally remove the lines starting "# diskscout" from ~/.zshrc.
