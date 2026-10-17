# VAG KWP2000 protocol reference

What this project knows about VAG's KWP2000 dialect, and where each fact came
from. Facts only — the implementations in `vagtool/` are our own.

Provenance tags:

| Tag | Source |
| --- | --- |
| `RT` | [Ross-Tech wiki](https://wiki.ross-tech.com/) |
| `JZ` | jazdw's [TP2.0 write-up](https://jazdw.net/tp20) and `jazdw/vag-blocks` |
| `PQ` | `RESEARCH/pq-flasher` (`tp20.py`, `kwp2000.py`) |
| `TC` | `RESEARCH/teensycanbusdisplay` (`vwtpkwp2k.c`) |
| `OE` | OBDeleven APK, `com.obdeleven.service.*` (`RESEARCH/OBD2/`) — read for interoperability, see "On the APKs" |
| `OURS` | our own observation or naming, not from a source |

## Transport: TP2.0

| Step | Bytes | Source |
| --- | --- | --- |
| Channel setup | `0x200: <module> C0 00 10 00 03 01` | JZ PQ TC |
| Setup response | `0x200+module: 00 D0 <ecu tx lo hi> <our tx lo hi> 01` | JZ PQ TC |
| Timing params | `A0 0F 8A FF 32 FF` → `A1 0F 8A FF 4A FF` | JZ PQ TC |
| Data frame | `<type:4><seq:4>` + 7 bytes; first frame starts with a 16-bit big-endian length | JZ PQ |
| Frame types | `0x1` last + expect ACK, `0x2` more follow | JZ PQ TC |
| ACK | `B0 \| ((seq+1) & 0x0F)` | PQ TC |
| Keepalive | `A3` → `A1 ...`, needed about once a second | TC |
| Disconnect | `A8` | JZ |

Engine ECU is module `0x01`, gateway `0x1F`. Bus is 500 kbit/s on OBD pins 6/14.

## Sessions — `10 <id>`, positive `50 <id>`

| Id | Name | Source |
| --- | --- | --- |
| `0x89` | standard diagnostic session | OE JZ PQ TC |
| `0x86` | engineering mode | OE |
| `0x85` | programming | PQ |
| `0x84` | end of line, ECU | OE |
| `0x83` | end of line, VW | OE |
| `0x81` | OBD / EOBD | OE |

OBDeleven names these `STANDARD`, `ENGINEERING`, `EOL_ECU`, `EOL_VW`, `OBD_EOBD`;
`pq-flasher` separately documents `0x85` for programming.

## Reading

| Function | Request | Positive | Source |
| --- | --- | --- | --- |
| ECU identification | `1A 9B` | `5A 9B ...` | OE JZ PQ |
| Short identification | `1A 91` | `5A 91 ...` | OE JZ |
| Long coding read | `1A 9A` | `5A 9A ...` | OE |
| Measuring block (group N) | `21 NN` | `61 NN` + 4 × (formula, A, B) | JZ PQ TC OE |
| Fault memory | `18 02 FF FF` | `58 <count>` + 3 bytes per DTC | OURS (ISO 14230) |
| Fault environment data | `21 6C` | `61 6C ...`, layout below | OE (response), OURS (request) |
| Clear faults | `14 FF 00` | `54` | OURS (ISO 14230) |

### `1A 9B` identification layout (`OE`)

```
5A 9B <part number:12> <software version:4> <coding length:1>
      <coding:3> <workshop identification:6> <system description:20>
      <subsystem blocks of 47 bytes...>
```

The first byte of the part number and of the software version carries a flag in
bit 7: mask it off before reading the ASCII. A 3-byte coding is `value &
0x7FFFFF`, shown as 7 decimal digits. `vagtool info` prints all of this, and the
software version is what a Coding-II write has to embed.

A coding value of length 3 is read as `value & 0x7FFFFF` and shown as 7 decimal
digits (`OE`) — so a 5-digit code like 21295 is `00 53 2F`.

## Security Access — VCDS function 16

```
->  27 03                     request seed
<-  67 03 SS SS SS SS         4-byte seed
    key = (code + seed) mod 2^32        <- the whole algorithm (OE)
->  27 04 KK KK KK KK         send key
<-  67 ...                    accepted
```

`code` is the decimal code the tool shows you, added to the seed as a plain
32-bit sum. `pq-flasher` independently documents access types 3 (request seed)
and 4 (send key) for the non-programming case, which matches.

## Basic Settings and Adaptation — one family of three routines

Both VCDS function 04 (basic settings) and function 10 (adaptation) are the
same three routines with a **mode byte** (`OE`):

```
31 BA <mode> <id>           open      positive: 71 ...   negative: 7F 31 <nrc>
31 B9 <mode> <id> [data]    run       positive: 71 ...   negative: 7F 31 <nrc>
32 B8 <mode> <id>           close     positive: 72 ...   negative: 7F 32 <nrc>
```

| mode | id | meaning |
| --- | --- | --- |
| `0x00` | measuring block group | basic settings for that group |
| `0x01` | `0x03` | adaptation, plain 2-byte channel value |
| `0x01` | `0x0A` | long adaptation |

An adaptation write appends the new value to the `31 B9` request as two bytes
(`%04X` of the decimal value). Measured values are read with `21 NN` while the
function runs. The names "open", "run" and "close" are `OURS`; OBDeleven calls
them in that order.

On **KWP1281** adaptation is different again: block `22 <channel> <data>` reads
and `2A <channel> <data> 00 <WSC>` writes (`OE`).

## Routine control — used by one-click apps

`31 <routine id> <args...>`, positive `71 <routine id>` (`OE`). The routine id
and arguments come from OBDeleven's server, not from the app.

## Coding-II — VCDS function 11, the DPF regeneration trigger

OBDeleven builds this for a KWP2000 control unit as (`OE`):

```
3B 9A <4 bytes> <WSC:2> <SW version in ASCII hex> 03 <code:3 bytes>
         ^ 05 97 38 DE     ^ 0x3039 = WSC 12345        ^ e.g. 00 53 2F = 21295
```

* `3B` is writeDataByLocalIdentifier, `9A` the coding identifier.
* `03` is the coding length; the code follows as 3 bytes.
* The **ECU's own software version is embedded as ASCII**, which is why a
  sequence captured from a different car cannot simply be replayed.
* The four bytes after `9A` are most likely **workshop identification** — the
  tester's own importer and equipment number, which the ECU stores next to the
  coding rather than validates. OBDeleven hardcodes `05 97 38 DE` for a control
  unit's coding, `05 94 E8 DE` on its long-coding path and `01 81 C8 F6` for the
  gateway, and writes the same shape over UDS as `2E F198 07 73 28 F6` (DID
  F198 is the UDS repair shop code). It never reads them back, so nothing here
  proves which value an ECU expects — capture yours rather than assume.
* This writes *a* coding value. On a control unit whose coding type makes
  Coding-II the same request as plain coding, that is the coding you are
  changing. `vagtool info` shows the current value first.

On **KWP1281** (K-line cars) the same function is a login block instead:
`2B <code:2> 00 <WSC:2>`, acknowledged with block `09` (`OE`). That is not our
ECU, which is KWP2000 over TP2.0.

## Fault codes

The two bytes a KWP2000 or KWP1281 ECU returns per fault carry both
representations of a VAG DTC (`OE`, verified against the published mapping):

```
bit 14 set   -> an SAE code:  letter = "PCBU"[(raw >> 12) & 3]
                              digit  = (raw >> 10) & 3
                              rest   = raw & 0x3FF, as three decimal digits
bit 14 clear -> a VAG-specific code: the raw value as five decimal digits
```

So raw `0x4034` is both VAG 16436 and P0052, and `0x41D7` is VAG 16855 = P0471.
The familiar 5-digit VAG number is simply the raw 16-bit value.

The status byte splits as (`OE`):

| Bits | Meaning |
| --- | --- |
| `0..6` | fault type, 0-15 in practice; see the table below |
| `7` | set = intermittent, clear = static |

Fault types (`OE`, VAG's wording): 1 upper limit exceeded, 2 lower limit not
reached, 3 mechanical malfunction, 4 no signal/communication, 5 no or incorrect
basic setting/adaptation, 6 short circuit to B+, 7 short circuit to ground,
8 implausible signal, 9 open circuit/short circuit to ground, 10 open
circuit/short circuit to B+, 11 open circuit, 12 electrical error in circuit,
13 please read DTC, 14 faulty, 15 cannot be checked currently. KWP1281 uses a
different, 83-entry table.

### Fault environment data (freeze frame)

A `21 6C` response decodes as (`OE` for the layout):

```
61 6C <2 bytes: the fault it belongs to> <priority> <frequency counter>
      <driving cycle> <mileage:3, km> <1 byte> <date/time:5> ...
```

The five date bytes are a packed bit field: seconds `& 0x3F`, minutes `>> 6`,
hour `>> 12 & 0x1F`, day `>> 17 & 0x1F`, month `>> 22 & 0x0F`, year
`>> 26 & 0x7F` plus 2000.

## Fault and label text

OBDeleven ships VAG's text table as `assets/lang/<lang>.json` in its APK:
72,231 entries, including `P######`/`B######`/`C######`/`U######` SAE
descriptions, `VAG#####` for the VAG-specific codes, `IDE#####` measured value
names, `MAS#####` text table values, `UNIT####` units and `FST#####` fault
types. The lookup key is the fault code plus `00` for an SAE code
(`P0471` -> `P047100`), or `VAG` plus the five digits (`VAG00281`) (`OE`).

This project does not carry that data. `vagtool dtc --labels <en.json|base.apk>`
reads it from a copy you already have, and works without it.

## Negative response codes

VAG's own wording for the codes a KWP2000 ECU returns (`OE`). These are far more
specific than the ISO 14230 table and several are exactly what a refused
regeneration looks like.

| Code | Meaning |
| --- | --- |
| `0x10` | Function cancelled, disruption in control module |
| `0x11`, `0x12`, `0x31` | Function not available |
| `0x13` | Function cannot be performed, message length or format incorrect |
| `0x22` | Function cancelled, marginal conditions have not been met |
| `0x24` | Procedure cannot be performed, incorrect sequence |
| `0x25` | Function cancelled, subsystem does not respond |
| `0x26` | Function cancelled, entry in DTC memory |
| `0x33` | Access authorization required, perform Login |
| `0x35` | Access authorization failed, incorrect Login code |
| `0x36` | Login not possible, number of access attempts exceeded |
| `0x37` | Login not possible, lock time not yet elapsed |
| `0x70` | Function cancelled, upload/download not possible |
| `0x71` | Function cancelled, data transfer error |
| `0x72` | Function cancelled, delete/programming unsuccessful |
| `0x73` | Function cancelled, incorrect data block sequence |
| `0x78` | Function started, control module busy (response pending) |
| `0x7E` | Function cannot be performed, incorrect diagnostic mode |
| `0x81` | Function cancelled, engine RPM too high |
| `0x82` | Function cancelled, engine RPM too low |
| `0x83` | Function cancelled, engine running |
| `0x84` | Function cancelled, engine not running |
| `0x85` | Function cancelled, engine not running long enough |
| `0x86` | Function cancelled, temperature too high |
| `0x87` | Function cancelled, temperature too low |
| `0x88` | Function cancelled, vehicle speed too high |
| `0x89` | Function cancelled, vehicle speed too low |
| `0x8A` | Function cancelled, accelerator pedal position exceeded |
| `0x8B` | Function cancelled, accelerator pedal position not reached |
| `0x8C` | Function cancelled, selector lever not in position 'N' |
| `0x8D` | Function cancelled, driving mode/gear not selected |
| `0x8F` | Function cancelled, brake pedal not depressed |
| `0x90` | Function cancelled, selector lever not in position 'P' |
| `0x91` | Function cancelled, converter clutch locked |
| `0x92` | Function cancelled, supply voltage too high |
| `0x93` | Function cancelled, supply voltage too low |

## On the APKs

`RESEARCH/OBD2/` holds retail Android packages. They were read to learn how
these ECUs expect to be addressed — service ids, request layouts, the
seed-and-key sum — because that information is published nowhere else. What is
recorded here and implemented in `vagtool/` is protocol fact and our own code;
no decompiled source was copied, and none of it belongs in a redistributable
form. Car Scanner (`com.ovz.carscanner`, .NET/MAUI) turned out to carry no
KWP2000 logic worth citing: its VAG procedures are descriptive text for
UDS-era cars and its routine data lives on its server.
