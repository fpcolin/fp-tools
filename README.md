# Flooring Partners Tools

One Windows desktop app for Flooring Partners that replaces two older ones:

- **QR codes** (was *QR Code Generator*): makes QR codes for web addresses and
  contact cards, with the company logo in the middle.
- **VendorCafe converter** (was *VendorCafe CSV Converter*): turns a Rollmaster
  invoice export into one line per invoice, ready to upload to VendorCafe.

It ships as a single self-contained `.exe` with no bundled DLLs and no runtime
to install. It can be installed for all users of a machine or for one user
only, and has a built-in, hardened updater.

## What it does

The window has a sidebar with four pages: **QR codes**, **VendorCafe
converter**, **Settings** and **Help**. The sidebar also shows the version and
whether an update is available.

### QR codes

- **Web address** or **Contact card** (first and last name required; email,
  mobile, work phone, company, job title and website optional; blank fields
  are left out).
- A live preview updates as you type, with the logo and corners as they will
  be saved.
- Saves a PNG with 10-pixel modules and a 4-module quiet zone, as before, to
  your chosen folder. If the name is taken it adds ` (1)`, ` (2)`… and never
  overwrites an existing file.
- Options: put the logo (or your own PNG/JPEG) in the middle, rounded corners,
  open the image afterwards, show it in its folder.

Codes use error-correction level H, with the same data encoding and mask
choice as the old app. They come out module-for-module identical; that was
checked against the Python `qrcode` package on thousands of inputs across all
40 QR versions. The logo is scaled and placed exactly as before.

### VendorCafe converter

Choose the exported file, say whether this customer uses a **PO number** or a
**work order number**, and click **Convert**. For each invoice the converter
keeps the first row and sets the line-item fields to represent the whole
invoice:

- Duplicate rows for the same **Invoice No** are dropped, keeping the first
- **Item Amount** and **Unit Price** are set to the **Invoice Total**
- **Item Sequence** and **Quantity** are set to `1`
- `1-` is removed from **Invoice No** (every occurrence, as before)
- Apostrophes are stripped from **Invoice Desc** and **Property Code**
- **Item Desc** is set to the cleaned **Invoice No**
- For work-order customers, **Job No** is dropped and **PO No** becomes **WO No**

Every field is handled as text, so amounts, leading zeros and dates are
written back exactly as exported. Output is byte-for-byte identical to the old
converter; that was checked on thousands of generated exports, including
malformed ones. The PO/WO choice is made each time, like the old app's two
buttons, and is not remembered.

## Installing

