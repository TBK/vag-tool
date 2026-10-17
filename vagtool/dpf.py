"""DPF-specific reading and monitoring for the PD/PPD 2.0 TDI (BMN).

Procedure this supports, per the Ross-Tech wiki page "Diesel Particle Filter
Emergency Regeneration", PD/PPD section:

  prerequisites  coolant above 50 C (group 002 field 4)
                 filter load below the spec VCDS shows (group 075 field 3)
                 electrical consumers on
  trigger        VCDS Coding-II (11), code 21295  -- see README, not automated
  drive          30-60 km/h, 1500-2500 rpm in 4th/5th, 15-20 minutes,
                 EGT before turbo above 700 C
  watch          group 070 field 1 regeneration status, field 3 counter/timer
                 group 075 fields 1/2/4 EGT, field 3 filter load
"""

import csv
import dataclasses
import time

from . import blocks

GROUP_ENGINE = 2
GROUP_REGEN = 70
GROUP_FILTER = 75

COOLANT_MIN_C = 50.0
EGT_TARGET_C = 700.0
# Ross-Tech quotes 30 g / 40 g thresholds for the common-rail engines; the PD
# page only says "below the spec VCDS shows". Treated as a caution, not truth.
LOAD_CAUTION_G = 30.0
LOAD_STOP_G = 40.0


@dataclasses.dataclass
class Snapshot:
    t: float
    rpm: blocks.Cell = None
    coolant: blocks.Cell = None
    regen_status: blocks.Cell = None
    regen_counter: blocks.Cell = None
    egt_pre_turbo: blocks.Cell = None
    egt_pre_dpf: blocks.Cell = None
    egt_post_dpf: blocks.Cell = None
    load: blocks.Cell = None

    @property
    def regen(self):
        if self.regen_status is None:
            return {"bits": "????????", "normal_regen": False, "forced_regen": False}
        return blocks.decode_regen_status(int(self.regen_status.value))


def read_snapshot(kwp, with_engine=True):
    snap = Snapshot(t=time.time())

    if with_engine:
        cells = blocks.decode_group(kwp.read_group(GROUP_ENGINE))
        if len(cells) >= 4:
            snap.rpm, snap.coolant = cells[0], cells[3]

    cells = blocks.decode_group(kwp.read_group(GROUP_REGEN))
    if len(cells) >= 3:
        snap.regen_status, snap.regen_counter = cells[0], cells[2]

    cells = blocks.decode_group(kwp.read_group(GROUP_FILTER))
    if len(cells) >= 4:
        snap.egt_pre_turbo, snap.egt_pre_dpf, snap.load, snap.egt_post_dpf = cells
    return snap


def check_prerequisites(snap):
    """Returns a list of (level, message); level is 'stop', 'warn' or 'ok'."""
    out = []

    if snap.coolant is None:
        out.append(("warn", "coolant temperature not read"))
    elif snap.coolant.unit != "C":
        out.append(("warn", f"coolant cell decoded as {snap.coolant.unit}, "
                            f"check group 002 field 4 by hand"))
    elif snap.coolant.value < COOLANT_MIN_C:
        out.append(("stop", f"coolant {snap.coolant.value:.0f} C is below the "
                            f"{COOLANT_MIN_C:.0f} C minimum - warm the engine first"))
    else:
        out.append(("ok", f"coolant {snap.coolant.value:.0f} C"))

    if snap.load is None:
        out.append(("warn", "filter load not read"))
    elif snap.load.unit == "g":
        if snap.load.value >= LOAD_STOP_G:
            out.append(("stop", f"filter load {snap.load.value:.1f} g is at or above "
                                f"{LOAD_STOP_G:.0f} g - do not force a regeneration, "
                                f"the filter needs replacing"))
        elif snap.load.value >= LOAD_CAUTION_G:
            out.append(("warn", f"filter load {snap.load.value:.1f} g is above "
                                f"{LOAD_CAUTION_G:.0f} g - compare against the spec "
                                f"VCDS shows before forcing anything"))
        else:
            out.append(("ok", f"filter load {snap.load.value:.1f} g"))
    else:
        out.append(("warn", f"filter load reads {blocks.format_cell(snap.load)} - "
                            f"compare against the spec VCDS shows"))

    regen = snap.regen
    if regen["forced_regen"]:
        out.append(("ok", "forced regeneration is already active"))
    elif regen["normal_regen"]:
        out.append(("ok", "a normal regeneration is running"))
    else:
        out.append(("ok", f"no regeneration active (status {regen['bits']})"))
    return out


