# Security policy

## Where things stand

Normandin Tunnel is a preview. Its design is written down in
[docs/PROTOCOL.md](docs/PROTOCOL.md) and
[docs/THREAT_MODEL.md](docs/THREAT_MODEL.md), and it is tested against a
deliberately hostile peer, but **no independent security review has been
done**. Known gaps are listed at the end of the threat model.

## Reporting a problem

Please do not open a public issue for a security problem.

Use **Report a vulnerability** under the repository's
[Security tab](https://github.com/NormandinSystems/normandin-tunnel/security).
That opens a private report only the maintainers can see.

It helps to include:

* what you found and why it matters,
* the steps or a small program that shows it,
* the version or commit you looked at, and your operating system.

You will get a reply as soon as we can manage. This is a small project with
no paid security team, so please allow some time. We will tell you what we
found, credit you if you wish, and publish a fix and a note in the changelog.

## What counts

Anything that lets someone other than the two paired people:

* read or change what is transferred,
* connect without knowing the code, or get more than one guess at it,
* make the safety codes match when a third party is in the middle,
* write a file outside the folder the receiver chose, or overwrite a file
  that the conflict setting says to keep,
* run or open something on the receiving device,
* crash the app or exhaust memory or disk from the other side of a session.

Also in scope: the optional pairing server in `crates/tunnel-rendezvous`, and
anything in this repository that would leak a code, a key or a file name.

Out of scope, because the design does not claim to prevent them: a device
that is already compromised, someone spoiling a code by answering first with a
wrong guess (threat T14), and the availability of the public relays.

## Supported versions

Only the latest release receives fixes.