Run `FPTools-<version>-setup.exe` from the
[latest release](https://github.com/fpcolin/fp-tools/releases/latest).

- **For all users (default).** Installs to
  `C:\Program Files\Flooring Partners\Tools`. This needs administrator
  approval. Only administrators can change the program files.
- **For me only.** Pick it on the first page of Setup. Installs to
  `%LOCALAPPDATA%\Programs\...` without admin rights.

Each user's settings live in their own profile, so people sharing a PC never
see or overwrite each other's settings:

```
%LOCALAPPDATA%\Flooring Partners\Tools\config.json
```

### Moving from the two old apps

The old apps were installed separately by each person (per-user installs).

- **Settings carry over.** The first time someone opens Flooring Partners
  Tools, their QR save folder, file name, logo, options and chosen export file
  are copied from the old apps' settings.
- **Old apps are removed on request.** Right after that, the program offers
  once to remove that person's copies of the old apps, by running their own
  uninstallers. If they decline, **Settings → Old apps → Remove old apps**
  does it later. Nothing is removed without asking.
- **Copies installed for all users** (only possible if IT deployed the old
  apps machine-wide) are removed by this installer when it runs as
  administrator; it's the *Remove the separate … apps* option in Setup, on by
  default. Unattended installs that must keep them can pass
  `/MERGETASKS="!removeoldapps"`.

Uninstalling an old app deletes its own settings folder, which is why the
settings are copied first.

### Unattended deployment (IT)

```
FPTools-3.0.0-setup.exe /VERYSILENT /SUPPRESSMSGBOXES /NORESTART /ALLUSERS
```

Always include **both** `/SUPPRESSMSGBOXES` and `/ALLUSERS` (or `/CURRENTUSER`).
Without them, a first install still shows the "all users or just me" question
even under `/VERYSILENT`. When Setup runs as SYSTEM, nobody can answer it, and
the deployment hangs.

| Switch | Effect |
|---|---|
| `/ALLUSERS` / `/CURRENTUSER` | Force machine-wide or per-user mode |
| `/DISABLEUPDATES` | Turn the in-app update check off (writes `policy.ini`) |
| `/ENABLEUPDATES` | Turn it back on |
| `/NORELAUNCH` | Never reopen the program after a silent install |
| `/MERGETASKS="!removeoldapps"` | Keep all-users copies of the old apps |
| `/DIR="..."` | Install folder. For all-users installs keep it inside Program Files, which only administrators can change. (The interactive installer enforces this.) |

The program is never relaunched after `/VERYSILENT`, or when Setup runs as
SYSTEM (RMM tools, Intune, GPO).

### `policy.ini`

The installer writes `policy.ini` next to the exe. In a machine-wide install
only administrators can edit it. The program reads it at startup:

```ini
[Install]
Scope=machine            ; written by Setup: machine or user

[Updates]
Enabled=1                ; 0 = no update checks (versions deployed by IT)
Proxy=http://proxy.example.com:8080   ; optional; "direct" = never use a proxy
ExtraRootCertificates=corp-root.pem   ; optional PEM, for TLS-inspecting firewalls
```

Without `Proxy=`, the update check uses `HTTPS_PROXY` if set, and otherwise
the user's manual proxy from Windows Internet Options (as the old apps did).
Automatic proxy scripts (PAC/WPAD) and proxies that need Windows sign-in
(NTLM/Kerberos) aren't supported. Basic authentication in the `Proxy=` URL
is, but a machine-wide `policy.ini` is readable by every user, so prefer an
unauthenticated proxy rule for the update host. The file may be saved as
UTF-8 or UTF-16. If it exists but can't be read, updates stay off.

### Application control (ThreatLocker and similar)

- **One file.** The install is a single exe. There are no DLLs in the program
  folder, and nothing is unpacked to `%TEMP%` at startup. The logo is compiled
  into the exe.
- **Few system DLLs.** The exe imports only these Windows DLLs:
  - `kernel32`, `ntdll`, plus the `api-ms-win-core-synch-l1-2-0` and
    `api-ms-win-core-registry-l1-1-0` API sets, which resolve to `kernelbase`
    (loaded into every process anyway);
  - `bcryptprimitives` (random numbers) and `ws2_32` (network, for the update
    check);
  - `user32` and `gdi32` (the window; every program with a window uses them);
  - `comdlg32` (the standard Open and Save As dialogs).

  Images are decoded and encoded in Rust, so no imaging DLLs are loaded. HTTPS
  is done in Rust too, so `winhttp`, `schannel` and `crypt32` are never
  loaded. The release build fails if a change would add any other DLL; see
  `build/allowed-dlls.txt`.
- **DLLs Windows loads on the program's behalf** (not in the import table, and
  the same for any program doing these things): the modern look of buttons and
  check boxes comes from Windows' own copy of `comctl32.dll`, selected by the
  exe's manifest; the Open/Save dialogs load the shell DLLs (`shell32` and
  friends) while they are open; the socket library loads helpers such as
  `mswsock.dll` and `dnsapi.dll` during the update check. With updates turned
  off, the last never happens.
