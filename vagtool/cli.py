"""Command line interface: python -m vagtool <command>"""

import argparse
import contextlib
import sys

from . import blocks, dpf, faults, logdecode
from .canbus import CanError, SocketCanBus
from .kwp2000 import (Kwp2000Client, NegativeResponse, SESSION_DIAGNOSTIC,
                      UnexpectedResponse, build_coding_ii_request, changes_ecu,
                      parse_identification)
from .tp20 import Tp20Channel, Tp20Error

LEVEL_PREFIX = {"ok": "  ok ", "warn": "warn ", "stop": "STOP "}


@contextlib.contextmanager
def connect(args):
    """Open the CAN interface, a TP2.0 channel and a diagnostic session."""
    try:
        bus = SocketCanBus(args.channel)
    except CanError as e:
        raise SystemExit(f"error: {e}\n"
                         f"hint: bring the interface up first, e.g.\n"
                         f"  sudo ip link set {args.channel} type can bitrate 500000\n"
                         f"  sudo ip link set {args.channel} up")
    channel = Tp20Channel(bus, module=args.module, debug=args.debug)
    try:
        channel.open()
    except Tp20Error as e:
        bus.close()
        raise SystemExit(f"error: {e}\n"
                         f"hint: ignition on, engine ECU is module 0x01, bus must be "
                         f"500 kbit/s")
    kwp = Kwp2000Client(channel, debug=args.debug)
    try:
        kwp.start_diagnostic_session(SESSION_DIAGNOSTIC)
        yield kwp
    finally:
        with contextlib.suppress(Exception):
            channel.close()
        bus.close()


def cmd_info(args):
    with connect(args) as kwp:
        body = kwp.read_ecu_identification()
        print(f"module {args.module:#04x} on {args.channel}")
        try:
            ident = parse_identification(body)
        except UnexpectedResponse:
            printable = "".join(chr(c) if 32 <= c < 127 else "." for c in body)
            print(f"  unparsed        {printable}")
            print(f"  raw             {body.hex(' ')}")
            return 0
        for line in ident.lines():
            print("  " + line)
        if args.raw:
            print(f"  raw             {ident.raw.hex(' ')}")
        if ident.sw_version:
            print(f"\nfor coding2: --sw-version {ident.sw_version}")
    return 0


def cmd_status(args):
    with connect(args) as kwp:
        snap = dpf.read_snapshot(kwp)
        for group in (dpf.GROUP_ENGINE, dpf.GROUP_REGEN, dpf.GROUP_FILTER):
            print(blocks.format_group(group, blocks.decode_group(kwp.read_group(group))))
        print()
        worst = "ok"
        for level, message in dpf.check_prerequisites(snap):
            print(LEVEL_PREFIX[level] + message)
            if level == "stop" or (level == "warn" and worst == "ok"):
                worst = level
        print()
        if worst == "stop":
            print("Do not force a regeneration until the above is resolved.")
        else:
            print("Regeneration prerequisites look acceptable. To start one, try "
                  "`security-access 21295` then `basic-settings 70`, or the "
                  "Coding-II route - see README.")
    return 0 if worst != "stop" else 1


def cmd_monitor(args):
    with connect(args) as kwp:
        print("polling groups 070 and 075 (Ctrl-C to stop)"
              + (f", logging to {args.csv}" if args.csv else ""))
        with dpf.Monitor(kwp, csv_path=args.csv, interval=args.interval) as monitor:
            try:
                monitor.run(duration=args.duration)
            except KeyboardInterrupt:
                print()
            print(f"{monitor.samples} samples, "
                  f"forced regeneration {'seen' if monitor.forced_seen else 'never seen'}")
    return 0


def cmd_blocks(args):
    with connect(args) as kwp:
        while True:
            for group in args.group:
                try:
                    body = kwp.read_group(group)
                except NegativeResponse as e:
                    print(f"{group:03d}  {e}")
                    continue
                print(blocks.format_group(group, blocks.decode_group(body)))
            if not args.watch:
                return 0
            print()


def _labels(args):
    if not args.labels:
        return None
    try:
        return faults.LabelTable.from_path(args.labels)
    except (OSError, ValueError) as e:
        print(f"warning: could not read labels from {args.labels}: {e}",
              file=sys.stderr)
        return None


