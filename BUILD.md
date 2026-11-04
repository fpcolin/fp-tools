# Building and releasing Flooring Partners App

## Layout

```
fp-tools/
├─ Cargo.toml              version (single source of truth), dependencies
├─ Cargo.lock              exact dependency versions - commit it
├─ build.rs                embeds icon, version info and manifest into the exe
├─ .cargo/config.toml      static C runtime on Windows
├─ assets/
│  ├─ fp.ico               app + installer icon
│  ├─ fp_logo.png          default embedded logo (compiled into the exe)
│  └─ app.manifest         Windows manifest (asInvoker, Win10+, long paths,
│                          per-monitor DPI, current-style controls)
├─ src/
│  ├─ main.rs              no arguments: the window; arguments: the CLI
│  ├─ lib.rs               module overview
│  ├─ app.rs               name, vendor, update URLs
│  ├─ config.rs            settings for both tools; copies the old apps' settings
│  ├─ old_apps.rs          finds and removes the old separate apps
│  ├─ cli.rs               command-line front end (scripts, testing)
│  ├─ converter.rs         the VendorCafe conversion
│  ├─ csv.rs               CSV reader/writer matching Python's csv module
│  ├─ generator.rs         URL/vCard -> PNG, save, open
│  ├─ vcard.rs             contact-card text
│  ├─ render.rs            modules, rounded corners, logo placement
│  ├─ qr/                  QR encoder (matches Python qrcode's segmentation)
│  ├─ image/               PNG decode/encode, JPEG decode, zlib, Lanczos resize
│  ├─ gui/                 the window (Windows only), see "The window" below
│  │  ├─ win32.rs          hand-written bindings: user32, gdi32, comdlg32
│  │  ├─ mod.rs            main window, page switching, message loop
│  │  ├─ canvas.rs         off-screen drawing (anti-aliased shapes, GDI text)
│  │  ├─ theme.rs          colours, fonts, owner-drawn buttons, icons
│  │  ├─ layout.rs         layout pass -> control positions + paint list
│  │  ├─ widgets.rs        creating and reading the standard controls
│  │  ├─ dialogs.rs        Open / Save As dialogs
│  │  ├─ updates.rs        sidebar update card, update flow, old-app removal
│  │  └─ *_page.rs         QR codes, VendorCafe converter, Settings, Help
│  └─ common/              shared plumbing
│     ├─ config_store.rs   per-user config.json, atomic writes
│     ├─ fsutil.rs         safe file creation in shared folders
│     ├─ http.rs           minimal HTTPS client (rustls)
│     ├─ json.rs           JSON reader/writer (Python-compatible output)
│     ├─ paths.rs          %LOCALAPPDATA%, %USERPROFILE%, program folder
│     ├─ policy.rs         policy.ini
│     ├─ shell.rs          open files/folders via explorer.exe
│     ├─ updater.rs        update check, download, verify, launch
│     └─ winreg.rs         read-only registry access (API set, no advapi32)
├─ vendor/getrandom-shim/  see "Why there is a vendored getrandom" below
├─ build/
│  ├─ installer.iss        Inno Setup script
│  ├─ publish.ps1          writes version.json for a built installer
│  ├─ check-imports.ps1    fails if the exe imports an unexpected DLL
│  └─ allowed-dlls.txt     the DLLs the exe may import
└─ .github/workflows/      ci.yml (every push), release.yml (version tags)
```

## Dependencies

Only three crates, all for HTTPS: `rustls` (TLS), `ring` (its cryptography,
also used here for SHA-256 and random numbers) and `webpki-roots` (Mozilla's
root certificates, compiled in). Everything else is hand-written or comes from
the standard library. That includes the QR encoder and the PNG and JPEG
codecs, which replace the Python `qrcode` and Pillow packages. The code avoids Windows DLLs where it can: settings
paths come from environment variables, files are opened with `explorer.exe`,
and admin policy comes from a file rather than the registry. That keeps the
import table at what the Rust standard library itself needs, plus the three
DLLs any window with file dialogs needs (`user32`, `gdi32`, `comdlg32`).

## The window

Plain Win32, no UI framework: standard EDIT, BUTTON (check box, radio) and
STATIC controls for anything that takes input or needs to be read by a
screen reader, and owner-drawn buttons and painted cards for the look of
the mock-up (design A). Points worth knowing before changing it:

