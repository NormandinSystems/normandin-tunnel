# User guide

Everything you need to install Normandin Tunnel and move files between two
computers. No technical knowledge needed.

* [Install](#install)
* [First start](#first-start)
* [Connect two devices](#connect-two-devices)
* [Send and receive](#send-and-receive)
* [Where your files go](#where-your-files-go)
* [Settings](#settings)
* [When something goes wrong](#when-something-goes-wrong)
* [Command line](#command-line)
* [Remove the app](#remove-the-app)

## Install

Both devices need the app. Download it from the
[Releases page](https://github.com/NormandinSystems/normandin-tunnel/releases).

### Windows 10 and 11

1. Download the file ending in `-setup.exe`.
2. Open it. Windows shows a blue box, **"Windows protected your PC"**. This
   appears because the installer is not code-signed yet. Choose **More info**,
   then **Run anyway**.
3. Follow the installer.

On some Windows 11 computers a feature called Smart App Control blocks
unsigned programs completely and offers no "Run anyway". In that case the app
cannot be installed on that computer until signed installers exist.

There is also an `.msi` file with the same program, for people who deploy
software with management tools.

### Linux

| Your system | Download | Install |
|---|---|---|
| Debian, Ubuntu, Mint | the `.deb` file | `sudo apt install ./the-file.deb` |
| Fedora, openSUSE | the `.rpm` file | `sudo dnf install ./the-file.rpm` |
| Anything else | the `.AppImage` file | `chmod +x the-file.AppImage`, then run it |

If the AppImage does not start, your system may be missing FUSE (the package
is usually called `libfuse2`).

### Check your download (optional)

Each release includes `SHA256SUMS.txt`, a list of fingerprints of the files.
To check that your download arrived intact:

* Windows (PowerShell): `Get-FileHash .\the-file.exe` and compare the result
  with the line for that file.
* Linux: `sha256sum the-file` and compare.

## First start

The first screen asks three things:

* **Name of this device.** The other person sees it when you connect.
* **Where to save what you receive.** A folder called `Tunnel` inside your
  Downloads folder, unless you change it.
* **Language.** English or French.

Your computer may ask whether Tunnel can use the network. Allow it on private
networks. Then choose **Get started**.

## Connect two devices

Both devices need to be online.

**On the first device**

1. Choose **Show a code**.
2. A code appears, for example `4821-orbit-velvet-candle`. Read it to the
   other person, or use **Copy code** and send it in a message.

A code works once and expires after 10 minutes. **New code** makes another.

**On the second device**

1. Choose **Enter a code**.
2. Type the code. Spacing and capital letters do not matter, and the first
   three letters of each word are enough: the app suggests the rest.
3. Choose **Connect**.

**On both devices**

Both screens show a 6-digit **safety code**, for example `418 293`.

* If both show the same number, choose **Codes match** on each device.
* If the numbers differ, choose **They don't match**. The session ends and
  nothing is sent. Start again with a new code.

This step matters. It is how you know you are connected to each other and not
to someone who guessed or overheard the code.

When both of you have confirmed, the session is open. The top of the window
shows who you are connected to and how:

* **Direct**: data travels straight between the two devices.
* **Through relay**: the devices could not reach each other directly, so the
  encrypted data passes through a relay. It may be slower.

## Send and receive

Either person can send at any time during the session.

**To send**

* Drop files or folders onto the window, or use **Send files** or
  **Send a folder**.
* For a short text or a link, type it in the **Message or link** box and
  choose **Send**.

**To receive**

When the other person sends something, a card appears showing who it is from,
how many files and folders, and the total size.

* **Show names** lists what is inside before you decide.
* **Save to** lets you pick another folder for this transfer.
* **Accept** starts it. **Decline** refuses it. Nothing is saved until you
  accept.

The card may carry a warning, for example that the transfer contains a
program or script, or a file that hides a program behind another extension
such as `report.pdf.exe`. Only open such files if you trust the sender. Tunnel
never opens or runs anything for you.

Links that arrive as text are never opened for you either. You can copy them.

**While a transfer runs**

* **Pause** and **Resume** work from either side.
* **Cancel** stops it. Files that already arrived completely are kept;
  unfinished ones are removed.
* If the connection drops, the app tries to reconnect for two minutes and
  then carries on from where it stopped.

**To finish**

Choose **End session**. Everything that finished is saved. To connect again
later you need a new code. A transfer that was still running cannot be
continued in a later session.

## Where your files go

* Into the save folder you chose, `Downloads/Tunnel` unless you changed it.
  **Show in folder** on a finished transfer opens it.
* If a file with the same name already exists, both are kept: the new one
  gets a number, like `photo (2).jpg`. You can change this in Settings.
* A name that is not allowed on your system is changed to a safe one, and the
  app tells you which.
* While a transfer runs, unfinished files sit in a hidden folder called
  `.tunnel-partial` inside the save folder. It is removed when the transfer
  is done or cancelled.

## Settings

| Section | What you can change |
|---|---|
| General | Device name, language, light or dark appearance, notifications |
| Receiving | Save folder, whether to ask where to save each time, what to do when a file already exists (keep both, replace, skip), whether to keep the original modified times, and whether received files are marked as downloaded so that your computer warns before a received program runs |
| Privacy | How long the list of past transfers is kept (off, 7, 30 or 90 days, forever) and whether it shows file names. The list is stored only on your device |
| Advanced | Your own relay or pairing server, the most files accepted in one transfer, and whether logs may include file names (off unless you turn it on for troubleshooting) |
| About | Version, and the folder where the app keeps its settings and history |

Most people never need the Advanced section.

## When something goes wrong

| The app says | What it means | What to do |
|---|---|---|
| Can't reach the pairing network | This device could not reach a relay | Check the internet connection. Some office or school networks block it |
| No device is showing that code right now | Nothing answered for that code's number | Check the number at the start. The code may have expired; ask for a new one |
| That code didn't work | The words were wrong, or the code was already used | Ask for a new code |
| This code expired | Ten minutes passed | Choose **New code** |
| Someone already used this code | Another device answered first | If it was not the person you expected, end the session and start over |
| Couldn't connect to the other device | The two devices paired but could not open a connection | A firewall may be blocking it. Try another network |
| The safety codes didn't match | One of you chose "They don't match" | Start over with a new code |
| The other device needs an update | The two apps are too far apart in version | Update both |
| Not enough space | The save folder's disk is too full | Free up space or choose another folder |

Other things to know:

* **Both devices must be online, even on the same Wi-Fi.** They find each
  other through relays on the internet.
* **Transfers are slow.** If the window says "Through relay", the data is
  taking the long way round. Two devices on the same network, or on ordinary
  home networks, usually connect directly.
* **Windows asked about the firewall and you chose "Cancel".** Allow Tunnel
  on private networks in Windows Security > Firewall, or direct connections
  on your own network may not work.

Still stuck? [Open an issue](https://github.com/NormandinSystems/normandin-tunnel/issues)
and say what you did, what you expected and what the app showed. Never paste a
connection code that is still in use.

## Command line

Each release also has a command-line program, `tunnel`. It does the same job
and works with the desktop app on the other side.

```sh
# Show a code, wait for the other device, then send
tunnel send ./photos report.pdf

# Type the code from the other device and receive into a folder
tunnel receive 4821-orbit-velvet-candle --to ~/Downloads

# Send a short message as well
tunnel send --text "meet at 5"
```

It asks you to confirm the safety code and each incoming transfer.

| Option | Meaning |
|---|---|
| `--to DIR` | Where `receive` saves. The current folder if not given |
| `--once` | `receive` stops after the first transfer |
| `--yes` | Answers yes to the safety code and to every offer. Only for when you control both devices |
| `--json` | Prints one JSON object per event, for scripts |
| `--name NAME` | The device name shown to the other side |
| `--relay URL` | Use this relay (can be repeated) |
| `--rendezvous URL` | Use your own pairing server |
| `--no-relay` | Direct connections only. Needs your own pairing server, because devices otherwise meet through a relay |
| `--data-dir DIR` | Where settings and history are kept |

Exit codes: 0 everything arrived, 1 the session failed, 2 declined, cancelled
or incomplete, 130 interrupted.

## Remove the app

* **Windows:** Settings > Apps > Installed apps > Normandin Tunnel > Uninstall.
* **Linux:** remove the package with your package manager, or delete the
  AppImage file.

Settings and history live in a separate folder, shown under Settings > About >
Data folder (`NormandinSystems/Tunnel` inside your user data folder). Delete
it to remove them too. Received files are yours and are never deleted.
