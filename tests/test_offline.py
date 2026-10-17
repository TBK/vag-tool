"""Offline tests: the whole stack against the simulated ECU, no hardware."""

import unittest

from vagtool import blocks, dpf, faults, logdecode, simecu
from vagtool.kwp2000 import (Kwp2000Client, NegativeResponse, SESSION_DIAGNOSTIC,
                             build_coding_ii_request, parse_identification)
from vagtool.tp20 import Tp20Channel


class StackTest(unittest.TestCase):
    def setUp(self):
        self.bus, self.ecu, self.stop, self.thread = simecu.spawn_on_pair()
        self.channel = Tp20Channel(self.bus, timeout=1.0)
        self.channel.open()
        self.kwp = Kwp2000Client(self.channel)
        self.kwp.start_diagnostic_session(SESSION_DIAGNOSTIC)

    def tearDown(self):
        self.channel.close()
        self.stop.set()
        self.thread.join(timeout=2)
        self.bus.close()

    def test_channel_addresses(self):
        self.assertEqual(self.channel.rx_addr, 0x300)
        self.assertEqual(self.channel.tx_addr, 0x740)

    def test_identification(self):
        ident = self.kwp.read_ecu_identification()
        self.assertIn(b"03G906016AB", ident)

    def test_group_75_decodes_to_sane_values(self):
        cells = blocks.decode_group(self.kwp.read_group(75))
        self.assertEqual(len(cells), 4)
        egt, load = cells[0], cells[2]
        self.assertEqual(egt.unit, "C")
        self.assertGreater(egt.value, 250)
        self.assertEqual(load.unit, "g")
        self.assertAlmostEqual(load.value, 28.0, delta=1.0)

    def test_unknown_group_is_a_negative_response(self):
        with self.assertRaises(NegativeResponse):
            self.kwp.read_group(200)

    def test_snapshot_and_prerequisites(self):
        snap = dpf.read_snapshot(self.kwp)
        self.assertAlmostEqual(snap.coolant.value, 85.0, delta=1.0)
        self.assertFalse(snap.regen["forced_regen"])
        levels = [level for level, _ in dpf.check_prerequisites(snap)]
        self.assertNotIn("stop", levels)

    def test_cold_engine_blocks_regeneration(self):
        self.ecu.coolant = 20.0
        snap = dpf.read_snapshot(self.kwp)
        levels = dict((msg, level) for level, msg in dpf.check_prerequisites(snap))
        self.assertIn("stop", levels.values())

    def test_overloaded_filter_blocks_regeneration(self):
        self.ecu.load_g = 45.0
        snap = dpf.read_snapshot(self.kwp)
        stops = [m for level, m in dpf.check_prerequisites(snap) if level == "stop"]
        self.assertTrue(any("replacing" in m for m in stops))

    def test_forced_regeneration_shows_up_in_the_monitor(self):
        # The simulator accepts the 21295 code inside any writing service; on the
        # real ECU this is the captured Coding-II sequence replayed with `raw`.
        self.kwp.request(bytes([0x3B, 0x00]) + (21295).to_bytes(2, "big"))
        with dpf.Monitor(self.kwp, interval=0.0) as monitor:
            monitor.run(duration=0.3, on_sample=None)
        self.assertTrue(monitor.forced_seen)
        self.assertGreater(monitor.samples, 1)
        snap = dpf.read_snapshot(self.kwp)
        self.assertTrue(snap.regen["forced_regen"])
        self.assertGreater(snap.egt_pre_turbo.value, 400)

    def test_dtc_read_and_clear(self):
        self.assertEqual(self.kwp.read_dtcs(), [(0x41D7, 0x88), (0x4802, 0x02)])
        decoded = faults.decode_faults(self.kwp.read_dtcs())
        self.assertEqual([f.code_text for f in decoded], ["P0471", "P2002"])
        self.assertTrue(decoded[0].intermittent)
        self.assertFalse(decoded[1].intermittent)
        self.assertEqual(decoded[1].status_text(), "lower limit not reached")
        self.kwp.clear_dtcs()
        self.assertEqual(self.kwp.read_dtcs(), [])

    def test_identification_is_parsed(self):
        ident = parse_identification(self.kwp.read_ecu_identification())
        self.assertEqual(ident.part_number, "03G906016AB")
        self.assertEqual(ident.sw_version, "0003")
        self.assertEqual(ident.coding_length, 3)
        self.assertEqual(ident.coding, 11)
        self.assertIn("EDC", ident.system_description)

    def test_freeze_frame(self):
        frame = faults.decode_freeze_frame(self.kwp.read_freeze_frame())
        self.assertEqual(frame.priority, 2)
        self.assertEqual(frame.frequency, 5)
        self.assertEqual(frame.driving_cycle, 0x28)
        self.assertEqual(frame.mileage_km, 184233)
        self.assertEqual(frame.timestamp, "2025-10-14 09:35:12")

    def test_csv_log(self):
        import tempfile, os, csv as csvmod
        path = os.path.join(tempfile.mkdtemp(), "log.csv")
        with dpf.Monitor(self.kwp, csv_path=path, interval=0.0) as monitor:
            monitor.run(duration=0.2, on_sample=None)
        with open(path) as fh:
            rows = list(csvmod.DictReader(fh))
        self.assertGreater(len(rows), 1)
        self.assertIn("egt_pre_turbo_c", rows[0])