CSV_COLUMNS = ["time", "rpm", "coolant_c", "regen_bits", "forced_regen", "normal_regen",
               "regen_counter", "egt_pre_turbo_c", "egt_pre_dpf_c", "egt_post_dpf_c",
               "filter_load", "filter_load_unit"]


def snapshot_row(snap):
    def v(cell):
        return "" if cell is None else round(float(cell.value), 2) if isinstance(
            cell.value, (int, float)) else cell.value
    regen = snap.regen
    return {
        "time": round(snap.t, 2),
        "rpm": v(snap.rpm),
        "coolant_c": v(snap.coolant),
        "regen_bits": regen["bits"],
        "forced_regen": int(regen["forced_regen"]),
        "normal_regen": int(regen["normal_regen"]),
        "regen_counter": v(snap.regen_counter),
        "egt_pre_turbo_c": v(snap.egt_pre_turbo),
        "egt_pre_dpf_c": v(snap.egt_pre_dpf),
        "egt_post_dpf_c": v(snap.egt_post_dpf),
        "filter_load": v(snap.load),
        "filter_load_unit": "" if snap.load is None else snap.load.unit,
    }


def format_line(snap, started=None):
    regen = snap.regen
    flag = "FORCED" if regen["forced_regen"] else ("regen" if regen["normal_regen"] else "-")
    elapsed = "" if started is None else f" {int(snap.t - started) // 60:02d}:" \
                                        f"{int(snap.t - started) % 60:02d}"
    def n(cell, fmt="{:.0f}"):
        return "----" if cell is None else fmt.format(float(cell.value))
    return (f"{time.strftime('%H:%M:%S', time.localtime(snap.t))}{elapsed}  "
            f"{flag:>6}  bits {regen['bits']}  "
            f"rpm {n(snap.rpm):>5}  cool {n(snap.coolant):>4}C  "
            f"EGT {n(snap.egt_pre_turbo):>4}/{n(snap.egt_pre_dpf):>4}/"
            f"{n(snap.egt_post_dpf):>4}C  "
            f"load {n(snap.load, '{:.1f}'):>5}"
            f"{'' if snap.load is None else snap.load.unit}  "
            f"ctr {n(snap.regen_counter, '{:.0f}')}")


class Monitor:
    """Polls the DPF groups and writes a CSV log. Safe to Ctrl-C."""

    def __init__(self, kwp, csv_path=None, interval=1.0, engine_every=5):
        self.kwp = kwp
        self.interval = interval
        self.engine_every = engine_every
        self.csv_path = csv_path
        self._writer = None
        self._file = None
        self.started = None
        self.samples = 0
        self.forced_seen = False
        self._last_engine = (None, None)

    def __enter__(self):
        if self.csv_path:
            self._file = open(self.csv_path, "w", newline="")
            self._writer = csv.DictWriter(self._file, fieldnames=CSV_COLUMNS)
            self._writer.writeheader()
        return self

    def __exit__(self, *exc):
        if self._file:
            self._file.close()

    def poll(self):
        with_engine = self.samples % self.engine_every == 0
        snap = read_snapshot(self.kwp, with_engine=with_engine)
        # Group 002 is read less often; keep showing the last values in between.
        if with_engine:
            self._last_engine = (snap.rpm, snap.coolant)
        else:
            snap.rpm, snap.coolant = self._last_engine
        if self.started is None:
            self.started = snap.t
        self.samples += 1
        if snap.regen["forced_regen"]:
            self.forced_seen = True
        if self._writer:
            self._writer.writerow(snapshot_row(snap))
            self._file.flush()
        return snap

    def run(self, duration=None, on_sample=print):
        end = None if duration is None else time.monotonic() + duration
        while end is None or time.monotonic() < end:
            tick = time.monotonic()
            snap = self.poll()
            if on_sample:
                on_sample(format_line(snap, self.started))
            self._wait(self.interval - (time.monotonic() - tick))

    def _wait(self, seconds):
        """Sleep between samples, keeping the TP2.0 channel alive. The ECU drops
        an idle channel after about a second, so a long --interval still needs
        channel tests in between."""
        deadline = time.monotonic() + seconds
        while True:
            remaining = deadline - time.monotonic()
            if remaining <= 0:
                return
            time.sleep(min(remaining, 0.3))
            if remaining > 0.3:
                self.kwp.channel.keepalive_if_idle(0.5)