- **Child processes.** It starts `explorer.exe` to open an image or show a
  file in its folder. When installing an update, it starts the downloaded
  installer from `%LOCALAPPDATA%\Flooring Partners\Tools\updates\`. When asked
  to remove the old apps, it runs their existing `unins000.exe`.
- **Updates.** Each release has a new file hash. Sign the releases (see
  BUILD.md); Setup, the uninstaller and the exe are all signed. Then allow
  the publisher certificate, so new versions don't need new policies.
  Otherwise, turn the in-app updater off and deploy versions centrally.

## Using it

Open **Flooring Partners Tools** from the Start menu.

- **QR codes:** choose *Web address* or *Contact card*, type, check the
  preview, click **Save QR code** (or press Enter).
- **VendorCafe converter:** *Choose file…*, pick *PO number* or *Work order
  number*, click **Convert** (or press Enter). The result line has
  **Show in folder**.
- Keyboard: Ctrl+1 / Ctrl+2 / Ctrl+3 switch pages, F1 opens Help, Tab moves
  between boxes and Alt + the underlined letter jumps to one.

If Windows blocks saving to a folder (usually Defender's "Controlled folder
access", OneDrive, or the file being open elsewhere), the program says so and
offers to choose another folder.

### From scripts

Every feature is also available on the command line, using the same per-user
settings as the window:

```
FPTools url "https://example.com/page"
FPTools vcard --first Ann --last Smith --mobile "+1 555 010 0000" --email ann@example.com
FPTools convert po "C:\Exports\invoices.csv"
FPTools set folder "C:\Users\Ann\QR Codes"
FPTools set convert-overwrite off
FPTools settings
FPTools check-updates
FPTools help
```

The exe is a Windows (not console) program, so `cmd` doesn't wait for it.
Use `start /wait FPTools.exe ...` (or `Start-Process -Wait` in PowerShell)
when the exit code matters.

## Updates

The program checks the latest GitHub release for a newer version shortly
after it starts, and the sidebar says *Version x is available* with an
**Install update** button. The download must come from this project's own
releases over HTTPS, and must match the SHA-256 in the release manifest. It's
saved in the user's own profile and stays locked from the checksum until the
installer finishes, so nothing can swap it in between. For a machine-wide
install, Windows asks for administrator approval. The program then reopens as
the user who was using it, not as the administrator.

## Changes in 3.0.1

- The logo has a visible **Reset to default** (back to the Flooring Partners
  logo) on the QR codes page and in Settings, and the name of the image in use
  is shown next to it.
- If your own logo file has been moved or deleted, the app now says so instead
  of quietly saving QR codes without a logo.

## Changes in 3.0

- The QR Code Generator and VendorCafe CSV Converter are now one program with
  one installer and a new window. Settings are copied over; the old apps are
  removed on request.
- Rewritten in Rust: a single exe, no Python runtime, no `qrcode` or Pillow.
- Live QR preview; clear success and error messages in the window instead of
  pop-ups; keyboard shortcuts; sharp at any display scaling.
- Installs for all users or for one user; multi-user safe throughout.
- Transparent PNG logos made with palette transparency are now blended
  properly, instead of showing a solid background.
- Security fixes:
  - Update downloads are pinned to this project's HTTPS releases.
    `file://`, `http://` and other hosts are refused.
  - The manifest's version can't be used to write files outside the
    download folder.
  - Downloads go to a fresh folder in the user's own profile. They stay locked
    from verification until the installer is done, including the moment
    Windows re-runs it with administrator rights.
  - Download sizes are capped, and every network step has a time limit. A
    silent or very slow server can no longer hang the update check.
  - In a machine-wide install, the program no longer reopens with
    administrator rights after an update.
  - QR images are created atomically, so two people saving to a shared
    folder at once can't overwrite each other, and a planted link can't
    redirect the write. The converted CSV is written through an unpredictable
    temporary name for the same reason.
  - File names are cleaned of Windows device names (`CON`, `NUL`…) and
    control characters.
  - vCard fields can no longer inject extra properties through pasted line
    breaks. Website URLs are no longer corrupted by escaping.
  - Embedded images are size-limited (no "decompression bomb").
  - Settings are written atomically, and a damaged value no longer crashes
    the program.
  - The release workflow is no longer open to command injection through a
    tag name, and its actions are pinned to commit hashes.

## Building from source

See [BUILD.md](BUILD.md). In short, with Rust and the Windows SDK installed:

```
cargo test
cargo build --release
iscc build\installer.iss
```

## Licence

Source code and documentation are released under the MIT Licence.

The Flooring Partners name, logo and icon (`assets/fp_logo.png`,
`assets/fp.ico`) are excluded and remain company property; no trademark rights
are granted. If you fork this project, replace those files and change the
constants in `src/app.rs` and `build.rs`.

See [LICENSE.txt](LICENSE.txt) for both.

## AI disclosure

Portions of this project were developed with the help of an AI assistant
(Anthropic's Claude).

The original working versions of the two programs were written by hand. AI
assistance was then used to refactor that code and build the packaging and
release tooling. Version 3's conversion from Python to Rust, the merge into
one program and the new window were also done with AI assistance. That
includes the hand-written QR encoder, image codecs, HTTP code and Windows UI
code, the updater hardening and the multi-user installer.

All AI-generated code is reviewed and tested before being committed, and the
maintainer is responsible for everything in this repository regardless of how
it was produced. It is noted here for transparency, not as a disclaimer of
ownership or accountability.