- **Layout is a pure function.** Each page's `layout()` returns a `Frame`:
  where every control goes and a paint list (cards, headings, icons, the
  preview), in DIPs. `relayout()` applies it for the current DPI and scroll
  position. Change the look by changing `layout()`; there is no other
  geometry anywhere.
- **Two pieces of state.** `Ui` holds the control handles and never changes
  after start-up. `Model` holds everything else in a `RefCell`, borrowed with
  `gui::with(|m| ...)` for short, self-contained steps. Never call anything
  that can run a message loop or send a message back (message boxes, file
  dialogs, `SetWindowText`, `SetFocus`, moving controls) inside `with`: read
  what you need, let go, then call Windows.
- **Drawing.** Shapes are rasterised in Rust into a DIB (anti-aliased, which
  GDI's own shapes are not), text is drawn by GDI with ClearType, and the
  result is copied to the screen in one go, so nothing flickers.
- **DPI.** Per-monitor v2. Fonts are recreated and everything is laid out
  again on `WM_DPICHANGED`.
- **Keyboard.** The message loop runs `IsDialogMessage`, so Tab, Shift+Tab,
  Alt+letter and Enter behave as in a dialog box. Ctrl+1..4 and F1 switch
  pages. Arrow keys between the PO/WO radio buttons are handled directly.
- **Background work** (update check, download, removing old apps) runs on a
  thread that sends an `Event` down a channel and posts `WM_APP_EVENT` to
  wake the window.
- **Closing for an update.** Setup closes the program through Restart Manager
  (`WM_QUERYENDSESSION` / `WM_ENDSESSION`); the window saves pending edits and
  exits there.

### Why there is a vendored getrandom

`ring` gets random numbers through the `getrandom` 0.2 crate. On Windows that
crate imports `bcrypt.dll` and `advapi32.dll`. The standard library already
imports `ProcessPrng` from `bcryptprimitives.dll`, which is the RNG
underneath `BCryptGenRandom` anyway. So `vendor/getrandom-shim` provides the
same API on top of it, wired in with `[patch.crates-io]` in Cargo.toml. That
removes two DLLs. `getrandom` 0.3 made the same change upstream, so delete the
shim once `ring` moves to 0.3. If a dependency update makes the patch stop
applying, `cargo` warns, and the import check fails the build.

## One-time setup

On the build machine:

1. Install Rust with [rustup](https://rustup.rs) (the MSVC toolchain, the default on Windows).
2. Install the Visual Studio Build Tools with the "Desktop development with
   C++" workload. That provides the linker, the C compiler `ring` needs, and the
   Windows SDK's `rc.exe`, which `build.rs` uses to embed the icon. To point at
   a specific `rc.exe`, set `RC`.
3. Install [Inno Setup](https://jrsoftware.org/isdl.php) 6.3 or later and put
   `iscc` on `PATH`.

## Building

From the repo root:

```
cargo test
cargo build --release
pwsh build\check-imports.ps1 target\release\FPTools.exe
iscc build\installer.iss
```

You get `dist\installer\FPTools-<version>-setup.exe`. If
`rc.exe` can't be found, the build warns and produces an exe without its icon.
Set `FP_REQUIRE_RESOURCES=1` to make that an error; the release workflow does.

The exe links the C runtime statically (`.cargo/config.toml`), so it doesn't
need the VC++ redistributable.

## Cutting a new version

Bump `version` in `Cargo.toml`, commit, then tag:

```
git tag v3.0.1
git push origin v3.0.1
```

That's the only place the version lives. The exe's file properties come from
it through `build.rs`, the workflow passes it to Inno Setup, and the workflow
refuses to build if the tag doesn't match it.

The workflow creates the GitHub release as a **draft**. Check it, then press
**Publish release**. Installed copies only see published releases, so
nothing reaches users until you do. To publish automatically, set the
repository variable `AUTO_PUBLISH` to `true`.

This is a new repository (`fpcolin/fp-tools`) with a new installer identity,
so the old apps' update checks never offer it; people move over when IT
installs it (or they run Setup), and the program then offers to remove the
old apps. If you would rather the old apps' updaters lead people here, publish
a final release of each old app whose installer just runs this one.

Settings survive version bumps. They are reset only when `config_schema` in
`src/app.rs` changes. Bump it only when the shape of `config.json` changes
(renaming or removing a key, or changing what a value means).

## How updates work

The manifest format is unchanged from version 2, so installed Python copies can
update to version 3:

```json
{ "version": "3.0.1",
  "url": "https://github.com/fpcolin/fp-tools/releases/download/v3.0.1/FPTools-3.0.1-setup.exe",
  "sha256": "...",
  "notes": "..." }
```

The flow:

1. GET `manifest_url` over HTTPS. The response is capped at 64 KiB. Each
   network step has a 6-second limit, as in the Python version, and the whole
   check gets 20 seconds. The limits apply to every individual read and write,
   so a server that stalls or trickles bytes can't hold the check open.
2. Validate the manifest:
   - `version` must be 1-4 dot-separated numbers.
   - `url` must start with `download_prefix` (this repo's releases) and end
     in `v<version>/<name>.exe`.
   - `sha256` must be 64 hex digits.

   Anything else is rejected. Redirects must stay on HTTPS.
3. If the version is newer, download into a new, randomly named folder under
   `%LOCALAPPDATA%\<vendor>\<app>\updates`. That's in the user's own profile,
   which other users can't open. `%TEMP%` isn't used because some sites point
   it at a shared drive. The file is created fresh and capped at 256 MiB.
4. Reopen the file with sharing that denies writes, renames and deletes, hash
   it from disk, and compare it with the manifest.
5. Start the installer with `/SILENT /SUPPRESSMSGBOXES /NORESTART`, plus
   `/ALLUSERS` or `/CURRENTUSER` from `policy.ini`, so the right install is
   updated when a machine has both. **Keep the lock until Setup finishes.**
   For a machine-wide install, Inno re-runs the same file elevated after the
   UAC prompt, so the file must stay locked through that moment. The window
   hides itself and calls `Launched::wait()` on a background thread.
   Setup closes the program itself when it's ready to replace files. If the
   user declines the prompt, `wait()` returns false and the program can carry
   on.

The installer is started *without* elevation. For a machine-wide install, Inno
Setup shows the UAC prompt itself. Because Setup began as the normal user, its
`runasoriginaluser` relaunch reopens the program as that user. If the updater
elevated Setup itself, the program would come back running as whichever
administrator approved the prompt.

Download folders older than an hour are removed by `cleanup_stale_downloads()`.
Call it at startup.

**What the checksum does and doesn't protect.** The SHA-256 lives in the same
manifest as the URL, so it protects against a corrupted or swapped download,
not against someone who can publish releases on the GitHub repo. Code-signing
the installer limits that risk (see below). Signing the manifest with a key
kept outside GitHub would close it fully. That is a reasonable next step if
the repository is ever shared more widely.

### Distributing via GitHub Releases

`manifest_url` uses `/releases/latest/download/version.json`. That is a
permanent redirect to the newest release's asset, so the URL never changes.
**Don't switch it to `api.github.com`.** The REST API allows 60
unauthenticated requests per hour per IP address. A whole office behind one
NAT address shares that limit, and updates would silently stop.

`publish.ps1` records a *versioned* installer URL in the manifest
(`/releases/download/v3.0.1/...`). That way a client that read the manifest
just before a new release still downloads the file that manifest describes.

Release assets on a **private** repo need authentication, which the app
doesn't have. Keep the repo public, publish releases from a separate public
repo, or host `version.json` and the installer elsewhere (an internal web
server, Azure Blob Storage). If you move hosting, change both `manifest_url`
and `download_prefix` in `src/app.rs`.

### Corporate networks

Proxy, in order of precedence:

1. `Proxy=` in `policy.ini` (`direct` turns proxies off).
2. The `HTTPS_PROXY` / `NO_PROXY` environment variables.
3. The user's manual proxy in Windows Internet Options (`ProxyServer` and
   `ProxyOverride`), which is what Python's `urllib` used.

The registry is read through the `api-ms-win-core-registry` API set, which
resolves to kernelbase. advapi32 is never loaded. Automatic configuration
scripts (PAC/WPAD) and NTLM/Kerberos proxy authentication would need WinHTTP
or SSPI, so they aren't supported.

Unlike Python, the program doesn't use the Windows certificate store; that
would need crypt32. On networks that decrypt and re-sign TLS, set
`ExtraRootCertificates=` in `policy.ini` to the inspection CA in PEM form.
Otherwise a manual check reports a certificate error that says exactly this,
and the automatic check stays quiet. **Tell IT before rollout**, or such
sites will silently stop updating.

## Multi-user installs

- **Program files.** Machine-wide installs go to Program Files, which only
  administrators can write. Per-user installs go to
  `%LOCALAPPDATA%\Programs`. `UsePreviousPrivileges` keeps an existing install
  in its current mode when it is upgraded.
- **Settings.** These are per user, in `%LOCALAPPDATA%`. Writes are atomic (a
  temp file plus a rename). If two copies of the program run for the same
  user, each change is merged into the latest file rather than overwriting it.
- **Output files.** These are written through randomly named temporary files
  created with "create new" semantics. In a shared or network folder, another
  user can't pre-create the name to block or redirect the write.
- **Uninstall.** Every user's settings are left in place. An uninstall runs
  as one account and can't reach the others.
- **The old apps.** They were per-user installs, which an administrator's
  Setup can't see. Each user's first run copies their old settings and then
  offers to run their old uninstallers (`old_apps.rs`). All-users copies, if
  any, are removed by Setup itself (`removeoldapps` task).
- **Install folder.** Interactive all-users installs must stay inside
  Program Files. A folder such as `C:\Tools` inherits "Users: Modify", so any
  user could replace the program for everyone. For silent installs, `/DIR=`
  is IT's choice.
- **Shared machines (Remote Desktop).** On these, an update closes every
  user's running copy (Inno's `CloseApplications`). Use `/DISABLEUPDATES` and
  deploy centrally instead.

## Code signing

Unsigned executables trigger SmartScreen and are treated as untrusted by
Defender and application-control products. Sign both the exe and the
installer. With ThreatLocker in particular, a policy on the publisher
certificate covers every future version. Without one, each release has a new
hash and needs a new policy.

The workflow signs automatically when `SIGNING_PFX_BASE64` and
`SIGNING_PFX_PASSWORD` are set under Settings → Secrets and variables →
Actions. Without them it skips signing. The step order matters:

- The exe is signed **before** Inno packages it. Otherwise the installed
  program is unsigned.
- Inno signs Setup and the uninstaller it embeds, through `SignTool=` in
  `installer.iss`. The workflow defines `Sign` and passes the command with
  ISCC's `/S`. That happens **before** `publish.ps1` hashes Setup. Signing
  rewrites the file, so an earlier checksum would never match, and every
  update would fail with "checksum mismatch".

To sign locally, run
`iscc /DSign "/Sfpsign=signtool sign /a /fd SHA256 /tr http://timestamp.digicert.com /td SHA256 $f" build\installer.iss`.

Always timestamp (`/tr`), or signatures stop validating when the certificate
expires.

### Self-signed (in-house) certificate

For a program that is only installed on company PCs, a certificate you make
yourself works as well as a bought one, at no cost:

1. On Windows, run `powershell -ExecutionPolicy Bypass -File
   build\new-signing-cert.ps1`. It creates `FP-CodeSigning.pfx` (secret: the
   private key) and `FP-CodeSigning.cer` (public), and copies the `.pfx` as
   base64 to the clipboard.
2. Add the GitHub secrets `SIGNING_PFX_BASE64` (the clipboard) and
   `SIGNING_PFX_PASSWORD`. The workflow notices the certificate is
   self-signed and trusts it on the build runner only, so that its signature
   check passes.
3. IT deploys `FP-CodeSigning.cer` to the PCs' **Trusted Root Certification
   Authorities** store (Group Policy: Computer Configuration → Policies →
   Windows Settings → Security Settings → Public Key Policies; or an Intune
   "Trusted certificate" profile). Windows then shows the signature as valid,
   with "Flooring Partners" as the publisher.
4. IT allows the certificate in ThreatLocker, so every future signed version
   is allowed without a new policy.

Limits: SmartScreen gives no reputation to a self-signed certificate, so an
installer downloaded with a web browser still gets the "Windows protected
your PC" warning. Installs pushed by IT and the in-app updater are not
affected (SmartScreen only checks files a browser marked as downloaded).
Anyone with the `.pfx` and its password can sign programs your PCs will
trust, so treat it like an admin password. Many CAs now issue OV certificates only on hardware tokens, which
can't be used this way. Azure Artifact Signing (around $10/month, OIDC, no
stored key) is the usual alternative. Replace the two signing steps with
`azure/artifact-signing-action`, pinned to a commit SHA, and use a **Public
Trust** certificate profile.

## CI hardening notes

- Values like the tag name reach scripts only through `env:`. They are never
  pasted into script text, where a crafted tag could inject commands.
- The build job has a read-only token. Only the small `publish` job can
  write to the repository, and it runs nothing but `gh` on the built files.
- Inno Setup is installed before any signing material is written to disk.
  The certificate is deleted as soon as the installer is built.
- Actions are pinned to commit SHAs. Update them deliberately.
- The workflow checks that `download_prefix` in `src/app.rs` matches the
  repository. After a rename or transfer, installed copies would otherwise
  reject every release.
- `choco install innosetup` and `rustup ... stable` take current versions.
  Pin them (`--version=x.y.z`, `rust-toolchain.toml`) if you want fully
  reproducible builds.
- Anyone who can push a tag can run the release workflow, and with it the
  signing secrets. Treat write access to the repo as equivalent to holding
  the certificate.

## Testing without Windows

The core builds and its tests run on Linux, apart from the Windows-only parts
of the updater and the window (`cargo test`; on Linux the exe is the CLI). A
cross-build for `x86_64-pc-windows-gnu` (MinGW) runs under Wine, and the
tests pass there too; the window runs under Wine as well, which is how its
screenshots were taken. MinGW builds import
`msvcrt.dll`, so use them for testing only. Release builds use the MSVC
toolchain with the static CRT.

## Troubleshooting

**`check-imports.ps1` fails.** A dependency added a DLL import. Run the script
locally to see which DLL it is. Prefer removing the dependency. If the DLL is
genuinely needed, add its exact name to `build/allowed-dlls.txt` and tell IT,
because their application-control policies may need updating. The check only
sees the import table. To list what loads at run time (for example Winsock's
helpers during the update check), use Process Monitor or Sysinternals
ListDLLs on a real machine.

**"Windows resources not embedded" warning.** `rc.exe` wasn't found. Install
the Windows SDK, or set `RC` to its full path.

**Update check says "invalid peer certificate: UnknownIssuer".** The network
inspects TLS. Set `ExtraRootCertificates=` in `policy.ini` to the
organisation's root CA in PEM format.

### Permission denied when saving

The usual cause is Microsoft Defender's Controlled folder access. It blocks
untrusted programs from writing to Documents, Pictures, Videos, Music,
Desktop and Favorites, and an unsigned exe is untrusted by definition. The
program explains this and suggests choosing another folder. A signed exe
builds reputation and stops being blocked.

The installer refuses to install *into* one of those folders interactively.
It checks the real shell-folder paths, so redirected and OneDrive-backed
folders are caught too. Confirm a Defender block in Windows Security → Virus &
threat protection → Protection history, or in Event Viewer (Windows Defender
→ Operational, event ID 1123).

### Image codecs

`src/image/` replaces Pillow:

- **PNG decoding** handles every colour type, bit depth, interlacing and
  transparency. It matched independently computed pixels on 372 test files.
- **PNG encoding** uses adaptive filters and DEFLATE with dynamic Huffman
  codes. Output is within about 10% of Pillow's size, smaller for QR codes,
  and a 1500×1500 code encodes in under 0.1 s.
- **JPEG decoding** implements libjpeg-turbo's integer IDCT, fancy upsampling
  and colour conversion. It was bit-identical to Pillow on 250 test files:
  baseline, progressive, every subsampling, restart markers, greyscale, RGB
  and CMYK.
- **Robustness.** The decoders were fuzzed with tens of thousands of
  truncated and corrupted files under overflow checks, with no panics. Size
  limits are checked before any allocation, and PNG decompression stops at
  the expected size, so a "zip bomb" can't use extra memory.
- **Tests.** `tests/images.rs` keeps reference checks and a short fuzz pass
  in CI.

### Embedded images

PNG (every colour type and bit depth, interlaced, transparency) and JPEG
(baseline and progressive, greyscale, CMYK) are supported. The format is
detected from the file contents, not the extension. Images over 40 megapixels
are refused. As in version 2, the logo is scaled into a square at about a
quarter of the code's width. With transparency it is blended over the code;
without, it is pasted. Error correction is always level H, so codes with a
logo stay readable.