class BlockFormulaTest(unittest.TestCase):
    def test_known_formulas(self):
        self.assertAlmostEqual(blocks.decode_cell(0x01, 100, 35).value, 700.0)
        self.assertEqual(blocks.decode_cell(0x1A, 10, 100).unit, "C")
        self.assertAlmostEqual(blocks.decode_cell(0x05, 70, 200).value, 700.0)

    def test_unknown_formula_is_flagged_raw(self):
        cell = blocks.decode_cell(0xAB, 0x12, 0x34)
        self.assertFalse(cell.known)
        self.assertEqual(cell.value, 0x1234)
        self.assertIn("formula 0xab", blocks.format_cell(cell))


SNIFF = """\
(1000.000000) can0 200#01C00010000301
(1000.010000) can0 201#00D00003400701
(1000.020000) can0 740#A00F8AFF32FF
(1000.030000) can0 300#A10F8AFF4AFF
(1000.040000) can0 740#1000021089
(1000.050000) can0 300#B1
(1000.060000) can0 300#1000025089
(1000.070000) can0 740#B1
(1000.080000) can0 740#1100042B00532F
(1000.090000) can0 300#B2
(1000.100000) can0 300#1100026B00
(1000.110000) can0 740#B2
"""


class LogDecodeTest(unittest.TestCase):
    def test_reassembles_and_flags_the_write(self):
        decoder = logdecode.SessionDecoder()
        messages = decoder.feed_log(SNIFF.splitlines())
        self.assertEqual(decoder.tester_tx, 0x740)
        self.assertEqual(decoder.ecu_tx, 0x300)
        payloads = [m.payload.hex() for m in messages]
        self.assertIn("1089", payloads)
        self.assertIn("5089", payloads)
        writes = [m for m in messages if m.is_write]
        self.assertEqual(len(writes), 2)
        self.assertEqual(writes[0].payload.hex(), "2b00532f")
        self.assertEqual(writes[0].direction, "tester")

    def test_plain_candump_format(self):
        frames = list(logdecode.parse_frames(["  can0  740   [5]  10 00 02 21 01"]))
        self.assertEqual(frames[0][1], 0x740)
        self.assertEqual(frames[0][2].hex(), "1000022101")


class KeepaliveTest(unittest.TestCase):
    """A long poll interval must not let the TP2.0 channel go idle."""

    def test_channel_survives_a_slow_interval(self):
        bus, ecu, stop, thread = simecu.spawn_on_pair()
        self.addCleanup(stop.set)
        self.addCleanup(bus.close)
        channel = Tp20Channel(bus, timeout=1.0)
        channel.open()
        kwp = Kwp2000Client(channel)
        kwp.start_diagnostic_session(SESSION_DIAGNOSTIC)
        with dpf.Monitor(kwp, interval=1.2) as monitor:
            monitor.run(duration=2.5, on_sample=None)
        self.assertGreaterEqual(monitor.samples, 2)
        # channel still usable after the idle gaps
        self.assertEqual(len(blocks.decode_group(kwp.read_group(75))), 4)
        channel.close()


