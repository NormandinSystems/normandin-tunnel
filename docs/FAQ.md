# Questions and answers

### Is it free?

Yes. There is nothing to pay, no account and no subscription. It is also open
source: the code is public under the MIT or Apache-2.0 licence, so anyone can
read it, change it and share it.

### Do both people need the app?

Yes. Both devices need Normandin Tunnel, either the desktop app or the
`tunnel` command.

### Which systems does it run on?

Windows 10 and 11, and Linux. There is no macOS, Android or iOS version yet.

### Does it work between two different networks?

Yes. The two devices can be in the same room or in different countries. Both
need internet access.

### Does it work without internet, on the same Wi-Fi?

Not out of the box. The devices find each other through relays on the
internet, even when they sit next to each other. Once paired, the files
themselves normally travel directly over your own network. To pair with no internet at
all, run a pairing server on your network: see
[Hosting your own servers](SELF_HOSTING.md).

### Is there a size limit?

A single file can be up to 1 TiB (about 1,100 GB). One transfer can hold up
to a million files by default. The practical limit is the free space on the
receiving device.

### Can anyone see my files?

No one between the two devices can read them. They are encrypted from one
device to the other. See [who can see what](HOW_IT_WORKS.md#who-can-see-what).

### Can you see my files, or who I send them to?

No. We run no servers that the app talks to, and the app sends us nothing: no
accounts, no analytics, no crash reports.

### What is the 6-digit safety code for?

It proves that the two devices are connected to each other and not to someone
in between. If both screens show the same number, you are fine. If they
differ, choose "They don't match" and start over.

### What happens if I mistype the code?

The app checks every word against its word list before connecting, so most
typos are caught on your own screen. If a wrong but valid code does reach the
other device, that code stops working and you make a new one. This is on
purpose: it is what stops anyone from guessing codes.

### Why did my code stop working?

A code works once and expires after 10 minutes. It also stops working after
any failed attempt to use it.

### Why does Windows warn me about the installer?

The installers are not code-signed yet. Signing certificates cost money or
need an approval process, and that has not been done for this preview. The
warning says the publisher is unknown; it does not mean something was found.
Choose "More info", then "Run anyway".

### Does the app update itself?

No. Check the
[Releases page](https://github.com/NormandinSystems/normandin-tunnel/releases)
for new versions.

### Has it been security-audited?

No. The design and its tests are public so that anyone can check them, and we
welcome reviews, but no independent audit has been done. Treat it as a
preview.

### Who runs the relays?

By default, the [iroh](https://iroh.computer) project. They are free, shared
and rate-limited, with no uptime promise. They cannot read what passes through
them. You can use your own instead.

### How is this different from other tools?

The idea is not new. [Magic Wormhole](https://github.com/magic-wormhole/magic-wormhole)
introduced pairing two computers with a short code, and
[croc](https://github.com/schollz/croc) and
[LocalSend](https://localsend.org) are well-known free tools in the same
space. Normandin Tunnel is a desktop app rather than a terminal command, pairs
across the internet by code with no server of its own, asks both people to
compare a safety code, and requires the receiver to accept every transfer.

### I found a bug. Where do I report it?

[Open an issue](https://github.com/NormandinSystems/normandin-tunnel/issues).
For a security problem, follow the [security policy](../SECURITY.md) instead.