def cmd_dtc(args):
    labels = _labels(args)
    with connect(args) as kwp:
        entries = kwp.read_dtcs()
        if not entries:
            print("no stored faults")
            return 0
        print(f"{len(entries)} stored fault(s):")
        for fault in faults.decode_faults(entries):
            print(f"  {fault.describe(labels)}")
        if not labels:
            print("\n(pass --labels <en.json|base.apk> for fault descriptions)")
    return 0


def cmd_freeze_frame(args):
    with connect(args) as kwp:
        try:
            frame = faults.decode_freeze_frame(kwp.read_freeze_frame())
        except (NegativeResponse, ValueError) as e:
            print(f"no fault environment data: {e}")
            return 1
        lines = frame.lines()
        if not lines:
            print(f"empty freeze frame: {frame.raw.hex(' ')}")
            return 1
        print("fault environment data (local identifier 0x6C):")
        for line in lines:
            print("  " + line)
        if args.raw:
            print(f"  raw                 {frame.raw.hex(' ')}")
    return 0


def cmd_clear_dtc(args):
    with connect(args) as kwp:
        kwp.clear_dtcs()
        print("fault memory cleared")
    return 0


def cmd_raw(args):
    payload = bytes.fromhex(args.hex.replace(",", " ").replace("0x", "").replace(" ", ""))
    if not payload:
        raise SystemExit("error: no bytes given")
    if changes_ecu(payload[0]) and not args.write:
        raise SystemExit(
            f"error: service {payload[0]:#04x} is not a known read-only service.\n"
            f"Re-run with --write if that is what you mean. Read the README section "
            f"on replaying a captured sequence first.")
    with connect(args) as kwp:
        try:
            response = kwp.request(payload)
        except NegativeResponse as e:
            print(f"<- {e}")
            return 1
        print(f"-> {payload.hex(' ')}")
        print(f"<- {response.hex(' ')}")
    return 0


def cmd_security_access(args):
    with connect(args) as kwp:
        try:
            seed, key = kwp.security_access(args.code)
        except NegativeResponse as e:
            print(f"refused: {e}")
            return 1
        print(f"login accepted with code {args.code}")
        print(f"  seed {seed:#010x}  key {key:#010x}  (key = code + seed)")
    return 0


def cmd_basic_settings(args):
    with connect(args) as kwp:
        if args.code is not None:
            seed, key = kwp.security_access(args.code)
            print(f"login accepted (seed {seed:#010x})")
        if args.stop:
            kwp.stop_basic_setting(args.group)
            print(f"basic setting {args.group:03d} stopped")
            return 0
        kwp.prepare_basic_setting(args.group)
        print(f"basic setting {args.group:03d} prepared (31 BA)")
        kwp.start_basic_setting(args.group)
        print(f"basic setting {args.group:03d} started (31 B9)")
        if args.monitor:
            print("monitoring; Ctrl-C to stop (this does NOT stop the setting)")
            with dpf.Monitor(kwp, csv_path=args.csv, interval=args.interval) as monitor:
                try:
                    monitor.run()
                except KeyboardInterrupt:
                    print()
    return 0


def cmd_coding2(args):
    request = build_coding_ii_request(args.code, args.sw_version,
                                      bytes.fromhex(args.prefix.replace(" ", "")),
                                      wsc=args.wsc)
    print(f"Coding-II request for code {args.code}:")
    print(f"  {request.hex(' ')}")
    if not args.write:
        print("\nNot sent. This writes the ECU's coding; re-run with --write once "
              "you have checked the bytes against a capture from this ECU "
              "(docs/PROTOCOL.md).")
        return 0
    with connect(args) as kwp:
        try:
            response = kwp.request(request)
        except NegativeResponse as e:
            print(f"refused: {e}")
            return 1
        print(f"accepted: {response.hex(' ')}")
    return 0


def cmd_decode_log(args):
    for kind, line in logdecode.decode_file(args.path, only_writes=args.only_writes):
        print(("note: " if kind == "note" else "") + line)
    return 0


def cmd_simecu(args):
    from . import simecu
    simecu.serve(args.channel, debug=args.debug)
    return 0