class SecurityAccessTest(unittest.TestCase):
    """Login and basic settings, as learned from the OBDeleven package."""

    def setUp(self):
        self.bus, self.ecu, self.stop, self.thread = simecu.spawn_on_pair()
        self.addCleanup(self.stop.set)
        self.addCleanup(self.bus.close)
        self.channel = Tp20Channel(self.bus, timeout=1.0)
        self.channel.open()
        self.addCleanup(self.channel.close)
        self.kwp = Kwp2000Client(self.channel)
        self.kwp.start_diagnostic_session(SESSION_DIAGNOSTIC)

    def test_key_is_code_plus_seed(self):
        seed, key = self.kwp.security_access(21295)
        self.assertEqual(seed, simecu.SEED)
        self.assertEqual(key, (21295 + simecu.SEED) & 0xFFFFFFFF)
        self.assertTrue(self.ecu.logged_in)

    def test_wrong_code_is_rejected_with_vag_wording(self):
        with self.assertRaises(NegativeResponse) as caught:
            self.kwp.security_access(12345)
        self.assertEqual(caught.exception.code, 0x35)
        self.assertIn("incorrect Login code", str(caught.exception))
        self.assertFalse(self.ecu.logged_in)

    def test_basic_setting_needs_login_first(self):
        with self.assertRaises(NegativeResponse) as caught:
            self.kwp.prepare_basic_setting(70)
        self.assertEqual(caught.exception.code, 0x33)
        self.assertIn("perform Login", str(caught.exception))

    def test_basic_setting_refuses_a_cold_engine(self):
        self.kwp.security_access(21295)
        self.ecu.coolant = 20.0
        with self.assertRaises(NegativeResponse) as caught:
            self.kwp.prepare_basic_setting(70)
        self.assertEqual(caught.exception.code, 0x87)
        self.assertIn("temperature too low", str(caught.exception))

    def test_basic_setting_runs_the_regeneration(self):
        self.kwp.security_access(21295)
        self.kwp.prepare_basic_setting(70)
        self.kwp.start_basic_setting(70)
        snap = dpf.read_snapshot(self.kwp)
        self.assertTrue(snap.regen["forced_regen"])
        self.kwp.stop_basic_setting(70)
        snap = dpf.read_snapshot(self.kwp)
        self.assertFalse(snap.regen["forced_regen"])

    def test_start_without_prepare_is_a_sequence_error(self):
        self.kwp.security_access(21295)
        with self.assertRaises(NegativeResponse) as caught:
            self.kwp.start_basic_setting(70)
        self.assertEqual(caught.exception.code, 0x24)


class CodingIIRequestTest(unittest.TestCase):
    def test_layout(self):
        req = build_coding_ii_request(21295, "0003", bytes.fromhex("059738DE"))
        self.assertEqual(req.hex(), "3b9a059738de30393030303303" + "00532f")

    def test_prefix_has_no_default_and_is_checked(self):
        with self.assertRaises(ValueError):
            build_coding_ii_request(21295, "0003", b"\x05\x97")
        with self.assertRaises(ValueError):
            build_coding_ii_request(0x800000, "0003", bytes(4))

    def test_wsc_is_two_bytes(self):
        req = build_coding_ii_request(1, "A", bytes(4), wsc=12345)
        self.assertEqual(req[6:8].hex(), "3039")


class FaultDecodingTest(unittest.TestCase):
    def test_sae_codes_unpack_from_the_raw_bytes(self):
        # bit 14 set -> SAE code: letter, one digit, three decimal digits
        self.assertEqual(faults.Fault(0x4034, 0).code_text, "P0052")
        self.assertEqual(faults.Fault(0x4802, 0).code_text, "P2002")
        self.assertEqual(faults.Fault(0x4034, 0).text_key, "P005200")
        self.assertEqual(faults.Fault(0x4034, 0).vag_code, 16436)

    def test_vag_codes_are_the_raw_value(self):
        fault = faults.Fault(281, 0x0B)
        self.assertFalse(fault.is_sae)
        self.assertEqual(fault.code_text, "00281")
        self.assertEqual(fault.text_key, "VAG00281")
        self.assertEqual(fault.status_text(), "open circuit")

    def test_status_bit_7_is_intermittent(self):
        self.assertTrue(faults.Fault(0x4034, 0x81).intermittent)
        self.assertFalse(faults.Fault(0x4034, 0x01).intermittent)
        self.assertEqual(faults.Fault(0x4034, 0x81).status_code, 1)

    def test_description_comes_from_the_label_table_when_present(self):
        labels = faults.LabelTable({"P200200": "Particulate Trap Bank 1\nEfficiency Below Threshold"})
        text = faults.Fault(0x4802, 0x02).describe(labels)
        self.assertIn("Particulate Trap Bank 1 / Efficiency", text)
        self.assertIn("lower limit not reached", text)

    def test_works_without_a_label_table(self):
        self.assertEqual(faults.Fault(0x4802, 0x00).describe(),
                         "P2002 (VAG 18434) static")


class LabelTableTest(unittest.TestCase):
    def test_reads_json_and_apk(self):
        import json, os, tempfile, zipfile
        tmp = tempfile.mkdtemp()
        data = {"P005200": "O2 Sensor Heater", "VAG00281": "Driving speed sensor"}

        plain = os.path.join(tmp, "en.json")
        with open(plain, "w") as fh:
            json.dump(data, fh)
        self.assertEqual(faults.LabelTable.from_path(plain).get("P005200"),
                         "O2 Sensor Heater")

        apk = os.path.join(tmp, "base.apk")
        with zipfile.ZipFile(apk, "w") as zf:
            zf.writestr("assets/lang/en.json", json.dumps(data))
            zf.writestr("classes.dex", "x")
        table = faults.LabelTable.from_path(apk)
        self.assertEqual(len(table), 2)
        self.assertEqual(table.get("VAG00281"), "Driving speed sensor")
        self.assertIsNone(table.get("P999900"))


if __name__ == "__main__":
    unittest.main()
