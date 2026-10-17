# vag-tool

KWP2000-over-TP2.0 diagnostics for an Audi A3 8P 2.0 TDI PD (engine code BMN,
Siemens PPD1.x) using a raw USB-CAN adapter on Linux SocketCAN, such as the
[8devices Korlan](https://www.8devices.com/products/korlan).

Rust, edition 2024, needs 1.99 or newer. The CAN layer is the
[`automotive`](https://crates.io/crates/automotive) crate, so the same code
reaches a SocketCAN interface today and a panda, Vector or J2534 device later;
everything above it - VW TP2.0 and VAG's KWP2000 dialect - is in this crate.

Verified end to end over a `vcan0` interface against the built-in simulated ECU:
the frames it puts on the wire match the TP2.0 examples in jazdw's write-up byte
for byte. **Not yet tested against the car.**

```sh
cargo build --release
./target/release/vag-tool --help
./target/release/vag-tool -c can0 status
./target/release/vag-tool -c can0 monitor --csv regen.csv
```

Below, `vag-tool` means that binary (`cargo install --path .` puts it on your
PATH).

## What works today

| Command | What it does |
| --- | --- |
| `info` | opens a TP2.0 channel to module 0x01, starts a diagnostic session, reads part number, software version, system and current coding |
| `status` | reads groups 002, 070, 075 once and checks the regeneration prerequisites |
| `monitor` | polls groups 070 + 075 (and 002 every 5th sample) during the drive, prints a line per sample, optional CSV |
| `blocks N [N...]` | reads arbitrary measuring block groups, `--watch` to loop |
| `dtc [--labels X]` | reads fault memory, decodes VAG/SAE codes and status, names them from VAG's text table if `assets/labels/` is there |
| `freeze-frame` | fault environment data: mileage, date, priority, driving cycle |
| `clear-dtc` | clears the fault memory |
| `security-access CODE` | logs in with a numeric code, VCDS function 16 (`27 03` / `27 04`) |
| `basic-settings N` | runs basic setting group N, VCDS function 04 (`31 BA` / `31 B9` / `32 B8`), `--code` to log in first, `--monitor` to watch |
| `coding2` | assembles a Coding-II write, VCDS function 11; prints it and only sends with `--write` |
| `raw '21 46'` | sends one KWP2000 request; anything that is not a known read-only service needs `--write` |
| `decode-log FILE` | reassembles a sniffed candump log into KWP2000 messages, flagging everything that is not a plain read |
| `simecu` | runs a fake PPD1 ECU on a vcan interface, for working without the car |

`vag-tool` is one binary with no runtime dependencies; `--debug` prints every
CAN frame, including traffic that is not ours.

A second, independent program lives in `tools/extract-labels`: it lifts VAG's
text table out of an Android package so faults get names. It shares no code
with `vag-tool` and builds on its own.

```sh
cargo build --release --workspace     # both binaries
```

## Connecting the Korlan

The Korlan ships in two connector variants
([datasheet r2.0](https://wiki.8devices.com/_media/korlan:korlan_usb2can_datasheet_r2.0.pdf),
[user guide](https://wiki.8devices.com/_media/korlan:korlan_usb2can_user_guide.pdf)):

* **Korlan USB2CAN OBD2** - plugs straight into the car's J1962 socket, no
  harness. It wires only four pins, which are exactly the ones the car needs:

  | OBD2 pin | Korlan |
  | --- | --- |
  | 4, 5 | GND |
  | 6 | CAN High |
  | 14 | CAN Low |

  All other pins, pin 16 (+12 V battery) included, are "not used and not
  connected", so nothing is drawn from or shorted to the car. 300 mm cable.

* **Korlan USB2CAN DB9** - needs an OBD2-to-DB9 cable. The DB9 follows CiA-303:
  pin 2 CAN_L, pin 7 CAN_H, pin 3 CAN GND (pin 6 is also GND, pin 5 is the
  shield through 100 Ω/0.1 µF). So the cable must join OBD 14 -> DB9 2,
  OBD 6 -> DB9 7, OBD 4/5 -> DB9 3. Cheap OBD-to-DB9 cables are wired to
  several different conventions; check yours with a multimeter first.

Either way the adapter is USB powered (5 V, under 0.3 W) with 2.5 kV galvanic
isolation, so the laptop and the car share no ground.

**Termination: the Korlan has none, and that is correct here.** The user guide
is explicit: "The USB2CAN does not have an internal termination. Device must be
used on a terminated CAN bus." A car's bus is already terminated at both ends,
so an adapter that added its own 120 Ω would load it. If you ever bench-test a
single ECU, you have to add termination yourself.

Before plugging in, a multimeter across OBD pins 6 and 14 with the ignition off
should read about 60 Ω - the two 120 Ω terminators in parallel. That confirms a
high-speed CAN pair is actually on those pins.

## Bringing up the adapter

```sh
sudo ip link set can0 type can bitrate 500000
sudo ip link set can0 up
vag-tool -c can0 info
```

The powertrain CAN on this car is 500 kbit/s. The engine ECU is TP2.0 logical
module `0x01`; the gateway is `0x19`. `vag-tool modules` lists all 89 by
name, and `-m` takes a name (`engine`, `gateway`), a VAG symbol (`DID_19`) or
an address (`0x19`); a bare number is decimal.

To work without the car:

```sh
sudo modprobe vcan
sudo ip link add dev vcan0 type vcan && sudo ip link set vcan0 up
vag-tool -c vcan0 simecu &            # fake ECU
vag-tool -c vcan0 status
vag-tool -c vcan0 --debug blocks 75   # see every TP2.0 frame
```

Run only one simulator at a time: two of them answer every request twice, and
the replies interleave into nonsense.

Tests need no hardware - they drive the simulator over an in-memory bus:

```sh
cargo test
```

## Starting the regeneration

The procedure for PD/PPD engines, from the
[Ross-Tech wiki](https://wiki.ross-tech.com/index.php/Diesel_Particle_Filter_Emergency_Regeneration):

1. **Prerequisites** - coolant above 50 °C (group 002 field 4), filter load
   below the spec VCDS shows (group 075 field 3), electrical consumers switched
   on. `vagtool status` checks the first two.
2. **Enable it** - VCDS function **Coding-II (11)**, enter **21295**.
   Note this is *not* a Login and *not* Security Access; the earlier project
   note calling it a login was wrong.
3. **Drive** - 30-60 km/h, 1500-2500 rpm in 4th or 5th, 15-20 minutes, exhaust
   temperature before the turbo above 700 °C. This engine has no standstill
   regeneration.
4. **Watch** - group 070 field 1 regeneration status (`xxxxxx1x` = forced
   regeneration active), field 3 the counter/timer; group 075 fields 1/2/4 the
   exhaust temperatures and field 3 the filter load. `vagtool monitor` shows
   all of these and logs them.

Step 2 has two routes, both now implemented.

### Route 1: try security access first (harmless, no writes)

```sh
vag-tool -c can0 security-access 21295
```

This is a seed-and-key exchange, not a write: the ECU either accepts the code
or returns a negative response, and nothing is stored either way. VAG's key is
just `(code + seed) mod 2^32`. If the ECU accepts it, try the basic-setting
route, which is what the tooling uses on engines that support it:

```sh
vag-tool -c can0 basic-settings 70 --code 21295 --monitor --csv regen.csv
```

A refusal is informative rather than a dead end, because VAG's codes are
specific - "marginal conditions have not been met", "entry in DTC memory",
"temperature too low", "engine not running long enough", "perform Login". The
tool prints that wording.

### Route 2: Coding-II, which needs one capture

Coding-II is a write to the ECU's coding:

```
3B 9A <4 bytes> <WSC:2> <software version in ASCII> 03 <code:3 bytes>
```

Two things stop this being guessable. The request embeds **the ECU's own
software version**, so a sequence captured from another car is wrong for yours.
And the four bytes after `9A` are not understood here - some tools uses
`05 97 38 DE` for a control unit's coding but different values elsewhere, so
they have to be read off a capture of this ECU. Once you have them, the tool
assembles the rest:

```sh
vag-tool coding2 --code 21295 --sw-version 0003 --prefix 059738DE
#   3b 9a 05 97 38 de 30 39 30 30 30 33 03 00 53 2f      (printed, not sent)
```

It only transmits with `--write`.

### Getting that capture

1. Borrow a VCDS or OBDeleven for ten minutes, or find someone on a VAG forum
   who has one.
2. Listen in parallel on the Korlan while they do Coding-II → 21295. Listening
   only, no frames of our own:

   ```sh
   sudo ip link set can0 type can bitrate 500000 listen-only on
   sudo ip link set can0 up
   candump -l can0          # or: vag-tool -c can0 --debug info
   ```

3. Find the sequence:

   ```sh
   vag-tool decode-log candump-*.log --only-writes
   ```

   Everything that is not a known read-only service is flagged. The Coding-II
   request should contain `53 2f` (21295).
4. Replay it:

   ```sh
   vag-tool -c can0 raw '<the flagged request bytes>' --write
   vag-tool -c can0 monitor --csv regen.csv
   ```

Replaying a captured write is still a write to the engine ECU. Read what you
captured before you send it.

## Safety

* If the filter load is above the ECU's maximum the filter must be replaced.
  Forcing a regeneration on an overloaded filter is a fire risk — `status`
  refuses to give a green light above 40 g, and cautions above 30 g, but those
  thresholds come from Ross-Tech's common-rail page and are a heuristic here:
  compare against the spec VCDS shows for this engine.
* A forced regeneration runs the exhaust at 600-700 °C. Outdoors, away from
  anything flammable.
* A clogged filter is usually a symptom. On these engines: differential
  pressure sensor or its cracked hoses, exhaust temperature sensor, injectors,
  or a diet of short trips. Fix the cause or it clogs again. `dtc` and
  `freeze-frame` are the place to start:

  ```
  $ vag-tool -c can0 dtc
  2 stored fault(s):
    P0471 (VAG 16855) - Exhaust Pressure Sensor "A" / Range/Performance [implausible signal] intermittent
    P2002 (VAG 18434) - Particulate Trap Bank 1 / Efficiency Below Threshold [lower limit not reached] static

  $ vag-tool -c can0 freeze-frame
    mileage             184233 km
    time stamp          2025-10-14 09:35:12
  ```

## Layout

```
src/link.rs        the CAN link: an automotive adapter, or an in-memory pair for tests
src/tp20.rs        VW TP2.0: channel setup, framing, acks, keepalive
src/kwp2000.rs     KWP2000 client, VAG negative responses, identification, Coding-II
src/blocks.rs      measuring block formula table and decoding
src/dpf.rs         DPF snapshot, prerequisite checks, monitor + CSV
src/faults.rs      fault code / status decoding, freeze frames, label lookup
src/logdecode.rs   offline TP2.0 reassembly of a sniffed session
src/simecu.rs      simulated PPD1 ECU
src/main.rs        command line interface
tests/offline.rs   whole stack against the simulator: cargo test

tools/extract-labels/   standalone: VAG's text table out of an Android package
src/modules.rs     the 89 VAG module addresses, by name
assets/labels/<lang>.tsv  that table, sorted, which `dtc` reads by itself
assets/labels-extra.tsv   codes the package lacks, folded in on extraction
```

A Python implementation of the same tool is in the first commit, if the history
is ever useful.

## Credit

The TP2.0 framing follows jazdw's [write-up](https://jazdw.net/tp20) and the
implementations in `pq-flasher` (`tp20.py`, Panda-based) and
`teensycanbusdisplay` (`vwtpkwp2k.c`). The measuring block formula
table is taken from those two projects — `jazdw/vag-blocks` (GPL-3.0, Jared
Wiltshire) and `seishuku/teensycanbusdisplay` (Matt Williams). Formulas neither
of them documents are left out: an unknown formula index is reported as a raw
16-bit value rather than silently mis-scaled.