def build_parser():
    p = argparse.ArgumentParser(
        prog="vagtool",
        description="KWP2000-over-TP2.0 diagnostics for VAG PQ35 (A3 8P 2.0 TDI PD/BMN) "
                    "over SocketCAN.")
    p.add_argument("-c", "--channel", default="can0", help="SocketCAN interface (default can0)")
    p.add_argument("-m", "--module", type=lambda s: int(s, 0), default=0x01,
                   help="TP2.0 logical module address (default 0x01, engine)")
    p.add_argument("--debug", action="store_true", help="print every CAN frame")
    sub = p.add_subparsers(dest="command", required=True)

    s = sub.add_parser("info", help="open a session and read ECU identification")
    s.add_argument("--raw", action="store_true", help="also print the raw bytes")
    s.set_defaults(func=cmd_info)

    s = sub.add_parser("status", help="read the DPF groups once and check prerequisites")
    s.set_defaults(func=cmd_status)

    s = sub.add_parser("monitor", help="poll the DPF groups continuously during the drive")
    s.add_argument("--csv", help="write samples to this file")
    s.add_argument("--interval", type=float, default=1.0, help="seconds between samples")
    s.add_argument("--duration", type=float, help="stop after this many seconds")
    s.set_defaults(func=cmd_monitor)

    s = sub.add_parser("blocks", help="read arbitrary measuring block groups")
    s.add_argument("group", nargs="+", type=lambda v: int(v, 0))
    s.add_argument("--watch", action="store_true", help="keep reading until Ctrl-C")
    s.set_defaults(func=cmd_blocks)

    s = sub.add_parser("dtc", help="read stored fault codes")
    s.add_argument("--labels", help="VAG text table: an en.json or an APK holding one")
    s.set_defaults(func=cmd_dtc)

    s = sub.add_parser("freeze-frame",
                       help="read fault environment data (mileage, date, counters)")
    s.add_argument("--raw", action="store_true")
    s.set_defaults(func=cmd_freeze_frame)

    s = sub.add_parser("clear-dtc", help="clear the fault memory")
    s.set_defaults(func=cmd_clear_dtc)

    s = sub.add_parser("raw", help="send one raw KWP2000 request, e.g. raw '21 46'")
    s.add_argument("hex")
    s.add_argument("--write", action="store_true",
                   help="allow a service that changes the ECU")
    s.set_defaults(func=cmd_raw)

    s = sub.add_parser("security-access",
                       help="log in with a numeric code (VCDS function 16)")
    s.add_argument("code", type=int)
    s.set_defaults(func=cmd_security_access)

    s = sub.add_parser("basic-settings",
                       help="run a basic setting group (VCDS function 04)")
    s.add_argument("group", type=lambda v: int(v, 0))
    s.add_argument("--code", type=int, help="log in with this code first")
    s.add_argument("--stop", action="store_true", help="stop the group instead")
    s.add_argument("--monitor", action="store_true",
                   help="poll the DPF groups once it is running")
    s.add_argument("--csv", help="write monitor samples to this file")
    s.add_argument("--interval", type=float, default=1.0)
    s.set_defaults(func=cmd_basic_settings)

    s = sub.add_parser("coding2",
                       help="assemble a Coding-II write (VCDS function 11)")
    s.add_argument("--code", type=int, required=True, help="e.g. 21295")
    s.add_argument("--sw-version", required=True,
                   help="the ECU's own software version string, from `info`")
    s.add_argument("--prefix", required=True,
                   help="the 4 bytes after 9A, taken from a capture of this ECU")
    s.add_argument("--wsc", type=int, default=12345, help="workshop code")
    s.add_argument("--write", action="store_true", help="actually send it")
    s.set_defaults(func=cmd_coding2)

    s = sub.add_parser("decode-log", help="reassemble a sniffed candump log into KWP2000")
    s.add_argument("path")
    s.add_argument("--only-writes", action="store_true",
                   help="show only messages that change the ECU")
    s.set_defaults(func=cmd_decode_log)

    s = sub.add_parser("simecu", help="run a fake ECU on a vcan interface")
    s.set_defaults(func=cmd_simecu)
    return p


def main(argv=None):
    args = build_parser().parse_args(argv)
    try:
        return args.func(args)
    except KeyboardInterrupt:
        return 130
    except (Tp20Error, CanError, NegativeResponse, UnexpectedResponse) as e:
        print(f"error: {e}", file=sys.stderr)
        return 1
