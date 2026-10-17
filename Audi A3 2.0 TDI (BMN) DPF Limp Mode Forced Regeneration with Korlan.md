# Audi A3 2.0 TDI (BMN) DPF Limp Mode: Forced Regeneration with Korlan

Oct 17, 2026 · @Jacob

There's no ready-made open-source app that does this, but the Korlan can do it with some Python scripting. The free libraries exist; the missing piece is the exact command sequence for this ECU. If you just want the car driving again this week, a cheap VAG-specific tool is much faster.

## What the Korlan is

The [8devices Korlan](https://www.8devices.com/products/korlan) is a raw USB-to-CAN adapter, not an ELM327 or J2534 scanner. It supports Linux SocketCAN, has an open-source CANAL DLL for Windows, and works with python-can.

- Commercial VAG apps (VCDS, OBDeleven, VCP, Carista) won't work with it. They all need their own hardware.
- It is a good fit for scripting your own diagnostics on Linux.

## What the car needs

The car is an Audi A3 8P with the 170 hp 2.0 TDI PD engine (code BMN). It uses a Siemens PPD1.x ECU, which talks KWP2000 over VW's TP2.0 protocol on 500 kbit/s CAN. [Ross-Tech documents the procedure](https://wiki.ross-tech.com/index.php/Diesel_Particle_Filter_Emergency_Regeneration) for PD/PPD engines:

- Enable regeneration by entering the code 21295, then watch measuring block 070. Field 1 shows regeneration status (xxxxxx1x means a forced regen is active); field 3 shows a regen counter/timer.
- On PD/PPD engines this is a regeneration while driving. The ECU raises exhaust temperature while you hold sustained load, typically motorway speed in a lower gear at about 3000 rpm for 15–20 minutes. Unlike later common-rail engines, these don't offer a standstill regen.

## Open-source route (Korlan + Linux)

1. [I-CAN-hack/pq-flasher](https://github.com/I-CAN-hack/pq-flasher) is the best starting point. It reflashes a PQ35 VW power-steering module and includes Python libraries for the TP2.0 transport layer and KWP2000. The A3 is also PQ35, so the transport code should apply. Write a short script with python-can and SocketCAN on the Korlan that:
   - opens a TP2.0 channel to the engine ECU (address 0x01)
   - starts a diagnostic session
   - sends the 21295 login
   - starts basic setting group 070
   - polls block 070 and the temperature blocks while you drive
2. [jared52005/Monitor](https://github.com/jared52005/Monitor) logs CAN and TP2.0 traffic into Wireshark and decodes KWP2000. Use it to check what your script actually sends.
3. [seishuku/teensycanbusdisplay](https://github.com/seishuku/teensycanbusdisplay) is a working example of reading TP2.0/KWP2000 measuring blocks, including DPF soot load.

The hard part is the exact KWP2000 service bytes for the login and basic-settings start on this Siemens ECU; they aren't guessed here. The reliable approach: borrow a VCDS or OBDeleven once, sniff the session with the Korlan in silent mode, and replay it in your script.

## Practical route

Any VAG-capable tool does this as a menu item:

- OBDeleven (Bluetooth dongle plus phone app)
- a genuine Ross-Tech VCDS cable
- a VCP interface

Hiring a VCDS owner from a local VAG forum for an hour also works.

## Check before you force a regen

- **Read the soot load first.** Check the DPF blocks (Ross-Tech uses 075) and the differential pressure. If the filter is loaded past the ECU's maximum, it must be replaced; forcing a regen then risks a fire. If the ECU refuses to start the regen, that's usually why.
- **Find out why it clogged.** Common causes on these engines:
  - a failed or misreading differential pressure sensor, or cracked sensor hoses (often the real cause of the limp mode)
  - a faulty exhaust temperature sensor
  - lots of short trips
  - injector faults
- Clear the fault codes after a successful regen, and check that block 070 shows the counter reset.

## Sources

- [8devices Korlan product page](https://www.8devices.com/products/korlan)
- [Ross-Tech Wiki: Diesel Particle Filter Emergency Regeneration](https://wiki.ross-tech.com/index.php/Diesel_Particle_Filter_Emergency_Regeneration)
- [GitHub topic: tp20](https://github.com/topics/tp20)
- [seishuku/teensycanbusdisplay README](https://github.com/seishuku/teensycanbusdisplay/blob/master/README.md)
