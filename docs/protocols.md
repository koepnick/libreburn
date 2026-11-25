# Protocol sources and credits

libreburn's drivers reuse protocol knowledge from these MIT-licensed projects.
Code taken from them keeps its original copyright and MIT notice.

- [MeerK40t](https://github.com/meerk40t/meerk40t): Ruida and EZCAD2 (LMC) drivers.
- [galvoplotter](https://pypi.org/project/galvoplotter): LMC protocol library, from the balor project.
- [Ruida notes, EduTech Wiki](https://edutechwiki.unige.ch/en/Ruida): packet format, ports and scrambling.

Opcodes marked "placeholder" in the driver crates have not yet been checked
against recorded fixtures from real boards (`hil/fixtures/`).

## Ruida (`libreburn-driver-ruida`)

Opcodes, number packing, the scramble ("swizzle") function, memory addresses
and the UDP/serial session rules follow MeerK40t's `meerk40t/ruida/`
(`rdjob.py`, `ruidasession.py`, `udp_transport.py`, `usb_transport.py`),
Copyright (c) 2021 meerk40t, MIT License. Only protocol facts were reused;
none of MeerK40t's GUI code is used.

| Item | Value | Status |
| --- | --- | --- |
| UDP ports | commands to 50200, replies on 40200 | from MeerK40t |
| UDP packet | big-endian 16-bit sum of the scrambled bytes, then scrambled payload | from MeerK40t |
| ACK / NAK on the wire (key 0x88) | `0xC6` / `0x48` (payload `0xCC` / `0xCF`) | from MeerK40t |
| Serial | same scrambled bytes, no checksum, no ACK, 9-byte memory replies | from MeerK40t |
| Coordinates | µm, 5 x 7-bit big-endian; relative moves 2 x 7-bit, ±8191 µm | from MeerK40t |
| Power | 14-bit, 16383 = 100 % | from MeerK40t |
| Rapid move option byte for jog (`D9 10 00 ...`) | `0x00` | unconfirmed |
| Per-move power change mid-layer (`C6 01/02` inline) | | unconfirmed |

`sim::RuidaSim` is a software controller that checks checksums, argument
lengths and the job file sum and records where the laser would fire. Every
driver test runs against it, including over a real localhost UDP socket.
