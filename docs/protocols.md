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
| Raster gaps (burn at 0 % power) sent as cuts at 0 % to keep layer speed | | unconfirmed |
| USB serial: 115200 8N1, RTS/CTS, DTR on (`transport = "serial"`, `serial_port`) | | from MeerK40t |

`sim::RuidaSim` is a software controller that checks checksums, argument
lengths and the job file sum and records where the laser would fire. Every
driver test runs against it, including over a real localhost UDP socket.

## EZCAD2 LMC galvo (`libreburn-driver-lmc`)

Opcodes, the 12-byte command and 3072-byte list packet formats, status bits,
unit conversions (fiber power `0..0xFFF`, Q-switch period `20000 / kHz`,
speed in galvo units per ms), the init sequence and the light-mode and
program-mode port handling follow MeerK40t's `meerk40t/balormk/controller.py`
(Copyright (c) 2021 meerk40t, MIT License), which descends from the balor
project. No MeerK40t GUI code is used.

Red-light framing sets only the guide-light output bit
(`framing.guide_light_port`; MeerK40t's default is 8) and only uses jump
commands, with the fiber master oscillator closed. The laser-enable output is
`connection.laser_enable_port` (default 0, as in MeerK40t's program mode). A
profile that puts the guide light on the same bit is refused, because that
is exactly the setting that makes "framing" fire the fiber source.

| Item | Status |
| --- | --- |
| Init values (laser mode, timing, standby, FPK) | MeerK40t defaults, unconfirmed |
| Guide-light bit 8 / enable bit 0 | MeerK40t defaults, check per machine |
| `.cor` correction files (both layouts) | loaded from `connection.correction_file`, from MeerK40t |
| USB: 9588:9899, interface 0, bulk OUT 0x02, IN 0x88 | from MeerK40t |

`sim::LmcSim` tracks port bits, MO state and list execution, and records
every mark attempt, every firing segment with its power, and every jump
traced with the guide light on.
